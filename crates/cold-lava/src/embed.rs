//! A terminal inside a pane: runs a program (Neovim) in a pseudo-terminal, parses its output
//! with `vt100`, and draws the resulting screen into a ratatui buffer.

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};

pub struct Embedded {
    parser: vt100::Parser,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Receiver<Vec<u8>>,
    size: (u16, u16),
    exited: bool,
    /// Messages the program sent us inside its output (`ESC ] 5379 ; kind ; payload BEL`).
    messages: Vec<(String, String)>,
    /// An incomplete message at the end of the last chunk.
    carry: Vec<u8>,
}

/// Start of a cold-lava message in the program's output (a private OSC sequence).
const MARK: &[u8] = b"\x1b]5379;";

fn nz(v: u16) -> NonZeroU16 {
    NonZeroU16::new(v.max(1)).unwrap()
}

impl Embedded {
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
    ) -> Result<Embedded> {
        let (rows, cols) = (rows.max(2), cols.max(10));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("couldn't open a pseudo-terminal")?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("couldn't start {program}"))?;
        drop(pair.slave);
        let rx = read_in_background(pair.master.try_clone_reader()?);
        let writer = pair.master.take_writer()?;
        Ok(Embedded {
            parser: vt100::Parser::new(nz(rows), nz(cols), 0),
            master: pair.master,
            writer,
            child,
            output: rx,
            size: (rows, cols),
            exited: false,
            messages: Vec::new(),
            carry: Vec::new(),
        })
    }

    /// Feed pending output into the screen.
    pub fn pump(&mut self) {
        loop {
            match self.output.try_recv() {
                Ok(bytes) => {
                    let screen_bytes = self.extract_messages(bytes);
                    self.parser.process(&screen_bytes);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.exited = true;
                    break;
                }
            }
        }
        if !self.exited && matches!(self.child.try_wait(), Ok(Some(_))) {
            self.exited = true;
        }
    }

    /// Take the messages received since the last call: (kind, payload).
    pub fn take_messages(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.messages)
    }

    /// Pull our messages out of the byte stream; return what's left for the screen.
    fn extract_messages(&mut self, chunk: Vec<u8>) -> Vec<u8> {
        let data = if self.carry.is_empty() {
            chunk
        } else {
            let mut c = std::mem::take(&mut self.carry);
            c.extend(chunk);
            c
        };
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if data[i] == 0x1b {
                let rest = &data[i..];
                if rest.starts_with(MARK) {
                    let body = &rest[MARK.len()..];
                    match body.iter().position(|&b| b == 0x07) {
                        Some(end) => {
                            let msg = String::from_utf8_lossy(&body[..end]).into_owned();
                            let (kind, payload) = msg.split_once(';').unwrap_or((msg.as_str(), ""));
                            self.messages.push((kind.to_string(), decode(payload)));
                            i += MARK.len() + end + 1;
                            continue;
                        }
                        None if body.len() < 64 * 1024 => {
                            self.carry = rest.to_vec();
                            break;
                        }
                        None => {} // not really ours: pass it through
                    }
                } else if rest.len() < MARK.len() && MARK.starts_with(rest) {
                    self.carry = rest.to_vec(); // might be the start of a message
                    break;
                }
            }
            out.push(data[i]);
            i += 1;
        }
        out
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(2), cols.max(10));
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        self.parser.set_size(nz(rows), nz(cols));
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn send_key(&mut self, k: KeyEvent) {
        let bytes = encode_key(k, self.parser.screen().application_cursor());
        self.send(&bytes);
    }

    /// Forward a mouse event if the program asked for mouse reporting. `col`/`row` are
    /// relative to the pane.
    pub fn send_mouse(&mut self, m: MouseEvent, col: u16, row: u16) {
        if self.parser.screen().mouse_protocol_mode() == vt100::MouseProtocolMode::None {
            return;
        }
        let (button, release) = match m.kind {
            MouseEventKind::Down(b) => (button_code(b), false),
            MouseEventKind::Up(b) => (button_code(b), true),
            MouseEventKind::Drag(b) => (button_code(b) + 32, false),
            MouseEventKind::ScrollUp => (64, false),
            MouseEventKind::ScrollDown => (65, false),
            _ => return,
        };
        let seq = format!(
            "\x1b[<{button};{};{}{}",
            col + 1,
            row + 1,
            if release { 'm' } else { 'M' }
        );
        self.send(seq.as_bytes());
    }

    /// Draw the screen into `area`. Returns the cursor position if it should be shown.
    pub fn render(&self, buf: &mut Buffer, area: Rect) -> Option<(u16, u16)> {
        let screen = self.parser.screen();
        for row in 0..area.height.min(self.size.0) {
            for col in 0..area.width.min(self.size.1) {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let mut style = Style::new()
                    .fg(color(cell.fgcolor()))
                    .bg(color(cell.bgcolor()));
                let mut m = Modifier::empty();
                if cell.bold() {
                    m |= Modifier::BOLD;
                }
                if cell.dim() {
                    m |= Modifier::DIM;
                }
                if cell.italic() {
                    m |= Modifier::ITALIC;
                }
                if cell.underline() {
                    m |= Modifier::UNDERLINED;
                }
                if cell.inverse() {
                    m |= Modifier::REVERSED;
                }
                style = style.add_modifier(m);
                let out = &mut buf[(area.x + col, area.y + row)];
                let text = cell.contents();
                out.set_symbol(if text.is_empty() { " " } else { text });
                out.set_style(style);
            }
        }
        if screen.hide_cursor() {
            return None;
        }
        let (r, c) = screen.cursor_position();
        (r < area.height && c < area.width).then_some((area.x + c, area.y + r))
    }

    #[cfg(test)]
    pub fn screen_text(&self) -> String {
        self.parser.screen().contents()
    }
}

impl Drop for Embedded {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// Read a pty's output on a thread; the channel disconnects when the program's side closes.
pub fn read_in_background(mut reader: Box<dyn Read + Send>) -> Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// Decode the payload encoding used by our Vim script: `%u<hex>;` stands for one character.
pub fn decode(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(pos) = rest.find("%u") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        match after
            .find(';')
            .and_then(|e| u32::from_str_radix(&after[..e], 16).ok().map(|c| (c, e)))
        {
            Some((code, e)) => {
                out.extend(char::from_u32(code));
                rest = &after[e + 1..];
            }
            None => {
                out.push_str("%u");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn button_code(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Bytes an xterm-compatible terminal would send for a key press.
pub fn encode_key(k: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    // xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl
    let modp = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let csi_letter = |letter: char| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[{n};{modp}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let mut out = match k.code {
        KeyCode::Char(c) if ctrl => {
            let b = match c {
                'a'..='z' => c as u8 - b'a' + 1,
                'A'..='Z' => c as u8 - b'A' + 1,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '7' | '/' => 0x1f,
                '?' | '8' => 0x7f,
                _ => return c.to_string().into_bytes(),
            };
            vec![b]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => csi_letter('A'),
        KeyCode::Down => csi_letter('B'),
        KeyCode::Right => csi_letter('C'),
        KeyCode::Left => csi_letter('D'),
        KeyCode::Home => csi_letter('H'),
        KeyCode::End => csi_letter('F'),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => return Vec::new(),
            };
            tilde(code)
        }
        _ => return Vec::new(),
    };
    // Alt+key is ESC followed by the key (for keys without their own modifier encoding).
    if alt
        && matches!(
            k.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace
        )
    {
        out.insert(0, 0x1b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn key_encoding() {
        let none = KeyModifiers::NONE;
        assert_eq!(encode_key(key(KeyCode::Char('x'), none), false), b"x");
        assert_eq!(
            encode_key(key(KeyCode::Char('é'), none), false),
            "é".as_bytes()
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('w'), KeyModifiers::CONTROL), false),
            [0x17]
        );
        assert_eq!(
            encode_key(key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
        assert_eq!(encode_key(key(KeyCode::Up, none), false), b"\x1b[A");
        assert_eq!(encode_key(key(KeyCode::Up, none), true), b"\x1bOA");
        assert_eq!(
            encode_key(key(KeyCode::Right, KeyModifiers::CONTROL), true),
            b"\x1b[1;5C"
        );
        assert_eq!(encode_key(key(KeyCode::Delete, none), false), b"\x1b[3~");
        assert_eq!(encode_key(key(KeyCode::F(5), none), false), b"\x1b[15~");
        assert_eq!(encode_key(key(KeyCode::Enter, none), false), b"\r");
        assert_eq!(encode_key(key(KeyCode::Backspace, none), false), [0x7f]);
    }

    #[test]
    fn messages_are_extracted_even_when_split() {
        let mut t = Embedded::spawn("true", &[], Path::new("."), 5, 20).unwrap();
        let screen =
            t.extract_messages(b"ab\x1b]5379;open;/v/K%u12b;lauea.md\x07cd\x1b]53".to_vec());
        assert_eq!(screen, b"abcd");
        let screen = t.extract_messages(b"79;follow;x%u9;y\x07ef".to_vec());
        assert_eq!(screen, b"ef");
        assert_eq!(
            t.take_messages(),
            [
                ("open".to_string(), "/v/Kīlauea.md".to_string()),
                ("follow".to_string(), "x\ty".to_string())
            ]
        );
        // Other escape sequences pass through untouched.
        assert_eq!(
            t.extract_messages(b"\x1b[1mbold\x1b]0;title\x07".to_vec()),
            b"\x1b[1mbold\x1b]0;title\x07"
        );
    }

    #[test]
    fn runs_a_program_in_a_pane() {
        let mut t = Embedded::spawn(
            "sh",
            &["-c".into(), "printf 'hello from the pty'".into()],
            Path::new("."),
            5,
            40,
        )
        .unwrap();
        for _ in 0..200 {
            t.pump();
            if t.screen_text().contains("hello") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(t.screen_text().contains("hello from the pty"));
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 5));
        t.render(&mut buf, Rect::new(0, 0, 40, 5));
        assert_eq!(buf[(0, 0)].symbol(), "h");
    }
}
