mod app;
mod board;
mod embed;
mod graph;
mod groups;
mod home;
mod kitty;
mod scene;
mod term;
mod vault;

use anyhow::Result;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use std::io::stdout;
use std::path::PathBuf;

const USAGE: &str = "cold-lava — Obsidian's graph view and canvas, in the terminal

Usage:
  cold-lava                           main menu: your vaults, their notes, and Neovim
  cold-lava VAULT_DIR                 main menu with that vault open
  cold-lava graph [VAULT_DIR]         go straight to the graph view
  cold-lava board [FILE.canvas|DIR]   whiteboard (Obsidian Canvas); no file = picker

Options:
  --braille   force braille rendering
  --pixels    force pixel rendering (Kitty graphics protocol)
  -h, --help  this help

The main menu finds the vaults you have in Obsidian automatically. Notes open in an
embedded Neovim when it's installed ($VISUAL/$EDITOR, or nvim on your PATH).
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

    match args.first().map(String::as_str) {
        Some("board") => {
            let target = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
            let root = board::find_vault_root(&target);
            let file = (target.extension().is_some_and(|e| e == "canvas")).then_some(target);
            with_terminal(|t| board::launch(t, &root, file, false))
        }
        Some("graph") => {
            let root = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
            let vault = vault::scan(&root)?;
            if vault.notes.is_empty() {
                anyhow::bail!("no .md notes found under {}", vault.root.display());
            }
            let mut app = app::App::new(vault, render);
            with_terminal(|t| app.run(t))
        }
        Some(dir) => {
            let path = PathBuf::from(dir);
            if !path.is_dir() {
                anyhow::bail!("not a folder: {dir}\n\n{USAGE}");
            }
            let mut home = home::Home::new(render, Some(path));
            with_terminal(|t| home.run(t))
        }
        None => {
            let mut home = home::Home::new(render, None);
            with_terminal(|t| home.run(t))
        }
    }
}

fn with_terminal(f: impl FnOnce(&mut ratatui::DefaultTerminal) -> Result<()>) -> Result<()> {
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    let res = f(&mut terminal);
    execute!(stdout(), DisableMouseCapture)?;
    ratatui::restore();
    res
}
