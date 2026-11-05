//! Terminal helpers shared by the graph view and the whiteboard.

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use std::io::{Write, stdout};
use std::path::Path;
use std::process::{Command, Stdio};

/// Leave the TUI, run `f` (e.g. an editor), and come back.
pub fn suspend<T>(terminal: &mut DefaultTerminal, f: impl FnOnce() -> T) -> Result<T> {
    execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    let out = f();
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    terminal.clear()?;
    Ok(out)
}

/// Open a file in $VISUAL / $EDITOR (default: nvim). Returns an error message on failure.
pub fn edit_file(
    terminal: &mut DefaultTerminal,
    path: &Path,
    cwd: &Path,
) -> Result<Option<String>> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "nvim".into());
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or("vi").to_string();
    let args: Vec<String> = parts.map(String::from).collect();
    let status = suspend(terminal, || {
        Command::new(&prog)
            .args(&args)
            .arg(path)
            .current_dir(cwd)
            .status()
    })?;
    Ok(status
        .err()
        .map(|e| format!("couldn't run {prog}: {e} (set $EDITOR)")))
}

/// Open a note in the Obsidian app via its URI scheme.
pub fn open_in_obsidian(vault_name: &str, rel_path: &str) -> bool {
    let url = format!(
        "obsidian://open?vault={}&file={}",
        url_encode(vault_name),
        url_encode(rel_path)
    );
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(&url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

pub fn write_raw(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let mut out = stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
