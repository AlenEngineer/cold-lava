mod app;
mod board;
mod commands;
mod config;
mod core;
mod daily;
mod embed;
mod ext;
mod graph;
mod groups;
mod home;
mod index;
mod kitty;
mod markdown;
mod reader;
mod rename;
mod scene;
mod search;
mod tasks;
mod term;
mod vault;
mod write;

use anyhow::Result;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use std::io::stdout;
use std::path::{Path, PathBuf};

const USAGE: &str = "cold-lava — Obsidian's graph view and canvas, in the terminal

Usage:
  cold-lava                           main menu: your vaults, their notes, and Neovim
  cold-lava VAULT_DIR                 main menu with that vault open
  cold-lava graph [VAULT_DIR]         go straight to the graph view
  cold-lava board [FILE.canvas|DIR]   whiteboard (Obsidian Canvas); no file = picker
  cold-lava daily VAULT_DIR [DATE]    open a day's daily note (created from your template)
  cold-lava agenda VAULT_DIR          dated tasks and daily notes, for other programs
  cold-lava search VAULT_DIR WORDS…   full-text search (words, \"quoted phrases\")
  cold-lava task toggle FILE:LINE     tick / untick a task, like Obsidian Tasks
  cold-lava rename VAULT_DIR NOTE NEW rename or move a note, updating every link to it
  cold-lava keys                      every command and its keys; checks config.toml

Options:
  --braille            force braille rendering
  --pixels             force pixel rendering (Kitty graphics protocol)
  --from / --to DATE   agenda range (YYYY-MM-DD; default: this month)
  --json               machine-readable output (agenda, task toggle)
  --id ID              task toggle: only if the line still matches this id from the agenda
  --path               daily: create the note if needed and print its path, no UI
  --limit N            search: matching lines shown per note (default 5)
  --dry-run            rename: show what would change, change nothing
  -V, --version        print the version
  -h, --help           this help

DATE is YYYY-MM-DD, today, tomorrow or yesterday. Press ? inside the app for keys.
Keys are set by command id under [keys] in ~/.config/cold-lava/config.toml.";

/// Command-line arguments: positional words, `--flag value` options and `--switch`es.
struct Args {
    words: Vec<String>,
    values: std::collections::HashMap<String, String>,
    switches: std::collections::HashSet<String>,
}

impl Args {
    fn parse() -> Result<Args> {
        const WITH_VALUE: &[&str] = &["from", "to", "id", "limit"];
        let mut a = Args {
            words: Vec::new(),
            values: Default::default(),
            switches: Default::default(),
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            if let Some(flag) = arg.strip_prefix("--") {
                if let Some((k, v)) = flag.split_once('=') {
                    a.values.insert(k.to_string(), v.to_string());
                } else if WITH_VALUE.contains(&flag) {
                    let v = it
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--{flag} needs a value"))?;
                    a.values.insert(flag.to_string(), v);
                } else {
                    a.switches.insert(flag.to_string());
                }
            } else if arg == "-h" {
                a.switches.insert("help".into());
            } else if arg == "-V" {
                a.switches.insert("version".into());
            } else {
                a.words.push(arg);
            }
        }
        Ok(a)
    }

    fn word(&self, i: usize) -> Option<&str> {
        self.words.get(i).map(String::as_str)
    }

    fn has(&self, s: &str) -> bool {
        self.switches.contains(s)
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("cold-lava: {e:#}");
        // Exit code 3 means "the task line changed": callers can refresh and retry.
        let code = if e.downcast_ref::<tasks::Changed>().is_some() {
            3
        } else {
            1
        };
        std::process::exit(code);
    }
}

fn run() -> Result<()> {
    let args = Args::parse()?;
    if args.has("help") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.has("version") {
        println!("cold-lava {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let render = if args.has("braille") {
        app::Render::Braille
    } else if args.has("pixels") {
        app::Render::Pixels
    } else {
        app::Render::Auto
    };
    let json = args.has("json");
    let registry = ext::Registry::builtin();
    config::load(registry.commands());

    match args.word(0) {
        Some("keys") => {
            print_keys("menu", registry.commands());
            print_keys("graph", &commands::entries(commands::GRAPH));
            print_keys("whiteboard", &commands::entries(commands::BOARD));
            print_keys("reader", &commands::entries(commands::READER));
            print_keys("pane", &commands::entries(commands::PANE));
            let errors = &config::get().errors;
            if !errors.is_empty() {
                let path = config::path().unwrap_or_default();
                eprintln!("{}:", path.display());
                for e in errors {
                    eprintln!("  {}", e.replace('\n', "\n  "));
                }
            }
            if !registry.refused().is_empty() {
                eprintln!("extensions:");
                for e in registry.refused() {
                    eprintln!("  {e}");
                }
            }
            if !errors.is_empty() || !registry.refused().is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
        Some("board") => {
            let target = PathBuf::from(args.word(1).unwrap_or("."));
            let root = board::find_vault_root(&target);
            let file = (target.extension().is_some_and(|e| e == "canvas")).then_some(target);
            with_terminal(|t| board::launch(t, &root, file, false))
        }
        Some("graph") => {
            let root = PathBuf::from(args.word(1).unwrap_or("."));
            let core = core::Core::open(&root)?;
            if core.index.vault.notes.is_empty() {
                anyhow::bail!("no .md notes found under {}", core.root().display());
            }
            let mut app = app::App::new(core, render);
            with_terminal(|t| app.run(t))
        }
        Some("agenda") => {
            let root = PathBuf::from(args.word(1).unwrap_or("."));
            let today = tasks::today();
            let from = match args.values.get("from") {
                Some(d) => daily::parse_cli_date(d)?,
                None => tasks::first_of_month(today),
            };
            let to = match args.values.get("to") {
                Some(d) => daily::parse_cli_date(d)?,
                None => tasks::last_of_month(from),
            };
            if to < from {
                anyhow::bail!("--to is before --from");
            }
            let agenda = tasks::agenda(&root, from, to)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&agenda)?);
            } else {
                print_agenda(&agenda);
            }
            Ok(())
        }
        Some("search") => {
            let root = PathBuf::from(args.word(1).unwrap_or("."));
            let query = args.words.get(2..).map(|w| w.join(" ")).unwrap_or_default();
            if query.trim().is_empty() {
                anyhow::bail!("usage: cold-lava search VAULT_DIR WORDS… [--json]");
            }
            let limit = match args.values.get("limit") {
                Some(n) => n
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--limit needs a number"))?,
                None => 5,
            };
            let ix = index::Index::build(vault::scan(&root)?);
            let hits = search::search(&ix, &query, limit);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&search::to_json(&ix, &hits))?
                );
            } else {
                for h in &hits {
                    let n = &ix.vault.notes[h.note];
                    for l in &h.lines {
                        println!("{}.md:{}: {}", n.rel, l.line, l.text.trim());
                    }
                    if h.lines.is_empty() {
                        println!("{}.md: (name matches)", n.rel);
                    }
                }
                if hits.is_empty() {
                    eprintln!("no matches");
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        Some("rename") => {
            let (Some(root), Some(note), Some(new)) = (args.word(1), args.word(2), args.word(3))
            else {
                anyhow::bail!(
                    "usage: cold-lava rename VAULT_DIR NOTE NEW [--dry-run] [--json]\n\
                     NOTE is a note's name or path; NEW is a new name (same folder), \
                     a folder ending in / (keeps the name), or a full path"
                );
            };
            let ix = index::Index::build(vault::scan(Path::new(root))?);
            let i = ix
                .by_rel(note)
                .or_else(|| ix.resolver.resolve(note))
                .ok_or_else(|| anyhow::anyhow!("no note called {note}"))?;
            let to = rename::target_rel(&format!("{}.md", ix.vault.notes[i].rel), new)?;
            let plan = rename::plan(&ix, i, &to)?;
            let dry = args.has("dry-run");
            if !dry {
                rename::apply(&plan)?;
            }
            if json {
                let out = serde_json::json!({ "applied": !dry, "plan": plan });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let verb = if dry { "would rename" } else { "renamed" };
                println!("{verb} {} → {}", plan.from, plan.to);
                for e in &plan.edits {
                    for c in &e.changes {
                        println!("  {}:{}  {}", e.file, c.line, c.after.trim());
                    }
                }
                println!("{} links in {} files", plan.links(), plan.files());
                for w in &plan.warnings {
                    eprintln!("warning: {w}");
                }
            }
            Ok(())
        }
        Some("daily") => {
            let root = board::find_vault_root(&PathBuf::from(args.word(1).unwrap_or(".")));
            let date = daily::parse_cli_date(args.word(2).unwrap_or("today"))?;
            let path = daily::ensure(&root, date)?;
            if args.has("path") {
                println!("{}", path.display());
                return Ok(());
            }
            let mut home = home::Home::new(render, Some(root), registry);
            home.focus_note(&path);
            with_terminal(|t| home.run(t))
        }
        Some("task") => {
            if args.word(1) != Some("toggle") {
                anyhow::bail!("usage: cold-lava task toggle FILE:LINE [--id ID] [--json]");
            }
            let spec = args
                .word(2)
                .ok_or_else(|| anyhow::anyhow!("usage: cold-lava task toggle FILE:LINE"))?;
            let (file, line) = tasks::locate(spec)?;
            let result = tasks::toggle(
                &file,
                line,
                args.values.get("id").map(String::as_str),
                tasks::today(),
            );
            match (result, json) {
                (Ok(r), true) => println!("{}", serde_json::to_string_pretty(&r)?),
                (Ok(r), false) => {
                    println!("{}:{}  {}", r.path, r.line, r.raw.trim());
                    if let Some(n) = &r.next {
                        println!("next occurrence on line {}: {}", n.line, n.raw.trim());
                    }
                    if let Some(w) = &r.warning {
                        eprintln!("warning: {w}");
                    }
                }
                (Err(e), true) => {
                    let changed = e.downcast_ref::<tasks::Changed>().is_some();
                    println!(
                        "{}",
                        serde_json::json!({"error": format!("{e:#}"), "changed": changed})
                    );
                    return Err(e);
                }
                (Err(e), false) => return Err(e),
            }
            Ok(())
        }
        Some(dir) => {
            let path = PathBuf::from(dir);
            if !path.is_dir() {
                anyhow::bail!("not a folder or command: {dir}\n\n{USAGE}");
            }
            let mut home = home::Home::new(render, Some(path), registry);
            with_terminal(|t| home.run(t))
        }
        None => {
            let mut home = home::Home::new(render, None, registry);
            with_terminal(|t| home.run(t))
        }
    }
}

fn print_keys<A: Copy>(screen: &str, table: &[commands::Entry<A>]) {
    let keys = commands::Keymap::configured(table);
    let id_width = table.iter().map(|c| c.id.len()).max().unwrap_or(0);
    let labels: Vec<String> = table.iter().map(|c| keys.label(&c.id)).collect();
    let key_width = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    println!("{screen}");
    for (c, label) in table.iter().zip(&labels) {
        let label = if label.is_empty() { "-" } else { label };
        println!("  {:id_width$}  {label:key_width$}  {}", c.id, c.title);
    }
    println!();
}

fn print_agenda(a: &tasks::Agenda) {
    println!("{} · {} to {}", a.vault, a.from, a.to);
    if a.days.is_empty() {
        println!("nothing dated in this range");
    }
    for (day, info) in &a.days {
        let note = if info.daily_note {
            "  (daily note)"
        } else {
            ""
        };
        println!("\n{day}{note}");
        for t in a
            .tasks
            .iter()
            .filter(|t| t.date.as_deref() == Some(day.as_str()))
        {
            let mark = match t.status {
                "done" => "x",
                "cancelled" => "-",
                "in_progress" => "/",
                _ => " ",
            };
            println!("  [{mark}] {}   {}:{}", t.text, t.file, t.line);
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
