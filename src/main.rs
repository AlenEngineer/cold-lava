mod app;
mod board;
mod graph;
mod groups;
mod kitty;
mod scene;
mod term;
mod vault;

use anyhow::Result;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use std::io::stdout;
use std::path::PathBuf;

const USAGE: &str = "vaultgraph — Obsidian's graph view and canvas, in the terminal

Usage:
  vaultgraph [VAULT_DIR]              graph view (default: current directory)
  vaultgraph board [FILE.canvas|DIR]  whiteboard (Obsidian Canvas); no file = picker

Options:
  --braille   force braille rendering
  --pixels    force pixel rendering (Kitty graphics protocol)
  -h, --help  this help

Press ? inside the app for keys.";

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    let render = if args.iter().any(|a| a == "--braille") {
        app::Render::Braille
    } else if args.iter().any(|a| a == "--pixels") {
        app::Render::Pixels
    } else {
        app::Render::Auto
    };
    args.retain(|a| !a.starts_with("--"));

    if args.first().map(String::as_str) == Some("board") {
        let target = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
        let root = board::find_vault_root(&target);
        let file = (target.extension().is_some_and(|e| e == "canvas")).then_some(target);
        return with_terminal(|t| board::launch(t, &root, file, false));
    }

    let root = PathBuf::from(args.first().cloned().unwrap_or_else(|| ".".into()));
    let vault = vault::scan(&root)?;
    if vault.notes.is_empty() {
        anyhow::bail!("no .md notes found under {}", vault.root.display());
    }
    let mut app = app::App::new(vault, render);
    with_terminal(|t| app.run(t))
}

fn with_terminal(f: impl FnOnce(&mut ratatui::DefaultTerminal) -> Result<()>) -> Result<()> {
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    let res = f(&mut terminal);
    execute!(stdout(), DisableMouseCapture)?;
    ratatui::restore();
    res
}
