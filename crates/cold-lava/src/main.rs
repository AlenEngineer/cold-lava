mod agenda;
mod app;
mod board;
mod commands;
mod config;
mod drive;
mod embed;
mod ext;
mod graph;
mod graph_ext;
mod groups;
mod hints;
mod history;
mod home;
mod image;
mod kitty;
mod list;
mod looks;
mod owned;
mod previews;
mod reader;
mod recipe_view;
mod refused;
mod say;
mod scene;
mod scene_view;
mod shape_editor;
mod table;
mod term;
mod tidy_view;
mod type_table;
mod views;
mod whichkey;

use cold_lava_engine::{
    act, bind, bookmarks, core, daily, edit, files, frontmatter, gateway, index, items,
    json_canvas, links, markdown, merge, owner, paths, problem, recipe, rename, resolver, search,
    shapes, style, tasks, templates, tidy, trash, types, workspace, write,
};

use anyhow::Result;
use owner::markdown::attributes;
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use say::{out, problem, told};
use std::collections::HashMap;
use std::io::stdout;
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
  cold-lava recipe WORKSPACE NAME     run a recipe, a named change to the whole workspace,
                                      shown as a diff per file
                                      (rename-tag --from TAG --to TAG);
                                      cold-lava recipe --list names them and their options
  cold-lava table WORKSPACE TYPE      a type's items as the table view shows them
  cold-lava tidy WORKSPACE            orphan notes, unused attachments, empty folders and
                                      unresolved links (read-only)
  cold-lava style WORKSPACE           how the workspace's notes are written, which new text
                                      follows, with the counts behind each choice;
                                      checks [style] in config.toml and .cold-lava/style.toml
  cold-lava keys                      every command and its keys; checks config.toml
  cold-lava ext list [WORKSPACE]      installed extensions, what each may do, and why any
                                      didn't load
  cold-lava types [WORKSPACE]         every type: where it comes from, what it found, its
                                      shapes and fields;
                                      checks [types] in config.toml and .cold-lava/types.toml
  cold-lava attributes [WORKSPACE]    how Markdown writes the attributes types read (a
                                      checkbox, 📅, [due:: …]) and where each reading comes
                                      from; checks [attributes] in config.toml and
                                      .cold-lava/attributes.toml
  cold-lava shapes [WORKSPACE]        every shape and glyph: where it comes from, its parts,
                                      regions, inputs and behaviour; checks [[shapes]] in
                                      config.toml and .cold-lava/shapes.toml
  cold-lava shapes [WORKSPACE] --render ID [--size WxH] [--rotate DEG] [--bind INPUT=SOURCE…]
                                      draw a shape as text (braille, labels, then its regions),
                                      its inputs bound to data and its regions filled; with
                                      --json, what it shows as data
  cold-lava drive [SCRIPT] [-- ARGS…] run cold-lava ARGS in a hidden terminal, typing and
                                      checking the screen as SCRIPT says (stdin if none),
                                      with an empty home unless --real-home
                                      (or --home DIR: that folder, kept)

Options:
  --braille            force braille rendering
  --pixels             force pixel rendering (Kitty graphics protocol)
  --from / --to DATE   agenda range (YYYY-MM-DD; default: this month)
  --from / --to TAG    recipe rename-tag: the tag to rename, and its new name
  --json               machine-readable output (agenda, items, types, shapes, task toggle,
                       item set, tidy, recipe, style, table: the view's own JSON)
  --id ID              task toggle: only if the line still matches this id from the agenda
  --rev REV            item set: only if the item is still this version (its rev)
  --remove             item set: take the field's value out instead of setting one
  --path               daily: create the note if needed and print its path, no UI
  --limit N            search: matching lines shown per note (default 5)
  --dry-run            rename, recipe, item set: show what would change, change nothing
  --sort FIELD         table: sort by FIELD (-FIELD: descending), as s and S do
  --no-backlinks       tidy: also list notes nothing links to that do link elsewhere
  --render ID          shapes: draw this shape, unbound (${a.title} shows as a.title)
  --size WxH           shapes --render: the terminal it is drawn on (default 60x18)
  --rotate DEG         shapes --render: turn it DEG degrees clockwise about its centre
  --bind INPUT=SOURCE  shapes --render, repeatable: bind an input to a query
                       ('type=volcano tag=eruption title=Erupted where vei >= 6'), a note
                       (note:PATH), labels (items:A, B) or text (text:…)
  --do GESTURE         shapes --render --bind: first do what the shape does on a gesture
                       (action:NAME, key:K, click:ON[:ITEM], drop:FROM:TO:ITEM), writing to
                       the workspace; prints each step, then the shape as it is after
  -V, --version        print the version
  -h, --help           this help

DATE is YYYY-MM-DD, today, tomorrow or yesterday. Press ? inside the app for keys.
Keys are set by command id under [keys] in ~/.config/cold-lava/config.toml.
COLD_LAVA_EDITOR=none opens notes in the reader even when Neovim is installed.";

/// Command-line arguments: positional words, `--flag value` options and `--switch`es.
struct Args {
    words: Vec<String>,
    /// `--bind`, which can be given more than once.
    binds: Vec<String>,
    values: std::collections::HashMap<String, String>,
    switches: std::collections::HashSet<String>,
}

impl Args {
    fn parse() -> Result<Args> {
        const WITH_VALUE: &[&str] = &[
            "from", "to", "id", "limit", "rev", "sort", "render", "size", "rotate", "do",
        ];
        let mut a = Args {
            words: Vec::new(),
            binds: Vec::new(),
            values: Default::default(),
            switches: Default::default(),
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            if let Some(flag) = arg.strip_prefix("--") {
                if flag == "bind" {
                    let v = it
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--bind needs INPUT=SOURCE"))?;
                    a.binds.push(v);
                } else if let Some(v) = flag.strip_prefix("bind=") {
                    a.binds.push(v.to_string());
                } else if let Some((k, v)) = flag.split_once('=') {
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

/// `cold-lava style`: one line per choice, with where it came from and the counts behind it.
fn print_style(s: &style::Style) {
    fn line<T>(name: &str, c: &style::Choice<T>, value: &str) {
        let counts: Vec<String> = c.counts.iter().map(|(k, n)| format!("{k} {n}")).collect();
        told!(
            "{name:<18} {value:<11} {:<30} {}",
            c.from.to_string(),
            counts.join(", ")
        );
    }
    told!("{} notes", s.notes);
    line("list marker", &s.list_marker, s.list_marker.value.name());
    line("links", &s.links, s.links.value.name());
    line("link path", &s.link_path, s.link_path.value.name());
    line("task fields", &s.task_fields, s.task_fields.value.name());
    line("headings", &s.headings, s.headings.value.name());
    line(
        "frontmatter lists",
        &s.frontmatter_lists,
        s.frontmatter_lists.value.name(),
    );
    line("tags", &s.tags, s.tags.value.name());
    let k = &s.key_order;
    let keys = match k.value.is_empty() {
        true => "(none)".to_string(),
        false => k.value.join(", "),
    };
    told!("{:<18} {keys}  ({})", "key order", k.from);
    for (field, d) in &s.dates {
        let counts: Vec<String> = (d.counts.iter())
            .filter(|(_, n)| **n > 0)
            .map(|(f, n)| format!("{f} {n}"))
            .collect();
        told!(
            "{:<18} {:<11} {:<30} {}",
            format!("date {field}"),
            d.value,
            d.from.to_string(),
            counts.join(", ")
        );
    }
}

/// The version, and the commit it was built from when built from a git checkout
/// (`build.rs`): `0.3.0 (1a2b3c4d5e6f)`.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), env!("COLD_LAVA_COMMIT_SUFFIX"));

fn main() {
    if let Err(e) = run() {
        // An error of several lines is the app's own (a usage): one message line each.
        let said = e
            .downcast_ref::<problem::Problem>()
            .map(problem::Problem::said);
        match said.as_deref() {
            Some([first, rest @ ..]) => {
                problem!("cold-lava: {first}");
                rest.iter().for_each(|line| problem!("{line}"));
            }
            _ => problem!("cold-lava: {e:#}"),
        }
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
        say::fixed(USAGE);
        told!();
        told!("A drive script has one step per line:");
        for line in drive::SYNTAX.lines() {
            told!("  {line}");
        }
        return Ok(());
    }
    if args.has("version") {
        told!("cold-lava {VERSION}");
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
    let mut registry = ext::Registry::load();
    config::load(registry.commands());
    let mut layers: Vec<types::Layer> = registry
        .manifests()
        .map(|m| {
            let types = m.types.iter().map(|t| (t.id.clone(), t.definition.clone()));
            types::Layer::extension(&m.name, types)
        })
        .collect();
    if let Some(path) = config::path() {
        layers.push(types::Layer::config(&path, config::get().types.clone()));
    }
    types::set_configured(layers);
    if let Some(path) = config::path() {
        let written = config::get().attributes.clone();
        attributes::set_configured(vec![attributes::Layer::config(&path, written)]);
    }
    let mut layers: Vec<shapes::Layer> = registry
        .manifests()
        .map(|m| {
            let shapes = m
                .shapes
                .iter()
                .map(|s| (s.id.clone(), s.definition.clone()));
            shapes::Layer::extension(&m.name, shapes)
        })
        .collect();
    if let Some(path) = config::path() {
        let c = config::get();
        layers.push(shapes::Layer::config(
            &path,
            c.shapes.clone(),
            c.glyphs.clone(),
        ));
    }
    shapes::set_configured(layers);

    match args.word(0) {
        Some("keys") => {
            print_keys("menu", registry.commands());
            print_keys("graph", &commands::entries(commands::GRAPH));
            print_keys("whiteboard", &commands::entries(commands::BOARD));
            print_keys("reader", &commands::entries(commands::READER));
            print_keys("pane", &commands::entries(commands::PANE));
            print_keys("shape editor", &commands::entries(commands::SHAPE_EDITOR));
            let errors = &config::get().errors;
            let warnings = &config::get().warnings;
            if !errors.is_empty() || !warnings.is_empty() {
                let path = config::path().unwrap_or_default();
                problem!("{}:", path.display());
                let lines = errors.iter().flat_map(|e| e.lines());
                lines.chain(warnings).for_each(|line| problem!("  {line}"));
            }
            if !registry.refused().is_empty() {
                problem!("extensions:");
                for e in registry.refused() {
                    problem!("  {e}");
                }
            }
            if !errors.is_empty() || !registry.refused().is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
        Some("ext") => {
            if args.word(1) != Some("list") {
                anyhow::bail!("usage: cold-lava ext list [WORKSPACE]");
            }
            print_extensions(&registry, args.word(2).map(Path::new));
            if !registry.mistakes().is_empty() {
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
                say::json(&found)?;
            } else {
                let counts = owned::workspace(&root, &registry).ok();
                let counts = counts.map(|v| type_counts(&v));
                print_types(&found, counts.as_ref());
                if !found.errors.is_empty() {
                    problem!("errors:");
                    for e in &found.errors {
                        problem!("  {e}");
                    }
                }
            }
            if !found.errors.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
        Some("attributes") => {
            let root = match args.word(1) {
                Some(dir) => PathBuf::from(dir),
                None => workspace::find_workspace_root(Path::new(".")),
            };
            let found = attributes::for_workspace(&root);
            if json {
                say::json(&found)?;
            } else {
                print_attributes(&found);
                for (title, lines) in [("notes:", &found.notes), ("errors:", &found.errors)] {
                    if !lines.is_empty() {
                        problem!("{title}");
                        for line in lines {
                            problem!("  {line}");
                        }
                    }
                }
            }
            if !found.errors.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
        Some("shapes") => {
            let root = match args.word(1) {
                Some(dir) => PathBuf::from(dir),
                None => workspace::find_workspace_root(Path::new(".")),
            };
            let found = shapes::for_workspace(&root);
            if let Some(id) = args.values.get("render") {
                let Some(shape) = found.get(id) else {
                    anyhow::bail!("no shape is called {id}; cold-lava shapes lists them");
                };
                let size = args.values.get("size").map_or("60x18", String::as_str);
                let (cols, rows) = (size.split_once('x'))
                    .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)))
                    .filter(|&(w, h)| w > 0 && h > 0)
                    .ok_or_else(|| anyhow::anyhow!("--size is WxH in cells, like 60x18"))?;
                let rotation = match args.values.get("rotate") {
                    None => 0.0,
                    Some(deg) => (deg.parse::<f64>().ok())
                        .filter(|d| d.is_finite())
                        .ok_or_else(|| anyhow::anyhow!("--rotate is degrees clockwise, like 30"))?,
                };
                let (drawn, contents, aspect) = match args.binds.is_empty() {
                    true => {
                        let aspect = shape.definition.aspect;
                        (std::sync::Arc::new(shape.unbound()), Vec::new(), aspect)
                    }
                    false => match bound_shape(&root, &found, id, &args, &mut registry, json)? {
                        Some(drawn) => drawn,
                        None => return Ok(()),
                    },
                };
                say::listing_block(&cold_lava_canvas::draw::preview(
                    drawn, cols, rows, aspect, rotation, &contents,
                ));
                return Ok(());
            }
            if json {
                say::json(&found)?;
            } else {
                print_shapes(&found);
                let errors: Vec<String> = (found.errors.iter())
                    .flat_map(problem::Problem::said)
                    .collect();
                for (title, lines) in [("notes:", &found.notes), ("errors:", &errors)] {
                    if !lines.is_empty() {
                        problem!("{title}");
                        for line in lines {
                            problem!("  {line}");
                        }
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
            with_terminal(|t| board::launch(t, &root, file, &mut registry))
        }
        Some("graph") => {
            let root = PathBuf::from(args.word(1).unwrap_or("."));
            let core = core::Core::open(&root)?;
            if core.index.workspace.notes.is_empty() {
                anyhow::bail!("no .md notes found under {}", core.root().display());
            }
            drop(core);
            let mut home = home::Home::new(render, Some(root), registry);
            home.open_graph_alone();
            with_terminal(|t| home.run(t))
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
            let agenda = tasks::agenda(&owned::workspace(&root, &registry)?, from, to);
            if json {
                say::json(&agenda)?;
            } else {
                print_agenda(&agenda);
            }
            Ok(())
        }
        Some("items") => {
            let (Some(root), Some(kind)) = (args.word(1), args.word(2)) else {
                let kinds = types::BUILT_IN.iter().chain(items::KINDS).copied();
                let kinds = kinds.collect::<Vec<_>>().join(", ");
                let usage = "usage: cold-lava items WORKSPACE KIND [--json]".to_string();
                let lines = [usage, format!("KIND is one of: {kinds} or a type")];
                return Err(problem::Problem::of(lines).into());
            };
            let workspace = owned::workspace(Path::new(root), &registry)?;
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
            let mut out = say::held();
            let notes = (workspace.notes.iter()).map(|n| workspace.items_once(n));
            let owned = (workspace.owned.iter()).map(|f| std::borrow::Cow::Borrowed(&f.items[..]));
            for items in notes.chain(owned) {
                for item in items.iter().filter(|i| i.data.answers_to(kind)) {
                    let written = if json {
                        out.json(item)
                    } else {
                        out.listing(format_args!(
                            "{}:{}  {}",
                            item.note,
                            item.line,
                            item_line(item)
                        ))
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
            let workspace = owned::workspace(Path::new(root), &registry)?;
            let types = &workspace.item_settings().types.types;
            let Some(def) = types.iter().find(|t| t.name == ty) else {
                let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
                anyhow::bail!("no type called {ty}; there are: {}", names.join(", "));
            };
            let query = gateway::Query {
                select: type_table::select(ty),
                page: gateway::Page::default(),
            };
            let found = gateway::query(&workspace, &query)?;
            let built = type_table::build(def, &found.items);
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
            let mut out = say::held();
            let written = match json {
                true => out.json_pretty(&view.sorted()),
                false => view
                    .text()
                    .iter()
                    .try_for_each(|l| out.listing(format_args!("{}", l.trim_end()))),
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
                say::json(&report)?;
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
                say::json(&search::to_json(&ix, &hits))?;
            } else {
                for h in &hits {
                    let n = &ix.workspace.notes[h.note];
                    for l in &h.lines {
                        out!("{}.md:{}: {}", n.rel, l.line, l.text.trim());
                    }
                    if h.lines.is_empty() {
                        out!("{}.md: (name matches)", n.rel);
                    }
                }
                if hits.is_empty() {
                    problem!("no matches");
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        Some("rename") => {
            let (Some(root), Some(note), Some(new)) = (args.word(1), args.word(2), args.word(3))
            else {
                let usage = "usage: cold-lava rename WORKSPACE NOTE NEW [--dry-run] [--json]\n\
                             NOTE is a note's name or path, or a folder; NEW is a new name \
                             (same folder), a folder ending in / (keeps the name), or a full \
                             path";
                return Err(problem::Problem::of([]).then(usage).into());
            };
            let mut core = owned::open(Path::new(root), &registry)?;
            let mut owners = owned::Owners::new(&mut registry, &core);
            owners.serve(&mut core, |c| !c.reading());
            let ix = &core.index;
            let folder = note.trim_matches('/');
            let plan = if !folder.is_empty() && ix.workspace.root.join(folder).is_dir() {
                rename::plan_folder(ix, folder, &rename::folder_target(folder, new)?)?
            } else {
                let i = ix
                    .by_rel(note)
                    .or_else(|| ix.resolver.resolve("", note))
                    .ok_or_else(|| anyhow::anyhow!("no note or folder called {note}"))?;
                let to = rename::target_rel(&format!("{}.md", ix.workspace.notes[i].rel), new)?;
                rename::plan(ix, i, &to)?
            };
            let mut plan = plan;
            let dry = args.has("dry-run");
            if !dry {
                (owners.relink(&mut core, &mut plan.edits))
                    .map_err(|e| anyhow::anyhow!("{e}; nothing was changed"))?;
                rename::apply(&plan)?;
                core.relinked(&plan.edits, |p| plan.moved(p));
                core.poll();
            }
            if json {
                let out = serde_json::json!({ "applied": !dry, "plan": plan });
                say::json(&out)?;
            } else {
                let verb = if dry { "would rename" } else { "renamed" };
                for m in &plan.moves {
                    told!("{verb} {} → {}", m.from, m.to);
                }
                for e in &plan.edits {
                    for c in &e.changes {
                        told!("  {}:{}  {}", e.file, c.line, c.after.trim());
                    }
                }
                for c in plan.bookmarks.iter().flat_map(|b| &b.changes) {
                    told!("  bookmark  {} → {}", c.before, c.after);
                }
                told!("{} links in {} files", plan.links(), plan.files());
                for w in &plan.warnings {
                    problem!("warning: {w}");
                }
            }
            Ok(())
        }
        Some("recipe") => {
            if args.has("list") {
                return print_recipes(json);
            }
            let (Some(root), Some(id)) = (args.word(1), args.word(2)) else {
                let usage = "usage: cold-lava recipe WORKSPACE NAME [--OPTION VALUE…] [--dry-run] \
                             [--json]\n\
                             cold-lava recipe --list shows the recipes and their options";
                return Err(problem::Problem::of([]).then(usage).into());
            };
            let r = recipe::find(id).ok_or_else(|| {
                anyhow::anyhow!("no recipe called {id}; cold-lava recipe --list shows them")
            })?;
            let options: recipe::Options = (r.options.iter())
                .filter_map(|o| Some((o.name.to_string(), args.values.get(o.name)?.clone())))
                .collect();
            let mut core = owned::open(Path::new(root), &registry)?;
            let mut owners = owned::Owners::new(&mut registry, &core);
            owners.serve(&mut core, |c| !c.reading());
            let mut plan = recipe::plan(&core.index, r, options)?;
            let dry = args.has("dry-run");
            if !dry && !plan.edits.is_empty() {
                (owners.relink(&mut core, &mut plan.edits))
                    .map_err(|e| anyhow::anyhow!("{e}; nothing was changed"))?;
                recipe::apply(&plan)?;
                core.relinked(&plan.edits, Path::to_path_buf);
                core.poll();
            }
            print_recipe_plan(&plan, !dry, json)
        }
        Some("style") => {
            let root = PathBuf::from(args.word(1).unwrap_or("."));
            let found = workspace::scan(&root)?.style();
            if json {
                say::json(&found)?;
            } else {
                print_style(&found);
            }
            let config = (config::get().errors.iter()).filter(|e| e.is_about(config::STYLE));
            let config: Vec<String> = config
                .map(|e| format!("config.toml {}", e.first()))
                .collect();
            for e in found.errors.iter().chain(&config) {
                problem!("{e}");
            }
            if !found.errors.is_empty() || !config.is_empty() {
                std::process::exit(1);
            }
            Ok(())
        }
        Some("daily") => {
            let root = workspace::find_workspace_root(&PathBuf::from(args.word(1).unwrap_or(".")));
            let date = daily::parse_cli_date(args.word(2).unwrap_or("today"))?;
            let path = daily::ensure(&root, date)?;
            if args.has("path") {
                out!("{}", path.display());
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
            let (root, found) = locate_note_workspace(&file);
            let rel = file.canonicalize().unwrap_or(file.clone());
            let rel = rel.strip_prefix(&root).unwrap_or(&rel).to_string_lossy();
            if let Some(owner) = owned::owner_of(&registry, &rel.replace('\\', "/")) {
                anyhow::bail!(
                    "{} is read by {owner}, not as a note, so a line doesn't name a task in it: \
                     tick it with `cold-lava item set FILE ID status x` (ids from `cold-lava \
                     items WORKSPACE task --json`)",
                    file.display()
                );
            }
            let result = tasks::toggle_in(
                &root,
                &file,
                line,
                args.values.get("id").map(String::as_str),
                tasks::today(),
            );
            match (result, json) {
                (Ok(r), true) => say::json(&r)?,
                (Ok(r), false) => {
                    told!("{}:{}  {}", r.path, r.line, r.raw.trim());
                    if let Some(n) = &r.next {
                        told!("next occurrence on line {}: {}", n.line, n.raw.trim());
                    }
                    if let Some(w) = &r.warning {
                        problem!("warning: {w}");
                    }
                }
                (Err(e), true) => {
                    let changed = e.downcast_ref::<tasks::Changed>().is_some();
                    say::json_line(
                        &serde_json::json!({"error": format!("{e:#}"), "changed": changed}),
                    )?;
                    if let Some(n) = guessed_note(&root, found) {
                        problem!("{n}");
                    }
                    return Err(e);
                }
                (Err(e), false) => {
                    if let Some(n) = guessed_note(&root, found) {
                        problem!("{n}");
                    }
                    return Err(e);
                }
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
            let (root, found) = locate_note_workspace(&path);
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
            let result = match owned::owner_of(&registry, &note) {
                Some(owner) if dry => Err(edit::EditError::Refused(format!(
                    "--dry-run: {note} is read by {owner}, which writes it only for a real edit"
                ))),
                Some(_) => {
                    let (rev, value) = (req.rev, req.value.clone());
                    owned::set_field(&root, &mut registry, &note, id, rev, field, value)
                        .map(|e| edit::Set {
                            text: String::new(),
                            changed: e.changed,
                            item: e.item,
                        })
                        .map_err(|e| match e.reason {
                            gateway::Reason::Changed => edit::EditError::Changed(e.message),
                            _ => edit::EditError::Refused(e.message),
                        })
                }
                None => edit::set_field(&root, &note, &req, dry),
            };
            match (result, json) {
                (Ok(set), true) => {
                    let out = serde_json::json!({
                        "applied": set.changed && !dry,
                        "changed": set.changed,
                        "item": set.item,
                    });
                    say::json(&out)?;
                }
                (Ok(set), false) => {
                    let verb = match (set.changed, dry) {
                        (false, _) => "unchanged",
                        (true, true) => "would set",
                        (true, false) => "set",
                    };
                    let item = &set.item;
                    told!("{verb} {}:{}  {}", item.note, item.line, item_line(item));
                    if item.id != id {
                        told!("new id {}", item.id);
                    }
                }
                (Err(e), true) => {
                    let changed = matches!(e, edit::EditError::Changed(_));
                    say::json_line(
                        &serde_json::json!({"error": e.to_string(), "changed": changed}),
                    )?;
                    if let Some(n) = guessed_note(&root, found) {
                        problem!("{n}");
                    }
                    return Err(e.into());
                }
                (Err(e), false) => {
                    if let Some(n) = guessed_note(&root, found) {
                        problem!("{n}");
                    }
                    return Err(e.into());
                }
            }
            Ok(())
        }
        Some(dir) => {
            let path = PathBuf::from(dir);
            if !path.is_dir() {
                let lines = [format!("not a folder or command: {dir}"), String::new()];
                return Err(problem::Problem::of(lines).then(USAGE).into());
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
    let script = drive::parse(&src).map_err(|e| {
        let lines = problem::Problem::line(format!("{name}:{}: {}", e.line, e.message));
        match e.then {
            Some(text) => lines.then(text),
            None => lines,
        }
    })?;
    if !drive::run(&name, &script, &std::env::current_exe()?, &theirs, home)? {
        std::process::exit(1);
    }
    Ok(())
}

/// `cold-lava recipe --list`: each recipe with its options.
fn print_recipes(json: bool) -> Result<()> {
    if json {
        say::json(recipe::BUILT_IN)?;
        return Ok(());
    }
    for r in recipe::BUILT_IN {
        told!("{}  {}", r.id, r.about);
        for o in r.options {
            told!("  --{} VALUE  {}", o.name, o.about);
        }
    }
    Ok(())
}

/// A recipe's plan: a diff per file, or with `json` the plan as JSON.
fn print_recipe_plan(plan: &recipe::Plan, applied: bool, json: bool) -> Result<()> {
    if json {
        let files: Vec<_> = (plan.edits.iter())
            .map(|e| {
                serde_json::json!({
                    "file": e.file,
                    "changes": e.links,
                    "owner": e.owned.as_ref().map(|o| &o.owner),
                    "diff": recipe::unified(&recipe::file_diff(e, 1)),
                })
            })
            .collect();
        let out = serde_json::json!({
            "recipe": plan.recipe.id,
            "options": plan.options,
            "applied": applied,
            "changes": plan.changes(),
            "files": files,
            "warnings": plan.warnings,
        });
        say::json(&out)?;
        return Ok(());
    }
    for e in &plan.edits {
        match &e.owned {
            Some(o) => out!("--- {} (through {})", e.file, o.owner),
            None => out!("--- {}", e.file),
        }
        say::listing_block(&recipe::unified(&recipe::file_diff(e, 1)));
    }
    for w in &plan.warnings {
        problem!("warning: {w}");
    }
    match (plan.edits.is_empty(), applied) {
        (true, _) => told!("{}: nothing to change", plan.title()),
        (false, true) => told!("{}: changed {}", plan.title(), plan.what()),
        (false, false) => told!(
            "{}: would change {} (dry run, nothing written)",
            plan.title(),
            plan.what()
        ),
    }
    Ok(())
}

fn print_tidy(r: &tidy::Report, no_backlinks: bool) {
    let section = |title: &str, n: usize| match n {
        0 => out!("{title}: none"),
        n => out!("{title}: {n}"),
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
        out!("  {}", line(o));
    }
    if !daily.is_empty() {
        out!("  daily notes:");
        for o in daily {
            out!("    {}", line(o));
        }
    }
    section("Unused attachments", r.unused_attachments.len());
    for a in &r.unused_attachments {
        out!("  {a}");
    }
    section("Empty folders", r.empty_folders.len());
    for f in &r.empty_folders {
        out!("  {f}/");
    }
    section("Unresolved links", r.unresolved.len());
    for u in &r.unresolved {
        out!("  {}", u.target);
        for l in &u.from {
            out!("    {}:{}  {}", l.note, l.line, l.context);
        }
    }
}

fn print_keys<A: Copy>(screen: &str, table: &[commands::Entry<A>]) {
    let keys = commands::Keymap::configured(table);
    let id_width = table.iter().map(|c| c.id.len()).max().unwrap_or(0);
    let labels: Vec<String> = table.iter().map(|c| keys.label(&c.id)).collect();
    let key_width = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    told!("{screen}");
    for (c, label) in table.iter().zip(&labels) {
        let label = if label.is_empty() { "-" } else { label };
        told!("  {:id_width$}  {label:key_width$}  {}", c.id, c.title);
    }
    told!();
}

/// `cold-lava ext list`: every extension, built in or installed, with what it may do; then the
/// ones that didn't load and why, and those a workspace carries, which aren't loaded.
fn print_extensions(registry: &ext::Registry, workspace: Option<&Path>) {
    let installed = ext::installed::user_dir()
        .map(|d| ext::installed::load(&d).0)
        .unwrap_or_default();
    for ext in registry.ids() {
        let m = registry.manifest(ext);
        let on_disk = installed.iter().find(|i| i.manifest.name == m.name);
        let version = match m.version.as_str() {
            "" => String::new(),
            v => format!(" {v}"),
        };
        let from = match on_disk {
            Some(i) => i.launch.dir.display().to_string(),
            None => "built in".to_string(),
        };
        told!("{}{version}  {from}", m.name);
        if !m.description.is_empty() {
            told!("  {}", m.description);
        }
        if let Some(i) = on_disk {
            told!(
                "  runs: {} ({})",
                i.launch.run.join(" "),
                i.launch.transport
            );
        }
        let permissions: Vec<String> = (m.permissions.iter())
            .map(|p| {
                serde_json::to_value(p)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
            })
            .map(Option::unwrap_or_default)
            .collect();
        match permissions.is_empty() {
            true => told!("  permissions: none"),
            false => told!("  permissions: {}", permissions.join(", ")),
        }
        for c in &m.commands {
            told!("  command: {}  {}", c.id, c.title);
        }
        if !m.types.is_empty() {
            let ids: Vec<&str> = m.types.iter().map(|t| t.id.as_str()).collect();
            told!("  types: {}", ids.join(", "));
        }
        for t in &m.file_types {
            let new = t.new.as_ref().map(|n| format!(", new: {n}"));
            let left_out = match registry.owner(&t.extension) {
                Some(o) if o == ext => String::new(),
                Some(o) => format!(" (left out: {} opens them)", registry.manifest(o).name),
                None => " (left out: cold-lava opens them)".into(),
            };
            told!(
                "  opens: .{}  {}{}{left_out}",
                t.extension,
                t.title,
                new.unwrap_or_default()
            );
        }
        for (n, s) in m.shows.iter().enumerate() {
            let left_out = match registry.shows(ext, n) {
                true => "",
                false => " (left out)",
            };
            told!(
                "  shows: notes with {}  {}{left_out}",
                s.describe(),
                s.title
            );
        }
        if let Some(log) = on_disk.and_then(|i| i.launch.log.as_ref()) {
            told!("  log: {}", log.display());
        }
    }
    if let Some(dir) = ext::installed::user_dir()
        && installed.is_empty()
    {
        told!("no extensions installed in {}", dir.display());
    }
    if let Some(root) = workspace {
        let (local, _) = ext::installed::load(&root.join(".cold-lava/extensions"));
        for i in local {
            told!("{}  {}", i.manifest.name, i.launch.dir.display());
            told!(
                "  not loaded: extensions inside a workspace aren't loaded yet; copy it to the \
                 extensions folder in ~/.config/cold-lava to use it"
            );
        }
    }
    if !registry.mistakes().is_empty() {
        problem!("didn't load:");
        for e in registry.mistakes() {
            problem!("  {e}");
        }
    }
}

/// `cold-lava shapes`: each shape with where it comes from, then what it is made of, the data
/// it takes and what it does, one line each; then the glyphs.
/// What `cold-lava shapes --render ID --bind …` draws: the shape bound to data, after the
/// gesture `--do` if there is one (each step printed), its extension asked for the regions it
/// computes. `None` when `--json` printed it as data instead.
type Drawn = (
    std::sync::Arc<cold_lava_canvas::Shape>,
    Vec<Vec<String>>,
    Option<f64>,
);

fn bound_shape(
    root: &Path,
    found: &shapes::Shapes,
    id: &str,
    args: &Args,
    registry: &mut ext::Registry,
    json: bool,
) -> Result<Option<Drawn>> {
    use anyhow::anyhow;
    let mut sources = Vec::new();
    for b in &args.binds {
        let (input, source) = b
            .split_once('=')
            .ok_or_else(|| anyhow!("--bind {b:?}: write it INPUT=SOURCE, like a='type=volcano'"))?;
        let source = bind::Source::parse(source).map_err(|e| anyhow!("--bind {input}: {e}"))?;
        sources.push((input.trim().to_string(), source));
    }
    let mut core = owned::open(root, registry)?;
    let mut owners = owned::Owners::new(registry, &core);
    let mut binding = bind::Binding::new(&mut core, found, id, sources).map_err(|e| anyhow!(e))?;
    let mut done = None;
    if let Some(g) = args.values.get("do") {
        let gesture = act::Gesture::parse(g).map_err(|e| anyhow!("--do {e}"))?;
        let ext = binding.extension().map(String::from);
        let refused = |e: gateway::Error| anyhow!("--do: {e}");
        let acted = match &ext {
            Some(name) => owners.as_extension(&mut core, name, |host| {
                binding.act(host, found, &gesture, 0)
            })?,
            None => binding.act(&mut act::Direct::user(&mut core), found, &gesture, 0),
        };
        let ran = match (acted.map_err(refused)?, &ext) {
            (act::Acted::Ran(d), _) => d,
            (act::Acted::Ask(ask), Some(name)) => match owners
                .ask_shape(&mut core, found, name, ask)?
            {
                act::Answered::Gesture { steps } => owners
                    .as_extension(&mut core, name, |host| binding.run(host, &gesture, &steps))?
                    .map_err(refused)?,
                act::Answered::Region { .. } => unreachable!("the core checks the answer's form"),
            },
            (act::Acted::Ask(_), None) => unreachable!("only an extension's shape asks"),
        };
        for (owner, news) in core.take_item_news() {
            if owner == bind::OWNER {
                binding.heard(&core, &news);
            }
        }
        for event in core.poll() {
            binding.noticed(&core, &event);
        }
        if let Some(to) = &ran.rebind {
            binding
                .rebind(&mut core, found, to)
                .map_err(|e| anyhow!(e))?;
        }
        if !json {
            for step in &ran.steps {
                match step {
                    act::Did::Edited { item, later: false } => told!("edited {item}"),
                    act::Did::Edited { item, later: true } => {
                        told!("asked its file's owner to edit {item}")
                    }
                    act::Did::Created { path } => told!("created {path}"),
                    act::Did::Recipe { title } => told!("ran {title}"),
                }
            }
            if let Some(to) = &ran.rebind {
                told!("now a {}", to.shape);
            }
        }
        done = Some(ran);
    }
    if let Some(name) = binding.extension().map(String::from) {
        for ask in binding.region_asks(0) {
            let act::Ask::Region { region, .. } = &ask else {
                continue;
            };
            let region = region.clone();
            match owners.ask_shape(&mut core, found, &name, ask) {
                Ok(act::Answered::Region { items }) => binding.answered(&region, items),
                Ok(_) => {}
                Err(e) => problem!("{e}"),
            }
        }
    }
    let shown = binding.shown();
    if json {
        let out = match done {
            Some(done) => serde_json::json!({"done": done, "shown": shown}),
            None => serde_json::to_value(&shown)?,
        };
        say::json(&out)?;
        return Ok(None);
    }
    let aspect = binding.shape().definition.aspect;
    Ok(Some((binding.canvas(), shown.contents(), aspect)))
}

fn print_shapes(found: &shapes::Shapes) {
    for s in &found.shapes {
        let d = &s.definition;
        let title = if d.title.is_empty() {
            String::new()
        } else {
            format!("  {}", d.title)
        };
        told!("{}  {}{title}", s.id, s.origin);
        let mut about = Vec::new();
        if !d.inputs.is_empty() {
            about.push(format!("inputs {}", d.inputs.join(", ")));
        }
        if let Some(key) = &d.key {
            about.push(format!("key {key}"));
        }
        if let Some(aspect) = d.aspect {
            about.push(format!("aspect {aspect}"));
        }
        if !about.is_empty() {
            told!("  {}", about.join(" · "));
        }
        let parts: Vec<String> = (d.parts.iter())
            .map(|p| match (p, p.id()) {
                (shapes::PartDef::Shape(n), _) => format!("{} ({})", n.id, n.shape),
                (shapes::PartDef::Text(t), id) => match (&t.at, id) {
                    (Some(shapes::Pos::Anchor(a)), _) => format!("text at {a}"),
                    (Some(shapes::Pos::Point([x, y])), _) => format!("text at [{x}, {y}]"),
                    (None, Some(id)) => format!("text {id}"),
                    (None, None) => "text".into(),
                },
                (p, Some(id)) => format!("{} {id}", p.kind()),
                (p, None) => p.kind().to_string(),
            })
            .collect();
        told!("  parts {}", parts.join(", "));
        for r in &d.regions {
            let shows = r.shows.as_ref().map(|e| format!(", shows {e}"));
            told!("  region {}: {}{}", r.id, r.area, shows.unwrap_or_default());
        }
        for b in &d.on {
            let gesture = match (&b.drop, &b.click, &b.key, &b.action) {
                (Some(d), ..) => format!("drop {} → {}", d.from, d.to),
                (_, Some(c), ..) => format!("click {c}"),
                (_, _, Some(k), _) => format!("key {k}"),
                (.., Some(a)) if b.title.is_empty() => format!("action {a}"),
                (.., Some(a)) => format!("action {a} ({})", b.title),
                _ => String::new(),
            };
            let steps: Vec<String> = (b.edit.iter().map(|_| "edit".to_string()))
                .chain(b.steps.iter().map(|s| match s {
                    shapes::Step::Edit(_) => "edit".into(),
                    shapes::Step::Create(_) => "create a note".into(),
                    shapes::Step::Rebind(r) => format!("rebind to {}", r.shape),
                }))
                .collect();
            told!("  on {gesture}: {}", steps.join(", "));
        }
        told!();
    }
    let glyphs: Vec<String> = (found.glyphs.iter())
        .map(|g| format!("{} {}", g.symbol, g.name))
        .collect();
    told!("glyphs  {}", glyphs.join("  "));
}

/// `cold-lava types`: each type with where it comes from, then each shape with the fields it
/// reads, where, and whether a value can be written back there.
fn print_types(found: &types::Types, counts: Option<&HashMap<String, (usize, usize)>>) {
    told!("{}  built in, read natively", found.native.join(", "));
    for t in &found.types {
        let d = &t.definition;
        told!();
        told!("{}  {}", t.name, t.origin);
        if let Some(counts) = counts {
            let (n, unparsed) = counts.get(&t.name).copied().unwrap_or_default();
            let mut line = format!("  found {n} item{}", if n == 1 { "" } else { "s" });
            if unparsed > 0 {
                let s = if unparsed == 1 { " didn't" } else { "s didn't" };
                line += &format!(" · {unparsed} value{s} parse");
            }
            told!("{line}");
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
            told!("  {}", about.join(" · "));
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
            let mut line = format!("on {}", shape.on.alias());
            if let Some(role) = shape.on.role() {
                line += &format!(" · role {}", role.name());
            }
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
            told!("  {line}");
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
                told!(
                    "    {field:name_width$}  {value:value_width$}  {source:source_width$}  {rw}"
                );
            }
        }
        if !d.style.is_empty() {
            told!("  looks");
            for (field, look) in &d.style {
                told!("    {field:name_width$}  {}", look_label(look));
            }
        }
    }
    if !found.notes.is_empty() {
        told!();
        told!("notes:");
        for n in &found.notes {
            told!("  {n}");
        }
    }
}

/// Items found and values that didn't parse, by type, in the notes and the files other owners
/// read: an item counts for its own type only.
fn type_counts(workspace: &workspace::Workspace) -> HashMap<String, (usize, usize)> {
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new();
    let notes = (workspace.notes.iter()).flat_map(|n| workspace.items(n).iter());
    let owned = (workspace.owned.iter()).flat_map(|f| &f.items);
    for item in notes.chain(owned) {
        if let items::Data::Typed(t) = &item.data {
            let c = counts.entry(t.kind.clone()).or_default();
            c.0 += 1;
            c.1 += t.unparsed;
        }
    }
    counts
}

/// `cold-lava attributes`: how Markdown writes each attribute a type can read, with where
/// each reading comes from.
fn print_attributes(found: &attributes::Readings) {
    told!("level, callout, fold  read from the note's structure");
    for r in &found.readings {
        let read: Vec<String> = r.read.iter().map(source_label).collect();
        told!();
        told!("{}  on {} · {}", r.name, r.on.name(), r.origin);
        match read.is_empty() {
            true => told!("  taken out: on no piece"),
            false => told!("  {}", read.join(", then ")),
        }
        if !r.with.is_empty() {
            told!("  only with {}", r.with);
        }
        let ticked = attributes::TICKED.contains(&r.name.as_str()) && r.on.name() == "item";
        if ticked && r.origin != types::Origin::BuiltIn {
            told!("  but {}", attributes::TICKING);
        }
    }
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
        ("file-type", &w.file_type),
        ("section", &w.section),
        ("language", &w.language),
        ("inside", &w.inside),
        ("attribute", &w.attribute),
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
        | S::Children(t)
        | S::Attribute(t) => t.clone(),
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
/// Where a note's workspace is, for commands given a file: its marker, the workspace list, or
/// (said out loud by [`guessed_note`] when something isn't found) its own folder.
fn locate_note_workspace(path: &Path) -> (PathBuf, workspace::Found) {
    let known: Vec<PathBuf> = home::discover(&[]).into_iter().map(|w| w.path).collect();
    workspace::locate_workspace(path, &known)
}

/// What to add to an error when the workspace was only guessed.
fn guessed_note(root: &Path, found: workspace::Found) -> Option<String> {
    (found == workspace::Found::Guessed).then(|| {
        format!(
            "note: no .obsidian or .cold-lava folder was found above the note, and it isn't in \
             a workspace you've opened, so {} was taken as its workspace; ids from \
             `cold-lava items` on another folder won't match here. Add .obsidian (or .cold-lava) \
             to the workspace's folder.",
            root.display()
        )
    })
}

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
    out!("{} · {} to {}", a.workspace, a.from, a.to);
    if a.days.is_empty() {
        out!("nothing dated in this range");
    }
    for (day, info) in &a.days {
        let note = if info.daily_note {
            "  (daily note)"
        } else {
            ""
        };
        out!();
        out!("{day}{note}");
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
            out!("  [{mark}] {}   {}:{}", t.text, t.file, t.line);
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
