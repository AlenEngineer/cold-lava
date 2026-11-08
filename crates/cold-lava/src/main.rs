mod agenda;
mod app;
mod board;
mod commands;
mod config;
mod drive;
mod embed;
mod ext;
mod graph;
mod groups;
mod hints;
mod history;
mod home;
mod kitty;
mod list;
mod looks;
mod reader;
mod scene;
mod table;
mod term;
mod tidy_view;
mod type_table;
mod whichkey;

use cold_lava_engine::{
    bookmarks, core, daily, edit, frontmatter, index, items, markdown, merge, paths, rename,
    search, tasks, templates, tidy, trash, types, workspace, write,
};

use anyhow::Result;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use std::collections::HashMap;
use std::io::{Write, stdout};
use std::path::{Path, PathBuf};

const USAGE: &str = "cold-lava — Obsidian's graph view and canvas, in the terminal

Usage:
  cold-lava                           main menu: your workspaces, their notes, and Neovim
  cold-lava WORKSPACE                 main menu with that workspace open
  cold-lava graph [WORKSPACE]         go straight to the graph view
  cold-lava board [FILE.canvas|DIR]   whiteboard (Obsidian Canvas); no file = picker
  cold-lava daily WORKSPACE [DATE]    open a day's daily note (created from your template)
  cold-lava agenda WORKSPACE          dated tasks and daily notes, for other programs
  cold-lava items WORKSPACE KIND      the workspace's tasks, lists, headings, blocks or a type's
                                      items, one per line
  cold-lava search WORKSPACE WORDS…   full-text search (words, \"quoted phrases\")
  cold-lava task toggle FILE:LINE     tick / untick a task, like Obsidian Tasks
  cold-lava item set FILE ID FIELD VALUE
                                      set one field of a typed item (ID from items --json),
                                      refused unless the note reads back as asked
  cold-lava rename WORKSPACE NOTE NEW rename or move a note or folder, updating every link
  cold-lava table WORKSPACE TYPE      a type's items as the table view shows them
  cold-lava tidy WORKSPACE            orphan notes, unused attachments, empty folders and
                                      unresolved links (read-only)
  cold-lava keys                      every command and its keys; checks config.toml
  cold-lava types [WORKSPACE]         every type: where it comes from, what it found, its
                                      shapes and fields;
                                      checks [types] in config.toml and .cold-lava/types.toml
  cold-lava drive [SCRIPT] [-- ARGS…] run cold-lava ARGS in a hidden terminal, typing and
                                      checking the screen as SCRIPT says (stdin if none),
                                      with an empty home unless --real-home
                                      (or --home DIR: that folder, kept)

Options:
  --braille            force braille rendering
  --pixels             force pixel rendering (Kitty graphics protocol)
  --from / --to DATE   agenda range (YYYY-MM-DD; default: this month)
  --json               machine-readable output (agenda, items, types, task toggle, item set,
                       tidy, table: the view's own JSON)
  --id ID              task toggle: only if the line still matches this id from the agenda
  --rev REV            item set: only if the item is still this version (its rev)
  --remove             item set: take the field's value out instead of setting one
  --path               daily: create the note if needed and print its path, no UI
  --limit N            search: matching lines shown per note (default 5)
  --dry-run            rename, item set: show what would change, change nothing
  --sort FIELD         table: sort by FIELD (-FIELD: descending), as s and S do
  --no-backlinks       tidy: also list notes nothing links to that do link elsewhere
  -V, --version        print the version
  -h, --help           this help

DATE is YYYY-MM-DD, today, tomorrow or yesterday. Press ? inside the app for keys.
Keys are set by command id under [keys] in ~/.config/cold-lava/config.toml.
COLD_LAVA_EDITOR=none opens notes in the reader even when Neovim is installed.";

/// Command-line arguments: positional words, `--flag value` options and `--switch`es.
struct Args {
    words: Vec<String>,
    values: std::collections::HashMap<String, String>,
    switches: std::collections::HashSet<String>,
}

impl Args {
    fn parse() -> Result<Args> {
        const WITH_VALUE: &[&str] = &["from", "to", "id", "limit", "rev", "sort"];
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
        // Exit code 3 means "the task line or item changed": callers can refresh and retry.
        let changed_item = matches!(
            e.downcast_ref::<edit::EditError>(),
            Some(edit::EditError::Changed(_))
        );
        let code = if e.downcast_ref::<tasks::Changed>().is_some() || changed_item {
            3
        } else {
            1
        };
        std::process::exit(code);
    }
}

fn run() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("drive") {
        return drive();
    }
    let args = Args::parse()?;
    if args.has("help") {
        println!("{USAGE}\n\nA drive script has one step per line:");
        for line in drive::SYNTAX.lines() {
            println!("  {line}");
        }
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
    let mut layers: Vec<types::Layer> = registry
        .manifests()
        .map(|m| types::Layer::extension(&m.name, &m.types))
        .collect();
    if let Some(path) = config::path() {
        layers.push(types::Layer::config(&path, config::get().types.clone()));
    }
    types::set_configured(layers);

    match args.word(0) {
        Some("keys") => {
            print_keys("menu", registry.commands());
            print_keys("graph", &commands::entries(commands::GRAPH));
            print_keys("whiteboard", &commands::entries(commands::BOARD));
            print_keys("reader", &commands::entries(commands::READER));
            print_keys("pane", &commands::entries(commands::PANE));
            let errors = &config::get().errors;
            let warnings = &config::get().warnings;
            if !errors.is_empty() || !warnings.is_empty() {
                let path = config::path().unwrap_or_default();
                eprintln!("{}:", path.display());
                for e in errors.iter().chain(warnings) {
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
        Some("types") => {
            let root = match args.word(1) {
                Some(dir) => PathBuf::from(dir),
                None => workspace::find_workspace_root(Path::new(".")),
            };
            let found = types::for_workspace(&root);
            if json {
                println!("{}", serde_json::to_string_pretty(&found)?);
            } else {
                let counts = workspace::scan(&root).ok().map(|v| type_counts(&v));
                print_types(&found, counts.as_ref());
                if !found.errors.is_empty() {
                    eprintln!("errors:");
                    for e in &found.errors {
                        eprintln!("  {e}");
                    }
                }
            }
            if !found.errors.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
        Some("board") => {
            let target = PathBuf::from(args.word(1).unwrap_or("."));
            let root = workspace::find_workspace_root(&target);
            let file = (target.extension().is_some_and(|e| e == "canvas")).then_some(target);
            with_terminal(|t| board::launch(t, &root, file, false))
        }
        Some("graph") => {
            let root = PathBuf::from(args.word(1).unwrap_or("."));
            let core = core::Core::open(&root)?;
            if core.index.workspace.notes.is_empty() {
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
            let agenda = tasks::agenda(&workspace::scan(&root)?, from, to);
            if json {
                println!("{}", serde_json::to_string_pretty(&agenda)?);
            } else {
                print_agenda(&agenda);
            }
            Ok(())
        }
        Some("items") => {
            let (Some(root), Some(kind)) = (args.word(1), args.word(2)) else {
                anyhow::bail!(
                    "usage: cold-lava items WORKSPACE KIND [--json]\nKIND is one of: {} or a type",
                    types::BUILT_IN
                        .iter()
                        .chain(items::KINDS)
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            };
            let workspace = workspace::scan(Path::new(root))?;
            let settings = workspace.item_settings();
            if !items::KINDS.contains(&kind) && !settings.matcher.names().any(|n| n == kind) {
                let kinds: Vec<&str> = types::BUILT_IN
                    .iter()
                    .chain(items::KINDS)
                    .copied()
                    .chain(
                        settings
                            .matcher
                            .names()
                            .filter(|n| !types::BUILT_IN.contains(n)),
                    )
                    .collect();
                anyhow::bail!(
                    "no item kind called {kind}; KIND is one of: {}",
                    kinds.join(", ")
                );
            }
            let mut out = std::io::stdout().lock();
            for note in &workspace.notes {
                for item in workspace
                    .items(note)
                    .iter()
                    .filter(|i| i.data.answers_to(kind))
                {
                    let written = if json {
                        writeln!(out, "{}", serde_json::to_string(item)?)
                    } else {
                        writeln!(out, "{}:{}  {}", item.note, item.line, item_line(item))
                    };
                    match written {
                        // The reader stopped early (`| head`): that's not an error.
                        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
                        r => r?,
                    }
                }
            }
            Ok(())
        }
        Some("table") => {
            let (Some(root), Some(ty)) = (args.word(1), args.word(2)) else {
                anyhow::bail!("usage: cold-lava table WORKSPACE TYPE [--sort FIELD] [--json]");
            };
            let workspace = workspace::scan(Path::new(root))?;
            let Some(built) = type_table::build(&workspace, ty) else {
                let names: Vec<String> = type_table::types(&workspace)
                    .into_iter()
                    .map(|(n, _)| n)
                    .collect();
                anyhow::bail!("no type called {ty}; there are: {}", names.join(", "));
            };
            let mut view = table::TableView::new(built.table);
            if let Some(field) = args.values.get("sort") {
                let (name, desc) = match field.strip_prefix('-') {
                    Some(name) => (name, true),
                    None => (field.as_str(), false),
                };
                let Some(c) = view.columns().iter().position(|c| c.name == name) else {
                    anyhow::bail!("{ty} has no field {name}");
                };
                view.sort_by(c, desc);
            }
            let mut out = std::io::stdout().lock();
            let written = match json {
                true => writeln!(out, "{}", serde_json::to_string_pretty(&view.sorted())?),
                false => view
                    .text()
                    .iter()
                    .try_for_each(|l| writeln!(out, "{}", l.trim_end())),
            };
            match written {
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
                r => Ok(r?),
            }
        }
        Some("tidy") => {
            let root = PathBuf::from(args.word(1).unwrap_or("."));
            let ix = index::Index::build(workspace::scan(&root)?);
            let report = tidy::report(&ix);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_tidy(&report, args.has("no-backlinks"));
            }
            Ok(())
        }
        Some("search") => {
            let root = PathBuf::from(args.word(1).unwrap_or("."));
            let query = args.words.get(2..).map(|w| w.join(" ")).unwrap_or_default();
            if query.trim().is_empty() {
                anyhow::bail!("usage: cold-lava search WORKSPACE WORDS… [--json]");
            }
            let limit = match args.values.get("limit") {
                Some(n) => n
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--limit needs a number"))?,
                None => 5,
            };
            let ix = index::Index::build(workspace::scan(&root)?);
            let hits = search::search(&ix, &query, limit);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&search::to_json(&ix, &hits))?
                );
            } else {
                for h in &hits {
                    let n = &ix.workspace.notes[h.note];
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
                    "usage: cold-lava rename WORKSPACE NOTE NEW [--dry-run] [--json]\n\
                     NOTE is a note's name or path, or a folder; NEW is a new name (same \
                     folder), a folder ending in / (keeps the name), or a full path"
                );
            };
            let ix = index::Index::build(workspace::scan(Path::new(root))?);
            let folder = note.trim_matches('/');
            let plan = if !folder.is_empty() && ix.workspace.root.join(folder).is_dir() {
                rename::plan_folder(&ix, folder, &rename::folder_target(folder, new)?)?
            } else {
                let i = ix
                    .by_rel(note)
                    .or_else(|| ix.resolver.resolve(note))
                    .ok_or_else(|| anyhow::anyhow!("no note or folder called {note}"))?;
                let to = rename::target_rel(&format!("{}.md", ix.workspace.notes[i].rel), new)?;
                rename::plan(&ix, i, &to)?
            };
            let dry = args.has("dry-run");
            if !dry {
                rename::apply(&plan)?;
            }
            if json {
                let out = serde_json::json!({ "applied": !dry, "plan": plan });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let verb = if dry { "would rename" } else { "renamed" };
                for m in &plan.moves {
                    println!("{verb} {} → {}", m.from, m.to);
                }
                for e in &plan.edits {
                    for c in &e.changes {
                        println!("  {}:{}  {}", e.file, c.line, c.after.trim());
                    }
                }
                for c in plan.bookmarks.iter().flat_map(|b| &b.changes) {
                    println!("  bookmark  {} → {}", c.before, c.after);
                }
                println!("{} links in {} files", plan.links(), plan.files());
                for w in &plan.warnings {
                    eprintln!("warning: {w}");
                }
            }
            Ok(())
        }
        Some("daily") => {
            let root = workspace::find_workspace_root(&PathBuf::from(args.word(1).unwrap_or(".")));
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
        Some("item") => {
            const USAGE: &str = "usage: cold-lava item set FILE ID FIELD (VALUE | --remove) \
                                 [--rev REV] [--dry-run] [--json]";
            let (Some("set"), Some(file), Some(id), Some(field)) =
                (args.word(1), args.word(2), args.word(3), args.word(4))
            else {
                anyhow::bail!(USAGE);
            };
            let value = match (args.word(5), args.has("remove")) {
                (Some(v), false) => serde_json::Value::String(v.to_string()),
                (None, true) => serde_json::Value::Null,
                _ => anyhow::bail!(USAGE),
            };
            let path = Path::new(file)
                .canonicalize()
                .map_err(|e| anyhow::anyhow!("{file}: {e}"))?;
            let root = workspace::find_workspace_root(&path);
            let note = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let req = edit::SetField {
                id,
                rev: args.values.get("rev").map(String::as_str),
                field,
                value,
            };
            let dry = args.has("dry-run");
            match (edit::set_field(&root, &note, &req, dry), json) {
                (Ok(set), true) => {
                    let out = serde_json::json!({
                        "applied": set.changed && !dry,
                        "changed": set.changed,
                        "item": set.item,
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                }
                (Ok(set), false) => {
                    let verb = match (set.changed, dry) {
                        (false, _) => "unchanged",
                        (true, true) => "would set",
                        (true, false) => "set",
                    };
                    let item = &set.item;
                    println!("{verb} {}:{}  {}", item.note, item.line, item_line(item));
                    if item.id != id {
                        println!("new id {}", item.id);
                    }
                }
                (Err(e), true) => {
                    let changed = matches!(e, edit::EditError::Changed(_));
                    println!(
                        "{}",
                        serde_json::json!({"error": e.to_string(), "changed": changed})
                    );
                    return Err(e.into());
                }
                (Err(e), false) => return Err(e.into()),
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

/// `drive [--real-home | --home DIR] [SCRIPT] [-- ARGS…]`, parsed apart from `Args`: ARGS belong
/// to the program under test.
fn drive() -> Result<()> {
    const USAGE: &str = "usage: cold-lava drive [--real-home | --home DIR] [SCRIPT] [-- ARGS…]";
    let mut words = std::env::args().skip(2);
    let mut ours: Vec<String> = words.by_ref().take_while(|w| w != "--").collect();
    let theirs: Vec<String> = words.collect();
    let mut home = drive::HomeChoice::Empty;
    if let Some(i) = ours.iter().position(|w| w == "--home") {
        let Some(dir) = ours.get(i + 1).cloned() else {
            anyhow::bail!(USAGE);
        };
        ours.drain(i..=i + 1);
        home = drive::HomeChoice::At(PathBuf::from(dir));
    }
    if let Some(i) = ours.iter().position(|w| w == "--real-home") {
        ours.remove(i);
        home = drive::HomeChoice::Real;
    }
    if ours.len() > 1 {
        anyhow::bail!(USAGE);
    }
    let (name, src) = match ours.pop() {
        Some(path) => {
            let src = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("couldn't read {path}: {e}"))?;
            (path, src)
        }
        None => {
            let mut src = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut src)?;
            ("<stdin>".to_string(), src)
        }
    };
    let script =
        drive::parse(&src).map_err(|e| anyhow::anyhow!("{name}:{}: {}", e.line, e.message))?;
    if !drive::run(&name, &script, &std::env::current_exe()?, &theirs, home)? {
        std::process::exit(1);
    }
    Ok(())
}

fn print_tidy(r: &tidy::Report, no_backlinks: bool) {
    let section = |title: &str, n: usize| match n {
        0 => println!("{title}: none"),
        n => println!("{title}: {n}"),
    };
    let orphans: Vec<&tidy::Orphan> = (r.orphans.iter())
        .filter(|o| no_backlinks || o.both_ways())
        .collect();
    let (daily, notes): (Vec<_>, Vec<_>) = orphans.iter().partition(|o| o.daily);
    match no_backlinks {
        true => section("Notes nothing links to", orphans.len()),
        false => section("Orphans (no links in or out)", orphans.len()),
    }
    let line = |o: &tidy::Orphan| match o.links_out {
        0 => o.note.clone(),
        n => format!("{}  (links to {n})", o.note),
    };
    for o in notes {
        println!("  {}", line(o));
    }
    if !daily.is_empty() {
        println!("  daily notes:");
        for o in daily {
            println!("    {}", line(o));
        }
    }
    section("Unused attachments", r.unused_attachments.len());
    for a in &r.unused_attachments {
        println!("  {a}");
    }
    section("Empty folders", r.empty_folders.len());
    for f in &r.empty_folders {
        println!("  {f}/");
    }
    section("Unresolved links", r.unresolved.len());
    for u in &r.unresolved {
        println!("  {}", u.target);
        for l in &u.from {
            println!("    {}:{}  {}", l.note, l.line, l.context);
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

/// `cold-lava types`: each type with where it comes from, then each shape with the fields it
/// reads, where, and whether a value can be written back there.
fn print_types(found: &types::Types, counts: Option<&HashMap<String, (usize, usize)>>) {
    println!("{}  built in, read natively", found.native.join(", "));
    for t in &found.types {
        let d = &t.definition;
        println!("\n{}  {}", t.name, t.origin);
        if let Some(counts) = counts {
            let (n, unparsed) = counts.get(&t.name).copied().unwrap_or_default();
            let mut line = format!("  found {n} item{}", if n == 1 { "" } else { "s" });
            if unparsed > 0 {
                let s = if unparsed == 1 { " didn't" } else { "s didn't" };
                line += &format!(" · {unparsed} value{s} parse");
            }
            println!("{line}");
        }
        let mut about = Vec::new();
        if let Some(base) = &d.extends {
            about.push(format!("extends {base}"));
        }
        if let Some(p) = d.priority {
            about.push(format!("priority {p}"));
        }
        if !d.requires.is_empty() {
            about.push(format!("requires {}", d.requires.join(", ")));
        }
        if !d.actions.is_empty() {
            about.push(format!("actions {}", d.actions.join(", ")));
        }
        if let Some(w) = &d.write {
            about.push(format!("write '{w}'"));
        }
        if !about.is_empty() {
            println!("  {}", about.join(" · "));
        }
        let name_width = d
            .fields
            .keys()
            .map(|f| f.chars().count())
            .max()
            .unwrap_or(0);
        let values: Vec<String> = d.fields.values().map(value_label).collect();
        let value_width = values.iter().map(|v| v.chars().count()).max().unwrap_or(0);
        for (i, shape) in d.from.iter().enumerate() {
            let mut line = format!("on {}", shape.on.name());
            let conditions = where_label(&shape.conditions);
            if !conditions.is_empty() {
                line += &format!(" · where {conditions}");
            }
            if let Some(m) = &shape.pattern {
                line += &format!(" · match '{m}'");
            }
            if !shape.key.is_empty() {
                line += &format!(" · key {}", shape.key.join(", "));
            }
            if !shape.required.is_empty() {
                line += &format!(" · required {}", shape.required.join(", "));
            }
            if d.from.len() > 1 {
                line = format!("shape {}: {line}", i + 1);
            }
            println!("  {line}");
            let read: Vec<(String, &str)> = d
                .fields
                .keys()
                .map(|f| match shape.fields.get(f) {
                    None => ("-".to_string(), "not read here"),
                    Some(sources) => {
                        let labels: Vec<String> = sources.0.iter().map(source_label).collect();
                        let rw = if sources.0.iter().any(types::Source::writable) {
                            "writable"
                        } else {
                            "read-only"
                        };
                        (labels.join(" or "), rw)
                    }
                })
                .collect();
            let source_width = read.iter().map(|r| r.0.chars().count()).max().unwrap_or(0);
            for ((field, value), (source, rw)) in d.fields.keys().zip(&values).zip(&read) {
                println!(
                    "    {field:name_width$}  {value:value_width$}  {source:source_width$}  {rw}"
                );
            }
        }
        if !d.style.is_empty() {
            println!("  looks");
            for (field, look) in &d.style {
                println!("    {field:name_width$}  {}", look_label(look));
            }
        }
    }
    if !found.notes.is_empty() {
        println!("\nnotes:");
        for n in &found.notes {
            println!("  {n}");
        }
    }
}

/// Items found and values that didn't parse, by type: an item counts for its own type only.
fn type_counts(workspace: &workspace::Workspace) -> HashMap<String, (usize, usize)> {
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
    for note in &workspace.notes {
        for item in workspace.items(note) {
            if let items::Data::Typed(t) = &item.data {
                let c = counts.entry(t.kind.clone()).or_default();
                c.0 += 1;
                c.1 += t.unparsed;
            }
        }
    }
    counts
}

fn value_label(v: &types::Value) -> String {
    match v {
        types::Value::Date { format: Some(f) } | types::Value::Datetime { format: Some(f) } => {
            format!("{} ({f})", v.name())
        }
        types::Value::Enum {
            values: types::EnumValues::Setting(s),
        } => format!("enum {s}"),
        types::Value::Enum { values } => {
            let mut names: Vec<&str> = Vec::new();
            for n in values.map().values() {
                if !names.contains(&n.as_str()) {
                    names.push(n);
                }
            }
            format!("enum {}", names.join("|"))
        }
        _ => v.name().to_string(),
    }
}

fn where_label(w: &types::Where) -> String {
    let mut parts = Vec::new();
    for (key, list) in [
        ("header", &w.header),
        ("tag", &w.tag),
        ("folder", &w.folder),
        ("section", &w.section),
        ("language", &w.language),
        ("inside", &w.inside),
    ] {
        if !list.is_empty() {
            parts.push(format!("{key} {}", list.join(", ")));
        }
    }
    for (key, value) in &w.property {
        parts.push(format!("property {key} = {value}"));
    }
    parts.join("; ")
}

fn source_label(s: &types::Source) -> String {
    use types::Source as S;
    let detail = match s {
        S::Capture(t)
        | S::Pattern(t)
        | S::Marker(t)
        | S::Inline(t)
        | S::Flag(t)
        | S::Column(t)
        | S::Property(t)
        | S::Children(t) => t.clone(),
        S::Flags(glyphs) => glyphs
            .iter()
            .map(|(g, n)| format!("{g} {n}"))
            .collect::<Vec<_>>()
            .join(", "),
        S::Info(part) => name_of(part),
        S::Inlines(kind) => name_of(kind),
        S::Note(part) => name_of(part),
        S::Section(part) => name_of(part),
        S::Rest(r) if r.without.is_empty() => String::new(),
        S::Rest(r) => format!("without {}", r.without.join(", ")),
        S::Checkbox(_) => String::new(),
    };
    match s {
        S::Pattern(_) => format!("pattern '{detail}'"),
        _ if detail.is_empty() => s.kind().to_string(),
        _ => format!("{} {detail}", s.kind()),
    }
}

/// A unit variant's name as written in TOML and JSON: `list-item`, `cyan`.
fn name_of(v: impl serde::Serialize) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn look_label(l: &types::Look) -> String {
    let mut parts = Vec::new();
    if let Some(c) = l.colour {
        parts.push(name_of(c));
    }
    if let Some(i) = &l.icon {
        parts.push(format!("icon {i}"));
    }
    for (on, name) in [
        (l.badge, "badge"),
        (l.dim, "dim"),
        (l.strike, "strike"),
        (l.bold, "bold"),
        (l.hide, "hide"),
        (l.link, "link"),
        (l.whole, "whole"),
    ] {
        if on {
            parts.push(name.to_string());
        }
    }
    for (value, look) in &l.by_value {
        parts.push(format!("{value}: {}", look_label(look)));
    }
    parts.join(", ")
}

/// An item in a few words, for `cold-lava items`.
fn item_line(item: &items::Item) -> String {
    match &item.data {
        items::Data::Typed(t) if item.data.answers_to("task") => {
            let field = |name: &str| t.fields.get(name).and_then(|v| v.as_str());
            let mut line = format!(
                "[{}] {}",
                field("status").unwrap_or(" "),
                field("text").unwrap_or_default()
            );
            for name in ["due", "scheduled", "start"] {
                if let Some(d) = field(name) {
                    line.push_str(&format!("  {name} {d}"));
                }
            }
            line
        }
        items::Data::List { text } => format!("- {text}"),
        items::Data::Heading { level, text } => format!("{} {text}", "#".repeat(*level as usize)),
        items::Data::Block => format!("^{}", item.block_id.as_deref().unwrap_or_default()),
        items::Data::Typed(t) => {
            let fields: Vec<String> = t
                .fields
                .iter()
                .map(|(name, value)| match value {
                    serde_json::Value::String(s) => format!("{name} {s}"),
                    serde_json::Value::Array(a) => {
                        let list: Vec<String> = a
                            .iter()
                            .map(|v| v.as_str().map_or_else(|| v.to_string(), String::from))
                            .collect();
                        format!("{name} [{}]", list.join(", "))
                    }
                    serde_json::Value::Null => format!("{name} -"),
                    v => format!("{name} {v}"),
                })
                .collect();
            format!("{}  {}", t.kind, fields.join(" · "))
        }
    }
}

fn print_agenda(a: &tasks::Agenda) {
    println!("{} · {} to {}", a.workspace, a.from, a.to);
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
