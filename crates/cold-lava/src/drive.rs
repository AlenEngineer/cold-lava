//! `cold-lava drive`: runs cold-lava in a pseudo-terminal, types what a script says, reads the
//! screen back (`vt100`) and checks it. The program under test is the real binary, so the
//! terminal setup, mouse mode and an embedded Neovim's own pty are all part of what's tested.

use crate::commands::Key;
use crate::say::{out, told};
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use std::io::Write;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// No script runs longer than this, whatever its steps say.
const OVERALL_TIMEOUT: Duration = Duration::from_secs(300);
/// How long, at most, the program gets to answer a lone Esc before the next key is typed.
const ESC_SETTLE: Duration = Duration::from_millis(500);
/// A pause in the output this long means the program finished drawing its frame.
const FRAME_SETTLE: Duration = Duration::from_millis(50);

#[derive(Debug, PartialEq)]
pub struct Script {
    pub cols: u16,
    pub rows: u16,
    pub env: Vec<(String, String)>,
    pub steps: Vec<Step>,
}

#[derive(Debug, PartialEq)]
pub struct Step {
    /// 1-based line in the script, for messages.
    pub line: usize,
    pub action: Action,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Send(Vec<Input>),
    WaitFor {
        text: String,
        timeout: Duration,
    },
    WaitGone {
        text: String,
        timeout: Duration,
    },
    Wait(Duration),
    Expect(String),
    ExpectNot(String),
    Screen,
    Exits {
        code: u32,
        timeout: Duration,
    },
    /// 1-based, like the terminal's own coordinates.
    Click {
        row: u16,
        col: u16,
    },
    /// Press the left button at `from`, move with it held to `to`, release there.
    Drag {
        from: (u16, u16),
        to: (u16, u16),
    },
    /// Turn the wheel `count` notches at a cell.
    Scroll {
        up: bool,
        at: (u16, u16),
        count: u16,
    },
    /// The screen shows TEXT inside rows `top`..=`bottom`, columns `left`..=`right` (1-based).
    ExpectIn {
        top: u16,
        left: u16,
        bottom: u16,
        right: u16,
        text: String,
    },
    /// Replace a file in the workspace, as another program (Obsidian, git) would.
    Write {
        file: PathBuf,
        text: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum Input {
    Key(Key),
    Text(String),
}

#[derive(Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
    /// Lines to say after the message: the steps there are, for a step that isn't one.
    pub then: Option<&'static str>,
}

pub const SYNTAX: &str = "\
size COLSxROWS          terminal size, before the first step (default 160x44)
env NAME=VALUE          environment for the program, before the first step
keys TOKEN…             each token a key (enter, esc, tab, up, pageup, ctrl+q, …) or literal text
type TEXT               the rest of the line, literally
wait-for [10s] TEXT     until the screen shows TEXT (default 5s)
wait-gone [10s] TEXT    until the screen no longer shows TEXT
wait 500ms              pause (prefer wait-for)
expect TEXT             the screen shows TEXT now
expect-not TEXT         the screen doesn't show TEXT now
screen                  print the screen
click ROW COL           left click (1-based)
drag ROW COL ROW2 COL2  press the left button at ROW COL, move to ROW2 COL2 with it held, release
scroll up|down ROW COL [N]
                        turn the mouse wheel N notches (default 1) over ROW COL
expect-in ROW COL ROW2 COL2 TEXT
                        the screen shows TEXT within rows ROW–ROW2 and columns COL–COL2 now
write FILE TEXT         replace FILE in the workspace (the last argument) with TEXT, as another
                        program would; \\n in TEXT is a line break
exits [10s] CODE        the program exits with CODE
# comment             a line starting with # is skipped";

pub fn parse(src: &str) -> Result<Script, ParseError> {
    let mut script = Script {
        cols: 160,
        rows: 44,
        env: Vec::new(),
        steps: Vec::new(),
    };
    for (i, raw) in src.lines().enumerate() {
        let line = i + 1;
        let then = None;
        let err = |message: String| ParseError {
            line,
            message,
            then,
        };
        let text = raw.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let (word, rest) = text.split_once(' ').unwrap_or((text, ""));
        let rest = rest.trim();
        let needs_text = |what: &str| {
            if rest.is_empty() {
                Err(err(format!("{word} needs {what}")))
            } else {
                Ok(rest.to_string())
            }
        };
        let header = matches!(word, "size" | "env");
        if header && !script.steps.is_empty() {
            return Err(err(format!("{word} must come before the first step")));
        }
        let action = match word {
            "size" => {
                let (c, r) = rest
                    .split_once('x')
                    .and_then(|(c, r)| Some((c.parse().ok()?, r.parse().ok()?)))
                    .filter(|&(c, r): &(u16, u16)| c >= 10 && r >= 2)
                    .ok_or_else(|| err(format!("size wants COLSxROWS, like 160x44: {rest}")))?;
                (script.cols, script.rows) = (c, r);
                None
            }
            "env" => {
                let (k, v) = rest
                    .split_once('=')
                    .filter(|(k, _)| !k.is_empty())
                    .ok_or_else(|| err(format!("env wants NAME=VALUE: {rest}")))?;
                script.env.push((k.to_string(), v.to_string()));
                None
            }
            "keys" => {
                needs_text("keys")?;
                let tokens = rest.split_whitespace().map(|t| match Key::parse(t) {
                    Some(k) => Input::Key(k),
                    None => Input::Text(t.to_string()),
                });
                Some(Action::Send(tokens.collect()))
            }
            "type" => Some(Action::Send(vec![Input::Text(needs_text("text")?)])),
            "wait-for" | "wait-gone" => {
                let (timeout, text) = timeout_and(rest);
                if text.is_empty() {
                    return Err(err(format!("{word} needs the text to wait for")));
                }
                let text = text.to_string();
                Some(if word == "wait-for" {
                    Action::WaitFor { text, timeout }
                } else {
                    Action::WaitGone { text, timeout }
                })
            }
            "wait" => Some(Action::Wait(duration(rest).ok_or_else(|| {
                err(format!("wait wants a duration, like 500ms or 2s: {rest}"))
            })?)),
            "expect" => Some(Action::Expect(needs_text("the text to look for")?)),
            "expect-not" => Some(Action::ExpectNot(needs_text("the text to look for")?)),
            "screen" => Some(Action::Screen),
            "exits" => {
                let (timeout, code) = timeout_and(rest);
                let code = code
                    .parse()
                    .map_err(|_| err(format!("exits wants an exit code: {rest}")))?;
                Some(Action::Exits { code, timeout })
            }
            "click" => {
                let mut nums = rest.split_whitespace().map(|n| n.parse::<u16>().ok());
                match (nums.next(), nums.next(), nums.next()) {
                    (Some(Some(row)), Some(Some(col)), None) if row > 0 && col > 0 => {
                        Some(Action::Click { row, col })
                    }
                    _ => return Err(err(format!("click wants ROW COL (from 1): {rest}"))),
                }
            }
            "drag" => match cells(rest).as_deref() {
                Some(&[r1, c1, r2, c2]) => Some(Action::Drag {
                    from: (r1, c1),
                    to: (r2, c2),
                }),
                _ => {
                    return Err(err(format!(
                        "drag wants ROW COL ROW2 COL2 (from 1): {rest}"
                    )));
                }
            },
            "scroll" => {
                let (dir, at) = rest.split_once(' ').unwrap_or((rest, ""));
                let up = match dir {
                    "up" => true,
                    "down" => false,
                    _ => return Err(err(format!("scroll wants up or down first: {rest}"))),
                };
                match cells(at).as_deref() {
                    Some(&[row, col]) => Some(Action::Scroll {
                        up,
                        at: (row, col),
                        count: 1,
                    }),
                    Some(&[row, col, count]) => Some(Action::Scroll {
                        up,
                        at: (row, col),
                        count,
                    }),
                    _ => {
                        return Err(err(format!(
                            "scroll wants up|down ROW COL [N] (from 1): {rest}"
                        )));
                    }
                }
            }
            "expect-in" => {
                let mut words = rest.splitn(5, ' ');
                let nums: Vec<&str> = words.by_ref().take(4).collect();
                let text = words.next().unwrap_or("").trim();
                match cells(&nums.join(" ")).as_deref() {
                    Some(&[top, left, bottom, right])
                        if !text.is_empty() && top <= bottom && left <= right =>
                    {
                        Some(Action::ExpectIn {
                            top,
                            left,
                            bottom,
                            right,
                            text: text.to_string(),
                        })
                    }
                    _ => {
                        return Err(err(format!(
                            "expect-in wants ROW COL ROW2 COL2 TEXT (from 1, top left first): {rest}"
                        )));
                    }
                }
            }
            "write" => match rest.split_once(' ') {
                Some((file, text)) if Path::new(file).is_relative() => Some(Action::Write {
                    file: file.into(),
                    text: text.replace("\\n", "\n"),
                }),
                _ => {
                    return Err(err(format!(
                        "write wants FILE TEXT, FILE in the workspace: {rest}"
                    )));
                }
            },
            _ => {
                let message = format!("unknown step {word:?}; steps are:");
                let then = Some(SYNTAX);
                return Err(ParseError {
                    line,
                    message,
                    then,
                });
            }
        };
        if let Some(action) = action {
            script.steps.push(Step { line, action });
        }
    }
    Ok(script)
}

/// "3 7" → [3, 7]: numbers from 1, as terminal cells are counted.
fn cells(s: &str) -> Option<Vec<u16>> {
    s.split_whitespace()
        .map(|n| n.parse::<u16>().ok().filter(|&n| n > 0))
        .collect()
}

/// "10s TEXT" → (10 s, "TEXT"); "TEXT" → (default, "TEXT").
fn timeout_and(rest: &str) -> (Duration, &str) {
    match rest.split_once(' ') {
        Some((first, text)) if duration(first).is_some() => (duration(first).unwrap(), text.trim()),
        _ if duration(rest).is_some() => (duration(rest).unwrap(), ""),
        _ => (DEFAULT_TIMEOUT, rest),
    }
}

/// "500ms", "2s", "1.5s".
fn duration(s: &str) -> Option<Duration> {
    let (num, unit) = match s.strip_suffix("ms") {
        Some(n) => (n, 0.001),
        None => (s.strip_suffix('s')?, 1.0),
    };
    let n: f64 = num.parse().ok()?;
    (n >= 0.0 && n.is_finite()).then(|| Duration::from_secs_f64(n * unit))
}

/// Answers the queries a terminal answers: the program waits for them (cursor position above
/// all: ratatui asks for it on `clear()`, and gives up if no answer comes).
#[derive(Default)]
struct Replies(Vec<u8>);

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                let _ = write!(self.0, "\x1b[{};{}R", row + 1, col + 1);
            }
            (None, 'n', 5) => self.0.extend(b"\x1b[0n"),
            (None, 'c', 0) => self.0.extend(b"\x1b[?62;22c"),
            _ => {}
        }
    }
}

struct Session {
    parser: vt100::Parser<Replies>,
    writer: Box<dyn Write + Send>,
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Receiver<Vec<u8>>,
    status: Option<ExitStatus>,
    cols: u16,
}

impl Session {
    fn spawn(
        script: &Script,
        program: &Path,
        args: &[String],
        home: Option<&Path>,
    ) -> Result<Session> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: script.rows,
                cols: script.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("couldn't open a pseudo-terminal")?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(std::env::current_dir()?);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(home) = home {
            cmd.env("HOME", home);
            for (var, dir) in [
                ("XDG_CONFIG_HOME", ".config"),
                ("XDG_DATA_HOME", ".local/share"),
                ("XDG_STATE_HOME", ".local/state"),
                ("XDG_CACHE_HOME", ".cache"),
            ] {
                cmd.env(var, home.join(dir));
            }
        }
        for (k, v) in &script.env {
            cmd.env(k, v);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("couldn't start {}", program.display()))?;
        drop(pair.slave);
        let output = crate::embed::read_in_background(pair.master.try_clone_reader()?);
        let writer = pair.master.take_writer()?;
        let nz = |v: u16| NonZeroU16::new(v).unwrap();
        Ok(Session {
            parser: vt100::Parser::new_with_callbacks(
                nz(script.rows),
                nz(script.cols),
                0,
                Replies::default(),
            ),
            writer,
            _master: pair.master,
            child,
            output,
            status: None,
            cols: script.cols,
        })
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        let replies = std::mem::take(&mut self.parser.callbacks_mut().0);
        if !replies.is_empty() {
            self.send(&replies);
        }
    }

    /// Take the program's output for up to `wait`; notice when it exits.
    fn pump(&mut self, wait: Duration) {
        match self.output.recv_timeout(wait) {
            Ok(bytes) => {
                self.feed(&bytes);
                while let Ok(bytes) = self.output.try_recv() {
                    self.feed(&bytes);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(wait),
        }
        if self.status.is_none() {
            self.status = self.child.try_wait().ok().flatten();
        }
    }

    /// Take the output until the program pauses (for up to a second), so a check reads a whole
    /// frame: a `wait-for` can match a frame of which the terminal has read only the first part.
    fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            match self.output.recv_timeout(FRAME_SETTLE) {
                Ok(bytes) => self.feed(&bytes),
                Err(_) => break,
            }
        }
        self.pump(Duration::ZERO);
    }

    /// Pump until `done` holds or `timeout` passes. False on timeout, or if the program exited
    /// without `done` holding.
    fn until(&mut self, timeout: Duration, mut done: impl FnMut(&Session) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if done(self) {
                return true;
            }
            if self.status.is_some() {
                // Drain what it wrote before exiting, then decide.
                self.pump(Duration::from_millis(50));
                return done(self);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            self.pump(left.min(Duration::from_millis(20)));
        }
    }

    /// A lone Esc followed at once by more bytes reads as Alt plus the next key, so `keys esc q`
    /// would never quit: the program must read the Esc before the next byte lands. Its answer
    /// (any output) shows it has; a slow machine gets up to `ESC_SETTLE` for it. Output still
    /// on its way from an earlier key would pass for that answer, so that is drained first.
    fn send_lone_esc(&mut self) {
        let quiet = Duration::from_millis(30);
        let deadline = Instant::now() + ESC_SETTLE;
        while Instant::now() < deadline {
            match self.output.recv_timeout(quiet) {
                Ok(bytes) => self.feed(&bytes),
                Err(_) => break,
            }
        }
        self.send(b"\x1b");
        let sent = Instant::now();
        if let Ok(bytes) = self.output.recv_timeout(ESC_SETTLE) {
            self.feed(&bytes);
            // On a slow machine the first output can still be a late frame from an earlier key,
            // drawn before the Esc was read. Once the output has paused, and a little time has
            // passed since the Esc, the program is back waiting for input and has read it.
            let floor = sent + Duration::from_millis(100);
            let deadline = sent + ESC_SETTLE;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let wait = Duration::from_millis(50).max(floor.saturating_duration_since(now));
                match self.output.recv_timeout(wait.min(deadline - now)) {
                    Ok(bytes) => self.feed(&bytes),
                    Err(_) if Instant::now() >= floor => break,
                    Err(_) => {}
                }
            }
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    fn text(&self) -> String {
        let rows: Vec<String> = self.parser.screen().rows(0, self.cols).collect();
        rows.join("\n")
    }

    /// Rows `top`..=`bottom`, columns `left`..=`right` (1-based), one line each.
    fn region(&self, top: u16, left: u16, bottom: u16, right: u16) -> String {
        let rows = self.parser.screen().rows(left - 1, right - left + 1);
        let rows: Vec<String> = rows
            .skip(top as usize - 1)
            .take((bottom - top) as usize + 1)
            .collect();
        rows.join("\n")
    }

    /// Send what `events` makes (SGR mouse reports), if the program listens to the mouse.
    fn mouse(&mut self, step: &str, events: impl FnOnce() -> String) -> Option<String> {
        if self.parser.screen().mouse_protocol_mode() == vt100::MouseProtocolMode::None {
            return Some(format!("{step}: the program isn't listening to the mouse"));
        }
        self.send(events().as_bytes());
        None
    }

    fn shows(&self, text: &str) -> bool {
        self.text().contains(text)
    }

    fn print_screen(&self, title: &str) {
        let (rows, cols) = self.parser.screen().size();
        out!("--- {title} ({cols}x{rows}) ---");
        for row in self.text().lines() {
            out!("{}", row.trim_end());
        }
        out!("---");
    }

    fn exited(&self) -> String {
        match &self.status {
            Some(s) => format!(" (the program exited with {})", describe(s)),
            None => String::new(),
        }
    }

    /// Kill the program and everything in its session (it's a session leader in the pty).
    fn kill(&mut self) {
        if self.status.is_some() {
            return;
        }
        if let Some(pid) = self.child.process_id() {
            kill_group(pid);
        }
        let _ = self.child.kill();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.kill();
    }
}

fn kill_group(pid: u32) {
    #[cfg(unix)]
    let _ = std::process::Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{pid}")])
        .stderr(std::process::Stdio::null())
        .status();
}

/// Run the script; `name` labels messages (`name:line: …`). Ok(false) when a step failed, after
/// printing what failed and the screen.
/// A fresh, empty folder to serve as the program's home, removed when the run ends.
struct EmptyHome(PathBuf);

impl EmptyHome {
    fn new() -> Result<EmptyHome> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cold-lava-drive-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("couldn't make an empty home in {}", dir.display()))?;
        Ok(EmptyHome(dir))
    }
}

impl Drop for EmptyHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The home folder the program under test gets.
pub enum HomeChoice {
    /// Yours.
    Real,
    /// An empty one, removed when the run ends.
    Empty,
    /// This folder (made if missing), left as the run leaves it, so a test can look inside.
    At(PathBuf),
}

/// Runs the script. Unless the home is `Real`, the program gets a home folder of its own (and
/// XDG folders inside it), so it neither reads your config nor adds to your list of workspaces.
pub fn run(
    name: &str,
    script: &Script,
    program: &Path,
    args: &[String],
    choice: HomeChoice,
) -> Result<bool> {
    let empty = matches!(choice, HomeChoice::Empty)
        .then(EmptyHome::new)
        .transpose()?;
    let home = match choice {
        HomeChoice::Real => None,
        HomeChoice::Empty => empty.as_ref().map(|h| h.0.clone()),
        HomeChoice::At(dir) => {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("couldn't make the home {}", dir.display()))?;
            Some(dir)
        }
    };
    let mut s = Session::spawn(script, program, args, home.as_deref())?;
    if let Some(pid) = s.child.process_id() {
        let name = name.to_string();
        let home = empty.as_ref().map(|h| h.0.clone());
        std::thread::spawn(move || {
            std::thread::sleep(OVERALL_TIMEOUT);
            told!("{name}: gave up after {}", secs(OVERALL_TIMEOUT));
            kill_group(pid);
            if let Some(home) = home {
                let _ = std::fs::remove_dir_all(home);
            }
            std::process::exit(1);
        });
    }
    for step in &script.steps {
        let failure = match &step.action {
            Action::Send(inputs) => {
                if s.status.is_some() {
                    Some(format!("can't type{}", s.exited()))
                } else {
                    let app_cursor = s.parser.screen().application_cursor();
                    for input in inputs {
                        match input {
                            Input::Key(k) => {
                                let bytes = k.bytes(app_cursor);
                                if bytes == b"\x1b" {
                                    s.send_lone_esc();
                                } else {
                                    s.send(&bytes);
                                }
                            }
                            Input::Text(t) => s.send(t.as_bytes()),
                        }
                    }
                    None
                }
            }
            Action::WaitFor { text, timeout } => (!s.until(*timeout, |s| s.shows(text)))
                .then(|| format!("waited {} for {text:?}{}", secs(*timeout), s.exited())),
            Action::WaitGone { text, timeout } => (!s.until(*timeout, |s| !s.shows(text)))
                .then(|| format!("waited {} for {text:?} to go away", secs(*timeout))),
            Action::Wait(d) => {
                s.until(*d, |_| false);
                None
            }
            Action::Expect(text) => {
                s.settle();
                (!s.shows(text)).then(|| format!("expected {text:?} on the screen{}", s.exited()))
            }
            Action::ExpectNot(text) => {
                s.settle();
                s.shows(text)
                    .then(|| format!("expected no {text:?} on the screen"))
            }
            Action::Screen => {
                s.settle();
                s.print_screen(&format!("{name}:{}", step.line));
                None
            }
            Action::Exits { code, timeout } => {
                s.until(*timeout, |s| s.status.is_some());
                match &s.status {
                    None => Some(format!(
                        "expected the program to exit with {code}, still running after {}",
                        secs(*timeout)
                    )),
                    Some(st) if st.exit_code() != *code || st.signal().is_some() => {
                        Some(format!("expected exit status {code}, got {}", describe(st)))
                    }
                    Some(_) => None,
                }
            }
            Action::Write { file, text } => {
                let workspace = args.last().map(Path::new).unwrap_or(Path::new("."));
                let path = workspace.join(file);
                write_beside(&path, text.as_bytes())
                    .err()
                    .map(|e| format!("couldn't write {}: {e}", path.display()))
            }
            Action::Click { row, col } => s.mouse("click", || {
                format!("\x1b[<0;{col};{row}M\x1b[<0;{col};{row}m")
            }),
            Action::Drag { from, to } => s.mouse("drag", || drag(*from, *to)),
            Action::Scroll { up, at, count } => s.mouse("scroll", || {
                let button = if *up { 64 } else { 65 };
                format!("\x1b[<{button};{};{}M", at.1, at.0).repeat(*count as usize)
            }),
            Action::ExpectIn {
                top,
                left,
                bottom,
                right,
                text,
            } => {
                s.settle();
                (!s.region(*top, *left, *bottom, *right)
                    .contains(text.as_str()))
                .then(|| {
                    format!(
                        "expected {text:?} within rows {top}–{bottom}, columns {left}–{right}{}",
                        s.exited()
                    )
                })
            }
        };
        if let Some(message) = failure {
            told!("{name}:{}: {message}", step.line);
            s.print_screen(&format!("screen at {name}:{}", step.line));
            s.kill();
            return Ok(false);
        }
    }
    s.kill();
    Ok(true)
}

/// Write a workspace file as a program that saves safely does: the whole text into a file
/// beside it, then that file renamed over it. Written in place the file is empty for a moment,
/// and whoever watches it may read it so. The file beside it has a name of this run's own that
/// nothing there has, so no file of the workspace is taken for it, and it is removed when the
/// rename can't be done. A link is written through, as a write in place would: the file it
/// leads to is the one replaced.
fn write_beside(path: &Path, text: &[u8]) -> std::io::Result<()> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let beside = loop {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!(".cold-lava-drive-{}-{n}.tmp", std::process::id());
        let beside = target.with_file_name(name);
        if beside.symlink_metadata().is_err() {
            break beside;
        }
    };
    let written = std::fs::write(&beside, text).and_then(|()| std::fs::rename(&beside, &target));
    if written.is_err() {
        let _ = std::fs::remove_file(&beside);
    }
    written
}

/// A left-button drag as SGR mouse reports: the press, a move through every cell on the way
/// (as a hand would), the release.
fn drag((r1, c1): (u16, u16), (r2, c2): (u16, u16)) -> String {
    let steps = r1.abs_diff(r2).max(c1.abs_diff(c2)).max(1);
    let at = |from: u16, to: u16, i: u16| {
        let (from, to) = (from as i32, to as i32);
        (from + (to - from) * i as i32 / steps as i32) as u16
    };
    let mut out = format!("\x1b[<0;{c1};{r1}M");
    for i in 1..=steps {
        out += &format!("\x1b[<32;{};{}M", at(c1, c2, i), at(r1, r2, i));
    }
    out + &format!("\x1b[<0;{c2};{r2}m")
}

fn describe(status: &ExitStatus) -> String {
    match status.signal() {
        Some(signal) => format!("signal {signal}"),
        None => format!("status {}", status.exit_code()),
    }
}

fn secs(d: Duration) -> String {
    if d.subsec_millis() == 0 {
        format!("{}s", d.as_secs())
    } else {
        format!("{}ms", d.as_millis())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(src: &str) -> Action {
        parse(src).unwrap().steps.remove(0).action
    }

    #[test]
    fn write_replaces_a_file_in_the_workspace() {
        assert_eq!(
            step("write Data/a.csv x,y\\n1,2\\n"),
            Action::Write {
                file: "Data/a.csv".into(),
                text: "x,y\n1,2\n".into()
            }
        );
        assert!(parse("write /etc/a x").is_err());
        assert!(parse("write a.csv").is_err());
    }

    #[test]
    fn keys_are_key_names_or_text() {
        assert_eq!(
            step("keys p kilau enter ctrl+q"),
            Action::Send(vec![
                Input::Key(Key::parse("p").unwrap()),
                Input::Text("kilau".into()),
                Input::Key(Key::parse("enter").unwrap()),
                Input::Key(Key::parse("ctrl+q").unwrap()),
            ])
        );
        assert_eq!(
            step("type  some text  with spaces"),
            Action::Send(vec![Input::Text("some text  with spaces".into())])
        );
    }

    #[test]
    fn waits_take_an_optional_timeout() {
        assert_eq!(
            step("wait-for Mauna Loa"),
            Action::WaitFor {
                text: "Mauna Loa".into(),
                timeout: Duration::from_secs(5)
            }
        );
        assert_eq!(
            step("wait-gone 10s Neovim"),
            Action::WaitGone {
                text: "Neovim".into(),
                timeout: Duration::from_secs(10)
            }
        );
        assert_eq!(step("wait 500ms"), Action::Wait(Duration::from_millis(500)));
        assert_eq!(
            step("exits 0"),
            Action::Exits {
                code: 0,
                timeout: Duration::from_secs(5)
            }
        );
        assert_eq!(
            step("exits 1.5s 3"),
            Action::Exits {
                code: 3,
                timeout: Duration::from_millis(1500)
            }
        );
    }

    #[test]
    fn header_comments_and_line_numbers() {
        let s =
            parse("# demo\nsize 80x24\nenv A=b=c\n\nexpect hi\n  screen  \nclick 3 7\n").unwrap();
        assert_eq!((s.cols, s.rows), (80, 24));
        assert_eq!(s.env, [("A".to_string(), "b=c".to_string())]);
        let lines: Vec<usize> = s.steps.iter().map(|s| s.line).collect();
        assert_eq!(lines, [5, 6, 7]);
        assert_eq!(s.steps[2].action, Action::Click { row: 3, col: 7 });
    }

    #[test]
    fn mouse_steps() {
        assert_eq!(
            step("drag 3 7 5 8"),
            Action::Drag {
                from: (3, 7),
                to: (5, 8)
            }
        );
        assert_eq!(
            step("scroll down 10 20 3"),
            Action::Scroll {
                up: false,
                at: (10, 20),
                count: 3
            }
        );
        assert_eq!(
            step("scroll up 10 20"),
            Action::Scroll {
                up: true,
                at: (10, 20),
                count: 1
            }
        );
        assert_eq!(
            drag((3, 7), (5, 8)),
            "\x1b[<0;7;3M\x1b[<32;7;4M\x1b[<32;8;5M\x1b[<0;8;5m"
        );
        assert_eq!(
            step("expect-in 2 3 20 80 Start  Here"),
            Action::ExpectIn {
                top: 2,
                left: 3,
                bottom: 20,
                right: 80,
                text: "Start  Here".into()
            }
        );
        for bad in [
            "drag 1 2 3",
            "drag 0 1 2 3",
            "scroll sideways 1 2",
            "scroll up 1",
            "expect-in 1 2 3 4",
            "expect-in 5 1 2 4 x",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn mistakes_name_their_line() {
        let line = |src: &str| parse(src).unwrap_err().line;
        assert_eq!(line("expect a\nsize 80x24"), 2);
        assert_eq!(line("\n\nfrobnicate"), 3);
        assert_eq!(line("size big"), 1);
        assert_eq!(line("wait soon"), 1);
        assert_eq!(line("expect"), 1);
        assert_eq!(line("wait-for 2s"), 1);
        assert_eq!(line("exits zero"), 1);
        assert_eq!(line("click 0 4"), 1);
    }

    #[test]
    fn key_bytes() {
        let bytes = |k: &str, app| Key::parse(k).unwrap().bytes(app);
        assert_eq!(bytes("ctrl+q", false), [0x11]);
        assert_eq!(bytes("enter", false), b"\r");
        assert_eq!(bytes("esc", false), [0x1b]);
        assert_eq!(bytes("shift+tab", false), b"\x1b[Z");
        assert_eq!(bytes("up", false), b"\x1b[A");
        assert_eq!(bytes("up", true), b"\x1bOA");
        assert_eq!(bytes("pageup", false), b"\x1b[5~");
        assert_eq!(bytes("alt+x", false), b"\x1bx");
        assert_eq!(bytes("G", false), b"G");
    }

    /// The program reads the Esc, takes 300 ms to answer, then looks for input already waiting:
    /// a `q` sent before the answer would have arrived with the Esc.
    #[cfg(unix)]
    #[test]
    fn waits_for_the_answer_to_a_lone_esc() {
        let program = "stty raw -echo; dd bs=1 count=1 >/dev/null 2>&1; sleep 0.3; \
            stty min 0 time 0; n=$(dd bs=1 count=1 2>/dev/null | wc -c); \
            if [ $n -eq 0 ]; then echo APART; else echo MERGED; fi; sleep 5";
        let script = parse("wait 300ms\nkeys esc q\nwait-for APART").unwrap();
        let args = ["-c".to_string(), program.to_string()];
        assert!(
            run(
                "esc",
                &script,
                Path::new("/bin/sh"),
                &args,
                HomeChoice::Empty
            )
            .unwrap()
        );
    }

    #[test]
    fn answers_the_cursor_position_query() {
        let mut p = vt100::Parser::new_with_callbacks(
            NonZeroU16::new(5).unwrap(),
            NonZeroU16::new(20).unwrap(),
            0,
            Replies::default(),
        );
        p.process(b"\x1b[3;7H\x1b[6n\x1b[c\x1b[0m");
        assert_eq!(p.callbacks().0, b"\x1b[3;7R\x1b[?62;22c");
    }
}
