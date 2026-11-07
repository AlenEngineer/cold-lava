//! The main menu: vaults on the left, the selected vault's notes in the middle, and the
//! selected note on the right, in the reader or in the editor extension's pane (a real,
//! embedded Neovim when it's installed).

use crate::app::{self, ACCENT, ACCENT_SOFT, DIM, HIT, MUTED, NOTE, Render};
use crate::commands::{
    self, Entry, HistoryAction, Key, KeyHint, Keymap, MenuAction, MenuCommand, PaneAction,
    ReaderAction,
};
use crate::core::{Core, Event as CoreEvent, Jump};
use crate::embed::Embedded;
use crate::ext::{
    CommandRequest, EditorRequest, Effect, ExtId, Extension, Host, Manifest, PaneId, PaneNews,
    PaneSpec, Registry,
};
use crate::hints::{Hints, Typed};
use crate::history::{Exit, History, Nav, Place};
use crate::markdown::{self, Flavour};
use crate::paths::{config_dir, home_dir};
use crate::reader::{Reader, Spot};
use crate::trash::{self, TrashOption, Trashed, Went};
use crate::{board, templates, term, vault, whichkey};
use anyhow::{Result, anyhow};
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

// ---------- vault discovery ----------

#[derive(Clone)]
pub struct VaultEntry {
    pub name: String,
    pub path: PathBuf,
    /// Where we found it: "obsidian", "recent", "here".
    pub source: &'static str,
    /// Last opened (ms since epoch), for sorting.
    pub ts: u64,
}

/// Where Obsidian keeps its list of vaults on each platform (including Flatpak and Snap installs).
fn obsidian_configs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = home_dir() {
        v.push(h.join("Library/Application Support/obsidian/obsidian.json"));
        v.push(h.join(".var/app/md.obsidian.Obsidian/config/obsidian/obsidian.json"));
        v.push(h.join("snap/obsidian/current/.config/obsidian/obsidian.json"));
    }
    if let Some(c) = config_dir() {
        v.push(c.join("obsidian/obsidian.json"));
    }
    if let Some(a) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(a).join("obsidian/obsidian.json"));
    }
    v
}

/// Parses Obsidian's `obsidian.json`: `{"vaults": {"<id>": {"path": "...", "ts": 123}}}`.
pub fn parse_obsidian_vaults(json: &str) -> Vec<(PathBuf, u64)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(map) = v.get("vaults").and_then(|x| x.as_object()) else {
        return Vec::new();
    };
    map.values()
        .filter_map(|e| {
            Some((
                PathBuf::from(e.get("path")?.as_str()?),
                e.get("ts").and_then(|t| t.as_u64()).unwrap_or(0),
            ))
        })
        .collect()
}

fn recent_file() -> Option<PathBuf> {
    config_dir().map(|c| c.join("cold-lava/vaults.json"))
}

fn load_recent() -> Vec<(PathBuf, u64)> {
    let Some(text) = recent_file().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v.get("vaults")
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some((
                PathBuf::from(e.get("path")?.as_str()?),
                e.get("opened").and_then(|t| t.as_u64()).unwrap_or(0),
            ))
        })
        .collect()
}

fn remember(path: &Path) {
    let Some(file) = recent_file() else { return };
    let mut list = load_recent();
    list.retain(|(p, _)| p != path);
    list.insert(0, (path.to_path_buf(), now_ms()));
    list.truncate(50);
    let json = serde_json::json!({
        "vaults": list.iter().map(|(p, t)| serde_json::json!({"path": p, "opened": t})).collect::<Vec<_>>()
    });
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        file,
        serde_json::to_string_pretty(&json).unwrap_or_default(),
    );
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn looks_like_vault(p: &Path) -> bool {
    p.join(".obsidian").is_dir()
        || std::fs::read_dir(p)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| e.path().extension().is_some_and(|x| x == "md"))
            })
            .unwrap_or(false)
}

/// Every vault we know about, most recently used first, without duplicates.
pub fn discover(extra: &[PathBuf]) -> Vec<VaultEntry> {
    let mut found: Vec<(PathBuf, u64, &'static str)> = Vec::new();
    for p in extra {
        found.push((p.clone(), u64::MAX, "here"));
    }
    for (p, t) in load_recent() {
        found.push((p, t, "recent"));
    }
    for cfg in obsidian_configs() {
        if let Ok(text) = std::fs::read_to_string(&cfg) {
            for (p, t) in parse_obsidian_vaults(&text) {
                found.push((p, t, "obsidian"));
            }
        }
    }
    // The bundled example vault, when run from the repository.
    for base in [
        std::env::current_dir().ok(),
        Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))),
    ]
    .into_iter()
    .flatten()
    {
        let ex = base.join("example-vault");
        if ex.is_dir() {
            found.push((ex, 1, "example"));
        }
    }
    let mut seen = HashSet::new();
    let mut out: Vec<VaultEntry> = Vec::new();
    for (p, ts, source) in found {
        let Ok(canon) = p.canonicalize() else {
            continue;
        };
        if !canon.is_dir() {
            continue;
        }
        // The bundled example vault only if no vault of that name is listed already.
        if source == "example" && out.iter().any(|e| e.name == "example-vault") {
            continue;
        }
        if let Some(e) = out.iter_mut().find(|e| e.path == canon) {
            e.ts = e.ts.max(ts);
            if e.source != "here" && source == "obsidian" {
                e.source = "obsidian";
            }
            continue;
        }
        if seen.insert(canon.clone()) {
            let name = canon
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push(VaultEntry {
                name,
                path: canon,
                source,
                ts,
            });
        }
    }
    out.sort_by(|a, b| {
        b.ts.cmp(&a.ts)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

// ---------- the note tree ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    Folder,
    Note,
    Board,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub kind: ItemKind,
}

/// All folders, notes and boards in a vault, as a depth-first sorted list
/// (folders before files, case-insensitive).
pub fn scan_tree(root: &Path) -> Vec<Item> {
    let mut entries: Vec<(String, ItemKind)> = WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let rel = e
                .path()
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let kind = if e.file_type().is_dir() {
                ItemKind::Folder
            } else {
                match e.path().extension()?.to_str()? {
                    "md" => ItemKind::Note,
                    "canvas" => ItemKind::Board,
                    _ => return None,
                }
            };
            Some((rel, kind))
        })
        .collect();
    // Sort by path components, folders first at each level.
    entries.sort_by(|(a, ak), (b, bk)| {
        let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
        for i in 0..pa.len().min(pb.len()) {
            if pa[i] != pb[i] {
                // A component is a folder if it isn't the last one, or the item itself is a folder.
                let fa = i + 1 < pa.len() || *ak == ItemKind::Folder;
                let fb = i + 1 < pb.len() || *bk == ItemKind::Folder;
                return fb
                    .cmp(&fa)
                    .then(pa[i].to_lowercase().cmp(&pb[i].to_lowercase()));
            }
        }
        pa.len().cmp(&pb.len())
    });
    entries
        .into_iter()
        .map(|(rel, kind)| {
            let file = rel.rsplit('/').next().unwrap_or(&rel);
            let name = match kind {
                ItemKind::Note => file.trim_end_matches(".md"),
                ItemKind::Board => file.trim_end_matches(".canvas"),
                ItemKind::Folder => file,
            }
            .to_string();
            Item {
                depth: rel.matches('/').count(),
                rel,
                name,
                kind,
            }
        })
        .collect()
}

/// The rows to show: folders collapsed unless expanded; with a filter, a flat list of matches.
pub fn visible(items: &[Item], expanded: &HashSet<String>, filter: &str) -> Vec<usize> {
    if !filter.is_empty() {
        let q = vault::search_key(filter);
        let mut hits: Vec<(usize, bool)> = items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.kind != ItemKind::Folder && vault::search_key(&it.rel).contains(&q)
            })
            .map(|(i, it)| (i, vault::search_key(&it.name).starts_with(&q)))
            .collect();
        hits.sort_by_key(|&(i, prefix)| (!prefix, items[i].name.to_lowercase()));
        return hits.into_iter().map(|(i, _)| i).collect();
    }
    items
        .iter()
        .enumerate()
        .filter(|(_, it)| {
            // Visible if every ancestor folder is expanded.
            let parts: Vec<&str> = it.rel.split('/').collect();
            (1..parts.len()).all(|n| expanded.contains(&parts[..n].join("/")))
        })
        .map(|(i, _)| i)
        .collect()
}

// ---------- the menu ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Vaults,
    Notes,
    /// The right pane: the reader, or the program an extension runs there.
    Pane,
    Backlinks,
}

/// A backlink as shown in the pane under the editor.
struct BackRow {
    name: String,
    path: PathBuf,
    line: usize,
    context: String,
}

enum Mode {
    Normal,
    Filter,
    /// Typing a new note's name; `templates` are the vault's (vault-relative without `.md`, and
    /// the name shown), and `template` is the chosen one's place in them, `None` for none.
    NewNote {
        name: String,
        templates: Vec<(String, String)>,
        template: Option<usize>,
    },
    AddVault(String),
    Help,
    /// Which key: the commands this screen has, by group, until a key is typed.
    Keys,
    /// Quick switcher: jump to a note by (fuzzy) name.
    Switcher {
        query: String,
        sel: usize,
    },
    /// Command palette: run a command by (fuzzy) name.
    Palette {
        query: String,
        sel: usize,
    },
    /// Full-text search.
    Search {
        query: String,
        sel: usize,
    },
    /// Typing a note's new name (or folder, when moving).
    Rename {
        note: PathBuf,
        input: String,
        moving: bool,
    },
    /// Showing what a rename will change before doing it.
    ConfirmRename {
        plan: Box<crate::rename::Plan>,
        scroll: usize,
    },
    /// Asking before a note goes to the trash.
    ConfirmDelete(Box<PendingDelete>),
    /// Undoing a delete whose old path is taken now: put it back under a free name?
    ConfirmRestore {
        trashed: Trashed,
        to: PathBuf,
    },
    Hints(Hints<HintTarget>),
    /// The reader's headings, to jump to one; `sel` is a row of `outline_hits`.
    Outline {
        query: String,
        sel: usize,
    },
    /// Where the reader's selected link or the selected backlink leads, shown over the reader.
    Peek {
        from: PeekFrom,
        view: Peek,
    },
}

/// A delete waiting for a yes: the content the user is deciding on, and what links to it.
struct PendingDelete {
    path: PathBuf,
    /// Vault-relative, without `.md`.
    rel: String,
    bytes: Vec<u8>,
    backlinks: Vec<BackRow>,
    /// Which trash it goes to, as the prompt says it.
    goes: String,
    /// It changed on disk after the first prompt, so this is the question again.
    changed: bool,
    scroll: usize,
}

/// Deletes that `note.undo-delete` can still put back.
const UNDO_DELETES: usize = 20;

/// What a peek looks at: Tab sweeps through these and Enter goes where the selected one leads.
#[derive(Clone, Copy, PartialEq)]
enum PeekFrom {
    Link,
    Backlink,
}

/// Where a link leads, as a peek shows it.
enum Peek {
    /// The note, in its own reader, at the link's heading, block or line.
    Note {
        path: PathBuf,
        reader: Box<Reader>,
    },
    /// No note by this name yet: following the link creates it.
    Missing(String),
    Url(String),
    Attachment {
        name: String,
        kind: String,
        /// Whether it is in the vault, so following it opens it.
        found: bool,
    },
}

/// Where a wiki or Markdown link written in a note leads.
enum Destination {
    /// A note, at a heading or block when the link names one.
    Note(PathBuf, Option<Jump>),
    /// No note by this name yet: following the link creates it.
    NewNote(String),
    /// A file that isn't a note (`map.pdf`, a canvas), and where it is if it's in the vault.
    Attachment { name: String, path: Option<PathBuf> },
}

/// What a link hint leads to: one of the reader's links, or a backlink.
#[derive(Clone, Copy)]
enum HintTarget {
    Link(usize),
    Backlink(usize),
}

/// What is inside a pane's border.
fn inner(a: Rect) -> Rect {
    Rect::new(
        a.x + 1,
        a.y + 1,
        a.width.saturating_sub(2),
        a.height.saturating_sub(2),
    )
}

/// One row of the search results: a note header, or one of its matching lines.
#[derive(Clone, Copy)]
enum SearchRow {
    Note(usize),
    Line(usize, usize),
}

pub struct Home {
    render: Render,
    vaults: Vec<VaultEntry>,
    vsel: usize,
    // The open vault
    root: Option<PathBuf>,
    items: Vec<Item>,
    expanded: HashSet<String>,
    rows: Vec<usize>,
    nsel: usize,
    nscroll: usize,
    filter: String,
    links: HashMap<String, (usize, usize)>,
    /// The open vault: index, file watching and change events.
    core: Option<Core>,
    backs: Vec<BackRow>,
    bsel: usize,
    show_backlinks: bool,
    keys: Keymap<MenuCommand>,
    reader_keys: Keymap<ReaderAction>,
    pane_keys: Keymap<PaneAction>,
    /// Recently shown notes, most recent first (the quick switcher's starting list).
    mru: Vec<PathBuf>,
    /// Back and forward, within the open vault.
    history: History,
    results: Vec<crate::search::NoteHits>,
    result_rows: Vec<SearchRow>,
    /// The overlay was opened from inside the pane's program: give it focus back afterwards.
    back_to_editor: bool,
    // Right pane
    registry: Registry,
    pane: Option<Pane>,
    /// Commands extensions asked to run (`command/run`), as (who asked, id, args). They run
    /// before the next frame is drawn, where the terminal is at hand.
    requested: Vec<(ExtId, String, Value)>,
    /// What extensions may still ask for this frame (see `call`).
    effects_left: usize,
    shown: Option<PathBuf>,
    pending: Option<(PathBuf, Instant)>,
    /// What was deleted this session, most recent last.
    trashed: Vec<Trashed>,
    /// A screen a link led to, gone to once the key that followed it is handled.
    go_to: Option<Place>,
    /// `Some` while the pane shows the reader: always without an editor, else after
    /// `note.read`.
    reader: Option<Reader>,
    /// The open vault's Markdown flavour.
    flavour: Flavour,
    zoom: bool,
    // UI
    focus: Focus,
    mode: Mode,
    msg: Option<(String, Instant)>,
    areas: [Rect; 4],
    quit: bool,
}

/// A program an extension runs in the right pane. The core draws it and types into it.
struct Pane {
    id: PaneId,
    owner: ExtId,
    term: Embedded,
}

/// What extensions may ask for in one frame. Applying an effect can call an extension again, so
/// without a limit two of them could echo each other forever.
const EFFECTS_PER_FRAME: usize = 256;

impl Home {
    pub fn new(render: Render, initial: Option<PathBuf>, registry: Registry) -> Home {
        let extra: Vec<PathBuf> = initial.iter().cloned().collect();
        let mut extra = extra;
        if extra.is_empty()
            && let Ok(cwd) = std::env::current_dir()
            && cwd.join(".obsidian").is_dir()
        {
            extra.push(cwd);
        }
        let vaults = discover(&extra);
        let mut h = Home {
            render,
            vaults,
            vsel: 0,
            root: None,
            items: Vec::new(),
            expanded: HashSet::new(),
            rows: Vec::new(),
            nsel: 0,
            nscroll: 0,
            filter: String::new(),
            links: HashMap::new(),
            core: None,
            backs: Vec::new(),
            bsel: 0,
            show_backlinks: true,
            keys: Keymap::configured(registry.commands()),
            reader_keys: Keymap::new(commands::READER),
            pane_keys: Keymap::new(commands::PANE),
            mru: Vec::new(),
            history: History::default(),
            results: Vec::new(),
            result_rows: Vec::new(),
            back_to_editor: false,
            registry,
            pane: None,
            requested: Vec::new(),
            effects_left: EFFECTS_PER_FRAME,
            shown: None,
            pending: None,
            trashed: Vec::new(),
            go_to: None,
            reader: None,
            flavour: markdown::flavour_for(Path::new("")),
            zoom: false,
            focus: Focus::Vaults,
            mode: Mode::Normal,
            msg: None,
            areas: [Rect::default(); 4],
            quit: false,
        };
        if !h.vaults.is_empty() {
            h.open_vault(0);
            if initial.is_some() {
                h.focus = Focus::Notes;
            }
        }
        match (crate::config::get().errors.len(), h.registry.refused()) {
            (0, []) => {}
            (0, [why]) => h.flash(why.clone()),
            (0, refused) => h.flash(format!(
                "{} extension commands or keys were left out: run cold-lava keys to see why",
                refused.len()
            )),
            (1, _) => h.flash("config.toml has a problem: run cold-lava keys to see it"),
            (n, _) => h.flash(format!(
                "config.toml has {n} problems: run cold-lava keys to see them"
            )),
        }
        h
    }

    /// Select a note (expanding its folders) and open it in the editor pane, focused.
    pub fn focus_note(&mut self, path: &Path) {
        let (Some(root), Ok(canon)) = (self.root.clone(), path.canonicalize()) else {
            return;
        };
        let Ok(rel) = canon.strip_prefix(&root) else {
            return;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let from = self.here();
        self.leave(from, &canon, None);
        self.reload_tree();
        let parts: Vec<&str> = rel.split('/').collect();
        for n in 1..parts.len() {
            self.expanded.insert(parts[..n].join("/"));
        }
        self.filter.clear();
        self.refresh_rows();
        if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == rel) {
            self.nsel = r;
        }
        self.pending = None;
        self.show(&canon);
        self.focus_pane();
    }

    /// The right pane takes focus if it shows something to focus.
    fn focus_pane(&mut self) {
        self.focus = if self.pane.is_some() || self.reader.is_some() {
            Focus::Pane
        } else {
            Focus::Notes
        };
    }

    fn open_today(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        match crate::daily::ensure(&root, crate::tasks::today()) {
            Ok(p) => self.focus_note(&p),
            Err(e) => self.flash(format!("couldn't create today's note: {e}")),
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            self.effects_left = EFFECTS_PER_FRAME;
            self.pump_pane();
            self.run_requested(terminal)?;
            if let Some(place) = self.go_to.take() {
                self.go_from_here(place, terminal)?;
            }
            self.load_pending();
            self.check_disk();
            terminal.draw(|f| self.draw(f))?;
            let ms = if self.pane.is_some() || self.pending.is_some() {
                15
            } else {
                250
            };
            if event::poll(Duration::from_millis(ms))? {
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind == KeyEventKind::Press => {
                            self.on_key(k, terminal)?
                        }
                        Event::Mouse(m) => self.on_mouse(m),
                        _ => {}
                    }
                    if self.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        self.save_editor();
        Ok(())
    }

    /// Bring the pane's screen up to date, and tell its extension what the program said and
    /// whether it ended.
    fn pump_pane(&mut self) {
        let Some(p) = self.pane.as_mut() else {
            return;
        };
        p.term.pump();
        let (pane, owner) = (p.id, p.owner);
        let (messages, exited) = (p.term.take_messages(), p.term.exited());
        for (kind, payload) in messages {
            let news = PaneNews::Message {
                pane,
                kind,
                payload,
            };
            self.call(owner, |e, host| e.pane(news, host));
        }
        if exited {
            self.pane = None;
            // Nothing shows the note now, so selecting it again opens it again.
            if self.reader.is_none() {
                self.shown = None;
                if self.focus == Focus::Pane {
                    self.focus = Focus::Notes;
                }
            }
            let news = PaneNews::Exited { pane, error: None };
            self.call(owner, |e, host| e.pane(news, host));
        }
    }

    /// Call an extension, then do what it asked for.
    fn call(&mut self, ext: ExtId, f: impl FnOnce(&mut dyn Extension, &mut Host<'_>)) {
        for effect in self.registry.call(ext, f) {
            if self.effects_left == 0 {
                let name = &self.registry.manifest(ext).name;
                let msg = format!("{name} asked for too much at once; the rest was skipped");
                return self.flash(msg);
            }
            self.effects_left -= 1;
            self.apply(ext, effect);
        }
    }

    fn apply(&mut self, ext: ExtId, effect: Effect) {
        match effect {
            Effect::Notify { message } => self.flash(message),
            Effect::EditorOpened { note } => {
                if self.shown.as_deref() != Some(note.as_path()) {
                    self.remember_recent(&note);
                    self.shown = Some(note.clone());
                    self.select_path(&note);
                    self.bsel = 0;
                    self.refresh_backlinks();
                }
            }
            Effect::RunCommand { id, args } => self.requested.push((ext, id, args)),
            Effect::Saved { note } => {
                if let Some(core) = self.core.as_mut() {
                    core.note_changed(&note);
                }
            }
            Effect::OpenPane { pane, spec } => self.open_pane(ext, pane, spec),
            Effect::PaneInput { pane, text } => {
                if let Some(p) = self
                    .pane
                    .as_mut()
                    .filter(|p| p.id == pane && p.owner == ext)
                {
                    p.term.send(text.as_bytes());
                }
            }
        }
    }

    /// Start `spec` in the pane. If it can't start, or another pane is open, the extension hears
    /// that its pane exited; an editor that can't start is dropped, and the reader shows notes.
    fn open_pane(&mut self, owner: ExtId, pane: PaneId, spec: PaneSpec) {
        let started = if self.pane.is_some() {
            Err(anyhow!("another pane is open"))
        } else {
            let a = self.areas[2];
            let cwd = self.root.clone().unwrap_or_else(|| PathBuf::from("."));
            Embedded::spawn(
                &spec.program,
                &spec.args,
                &cwd,
                a.height.max(5),
                a.width.max(20),
            )
        };
        let error = match started {
            Ok(term) => {
                self.pane = Some(Pane {
                    id: pane,
                    owner,
                    term,
                });
                return;
            }
            Err(e) => format!("{e:#}"),
        };
        // The whole error goes to the extension: on the status line it would hide the rest.
        let fallback = if self.registry.editor() == Some(owner) {
            self.registry.drop_editor();
            "; showing the reader"
        } else {
            ""
        };
        self.flash(format!("couldn't start {}{fallback}", spec.program));
        let news = PaneNews::Exited {
            pane,
            error: Some(error),
        };
        self.call(owner, |e, host| e.pane(news, host));
    }

    /// Run the commands extensions asked for before this frame. Those they ask for meanwhile
    /// wait for the next one, so a command that keeps running itself can't freeze the menu.
    fn run_requested(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        for (from, id, args) in std::mem::take(&mut self.requested) {
            self.run_command(from, &id, args, terminal)?;
        }
        Ok(())
    }

    /// A command an extension asked to run, found in the menu's table like a key's. Only
    /// `note.follow-link`, which needs its `args`, isn't in it; `search.open` takes a `query`.
    fn run_command(
        &mut self,
        from: ExtId,
        id: &str,
        args: Value,
        terminal: &mut DefaultTerminal,
    ) -> Result<()> {
        #[derive(Deserialize)]
        struct FollowLink {
            from: PathBuf,
            link: String,
        }
        #[derive(Deserialize)]
        struct Search {
            query: String,
        }
        let found = self.registry.commands().iter().find(|c| c.id == id);
        let done = match found.map(|c| c.action) {
            _ if id == "note.follow-link" => serde_json::from_value(args)
                .map(|FollowLink { from, link }| self.follow(&from, &link))
                .map_err(|e| e.to_string()),
            None => Err("there is no such command".to_string()),
            Some(c) if !self.enabled(c) => Err("it can't run now".to_string()),
            Some(MenuCommand::Core(MenuAction::Search)) if !args.is_null() => {
                serde_json::from_value(args)
                    .map(|Search { query }| self.search_for(query))
                    .map_err(|e| e.to_string())
            }
            Some(c) => return self.run_menu_command(c, args, terminal),
        };
        if let Err(e) = done {
            let name = &self.registry.manifest(from).name;
            let msg = format!("{name} couldn't run {id}: {e}");
            self.flash(msg);
        }
        Ok(())
    }

    /// A menu command, from a key, the palette or an extension. Only extensions' commands take
    /// `args`.
    fn run_menu_command(
        &mut self,
        c: MenuCommand,
        args: Value,
        terminal: &mut DefaultTerminal,
    ) -> Result<()> {
        match c {
            MenuCommand::Core(a) => self.run_action(a, terminal)?,
            MenuCommand::Ext(ext, i) => {
                let id = self.registry.manifest(ext).commands[i].id.clone();
                let request = CommandRequest::Run { id, args };
                self.call(ext, |e, host| e.command(request, host));
            }
        }
        Ok(())
    }

    // ---------- vaults & notes ----------

    fn open_vault(&mut self, i: usize) {
        let Some(v) = self.vaults.get(i).cloned() else {
            return;
        };
        self.vsel = i;
        self.root = Some(v.path.clone());
        self.flavour = markdown::flavour_for(&v.path);
        self.expanded.clear();
        self.filter.clear();
        self.nsel = 0;
        self.nscroll = 0;
        self.core = None; // a different vault: start a fresh core
        self.history.clear();
        self.reload_tree();
        // Start with top-level folders open if the vault is small enough to take in at once.
        if self.items.len() <= 60 {
            for it in &self.items {
                if it.kind == ItemKind::Folder && it.depth == 0 {
                    self.expanded.insert(it.rel.clone());
                }
            }
            self.refresh_rows();
        }
        if self.core.as_ref().is_some_and(|c| !c.is_live()) {
            self.flash("can't watch this folder for changes; press r to refresh");
        }
        remember(&v.path);
        self.select_row(0);
    }

    fn reload_tree(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.set_items(scan_tree(&root));
        self.links.clear();
        match self.core.as_mut() {
            Some(core) => core.rescan_now(),
            None => self.core = Core::open(&root).ok(),
        }
        if let Some(ix) = self.core.as_ref().map(|c| &c.index) {
            for (i, n) in ix.vault.notes.iter().enumerate() {
                self.links.insert(format!("{}.md", n.rel), ix.counts(i));
            }
        }
        self.refresh_rows();
        self.refresh_backlinks();
    }

    /// Backlinks of the note in the editor pane.
    fn refresh_backlinks(&mut self) {
        self.backs.clear();
        let (Some(ix), Some(shown)) = (self.core.as_ref().map(|c| &c.index), &self.shown) else {
            return;
        };
        let Some(i) = ix.by_path(shown) else { return };
        self.backs = ix
            .backlinks(i)
            .iter()
            .map(|b| BackRow {
                name: ix.vault.notes[b.from].name.clone(),
                path: ix.path(b.from),
                line: b.line,
                context: b.context.clone(),
            })
            .collect();
        self.bsel = self.bsel.min(self.backs.len().saturating_sub(1));
    }

    /// Where a link written in `from` leads, resolved the way Obsidian resolves it.
    fn destination(&self, from: &Path, raw: &str) -> Option<Destination> {
        let link = crate::index::split_link(raw);
        let ix = &self.core.as_ref()?.index;
        let jump = link
            .heading
            .map(Jump::Heading)
            .or(link.block.map(Jump::Block));
        if link.note.is_empty() {
            return Some(Destination::Note(from.to_path_buf(), jump));
        }
        if let Some(i) = ix.resolver.resolve(&link.note) {
            return Some(Destination::Note(ix.path(i), jump));
        }
        if vault::attachment_kind(&link.note).is_some() {
            let path = ix.attachment(&link.note, ix.by_path(from));
            return Some(Destination::Attachment {
                name: link.note,
                path,
            });
        }
        Some(Destination::NewNote(link.note))
    }

    /// Follow a link written in `from`: open the target note (at its heading or block), or
    /// create it if it doesn't exist yet, like clicking an unresolved link in Obsidian.
    /// Attachments open in the system's app, whiteboards in the board; a missing one is
    /// reported, never created.
    fn follow(&mut self, from: &Path, raw: &str) {
        let here = self.here();
        let Some(to) = self.destination(from, raw) else {
            return;
        };
        match to {
            Destination::Note(path, jump) => {
                if path == from && jump.is_none() {
                    return;
                }
                self.leave(here, &path, jump.as_ref());
                self.show_at(&path, jump);
                self.select_path(&path);
            }
            Destination::NewNote(name) => {
                let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
                    return;
                };
                let path = ix.new_note_path(&name, ix.by_path(from));
                if let Err(e) = crate::write::create_if_missing(&path, b"") {
                    return self.flash(format!("couldn't create the note: {e}"));
                }
                let rel = path
                    .strip_prefix(self.root.as_deref().unwrap_or(Path::new("")))
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.flash(format!("created {rel}"));
                self.reload_tree();
                self.leave(here, &path, None);
                self.show_at(&path, None);
                self.select_path(&path);
            }
            Destination::Attachment { name, path: None } => {
                self.flash(format!("{name} isn't in the vault"))
            }
            Destination::Attachment {
                path: Some(path), ..
            } if vault::attachment_kind(&path.to_string_lossy()) == Some("whiteboard") => {
                self.go_to = Some(Place::Board { path })
            }
            Destination::Attachment {
                name,
                path: Some(path),
            } => {
                if term::open_url(&path.to_string_lossy()) {
                    self.flash(format!("opened {name}"));
                } else {
                    self.flash(format!("couldn't open {name}"));
                }
            }
        }
    }

    /// Where you are, as Back would return to it: the search, else the note in the pane.
    fn here(&self) -> Option<Place> {
        if let Mode::Search { query, sel } = &self.mode {
            return Some(Place::Search {
                query: query.clone(),
                sel: *sel,
            });
        }
        Some(Place::Note {
            path: self.shown.clone()?,
            spot: self.reader.as_ref().map(Reader::spot),
        })
    }

    /// Going to `path` (at `jump`) from `from`: `from` becomes the way back, unless it is that
    /// same note with nowhere to jump to.
    fn leave(&mut self, from: Option<Place>, path: &Path, jump: Option<&Jump>) {
        let same = matches!(&from, Some(Place::Note { path: p, .. }) if p == path);
        if let Some(from) = from.filter(|_| jump.is_some() || !same) {
            self.history.visit(from);
        }
    }

    fn travel(&mut self, a: HistoryAction, terminal: &mut DefaultTerminal) -> Result<()> {
        let here = self.here();
        let to = match a {
            HistoryAction::Back => self.history.back(here),
            HistoryAction::Forward => self.history.forward(here),
        };
        match to {
            Some(place) => self.go(place, terminal),
            None => Ok(()),
        }
    }

    /// Go to the graph or a board from here, which becomes the way back.
    fn go_from_here(&mut self, to: Place, terminal: &mut DefaultTerminal) -> Result<()> {
        if let Some(here) = self.here() {
            self.history.visit(here);
        }
        self.go(to, terminal)
    }

    /// Go to `place`. The graph and boards run until they are left, maybe for another place.
    fn go(&mut self, place: Place, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut next = Some(place);
        while let Some(place) = next.take() {
            next = match place {
                Place::Note { path, spot } => {
                    self.return_to_note(&path, spot);
                    None
                }
                Place::Search { query, sel } => {
                    self.search_for(query);
                    if let Mode::Search { sel: s, .. } = &mut self.mode {
                        *s = sel.min(self.result_rows.len().saturating_sub(1));
                    }
                    None
                }
                Place::Graph { selected } => self.run_graph(terminal, selected)?,
                Place::Board { path } => self.run_board(terminal, path)?,
            };
        }
        Ok(())
    }

    /// Where leaving the graph or a board leads. Quitting goes to the menu, so Back comes back.
    fn left(&mut self, here: Place, exit: Exit) -> Option<Place> {
        match exit {
            Exit::Quit => {
                self.history.visit(here);
                None
            }
            Exit::Back => self.history.back(Some(here)),
            Exit::Forward => self.history.forward(Some(here)),
            Exit::Board(path) => {
                self.history.visit(here);
                Some(Place::Board { path })
            }
        }
    }

    fn return_to_note(&mut self, path: &Path, spot: Option<Spot>) {
        if !path.exists() {
            let name = path.file_stem().unwrap_or_default().to_string_lossy();
            return self.flash(format!("{name} is gone"));
        }
        self.mode = Mode::Normal;
        self.pending = None;
        self.show(path);
        self.select_path(path);
        if let (Some(r), Some(spot)) = (self.reader.as_mut(), spot) {
            r.restore(spot);
        }
        // Into the reader, where Back still works; the editor's pane would take the keys.
        if self.reader.is_some() {
            self.focus = Focus::Pane;
        }
    }

    /// Select a path in the tree (opening its folders), without opening it again.
    fn select_path(&mut self, path: &Path) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let Ok(rel) = path.strip_prefix(&root) else {
            return;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let parts: Vec<&str> = rel.split('/').collect();
        for n in 1..parts.len() {
            self.expanded.insert(parts[..n].join("/"));
        }
        if !self.filter.is_empty() && !self.rows.iter().any(|&i| self.items[i].rel == rel) {
            self.filter.clear();
        }
        self.refresh_rows();
        if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == rel) {
            self.nsel = r;
        }
    }

    /// Replace the tree's items, keeping the selection on the same path when it's still there
    /// (`rows` index the old items, so it's read before they go).
    fn set_items(&mut self, items: Vec<Item>) {
        let keep = self.current().map(|it| it.rel.clone());
        self.items = items;
        self.rows_keeping(keep);
    }

    fn refresh_rows(&mut self) {
        let keep = self.current().map(|it| it.rel.clone());
        self.rows_keeping(keep);
    }

    fn rows_keeping(&mut self, keep: Option<String>) {
        self.rows = visible(&self.items, &self.expanded, &self.filter);
        self.nsel = keep
            .and_then(|r| self.rows.iter().position(|&i| self.items[i].rel == r))
            .unwrap_or(self.nsel)
            .min(self.rows.len().saturating_sub(1));
    }

    fn current(&self) -> Option<&Item> {
        self.rows.get(self.nsel).map(|&i| &self.items[i])
    }

    fn current_path(&self) -> Option<PathBuf> {
        Some(self.root.as_ref()?.join(&self.current()?.rel))
    }

    fn select_row(&mut self, r: usize) {
        if self.rows.is_empty() {
            return;
        }
        self.nsel = r.min(self.rows.len() - 1);
        if let (Some(it), Some(p)) = (self.current().cloned(), self.current_path())
            && it.kind == ItemKind::Note
        {
            // Show it after a short pause, so holding j doesn't open every note on the way.
            self.pending = Some((p, Instant::now() + Duration::from_millis(120)));
        }
    }

    fn load_pending(&mut self) {
        match &self.pending {
            Some((_, t)) if Instant::now() >= *t => {}
            _ => return,
        }
        let (path, _) = self.pending.take().unwrap();
        self.show(&path);
    }

    /// Show a note in the right pane (reader, or embedded editor).
    fn show(&mut self, path: &Path) {
        self.show_at(path, None);
    }

    fn show_at(&mut self, path: &Path, jump: Option<Jump>) {
        let same = self.shown.as_deref() == Some(path);
        if same && jump.is_none() {
            return;
        }
        if !same {
            self.bsel = 0;
            self.remember_recent(path);
            self.shown = Some(path.to_path_buf());
            self.refresh_backlinks();
        }
        if self.registry.editor().is_none() || self.reader.is_some() {
            if !same || self.reader.is_none() {
                self.reader = Some(Reader::new(self.read_shown()));
            }
            if let (Some(r), Some(j)) = (self.reader.as_mut(), jump.clone()) {
                r.jump(j);
            }
        }
        // The editor follows along while the reader is shown, so switching back finds the note.
        let Some(editor) = self.registry.editor() else {
            return;
        };
        let request = EditorRequest::Open {
            note: path.to_path_buf(),
            jump: jump.clone(),
        };
        self.call(editor, |e, host| e.editor(request, host));
        if self.registry.editor().is_none() {
            // It couldn't start: show the note in the reader instead.
            self.shown = None;
            self.show_at(path, jump);
        }
    }

    /// Apply what changed on disk (from the core's events).
    fn check_disk(&mut self) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        let events = core.poll();
        if events.is_empty() {
            return;
        }
        self.sync_with_core(&events);
    }

    /// Bring the tree, link counts, backlinks and reader up to date with the core's index.
    fn sync_with_core(&mut self, events: &[CoreEvent]) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let tree_changed = events.iter().any(|e| {
            !matches!(
                e,
                CoreEvent::NoteChanged(_) | CoreEvent::GraphSettingsChanged
            )
        });
        if tree_changed {
            self.set_items(scan_tree(&root));
        }
        self.links.clear();
        if let Some(ix) = self.core.as_ref().map(|c| &c.index) {
            for (i, n) in ix.vault.notes.iter().enumerate() {
                self.links.insert(format!("{}.md", n.rel), ix.counts(i));
            }
        }
        self.refresh_rows();
        self.refresh_backlinks();
        let shown_rel = self
            .shown
            .as_ref()
            .and_then(|p| p.strip_prefix(&root).ok())
            .map(|r| r.to_string_lossy().replace('\\', "/"));
        let shown_rel = shown_rel.as_deref().map(|r| r.trim_end_matches(".md"));
        let reread = events.iter().any(|e| match e {
            CoreEvent::NoteChanged(rel)
            | CoreEvent::NoteCreated(rel)
            | CoreEvent::NoteRemoved(rel) => Some(rel.as_str()) == shown_rel,
            CoreEvent::Rescanned => true,
            CoreEvent::BoardChanged(_) | CoreEvent::GraphSettingsChanged => false,
        });
        if reread {
            self.reload_reader();
        } else if let Some(r) = self.reader.as_mut() {
            r.invalidate();
        }
    }

    /// The shown note, parsed.
    fn read_shown(&self) -> markdown::Doc {
        match &self.shown {
            Some(p) => self.read_note(p),
            None => markdown::parse("", &self.flavour),
        }
    }

    fn read_note(&self, path: &Path) -> markdown::Doc {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        markdown::parse(&text, &self.flavour)
    }

    /// Read the shown note again, keeping the reader's place.
    fn reload_reader(&mut self) {
        let doc = self.read_shown();
        if let Some(r) = self.reader.as_mut() {
            r.set_doc(doc);
        }
    }

    fn flash(&mut self, s: impl Into<String>) {
        self.msg = Some((s.into(), Instant::now()));
    }

    // ---------- input ----------

    fn on_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // Keys go straight to the pane's program, except pane.leave (back to the menu).
        if self.focus == Focus::Pane && self.reader.is_none() {
            match (self.pane_keys.get(k), self.pane.as_mut()) {
                (None, Some(p)) => p.term.send_key(k),
                (Some(PaneAction::Leave), _) | (_, None) => self.focus = Focus::Notes,
            }
            return Ok(());
        }
        if let Mode::Hints(hints) = &mut self.mode {
            let plain = !k
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
            match k.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Char(c) if plain => match hints.type_char(c) {
                    Typed::Pending => {}
                    Typed::NoMatch => {
                        let typed = hints.typed().to_string();
                        self.mode = Mode::Normal;
                        self.flash(format!("no link labelled {typed}"));
                    }
                    Typed::Chosen(target) => {
                        self.mode = Mode::Normal;
                        self.follow_hint(target);
                    }
                },
                _ => {}
            }
            return Ok(());
        }
        if matches!(self.mode, Mode::Peek { .. }) {
            self.peek_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Switcher { .. }) {
            self.switcher_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Outline { .. }) {
            self.outline_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Palette { .. }) {
            return self.palette_key(k, terminal);
        }
        if matches!(self.mode, Mode::Search { .. }) {
            self.search_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Rename { .. } | Mode::ConfirmRename { .. }) {
            self.rename_key(k);
            return Ok(());
        }
        if matches!(
            self.mode,
            Mode::ConfirmDelete(_) | Mode::ConfirmRestore { .. }
        ) {
            self.delete_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Keys) {
            self.mode = Mode::Normal;
            if !self.key_hints().iter().any(|h| h.key == Key::from(k)) {
                return Ok(());
            }
        }
        match &mut self.mode {
            Mode::Help => {
                self.mode = Mode::Normal;
                return Ok(());
            }
            Mode::Filter => {
                match k.code {
                    KeyCode::Esc => {
                        self.filter.clear();
                        self.mode = Mode::Normal;
                        self.refresh_rows();
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        self.focus = Focus::Notes;
                        self.select_row(self.nsel);
                    }
                    KeyCode::Down => self.select_row(self.nsel + 1),
                    KeyCode::Up => self.select_row(self.nsel.saturating_sub(1)),
                    KeyCode::Backspace => {
                        self.filter.pop();
                        self.nsel = 0;
                        self.refresh_rows();
                        self.select_row(0);
                    }
                    KeyCode::Char('u') if ctrl => {
                        self.filter.clear();
                        self.nsel = 0;
                        self.refresh_rows();
                    }
                    KeyCode::Char(_) if ctrl => {}
                    KeyCode::Char(c) => {
                        self.filter.push(c);
                        self.nsel = 0;
                        self.refresh_rows();
                        self.select_row(0);
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::NewNote {
                name,
                templates,
                template,
            } => {
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        name.pop();
                    }
                    KeyCode::Tab | KeyCode::Down if !templates.is_empty() => {
                        *template = match *template {
                            None => Some(0),
                            Some(i) if i + 1 < templates.len() => Some(i + 1),
                            Some(_) => None,
                        };
                    }
                    KeyCode::BackTab | KeyCode::Up if !templates.is_empty() => {
                        *template = match *template {
                            None => Some(templates.len() - 1),
                            Some(0) => None,
                            Some(i) => Some(i - 1),
                        };
                    }
                    KeyCode::Char('u') if ctrl => name.clear(),
                    KeyCode::Char(_) if ctrl => {}
                    KeyCode::Char(c) => name.push(c),
                    KeyCode::Enter => {
                        let text = name.trim().to_string();
                        let template = template.map(|i| templates[i].0.clone());
                        self.mode = Mode::Normal;
                        if !text.is_empty() {
                            self.create_note(&text, template.as_deref())?;
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::AddVault(name) => {
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        name.pop();
                    }
                    KeyCode::Char('u') if ctrl => name.clear(),
                    KeyCode::Char(_) if ctrl => {}
                    KeyCode::Char(c) => name.push(c),
                    KeyCode::Enter => {
                        let text = name.trim().to_string();
                        self.mode = Mode::Normal;
                        if !text.is_empty() {
                            self.add_vault(&text);
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::Switcher { .. }
            | Mode::Palette { .. }
            | Mode::Search { .. }
            | Mode::Rename { .. }
            | Mode::ConfirmRename { .. }
            | Mode::ConfirmDelete(_)
            | Mode::ConfirmRestore { .. }
            | Mode::Hints(_)
            | Mode::Outline { .. }
            | Mode::Peek { .. } => return Ok(()),
            Mode::Normal | Mode::Keys => {}
        }

        if self.focus == Focus::Pane
            && let Some(a) = self.reader_keys.get(k)
        {
            return self.run_reader_action(a, terminal);
        }
        // A command whose guard fails leaves the key to the focused pane (Esc, for one).
        if let Some(c) = self.keys.get(k).filter(|&c| self.enabled(c)) {
            return self.run_menu_command(c, Value::Null, terminal);
        }
        match self.focus {
            Focus::Vaults => self.vault_key(k),
            Focus::Notes => self.notes_key(k, terminal)?,
            Focus::Backlinks => self.backlinks_key(k),
            Focus::Pane => {}
        }
        Ok(())
    }

    /// The which-key popup's lines: in the reader its own commands, then the menu's on keys
    /// it leaves free; only commands that can run now.
    fn key_hints(&self) -> Vec<KeyHint> {
        let reading = self.focus == Focus::Pane && self.reader.is_some();
        let mut hints = Vec::new();
        if reading {
            let table = commands::entries(commands::READER);
            hints = commands::key_hints(&table, &self.reader_keys, |_| false, |_| true);
        }
        hints.extend(commands::key_hints(
            self.registry.commands(),
            &self.keys,
            |k| reading && self.reader_keys.has(k),
            |c| c != MenuCommand::Core(MenuAction::Keys) && self.enabled(c),
        ));
        hints
    }

    fn enabled(&self, c: MenuCommand) -> bool {
        match c {
            MenuCommand::Core(MenuAction::ClearFilter) => !self.filter.is_empty(),
            MenuCommand::Core(MenuAction::NewNote | MenuAction::Delete) => self.root.is_some(),
            MenuCommand::Core(MenuAction::UndoDelete) => !self.trashed.is_empty(),
            MenuCommand::Core(MenuAction::Read) => {
                self.running_editor().is_some() && self.shown.is_some()
            }
            MenuCommand::Core(MenuAction::Hints | MenuAction::Outline) => {
                self.reader.is_some() && self.shown.is_some()
            }
            MenuCommand::Core(MenuAction::Peek) => match self.focus {
                Focus::Backlinks => !self.backs.is_empty(),
                _ => self
                    .reader
                    .as_ref()
                    .is_some_and(|r| r.selected_link().is_some()),
            },
            MenuCommand::Core(MenuAction::History(HistoryAction::Back)) => {
                self.history.peek_back().is_some()
            }
            MenuCommand::Core(MenuAction::History(HistoryAction::Forward)) => {
                self.history.peek_forward().is_some()
            }
            _ => true,
        }
    }

    fn run_reader_action(&mut self, a: ReaderAction, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(r) = self.reader.as_mut() else {
            return Ok(());
        };
        match a {
            ReaderAction::Down => r.scroll_by(1),
            ReaderAction::Up => r.scroll_by(-1),
            ReaderAction::PageDown => r.page(1),
            ReaderAction::PageUp => r.page(-1),
            ReaderAction::Top => r.top(),
            ReaderAction::Bottom => r.bottom(),
            ReaderAction::NextLink => r.next_link(),
            ReaderAction::PreviousLink => r.previous_link(),
            ReaderAction::NextHeading => r.next_heading(),
            ReaderAction::PreviousHeading => r.previous_heading(),
            ReaderAction::Follow => self.follow_selected(),
            ReaderAction::Edit => {
                let (Some(p), Some(root)) = (self.shown.clone(), self.root.clone()) else {
                    return Ok(());
                };
                if let Some(err) = term::edit_file(terminal, &p, &root)? {
                    self.flash(err);
                }
                self.reload_reader();
            }
            ReaderAction::Back => self.focus = Focus::Notes,
        }
        Ok(())
    }

    /// Follow the reader's selected link: notes like from the editor, URLs in the browser.
    fn follow_selected(&mut self) {
        let (Some(link), Some(from)) = (
            self.reader.as_ref().and_then(Reader::selected_link),
            self.shown.clone(),
        ) else {
            return;
        };
        let target = link.target.clone();
        if link.kind == markdown::LinkKind::Url {
            if !term::open_url(&target) {
                self.flash(format!("couldn't open {target}"));
            }
            return;
        }
        self.follow(&from, &target);
    }

    /// Label the reader's links and the backlinks on screen, for `f` then a label to follow one.
    fn show_hints(&mut self) {
        let links = self.reader.as_ref().map(Reader::visible_links);
        let links = links.unwrap_or_default().into_iter().map(|(i, ..)| i);
        let backs = self.visible_backlinks(inner(self.areas[3]).height);
        let targets = links
            .map(HintTarget::Link)
            .chain(backs.map(HintTarget::Backlink));
        let hints = Hints::new(targets);
        if hints.is_empty() {
            return self.flash("no links on screen");
        }
        self.mode = Mode::Hints(hints);
    }

    fn follow_hint(&mut self, target: HintTarget) {
        match target {
            HintTarget::Link(i) => {
                let Some(r) = self.reader.as_mut() else {
                    return;
                };
                r.select(i);
                self.focus = Focus::Pane;
                self.follow_selected();
            }
            HintTarget::Backlink(i) => {
                self.bsel = i;
                self.open_backlink(i);
            }
        }
    }

    /// Peeks at the selected link or backlink, or closes the peek when there is none.
    fn peek(&mut self, from: PeekFrom) {
        let view = match from {
            PeekFrom::Link => self.peek_link(),
            PeekFrom::Backlink => {
                let b = self.backs.get(self.bsel);
                b.map(|b| self.peek_note(b.path.clone(), Some(Jump::Line(b.line))))
            }
        };
        self.mode = match view {
            Some(view) => Mode::Peek { from, view },
            None => Mode::Normal,
        };
    }

    /// Where the reader's selected link leads, resolved the way `follow` resolves it.
    fn peek_link(&self) -> Option<Peek> {
        let link = self.reader.as_ref()?.selected_link()?;
        if link.kind == markdown::LinkKind::Url {
            return Some(Peek::Url(link.target.clone()));
        }
        Some(
            match self.destination(self.shown.as_deref()?, &link.target)? {
                Destination::Note(path, jump) => self.peek_note(path, jump),
                Destination::NewNote(name) => Peek::Missing(name),
                Destination::Attachment { name, path } => Peek::Attachment {
                    kind: attachment_label(&name).unwrap_or_default(),
                    found: path.is_some(),
                    name,
                },
            },
        )
    }

    fn peek_note(&self, path: PathBuf, jump: Option<Jump>) -> Peek {
        let mut reader = Reader::new(self.read_note(&path));
        if let Some(j) = jump {
            reader.jump(j);
        }
        Peek::Note {
            path,
            reader: Box::new(reader),
        }
    }

    /// The reader's keys scroll the peek, sweep the selection and go where it leads.
    fn peek_key(&mut self, k: KeyEvent) {
        let Mode::Peek { from, view } = &mut self.mode else {
            return;
        };
        let from = *from;
        if self.keys.get(k) == Some(MenuCommand::Core(MenuAction::Peek)) {
            self.mode = Mode::Normal;
            return;
        }
        let Some(a) = self.reader_keys.get(k) else {
            return;
        };
        if let Peek::Note { reader, .. } = &mut *view {
            match a {
                ReaderAction::Down => reader.scroll_by(1),
                ReaderAction::Up => reader.scroll_by(-1),
                ReaderAction::PageDown => reader.page(1),
                ReaderAction::PageUp => reader.page(-1),
                ReaderAction::Top => reader.top(),
                ReaderAction::Bottom => reader.bottom(),
                _ => {}
            }
        }
        let n = self.backs.len().max(1);
        match (a, from) {
            (ReaderAction::NextLink, PeekFrom::Link) => {
                if let Some(r) = self.reader.as_mut() {
                    r.next_link();
                }
                self.peek(from);
            }
            (ReaderAction::PreviousLink, PeekFrom::Link) => {
                if let Some(r) = self.reader.as_mut() {
                    r.previous_link();
                }
                self.peek(from);
            }
            (ReaderAction::NextLink, PeekFrom::Backlink) => {
                self.bsel = (self.bsel + 1) % n;
                self.peek(from);
            }
            (ReaderAction::PreviousLink, PeekFrom::Backlink) => {
                self.bsel = (self.bsel + n - 1) % n;
                self.peek(from);
            }
            (ReaderAction::Follow, PeekFrom::Link) => {
                self.mode = Mode::Normal;
                self.follow_selected();
            }
            (ReaderAction::Follow, PeekFrom::Backlink) => {
                self.mode = Mode::Normal;
                self.open_backlink(self.bsel);
            }
            (ReaderAction::Back, _) => self.mode = Mode::Normal,
            _ => {}
        }
    }

    /// The backlinks the pane shows when it has `rows` rows: it scrolls to keep the selected one
    /// in view.
    fn visible_backlinks(&self, rows: u16) -> std::ops::Range<usize> {
        let h = rows as usize;
        let start = (self.bsel + 1).saturating_sub(h);
        start..(start + h).min(self.backs.len())
    }

    fn draw_hints(&self, buf: &mut ratatui::buffer::Buffer, hints: &Hints<HintTarget>) {
        let [_, _, ea, ba] = self.areas;
        let ba = inner(ba);
        let links = self.reader.as_ref().map(Reader::visible_links);
        let links = links.unwrap_or_default();
        let backs = self.visible_backlinks(ba.height);
        for (label, target) in hints.matching() {
            let at = match *target {
                HintTarget::Link(i) => links
                    .iter()
                    .find(|l| l.0 == i)
                    .map(|&(_, row, col)| (ea, ea.x + col, ea.y + row)),
                HintTarget::Backlink(i) => backs
                    .contains(&i)
                    .then(|| (ba, ba.x + 1, ba.y + (i - backs.start) as u16)),
            };
            if let Some((area, x, y)) = at {
                hints.draw(buf, area, x, y, label);
            }
        }
    }

    fn run_action(&mut self, a: MenuAction, terminal: &mut DefaultTerminal) -> Result<()> {
        match a {
            MenuAction::Hints => self.show_hints(),
            MenuAction::Outline => self.open_outline(),
            MenuAction::Peek => match self.focus {
                Focus::Backlinks => self.peek(PeekFrom::Backlink),
                _ => self.peek(PeekFrom::Link),
            },
            // Shares pane.leave's default key, so that key goes back and forth; it never quits.
            MenuAction::FocusEditor => {
                if self.pane.is_some() {
                    self.focus = Focus::Pane;
                }
            }
            MenuAction::Quit => self.quit = true,
            MenuAction::Help => self.mode = Mode::Help,
            MenuAction::Keys => self.mode = Mode::Keys,
            MenuAction::FocusNext => {
                let has_backs = self.show_backlinks && !self.backs.is_empty();
                self.focus = match self.focus {
                    Focus::Vaults => Focus::Notes,
                    Focus::Notes if self.pane.is_some() || self.reader.is_some() => Focus::Pane,
                    Focus::Notes | Focus::Pane if has_backs => Focus::Backlinks,
                    _ => Focus::Vaults,
                }
            }
            MenuAction::Palette => {
                self.mode = Mode::Palette {
                    query: String::new(),
                    sel: 0,
                }
            }
            MenuAction::Switcher => self.open_switcher(),
            MenuAction::Rename => self.start_rename(false),
            MenuAction::Move => self.start_rename(true),
            MenuAction::Delete => self.start_delete(false),
            MenuAction::UndoDelete => self.undo_delete(),
            MenuAction::Search => self.open_search(),
            MenuAction::ToggleBacklinks => {
                self.show_backlinks = !self.show_backlinks;
                if !self.show_backlinks && self.focus == Focus::Backlinks {
                    self.focus = Focus::Notes;
                }
            }
            MenuAction::FocusPrevious => {
                self.focus = if self.focus == Focus::Notes {
                    Focus::Vaults
                } else {
                    Focus::Notes
                }
            }
            MenuAction::Filter => {
                self.mode = Mode::Filter;
                self.focus = Focus::Notes;
            }
            MenuAction::ClearFilter => {
                self.filter.clear();
                self.refresh_rows();
            }
            MenuAction::Zoom => self.zoom = !self.zoom,
            MenuAction::Graph => self.go_from_here(Place::Graph { selected: None }, terminal)?,
            MenuAction::Boards => self.open_boards(terminal, None)?,
            MenuAction::History(a) => self.travel(a, terminal)?,
            MenuAction::NewNote => {
                self.mode = Mode::NewNote {
                    name: String::new(),
                    templates: self.templates(),
                    template: None,
                }
            }
            MenuAction::Today => self.open_today(),
            MenuAction::AddVault => self.mode = Mode::AddVault(String::new()),
            MenuAction::Rescan => {
                if let Some(core) = self.core.as_mut() {
                    core.rescan(); // applied on the next poll, as a Rescanned event
                }
                self.flash("rescanning");
            }
            MenuAction::OpenInObsidian => self.open_in_obsidian(),
            MenuAction::EditFullscreen => self.edit_fullscreen(terminal)?,
            MenuAction::Read => {
                if self.reader.take().is_none() {
                    self.save_editor();
                    self.reader = Some(Reader::new(self.read_shown()));
                }
            }
        }
        Ok(())
    }

    fn vault_key(&mut self, k: KeyEvent) {
        let n = self.vaults.len();
        if n == 0 {
            return;
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.open_vault((self.vsel + 1).min(n - 1)),
            KeyCode::Up | KeyCode::Char('k') => self.open_vault(self.vsel.saturating_sub(1)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.focus = Focus::Notes,
            _ => {}
        }
    }

    fn notes_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let page = self.areas[1].height.saturating_sub(2).max(1) as usize;
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.select_row(self.nsel + 1),
            KeyCode::Up | KeyCode::Char('k') => self.select_row(self.nsel.saturating_sub(1)),
            KeyCode::PageDown => self.select_row(self.nsel + page),
            KeyCode::PageUp => self.select_row(self.nsel.saturating_sub(page)),
            KeyCode::Home => self.select_row(0),
            KeyCode::End => self.select_row(usize::MAX),
            KeyCode::Left | KeyCode::Char('h') => {
                let Some(it) = self.current().cloned() else {
                    return Ok(());
                };
                if it.kind == ItemKind::Folder
                    && self.expanded.contains(&it.rel)
                    && self.filter.is_empty()
                {
                    self.expanded.remove(&it.rel);
                    self.refresh_rows();
                } else if let Some((parent, _)) =
                    it.rel.rsplit_once('/').filter(|_| self.filter.is_empty())
                {
                    let parent = parent.to_string();
                    if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == parent) {
                        self.select_row(r);
                    }
                } else {
                    self.focus = Focus::Vaults;
                }
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => {
                let Some(it) = self.current().cloned() else {
                    return Ok(());
                };
                match it.kind {
                    ItemKind::Folder => {
                        if !self.expanded.insert(it.rel.clone()) && k.code == KeyCode::Enter {
                            self.expanded.remove(&it.rel);
                        }
                        self.refresh_rows();
                    }
                    ItemKind::Board => {
                        let p = self.current_path();
                        self.open_boards(terminal, p)?;
                    }
                    ItemKind::Note => {
                        if let Some(p) = self.current_path() {
                            self.pending = None;
                            self.show(&p);
                        }
                        self.focus_pane();
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ---------- quick switcher & search ----------

    fn remember_recent(&mut self, path: &Path) {
        self.mru.retain(|p| p != path);
        self.mru.insert(0, path.to_path_buf());
        self.mru.truncate(50);
    }

    fn open_switcher(&mut self) {
        if self.core.is_none() {
            return;
        }
        self.back_to_editor = self.focus == Focus::Pane;
        if self.focus == Focus::Pane {
            self.focus = Focus::Notes;
        }
        self.mode = Mode::Switcher {
            query: String::new(),
            sel: 0,
        };
    }

    /// The search, with `query` typed in.
    fn search_for(&mut self, query: String) {
        self.open_search();
        if let Mode::Search { query: q, .. } = &mut self.mode {
            *q = query;
        }
        self.run_search();
    }

    fn open_search(&mut self) {
        if self.core.is_none() {
            return;
        }
        self.back_to_editor = self.focus == Focus::Pane;
        if self.focus == Focus::Pane {
            self.focus = Focus::Notes;
        }
        self.mode = Mode::Search {
            query: String::new(),
            sel: 0,
        };
        self.results.clear();
        self.result_rows.clear();
    }

    fn close_overlay(&mut self) {
        self.mode = Mode::Normal;
        if self.back_to_editor {
            self.focus_pane();
        }
        self.back_to_editor = false;
    }

    /// The quick switcher's list: (note index, hit). Empty query: recent notes, then the rest.
    fn switcher_hits(&self, query: &str) -> Vec<crate::search::SwitchHit> {
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return Vec::new();
        };
        let mut hits = crate::search::switcher(ix, query);
        if query.trim().is_empty() {
            let rank = |h: &crate::search::SwitchHit| {
                let p = &ix.vault.notes[h.note].path;
                self.mru.iter().position(|m| m == p).unwrap_or(usize::MAX)
            };
            hits.sort_by(|a, b| {
                rank(a).cmp(&rank(b)).then_with(|| {
                    ix.vault.notes[a.note]
                        .rel
                        .to_lowercase()
                        .cmp(&ix.vault.notes[b.note].rel.to_lowercase())
                })
            });
        }
        hits
    }

    /// Whether the switcher offers "create note" (a query that isn't exactly an existing name).
    fn offers_create(&self, query: &str) -> bool {
        let q = vault::search_key(query.trim());
        !q.is_empty()
            && self.core.as_ref().map(|c| &c.index).is_some_and(|ix| {
                !ix.vault.notes.iter().any(|n| {
                    vault::search_key(&n.name) == q
                        || n.aliases.iter().any(|a| vault::search_key(a) == q)
                })
            })
    }

    fn switcher_key(&mut self, k: KeyEvent) {
        let Mode::Switcher { query, sel } = &mut self.mode else {
            return;
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let (mut q, mut s) = (query.clone(), *sel);
        let total = self.switcher_hits(&q).len() + self.offers_create(&q) as usize;
        match k.code {
            KeyCode::Esc => return self.close_overlay(),
            KeyCode::Down | KeyCode::Tab => s = (s + 1).min(total.saturating_sub(1)),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
                s = (s + 1).min(total.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::BackTab => s = s.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => s = s.saturating_sub(1),
            KeyCode::Backspace => {
                q.pop();
                s = 0;
            }
            KeyCode::Enter => {
                let hits = self.switcher_hits(&q);
                self.mode = Mode::Normal;
                let from = self.here();
                if let Some(h) = hits.get(s) {
                    let path = self
                        .core
                        .as_ref()
                        .map(|c| &c.index)
                        .map(|ix| ix.path(h.note))
                        .unwrap();
                    self.open_found(from, &path, None);
                } else if self.offers_create(&q) {
                    // Same place a new note from a link goes (Obsidian's new-note location setting).
                    let path = self.core.as_ref().map(|c| &c.index).map(|ix| {
                        let from = self.shown.as_ref().and_then(|p| ix.by_path(p));
                        ix.new_note_path(q.trim(), from)
                    });
                    if let Some(path) = path
                        && crate::write::create_if_missing(&path, b"").is_ok()
                    {
                        self.flash(format!("created {}", q.trim()));
                        self.reload_tree();
                        self.open_found(from, &path, None);
                    }
                }
                self.back_to_editor = false;
                return;
            }
            KeyCode::Char(c) if !ctrl => {
                q.push(c);
                s = 0;
            }
            _ => {}
        }
        self.mode = Mode::Switcher { query: q, sel: s };
    }

    /// The outline's rows, in the note's order: (heading, match).
    fn outline_hits(&self, query: &str) -> Vec<(usize, crate::search::Fuzzy)> {
        let headings = self.reader.as_ref().map_or(&[][..], Reader::headings);
        let hits = headings.iter().enumerate();
        hits.filter_map(|(i, h)| Some((i, crate::search::fuzzy(query, &h.text)?)))
            .collect()
    }

    /// The row the outline selects for `query`: the heading being read, or the best match.
    fn outline_start(&self, query: &str) -> usize {
        if query.trim().is_empty() {
            let section = self.reader.as_ref().map(Reader::section);
            return section.and_then(|s| s.last().copied()).unwrap_or(0);
        }
        let hits = self.outline_hits(query);
        let best = hits.iter().map(|(_, m)| m.score).max();
        hits.iter()
            .position(|(_, m)| Some(m.score) == best)
            .unwrap_or(0)
    }

    fn open_outline(&mut self) {
        if self.reader.as_ref().is_none_or(|r| r.headings().is_empty()) {
            return self.flash("no headings in this note");
        }
        self.mode = Mode::Outline {
            query: String::new(),
            sel: self.outline_start(""),
        };
    }

    fn outline_key(&mut self, k: KeyEvent) {
        let Mode::Outline { query, sel } = &self.mode else {
            return;
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let (mut q, mut s) = (query.clone(), *sel);
        let hits = self.outline_hits(&q);
        let last = hits.len().saturating_sub(1);
        match k.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Down | KeyCode::Tab => s = (s + 1).min(last),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => s = (s + 1).min(last),
            KeyCode::Up | KeyCode::BackTab => s = s.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => s = s.saturating_sub(1),
            KeyCode::Backspace => {
                q.pop();
                s = self.outline_start(&q);
            }
            KeyCode::Char('u') if ctrl => {
                q.clear();
                s = self.outline_start(&q);
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                if let Some(&(i, _)) = hits.get(s) {
                    self.jump_to_heading(i);
                }
                return;
            }
            KeyCode::Char(c) if !ctrl => {
                q.push(c);
                s = self.outline_start(&q);
            }
            _ => {}
        }
        self.mode = Mode::Outline { query: q, sel: s };
    }

    /// Scrolls the reader to heading `i`; Back returns to where it was.
    fn jump_to_heading(&mut self, i: usize) {
        let from = self.here();
        let Some(r) = self.reader.as_mut() else {
            return;
        };
        r.go_to_heading(i);
        self.focus = Focus::Pane;
        if let Some(from) = from.filter(|f| Some(f) != self.here().as_ref()) {
            self.history.visit(from);
        }
    }

    fn palette_hits(&self, query: &str) -> Vec<commands::Match<'_>> {
        commands::matching(self.registry.commands(), query)
            .into_iter()
            .filter(|m| self.enabled(m.command.action))
            .collect()
    }

    fn palette_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let Mode::Palette { query, sel } = &mut self.mode else {
            return Ok(());
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let (mut q, mut s) = (query.clone(), *sel);
        let total = self.palette_hits(&q).len();
        match k.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return Ok(());
            }
            KeyCode::Down | KeyCode::Tab => s = (s + 1).min(total.saturating_sub(1)),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
                s = (s + 1).min(total.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::BackTab => s = s.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => s = s.saturating_sub(1),
            KeyCode::Backspace => {
                q.pop();
                s = 0;
            }
            KeyCode::Char('u') if ctrl => {
                q.clear();
                s = 0;
            }
            KeyCode::Enter => {
                let action = self.palette_hits(&q).get(s).map(|m| m.command.action);
                self.mode = Mode::Normal;
                return match action {
                    Some(c) => self.run_menu_command(c, Value::Null, terminal),
                    None => Ok(()),
                };
            }
            KeyCode::Char(c) if !ctrl => {
                q.push(c);
                s = 0;
            }
            _ => {}
        }
        self.mode = Mode::Palette { query: q, sel: s };
        Ok(())
    }

    fn run_search(&mut self) {
        let Mode::Search { query, sel } = &mut self.mode else {
            return;
        };
        *sel = 0;
        let q = query.clone();
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        self.results = crate::search::search(ix, &q, 8);
        self.result_rows.clear();
        for (i, r) in self.results.iter().enumerate() {
            self.result_rows.push(SearchRow::Note(i));
            for l in 0..r.lines.len() {
                self.result_rows.push(SearchRow::Line(i, l));
            }
        }
    }

    fn search_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.result_rows.len();
        let page = 10;
        let Mode::Search { query, sel } = &mut self.mode else {
            return;
        };
        match k.code {
            KeyCode::Esc => self.close_overlay(),
            KeyCode::Down | KeyCode::Tab => *sel = (*sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
                *sel = (*sel + 1).min(n.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::BackTab => *sel = sel.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => *sel = sel.saturating_sub(1),
            KeyCode::PageDown => *sel = (*sel + page).min(n.saturating_sub(1)),
            KeyCode::PageUp => *sel = sel.saturating_sub(page),
            KeyCode::Backspace => {
                query.pop();
                self.run_search();
            }
            KeyCode::Char(c) if !ctrl => {
                query.push(c);
                self.run_search();
            }
            KeyCode::Enter => {
                let s = *sel;
                let target = self.result_rows.get(s).and_then(|row| {
                    let (ri, li) = match *row {
                        SearchRow::Note(r) => (r, None),
                        SearchRow::Line(r, l) => (r, Some(l)),
                    };
                    let hit = self.results.get(ri)?;
                    let line = li
                        .and_then(|l| hit.lines.get(l))
                        .or(hit.lines.first())
                        .map(|l| l.line);
                    Some((self.core.as_ref().map(|c| &c.index)?.path(hit.note), line))
                });
                let from = self.here();
                self.mode = Mode::Normal;
                if let Some((path, line)) = target {
                    self.open_found(from, &path, line.map(Jump::Line));
                }
                self.back_to_editor = false;
            }
            _ => {}
        }
    }

    // ---------- rename & move ----------

    /// The note to rename: the one selected in the tree, else the one in the editor.
    fn rename_target(&self) -> Option<PathBuf> {
        match (self.focus, self.current()) {
            (Focus::Notes, Some(it)) if it.kind == ItemKind::Note => self.current_path(),
            (Focus::Notes, Some(_)) => None,
            _ => self.shown.clone().or_else(|| {
                self.current()
                    .filter(|it| it.kind == ItemKind::Note)
                    .and(self.current_path())
            }),
        }
    }

    fn start_rename(&mut self, moving: bool) {
        let (Some(note), Some(root)) = (self.rename_target(), self.root.clone()) else {
            return self.flash("select a note to rename or move");
        };
        let rel = note
            .strip_prefix(&root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let rel = rel.trim_end_matches(".md");
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
        let input = if moving {
            if dir.is_empty() {
                String::new()
            } else {
                format!("{dir}/")
            }
        } else {
            name.to_string()
        };
        self.mode = Mode::Rename {
            note,
            input,
            moving,
        };
    }

    /// Ask the editor to save, and give it a moment, so what's read from disk next is what's on
    /// screen. The editor can't say when it's done yet, hence the wait.
    fn save_editor(&mut self) {
        let Some(editor) = self.running_editor() else {
            return;
        };
        self.call(editor, |e, host| e.editor(EditorRequest::Save, host));
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(25));
            if let Some(p) = self.pane.as_mut() {
                p.term.pump();
            }
        }
    }

    /// The editor, if its program runs in the pane (perhaps behind the reader).
    fn running_editor(&self) -> Option<ExtId> {
        let editor = self.registry.editor()?;
        let pane = self.pane.as_ref().filter(|p| p.owner == editor);
        pane.map(|_| editor)
    }

    fn rename_key(&mut self, k: KeyEvent) {
        match &mut self.mode {
            Mode::Rename {
                note,
                input,
                moving,
            } => match k.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => input.clear(),
                KeyCode::Char(_) if k.modifiers.contains(KeyModifiers::CONTROL) => {}
                KeyCode::Char(c) => input.push(c),
                KeyCode::Enter => {
                    let (note, input, moving) = (note.clone(), input.clone(), *moving);
                    self.mode = Mode::Normal;
                    self.plan_rename(&note, &input, moving);
                }
                _ => {}
            },
            Mode::ConfirmRename { plan, scroll } => match k.code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.mode = Mode::Normal;
                    self.flash("rename cancelled");
                }
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Enter | KeyCode::Char('y') => {
                    let plan = (**plan).clone();
                    self.mode = Mode::Normal;
                    self.apply_rename(plan);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn plan_rename(&mut self, note: &Path, input: &str, moving: bool) {
        // A folder typed without a trailing slash still means "move into it".
        let input =
            if moving && !input.ends_with('/') && !input.ends_with(".md") && !input.is_empty() {
                format!("{input}/")
            } else if moving && input.is_empty() {
                return self.flash("type a folder, e.g. Archive/ (or / for the vault root)");
            } else {
                input.to_string()
            };
        self.save_editor();
        self.reload_tree();
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let Some(i) = ix.by_path(note) else {
            return self.flash("that note is no longer there");
        };
        let from = format!("{}.md", ix.vault.notes[i].rel);
        // Moving to the root: "/" means "no folder".
        let target = if input == "/" {
            Ok(format!("{}.md", ix.vault.notes[i].name))
        } else {
            crate::rename::target_rel(&from, &input)
        };
        match target.and_then(|to| crate::rename::plan(ix, i, &to)) {
            Ok(plan) => {
                self.mode = Mode::ConfirmRename {
                    plan: Box::new(plan),
                    scroll: 0,
                }
            }
            Err(e) => self.flash(format!("can't rename: {e}")),
        }
    }

    fn apply_rename(&mut self, plan: crate::rename::Plan) {
        if let Err(e) = crate::rename::apply(&plan) {
            self.reload_tree();
            return self.flash(format!("rename failed: {e}"));
        }
        let (old, new) = (plan.from_path.clone(), plan.to_path.clone());
        if let Some(editor) = self.running_editor() {
            let request = EditorRequest::Renamed {
                from: old.clone(),
                to: new.clone(),
            };
            self.call(editor, |e, host| e.editor(request, host));
        }
        if self.shown.as_deref() == Some(old.as_path()) {
            self.shown = Some(new.clone());
        }
        self.mru.retain(|p| p != &old);
        self.reload_tree();
        self.select_path(&new);
        let links = plan.links();
        let moved_own = plan.edits.iter().any(|e| e.path == plan.from_path) as usize;
        let files = plan.files() - moved_own;
        let moved = dir_of_rel(&plan.from) != dir_of_rel(&plan.to);
        let verb = if moved { "moved" } else { "renamed" };
        self.flash(if links == 0 {
            format!("{verb} to {} (no links needed updating)", plan.to)
        } else {
            format!(
                "{verb} to {} · {} updated in {}",
                plan.to,
                plural(links, "link"),
                plural(files.max(1), "file")
            )
        });
    }

    fn draw_confirm_rename(&self, f: &mut Frame, area: Rect) {
        let Mode::ConfirmRename { plan, scroll } = &self.mode else {
            return;
        };
        let w = (area.width * 3 / 4).clamp(50.min(area.width), 120);
        let h = (area.height * 2 / 3).clamp(10.min(area.height), 34);
        let r = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + area.height / 6,
            w,
            h,
        );
        f.render_widget(Clear, r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .title(Line::styled(
                " Rename ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::styled(
                " ⏎ rename · Esc cancel · ↑↓ scroll ",
                Style::new().fg(MUTED),
            ));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(format!(" {}", plan.from), Style::new().fg(MUTED)),
                Span::styled("  →  ", Style::new().fg(ACCENT)),
                Span::styled(
                    plan.to.clone(),
                    Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::styled(
                match plan.links() {
                    0 => " No links point to it, so only the file moves.".to_string(),
                    n => format!(
                        " {} in {} will be updated:",
                        plural(n, "link"),
                        plural(plan.files(), "file")
                    ),
                },
                Style::new().fg(NOTE),
            ),
        ];
        for w in &plan.warnings {
            lines.push(Line::styled(format!(" ! {w}"), Style::new().fg(HIT)));
        }
        lines.push(Line::raw(""));
        let mut detail = Vec::new();
        for e in &plan.edits {
            detail.push(Line::from(vec![
                Span::styled(
                    format!(" ▤ {}", e.file.trim_end_matches(".md")),
                    Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {} link{}", e.links, if e.links == 1 { "" } else { "s" }),
                    Style::new().fg(DIM),
                ),
            ]));
            for c in e.changes.iter().take(3) {
                let mut t: String = c
                    .after
                    .trim()
                    .chars()
                    .take(inner.width.saturating_sub(10) as usize)
                    .collect();
                if c.after.trim().chars().count() > t.chars().count() {
                    t.push('…');
                }
                detail.push(Line::from(vec![
                    Span::styled(format!("   {:>4}  ", c.line), Style::new().fg(DIM)),
                    Span::styled(t, Style::new().fg(NOTE)),
                ]));
            }
            if e.changes.len() > 3 {
                detail.push(Line::styled(
                    format!("         … {} more lines", e.changes.len() - 3),
                    Style::new().fg(DIM),
                ));
            }
        }
        let room = (inner.height as usize).saturating_sub(lines.len());
        let start = (*scroll).min(detail.len().saturating_sub(room));
        lines.extend(detail.into_iter().skip(start).take(room));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    // ---------- delete & undo ----------

    /// The note or board to delete: the tree's selection, else the note in the pane.
    fn delete_target(&self) -> Result<PathBuf, &'static str> {
        match (self.focus, self.current()) {
            (Focus::Notes, Some(it)) if it.kind == ItemKind::Folder => {
                Err("cold-lava doesn't delete folders yet: delete the notes in it, or use Obsidian")
            }
            (Focus::Notes, Some(_)) => self.current_path().ok_or("select a note to delete"),
            _ => self.rename_target().ok_or("select a note to delete"),
        }
    }

    /// Ask before deleting, showing the notes whose links will break. `changed`: asking again
    /// because the note changed on disk after the first time.
    fn start_delete(&mut self, changed: bool) {
        let path = match (&self.mode, self.delete_target()) {
            (Mode::ConfirmDelete(p), _) => p.path.clone(),
            (_, Ok(p)) => p,
            (_, Err(why)) => return self.flash(why),
        };
        self.mode = Mode::Normal;
        if self.shown.as_deref() == Some(path.as_path()) {
            self.save_editor();
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            self.reload_tree();
            return self.flash("that note is no longer there");
        };
        let rel = path
            .strip_prefix(&root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let rel = rel.trim_end_matches(".md").to_string();
        let mut backlinks = Vec::new();
        if let Some(ix) = self.core.as_ref().map(|c| &c.index)
            && let Some(i) = ix.by_path(&path)
        {
            backlinks = ix
                .backlinks(i)
                .iter()
                .filter(|b| b.from != i)
                .map(|b| BackRow {
                    name: ix.vault.notes[b.from].name.clone(),
                    path: ix.path(b.from),
                    line: b.line,
                    context: b.context.clone(),
                })
                .collect();
        }
        let goes = match TrashOption::of_vault(&root) {
            TrashOption::System if trash::system_bin().is_some() => "goes to the system trash.",
            TrashOption::System | TrashOption::Local => "goes to the vault's .trash folder.",
            TrashOption::Permanent => {
                "goes to .trash: this vault deletes permanently in Obsidian, cold-lava never does."
            }
        };
        self.mode = Mode::ConfirmDelete(Box::new(PendingDelete {
            path,
            rel,
            bytes,
            backlinks,
            goes: goes.to_string(),
            changed,
            scroll: 0,
        }));
    }

    fn delete_key(&mut self, k: KeyEvent) {
        let yes = matches!(k.code, KeyCode::Enter | KeyCode::Char('y'));
        let no = matches!(k.code, KeyCode::Esc | KeyCode::Char('n'));
        match &mut self.mode {
            Mode::ConfirmDelete(p) => match k.code {
                _ if no => {
                    self.mode = Mode::Normal;
                    self.flash("delete cancelled");
                }
                KeyCode::Down | KeyCode::Char('j') => p.scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => p.scroll = p.scroll.saturating_sub(1),
                _ if yes => self.apply_delete(),
                _ => {}
            },
            Mode::ConfirmRestore { .. } if yes || no => {
                let Mode::ConfirmRestore { trashed, to } =
                    std::mem::replace(&mut self.mode, Mode::Normal)
                else {
                    return;
                };
                if yes {
                    self.restore(trashed, &to);
                } else {
                    self.trashed.push(trashed);
                    self.flash("undo cancelled");
                }
            }
            _ => {}
        }
    }

    fn apply_delete(&mut self) {
        let (Mode::ConfirmDelete(p), Some(root)) = (&self.mode, self.root.clone()) else {
            return;
        };
        let system = trash::system_bin();
        let t = match trash::delete(&root, &p.path, &p.bytes, system.as_ref()) {
            Ok(t) => t,
            Err(crate::write::WriteError::Changed(_)) => return self.start_delete(true),
            Err(e) => {
                self.mode = Mode::Normal;
                return self.flash(format!("couldn't delete: {e}"));
            }
        };
        let Mode::ConfirmDelete(p) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        let was_shown = self.shown.as_deref() == Some(p.path.as_path());
        if was_shown {
            if let Some(editor) = self.running_editor() {
                let request = EditorRequest::Deleted {
                    note: p.path.clone(),
                };
                self.call(editor, |e, host| e.editor(request, host));
            }
            self.shown = None;
            self.pending = None;
        }
        self.mru.retain(|m| m != &p.path);
        self.history.forget(&p.path);
        self.reload_tree();
        if was_shown {
            match (self.current().map(|it| it.kind), self.current_path()) {
                (Some(ItemKind::Note), Some(next)) => self.show(&next),
                _ if self.reader.is_some() => self.reload_reader(),
                _ => {}
            }
        }
        let name = p.rel.rsplit('/').next().unwrap_or(&p.rel).to_string();
        let undo = self.keys.first_label("note.undo-delete");
        self.flash(match (t.went, TrashOption::of_vault(&root)) {
            (Went::VaultTrash, TrashOption::Permanent) => format!(
                "moved {name} to .trash: this vault deletes permanently in Obsidian, cold-lava never does · {undo} undo"
            ),
            (Went::VaultTrash, TrashOption::Local) => {
                format!("deleted {name} (to .trash) · {undo} undo")
            }
            (Went::VaultTrash, TrashOption::System) => format!(
                "deleted {name} (to .trash: the system trash couldn't take it) · {undo} undo"
            ),
            (Went::System, _) => format!("deleted {name} · {undo} undo"),
        });
        self.trashed.push(t);
        if self.trashed.len() > UNDO_DELETES {
            self.trashed.remove(0);
        }
    }

    /// Put the last deleted note back, or ask about a free name if its path is taken now.
    fn undo_delete(&mut self) {
        let Some(t) = self.trashed.pop() else {
            return self.flash("nothing to undo");
        };
        if t.from.exists() {
            let to = trash::free_path(&t.from);
            self.mode = Mode::ConfirmRestore { trashed: t, to };
            return;
        }
        let to = t.from.clone();
        self.restore(t, &to);
    }

    fn restore(&mut self, t: Trashed, to: &Path) {
        if let Err(e) = trash::restore(&t, to) {
            return self.flash(format!("couldn't put it back: {e}"));
        }
        self.reload_tree();
        self.select_path(to);
        if to.extension().is_some_and(|e| e == "md") {
            self.pending = None;
            self.show(to);
        }
        let rel = self
            .root
            .as_deref()
            .and_then(|r| to.strip_prefix(r).ok())
            .unwrap_or(to);
        self.flash(format!("restored {}", rel.display()));
    }

    fn draw_confirm_delete(&self, f: &mut Frame, area: Rect) {
        let (title, hint, lines, detail, scroll) = match &self.mode {
            Mode::ConfirmDelete(p) => {
                let name = p.rel.rsplit('/').next().unwrap_or(&p.rel);
                let mut lines = vec![Line::from(vec![
                    Span::styled(" Delete ", Style::new().fg(NOTE)),
                    Span::styled(
                        p.rel.clone(),
                        Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("?", Style::new().fg(NOTE)),
                ])];
                if p.changed {
                    lines.push(Line::styled(
                        " ! It changed on disk since you asked: this is the new version.",
                        Style::new().fg(HIT),
                    ));
                }
                let mut names: Vec<&str> = Vec::new();
                for b in &p.backlinks {
                    if !names.contains(&b.name.as_str()) {
                        names.push(&b.name);
                    }
                }
                let note = p.path.extension().is_some_and(|e| e == "md");
                lines.push(Line::styled(
                    match names.len() {
                        0 if !note => " Links to it in notes will become unresolved.".to_string(),
                        0 => " Nothing links to it.".to_string(),
                        n => format!(
                            " {} to it: {}. Their links will become unresolved.",
                            if n == 1 {
                                "1 note links".to_string()
                            } else {
                                format!("{n} notes link")
                            },
                            names.join(", ")
                        ),
                    },
                    Style::new().fg(NOTE),
                ));
                lines.push(Line::styled(
                    format!(" {name} {}", p.goes),
                    Style::new().fg(MUTED),
                ));
                lines.push(Line::raw(""));
                let inner_w = popup_width(area).saturating_sub(2) as usize;
                let detail: Vec<Line> = p
                    .backlinks
                    .iter()
                    .map(|b| {
                        let head = format!(" ▤ {}  {:>4}  ", b.name, b.line);
                        let room = inner_w.saturating_sub(head.chars().count());
                        let mut context: String = b.context.trim().chars().take(room).collect();
                        if b.context.trim().chars().count() > room {
                            context.pop();
                            context.push('…');
                        }
                        Line::from(vec![
                            Span::styled(
                                format!(" ▤ {}", b.name),
                                Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(format!("  {:>4}  ", b.line), Style::new().fg(DIM)),
                            Span::styled(context, Style::new().fg(NOTE)),
                        ])
                    })
                    .collect();
                (
                    " Delete ",
                    " y delete · n cancel · ↑↓ scroll ",
                    lines,
                    detail,
                    p.scroll,
                )
            }
            Mode::ConfirmRestore { trashed, to } => {
                let file = |p: &Path| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                let lines = vec![
                    Line::styled(
                        format!(" {} exists now.", file(&trashed.from)),
                        Style::new().fg(NOTE),
                    ),
                    Line::styled(
                        format!(" Put the deleted one back as {}?", file(to)),
                        Style::new().fg(NOTE),
                    ),
                ];
                (
                    " Undo delete ",
                    " y restore · n cancel ",
                    lines,
                    Vec::new(),
                    0,
                )
            }
            _ => return,
        };
        let w = popup_width(area);
        // Word wrapping can take a row more than the width alone says.
        let rows = |l: &Line| {
            l.width()
                .div_ceil(w.saturating_sub(2).max(1) as usize)
                .max(1)
        };
        let tall: usize = lines.iter().map(rows).sum::<usize>() + 1 + detail.len();
        let h = (tall as u16 + 2).clamp(6.min(area.height), 24);
        let r = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + area.height / 6,
            w,
            h.min(area.height),
        );
        f.render_widget(Clear, r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(HIT))
            .title(Line::styled(
                title,
                Style::new().fg(HIT).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::styled(hint, Style::new().fg(MUTED)));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let mut lines = lines;
        let room = (inner.height as usize).saturating_sub(lines.len());
        let start = scroll.min(detail.len().saturating_sub(room));
        lines.extend(detail.into_iter().skip(start).take(room));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    /// Open a note picked in the switcher or search, and give the editor focus.
    fn open_found(&mut self, from: Option<Place>, path: &Path, jump: Option<Jump>) {
        self.leave(from, path, jump.as_ref());
        self.pending = None;
        self.show_at(path, jump);
        self.select_path(path);
        self.focus_pane();
    }

    fn backlinks_key(&mut self, k: KeyEvent) {
        let n = self.backs.len();
        if n == 0 {
            self.focus = Focus::Notes;
            return;
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.bsel = (self.bsel + 1).min(n - 1),
            KeyCode::Up | KeyCode::Char('k') => self.bsel = self.bsel.saturating_sub(1),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.open_backlink(self.bsel),
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.focus = Focus::Notes,
            _ => {}
        }
    }

    /// Open the note a backlink comes from, at the line with the link.
    fn open_backlink(&mut self, i: usize) {
        let Some(b) = self.backs.get(i) else { return };
        let (path, line) = (b.path.clone(), b.line);
        let from = self.here();
        self.leave(from, &path, Some(&Jump::Line(line)));
        self.pending = None;
        self.show_at(&path, Some(Jump::Line(line)));
        self.select_path(&path);
        self.focus_pane();
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if let Mode::Peek { view, .. } = &mut self.mode {
            match (m.kind, &mut *view) {
                (MouseEventKind::ScrollDown, Peek::Note { reader, .. }) => reader.scroll_by(3),
                (MouseEventKind::ScrollUp, Peek::Note { reader, .. }) => reader.scroll_by(-3),
                (MouseEventKind::Down(_), _) => self.mode = Mode::Normal,
                _ => {}
            }
            return;
        }
        let pos = Position::new(m.column, m.row);
        let [va, na, ea, ba] = self.areas;
        if ba.contains(pos) {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                let r = (m.row.saturating_sub(ba.y + 1)) as usize;
                if r < self.backs.len() && m.row > ba.y {
                    self.focus = Focus::Backlinks;
                    if r == self.bsel {
                        self.open_backlink(r);
                    } else {
                        self.bsel = r;
                    }
                }
            }
            return;
        }
        if ea.contains(pos)
            && let Some(r) = self.reader.as_mut()
        {
            match m.kind {
                MouseEventKind::ScrollDown => r.scroll_by(3),
                MouseEventKind::ScrollUp => r.scroll_by(-3),
                MouseEventKind::Down(MouseButton::Left) => {
                    let link = r.link_at((m.row - ea.y) as usize, (m.column - ea.x) as usize);
                    if let Some(i) = link {
                        r.select(i);
                    }
                    self.focus = Focus::Pane;
                    if link.is_some() {
                        self.follow_selected();
                    }
                }
                _ => {}
            }
            return;
        }
        if ea.contains(pos) {
            if let Some(p) = self.pane.as_mut() {
                if matches!(m.kind, MouseEventKind::Down(_)) {
                    self.focus = Focus::Pane;
                }
                p.term.send_mouse(m, m.column - ea.x, m.row - ea.y);
            }
            return;
        }
        let inside = |a: Rect| inner(a).contains(pos);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if !self.zoom && inside(va) {
                    self.focus = Focus::Vaults;
                    let i = (m.row - va.y - 1) as usize;
                    if i < self.vaults.len() && i != self.vsel {
                        self.open_vault(i);
                    }
                } else if !self.zoom && inside(na) {
                    self.focus = Focus::Notes;
                    let r = self.nscroll + (m.row - na.y - 1) as usize;
                    if r < self.rows.len() {
                        if r == self.nsel
                            && self.current().is_some_and(|it| it.kind == ItemKind::Folder)
                        {
                            let rel = self.current().unwrap().rel.clone();
                            if !self.expanded.remove(&rel) {
                                self.expanded.insert(rel);
                            }
                            self.refresh_rows();
                        } else {
                            self.select_row(r);
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown if inside(na) => self.select_row(self.nsel + 3),
            MouseEventKind::ScrollUp if inside(na) => self.select_row(self.nsel.saturating_sub(3)),
            _ => {}
        }
    }

    // ---------- actions ----------

    /// The graph view, with `selected` selected, until it is left; then where to go next.
    fn run_graph(
        &mut self,
        terminal: &mut DefaultTerminal,
        selected: Option<String>,
    ) -> Result<Option<Place>> {
        // The graph borrows the menu's live vault and gives it back, so there's one watcher and
        // changes made while the graph is open show up here too.
        let Some(core) = self.core.take() else {
            return Ok(None);
        };
        if core.index.vault.notes.is_empty() {
            self.core = Some(core);
            self.flash("this vault has no notes yet");
            return Ok(None);
        }
        let mut graph = app::App::new(core, self.render);
        graph.navigate(Nav::of(&self.history), selected.as_deref());
        let result = graph.run(terminal);
        let here = Place::Graph {
            selected: graph.selected_key(),
        };
        let exit = graph.exit();
        self.core = Some(graph.into_core());
        terminal.clear()?;
        // The graph consumed the core's events while it was open, so catch up as after a rescan.
        self.sync_with_core(&[CoreEvent::Rescanned]);
        self.check_disk();
        result?;
        Ok(self.left(here, exit))
    }

    /// A board, the picked one when `file` is `None`.
    fn open_boards(&mut self, terminal: &mut DefaultTerminal, file: Option<PathBuf>) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let path = match file {
            Some(p) => Some(p),
            None => board::pick_board(terminal, &root)?,
        };
        match path {
            Some(path) => self.go_from_here(Place::Board { path }, terminal),
            None => Ok(terminal.clear()?),
        }
    }

    /// The board at `path` until it is left; then where to go next.
    fn run_board(
        &mut self,
        terminal: &mut DefaultTerminal,
        path: PathBuf,
    ) -> Result<Option<Place>> {
        let Some(root) = self.root.clone() else {
            return Ok(None);
        };
        let exit = board::open(terminal, &root, path.clone(), Some(Nav::of(&self.history)))?;
        terminal.clear()?;
        self.reload_tree();
        Ok(self.left(Place::Board { path }, exit))
    }

    fn edit_fullscreen(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let (Some(p), Some(root)) = (self.current_path(), self.root.clone()) else {
            return Ok(());
        };
        if self.current().is_some_and(|it| it.kind == ItemKind::Note) {
            if let Some(err) = term::edit_file(terminal, &p, &root)? {
                self.flash(err);
            }
            if self.reader.is_some() && self.shown.as_deref() == Some(p.as_path()) {
                self.reload_reader();
            } else {
                self.shown = None;
                self.select_row(self.nsel);
            }
        }
        Ok(())
    }

    fn open_in_obsidian(&mut self) {
        let (Some(it), Some(v)) = (self.current().cloned(), self.vaults.get(self.vsel)) else {
            return;
        };
        let ok = term::open_in_obsidian(&v.name, &it.rel);
        self.flash(if ok {
            "opened in Obsidian"
        } else {
            "couldn't launch Obsidian"
        });
    }

    /// The vault's templates, from the folders its Templates settings name.
    fn templates(&self) -> Vec<(String, String)> {
        let (Some(root), Some(core)) = (&self.root, &self.core) else {
            return Vec::new();
        };
        let settings = templates::Settings::load(root);
        let mut found: Vec<(String, String)> = core
            .index
            .vault
            .notes
            .iter()
            .filter(|n| settings.is_template(&n.rel))
            .map(|n| (n.rel.clone(), settings.name(&n.rel).to_string()))
            .collect();
        found.sort_by_key(|(_, name)| vault::search_key(name));
        found
    }

    /// Create the note `name` in the selected folder, from `template` (vault-relative, without
    /// `.md`) if one is given, and open it.
    fn create_note(&mut self, name: &str, template: Option<&str>) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        // In the selected folder, or next to the selected note.
        let folder = match self.current() {
            Some(it) if it.kind == ItemKind::Folder => it.rel.clone(),
            Some(it) => it
                .rel
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default(),
            None => String::new(),
        };
        let file = if name.ends_with(".md") {
            name.to_string()
        } else {
            format!("{name}.md")
        };
        let rel = if folder.is_empty() {
            file
        } else {
            format!("{folder}/{file}")
        };
        let path = root.join(&rel);
        if path.exists() {
            self.flash(format!("{rel} already exists"));
        } else {
            let title = name.trim_end_matches(".md");
            let filled = template
                .map(|t| templates::new_note(&root, t, title.rsplit('/').next().unwrap_or(title)));
            let body = match filled {
                Some(Ok(body)) => body,
                Some(Err(e)) => {
                    self.flash(format!("{e:#}"));
                    return Ok(());
                }
                None => format!("# {title}\n\n"),
            };
            crate::write::create(&path, body.as_bytes())?;
            if !folder.is_empty() {
                self.expanded.insert(folder);
            }
            self.flash(match template {
                Some(t) => format!("created {rel} from {}", t.rsplit('/').next().unwrap_or(t)),
                None => format!("created {rel}"),
            });
        }
        // Make sure the new note is visible, even if a filter was active.
        self.filter.clear();
        self.reload_tree();
        if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == rel) {
            self.nsel = r;
            self.pending = None;
            self.show(&path);
            self.focus_pane();
        }
        Ok(())
    }

    fn add_vault(&mut self, text: &str) {
        let p = match text.strip_prefix("~/") {
            Some(rest) => home_dir()
                .map(|h| h.join(rest))
                .unwrap_or_else(|| PathBuf::from(text)),
            None => PathBuf::from(text),
        };
        match p.canonicalize() {
            Ok(c) if c.is_dir() => {
                if !looks_like_vault(&c) {
                    self.flash("added (no notes in that folder yet)");
                }
                remember(&c);
                self.vaults = discover(&[]);
                if let Some(i) = self.vaults.iter().position(|v| v.path == c) {
                    self.open_vault(i);
                    self.focus = Focus::Notes;
                }
            }
            _ => self.flash(format!("not a folder: {text}")),
        }
    }

    // ---------- drawing ----------

    fn block(&self, title: Line<'static>, focus: Focus) -> Block<'static> {
        let active = self.focus == focus;
        Block::bordered()
            .border_type(if active {
                BorderType::Thick
            } else {
                BorderType::Rounded
            })
            .border_style(Style::new().fg(if active { ACCENT } else { DIM }))
            .title(title)
    }

    fn draw(&mut self, f: &mut Frame) {
        let [main, status] =
            Layout::vertical([Constraint::Min(5), Constraint::Length(2)]).areas(f.area());
        let (va, na, ea) = if self.zoom {
            (Rect::default(), Rect::default(), main)
        } else {
            let [a, b, c] = Layout::horizontal([
                Constraint::Length(26),
                Constraint::Length(38),
                Constraint::Min(20),
            ])
            .areas(main);
            (a, b, c)
        };
        // Backlinks under the editor.
        let (ea, ba) = if self.show_backlinks && self.shown.is_some() && ea.height > 12 {
            let want = if self.backs.is_empty() {
                3
            } else {
                self.backs.len() as u16 + 2
            };
            let h = want.min(ea.height / 3).max(3);
            let [e, b] = Layout::vertical([Constraint::Min(5), Constraint::Length(h)]).areas(ea);
            (e, b)
        } else {
            (ea, Rect::default())
        };
        let editor_inner = inner(ea);
        self.areas = [va, na, editor_inner, ba];

        if !self.zoom {
            self.draw_vaults(f, va);
            self.draw_notes(f, na);
        }
        self.draw_editor(f, ea, editor_inner);
        if ba.height > 0 {
            self.draw_backlinks(f, ba);
        }
        if let Mode::Hints(hints) = &self.mode {
            self.draw_hints(f.buffer_mut(), hints);
        }
        self.draw_status(f, status);
        if matches!(self.mode, Mode::Peek { .. }) {
            self.draw_peek(f, main);
        }
        if matches!(self.mode, Mode::Switcher { .. }) {
            self.draw_switcher(f, main);
        }
        if matches!(self.mode, Mode::Palette { .. }) {
            self.draw_palette(f, main);
        }
        if matches!(self.mode, Mode::Outline { .. }) {
            self.draw_outline(f, main);
        }
        if matches!(self.mode, Mode::Search { .. }) {
            self.draw_search(f, main);
        }
        if matches!(self.mode, Mode::ConfirmRename { .. }) {
            self.draw_confirm_rename(f, main);
        }
        if matches!(
            self.mode,
            Mode::ConfirmDelete(_) | Mode::ConfirmRestore { .. }
        ) {
            self.draw_confirm_delete(f, main);
        }
        if let Mode::Rename { input, moving, .. } = &self.mode {
            let (title, label) = if *moving {
                (" move to folder · ⏎ preview · Esc cancel ", "folder: ")
            } else {
                (" rename · ⏎ preview · Esc cancel ", "new name: ")
            };
            prompt(f, main, title, label, input);
        }
        match &self.mode {
            Mode::Help => {
                let editor = self.registry.editor().map(|e| self.registry.manifest(e));
                let menu = (self.registry.commands(), &self.keys);
                draw_help(f, main, menu, &self.reader_keys, &self.pane_keys, editor)
            }
            Mode::Keys => whichkey::draw(f, main, &self.key_hints()),
            Mode::NewNote {
                name,
                templates,
                template,
            } => draw_new_note(f, main, name, templates, *template),
            Mode::AddVault(name) => {
                prompt(
                    f,
                    main,
                    " add a vault · ⏎ add · Esc cancel ",
                    "folder: ",
                    name,
                );
            }
            _ => {}
        }
    }

    /// Where the peek points from: the selected link in the reader, or the selected backlink.
    fn peek_anchor(&self, from: PeekFrom) -> Option<(u16, u16)> {
        let [_, _, ea, ba] = self.areas;
        match from {
            PeekFrom::Link => {
                let (row, col) = self.reader.as_ref()?.selected_at()?;
                Some((ea.x + col, ea.y + row))
            }
            PeekFrom::Backlink => {
                let ba = inner(ba);
                let shown = self.visible_backlinks(ba.height);
                let row = self.bsel.checked_sub(shown.start)? as u16;
                Some((ba.x + 1, ba.y + row))
            }
        }
    }

    fn draw_peek(&mut self, f: &mut Frame, screen: Rect) {
        let Mode::Peek { from, .. } = &self.mode else {
            return;
        };
        let ea = self.areas[2];
        let anchor = self.peek_anchor(*from).unwrap_or((ea.x, ea.y));
        let w = ea.width.clamp(40, 80).min(screen.width);
        let room = w.saturating_sub(2);
        let enter = self.reader_keys.label("reader.follow");
        let ix = self.core.as_ref().map(|c| &c.index);
        let root = self.root.as_deref().unwrap_or(Path::new(""));
        let Mode::Peek { view, .. } = &mut self.mode else {
            return;
        };
        let (title, body) = match &mut *view {
            Peek::Note { path, reader } => {
                reader.layout(room, &|raw| resolves(ix, raw));
                let rel = path.strip_prefix(root).unwrap_or(path);
                let rel = rel.to_string_lossy();
                let place = rel.trim_end_matches(".md").to_string() + &breadcrumb(reader, path);
                (format!(" {place} "), None)
            }
            Peek::Missing(name) => (
                format!(" {name} "),
                Some(vec![Line::styled(
                    format!("No note named {name} yet · {enter} creates it"),
                    Style::new().fg(MUTED),
                )]),
            ),
            Peek::Url(url) => (
                " link ".to_string(),
                Some(vec![
                    Line::styled(url.clone(), Style::new().fg(NOTE)),
                    Line::styled(
                        format!("{enter} opens in the browser"),
                        Style::new().fg(MUTED),
                    ),
                ]),
            ),
            Peek::Attachment { name, kind, found } => (
                format!(" {name} "),
                Some(vec![
                    Line::styled(kind.clone(), Style::new().fg(NOTE)),
                    Line::styled(
                        match found {
                            true => format!("{enter} opens it"),
                            false => format!("{name} isn't in the vault"),
                        },
                        Style::new().fg(MUTED),
                    ),
                ]),
            ),
        };
        let rows = match (&body, &*view) {
            (Some(lines), _) => lines
                .iter()
                .map(|l| l.width().div_ceil(room as usize).max(1))
                .sum(),
            (None, Peek::Note { reader, .. }) => reader.line_count(),
            (None, _) => 0,
        };
        let most = (screen.height * 2 / 5).clamp(6, 20);
        let h = (rows as u16 + 2).min(most);
        let area = popup_near(anchor, w, h, screen);
        f.render_widget(Clear, area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .title(Line::styled(
                cut_left(&title, area.width.saturating_sub(4) as usize),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        f.render_widget(block, area);
        match (body, &mut *view) {
            (Some(lines), _) => {
                f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner)
            }
            (None, Peek::Note { reader, .. }) => reader.render(inner, f.buffer_mut()),
            (None, _) => {}
        }
    }

    fn draw_switcher(&self, f: &mut Frame, area: Rect) {
        let Mode::Switcher { query, sel } = &self.mode else {
            return;
        };
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let inner = picker_frame(f, area, " Go to note ", " ↑↓ choose · ⏎ open · Esc close ");
        let hits = self.switcher_hits(query);
        let create = self.offers_create(query);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(
                    if query.is_empty() {
                        "  type a name (letters in order are enough)".to_string()
                    } else {
                        format!("  {} notes", hits.len())
                    },
                    Style::new().fg(MUTED),
                ),
            ]),
            Line::raw(""),
        ];
        let rows = inner.height.saturating_sub(2) as usize;
        let total = hits.len() + create as usize;
        let start = sel.saturating_sub(rows.saturating_sub(1));
        let width = inner.width as usize;
        for i in start..(start + rows).min(total) {
            let selected = i == *sel;
            let base = if selected {
                Style::new().fg(Color::Black).bg(ACCENT)
            } else {
                Style::new().fg(NOTE)
            };
            let hl = if selected {
                base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(HIT).add_modifier(Modifier::BOLD)
            };
            if i == hits.len() {
                lines.push(Line::styled(
                    format!(" + Create note \"{}\"", query.trim()),
                    if selected { base } else { Style::new().fg(HIT) },
                ));
                continue;
            }
            let hit = &hits[i];
            let note = &ix.vault.notes[hit.note];
            let label = hit.alias.clone().unwrap_or_else(|| note.name.clone());
            let mut spans = vec![Span::styled(" ", base)];
            for (ci, ch) in label.chars().enumerate() {
                spans.push(Span::styled(
                    ch.to_string(),
                    if hit.positions.contains(&ci) {
                        hl
                    } else {
                        base
                    },
                ));
            }
            let mut extra = String::new();
            if hit.alias.is_some() {
                extra.push_str(&format!("  → {}", note.name));
            }
            let folder = note.rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            let (inc, _) = ix.counts(hit.note);
            let right = format!("{folder}  ←{inc} ");
            let used = 1 + label.chars().count() + extra.chars().count();
            let pad = width.saturating_sub(used + right.chars().count());
            let dim = if selected {
                base
            } else {
                Style::new().fg(MUTED)
            };
            spans.push(Span::styled(extra, dim));
            spans.push(Span::styled(" ".repeat(pad), base));
            spans.push(Span::styled(right, dim));
            lines.push(Line::from(spans));
        }
        if total == 0 {
            lines.push(Line::styled(" no notes match", Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_outline(&self, f: &mut Frame, area: Rect) {
        let (Mode::Outline { query, sel }, Some(r)) = (&self.mode, &self.reader) else {
            return;
        };
        let inner = picker_frame(f, area, " Outline ", " ↑↓ choose · ⏎ jump · Esc close ");
        let headings = r.headings();
        let hits = self.outline_hits(query);
        let reading = r.section().last().copied();
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(
                    format!("  {} of {} headings", hits.len(), headings.len()),
                    Style::new().fg(MUTED),
                ),
            ]),
            Line::raw(""),
        ];
        let top = headings.iter().map(|h| h.level).min().unwrap_or(1);
        let rows = inner.height.saturating_sub(2) as usize;
        let start = sel.saturating_sub(rows.saturating_sub(1));
        for (row, (i, m)) in hits.iter().enumerate().skip(start).take(rows) {
            let h = &headings[*i];
            let selected = row == *sel;
            let here = Some(*i) == reading;
            let base = match (selected, here) {
                (true, _) => Style::new().fg(Color::Black).bg(ACCENT),
                (false, true) => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                (false, false) => Style::new().fg(NOTE),
            };
            let hl = if selected {
                base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(HIT).add_modifier(Modifier::BOLD)
            };
            let indent = "  ".repeat((h.level - top) as usize);
            let mark = if here { "▸ " } else { "  " };
            let mut spans = vec![Span::styled(format!(" {indent}{mark}"), base)];
            for (ci, ch) in h.text.chars().enumerate() {
                let style = if m.positions.contains(&ci) { hl } else { base };
                spans.push(Span::styled(ch.to_string(), style));
            }
            if selected {
                let used: usize = spans.iter().map(Span::width).sum();
                let pad = (inner.width as usize).saturating_sub(used);
                spans.push(Span::styled(" ".repeat(pad), base));
            }
            lines.push(Line::from(spans));
        }
        if hits.is_empty() {
            lines.push(Line::styled(" no headings match", Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_palette(&self, f: &mut Frame, area: Rect) {
        let Mode::Palette { query, sel } = &self.mode else {
            return;
        };
        let inner = picker_frame(f, area, " Commands ", " ↑↓ choose · ⏎ run · Esc close ");
        let hits = self.palette_hits(query);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(
                    if query.is_empty() {
                        "  type a command's name or id".to_string()
                    } else {
                        format!("  {}", plural(hits.len(), "command"))
                    },
                    Style::new().fg(MUTED),
                ),
            ]),
            Line::raw(""),
        ];
        let rows = inner.height.saturating_sub(2) as usize;
        let start = sel.saturating_sub(rows.saturating_sub(1));
        let width = inner.width as usize;
        for (i, m) in hits.iter().enumerate().skip(start).take(rows) {
            let selected = i == *sel;
            let base = if selected {
                Style::new().fg(Color::Black).bg(ACCENT)
            } else {
                Style::new().fg(NOTE)
            };
            let dim = if selected {
                base
            } else {
                Style::new().fg(MUTED)
            };
            let hl = if selected {
                base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::new().fg(HIT).add_modifier(Modifier::BOLD)
            };
            let c = m.command;
            let mut spans = vec![Span::styled(" ", base)];
            let mut push = |text: &str, hits: &[usize], style: Style| {
                for (ci, ch) in text.chars().enumerate() {
                    let st = if hits.contains(&ci) { hl } else { style };
                    spans.push(Span::styled(ch.to_string(), st));
                }
            };
            push(&c.title, &m.title_hits, base);
            push("  ", &[], base);
            push(&c.id, &m.id_hits, dim);
            let right = format!("{} ", self.keys.label(&c.id));
            let used = 3 + c.title.chars().count() + c.id.chars().count();
            let pad = width.saturating_sub(used + right.chars().count());
            spans.push(Span::styled(" ".repeat(pad), base));
            let keys = if selected {
                base
            } else {
                Style::new().fg(ACCENT)
            };
            spans.push(Span::styled(right, keys));
            lines.push(Line::from(spans));
        }
        if hits.is_empty() {
            lines.push(Line::styled(" no commands match", Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_search(&self, f: &mut Frame, area: Rect) {
        let Mode::Search { query, sel } = &self.mode else {
            return;
        };
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let w = (area.width * 4 / 5).clamp(50.min(area.width), 140);
        let h = area.height.saturating_sub(4).max(8.min(area.height));
        let r = Rect::new(
            area.x + (area.width - w) / 2,
            area.y + 2.min(area.height / 8),
            w,
            h,
        );
        f.render_widget(Clear, r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(ACCENT))
            .title(Line::styled(
                " Search ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::styled(
                " ↑↓ choose · ⏎ open at the match · Esc close ",
                Style::new().fg(MUTED),
            ));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let matches: usize = self.results.iter().map(|r| r.count).sum();
        let summary = if query.trim().is_empty() {
            "  words must all appear in a note · \"quotes\" for a phrase".to_string()
        } else if self.results.is_empty() {
            "  nothing found".to_string()
        } else {
            format!(
                "  {} notes · {} matching lines",
                self.results.len(),
                matches
            )
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(summary, Style::new().fg(MUTED)),
            ]),
            Line::raw(""),
        ];
        let rows = inner.height.saturating_sub(2) as usize;
        let start = sel.saturating_sub(rows.saturating_sub(1));
        let width = inner.width as usize;
        for (i, row) in self.result_rows.iter().enumerate().skip(start).take(rows) {
            let selected = i == *sel;
            match *row {
                SearchRow::Note(ri) => {
                    let res = &self.results[ri];
                    let note = &ix.vault.notes[res.note];
                    let st = if selected {
                        Style::new()
                            .fg(Color::Black)
                            .bg(ACCENT)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD)
                    };
                    let folder = note
                        .rel
                        .rsplit_once('/')
                        .map(|(d, _)| format!("  {d}"))
                        .unwrap_or_default();
                    let more = if res.count > res.lines.len() {
                        format!("  +{} more", res.count - res.lines.len())
                    } else {
                        String::new()
                    };
                    lines.push(Line::from(vec![
                        Span::styled(format!(" ▤ {} ", note.name), st),
                        Span::styled(folder, Style::new().fg(MUTED)),
                        Span::styled(format!("  {} lines{more}", res.count), Style::new().fg(DIM)),
                    ]));
                }
                SearchRow::Line(ri, li) => {
                    let l = &self.results[ri].lines[li];
                    let num = format!("   {:>4}  ", l.line);
                    let base = if selected {
                        Style::new().fg(Color::Black).bg(ACCENT)
                    } else {
                        Style::new().fg(NOTE)
                    };
                    let hl = if selected {
                        base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                    } else {
                        Style::new().fg(Color::Black).bg(HIT)
                    };
                    let mut spans = vec![Span::styled(
                        num.clone(),
                        if selected { base } else { Style::new().fg(DIM) },
                    )];
                    spans.extend(highlight(
                        &l.text,
                        &l.ranges,
                        base,
                        hl,
                        width.saturating_sub(num.chars().count()),
                    ));
                    lines.push(Line::from(spans));
                }
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_vaults(&self, f: &mut Frame, area: Rect) {
        let title = Line::from(vec![
            Span::styled(
                " cold-lava ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("vaults ", Style::new().fg(MUTED)),
        ]);
        let block = self.block(title, Focus::Vaults);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if self.vaults.is_empty() {
            f.render_widget(
                Paragraph::new("No vaults found.\n\nPress a to add a folder.")
                    .style(Style::new().fg(MUTED))
                    .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }
        let w = inner.width as usize;
        let lines: Vec<Line> = self
            .vaults
            .iter()
            .enumerate()
            .take(inner.height as usize)
            .map(|(i, v)| {
                let sel = i == self.vsel;
                let mark = match v.source {
                    "obsidian" => "◆",
                    "example" => "✦",
                    _ => "◇",
                };
                let mut name: String = v.name.chars().take(w.saturating_sub(3)).collect();
                if v.name.chars().count() > w.saturating_sub(3) {
                    name.pop();
                    name.push('…');
                }
                let row = format!("{mark} {name:<width$}", width = w.saturating_sub(2));
                let st = if sel && self.focus == Focus::Vaults {
                    Style::new().fg(Color::Black).bg(ACCENT)
                } else if sel {
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(NOTE)
                };
                Line::styled(row, st)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_notes(&mut self, f: &mut Frame, area: Rect) {
        let vname = self
            .vaults
            .get(self.vsel)
            .map(|v| v.name.clone())
            .unwrap_or_default();
        let counts = format!(
            "{} notes ",
            self.items
                .iter()
                .filter(|i| i.kind == ItemKind::Note)
                .count()
        );
        let title = Line::from(vec![
            Span::styled(
                format!(" {vname} "),
                Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(counts, Style::new().fg(MUTED)),
        ]);
        let mut block = self.block(title, Focus::Notes);
        if !self.filter.is_empty() || matches!(self.mode, Mode::Filter) {
            let cursor = if matches!(self.mode, Mode::Filter) {
                "▏"
            } else {
                ""
            };
            block = block.title_bottom(Line::from(Span::styled(
                format!(" /{}{cursor} ", self.filter),
                Style::new().fg(HIT),
            )));
        }
        let inner = block.inner(area);
        f.render_widget(block, area);
        let h = inner.height as usize;
        if self.nsel < self.nscroll {
            self.nscroll = self.nsel;
        } else if h > 0 && self.nsel >= self.nscroll + h {
            self.nscroll = self.nsel + 1 - h;
        }
        let w = inner.width as usize;
        let flat = !self.filter.is_empty();
        let mut lines = Vec::new();
        for (r, &i) in self.rows.iter().enumerate().skip(self.nscroll).take(h) {
            let it = &self.items[i];
            let sel = r == self.nsel;
            let indent = if flat {
                String::new()
            } else {
                "  ".repeat(it.depth)
            };
            let (icon, icon_color) = match it.kind {
                ItemKind::Folder if self.expanded.contains(&it.rel) => ("▾ ", MUTED),
                ItemKind::Folder => ("▸ ", MUTED),
                ItemKind::Board => ("▦ ", Color::Rgb(83, 223, 221)),
                ItemKind::Note => ("• ", DIM),
            };
            let label = if flat {
                it.rel.trim_end_matches(".md").to_string()
            } else {
                it.name.clone()
            };
            let badge = match (it.kind, self.links.get(&it.rel)) {
                (ItemKind::Note, Some(&(inc, _))) if inc > 0 => format!("←{inc}"),
                _ => String::new(),
            };
            let room = w.saturating_sub(indent.chars().count() + 2 + badge.chars().count() + 1);
            let mut text: String = label.chars().take(room).collect();
            if label.chars().count() > room && room > 0 {
                text.pop();
                text.push('…');
            }
            let pad = w.saturating_sub(
                indent.chars().count() + 2 + text.chars().count() + badge.chars().count(),
            );
            let (fg, bg) = if sel && self.focus == Focus::Notes {
                (Color::Black, Some(ACCENT))
            } else if sel {
                (ACCENT, None)
            } else if it.kind == ItemKind::Folder {
                (ACCENT_SOFT, None)
            } else {
                (NOTE, None)
            };
            let mut st = Style::new().fg(fg);
            if let Some(b) = bg {
                st = st.bg(b);
            }
            if it.kind == ItemKind::Folder || sel {
                st = st.add_modifier(Modifier::BOLD);
            }
            let icon_st = if bg.is_some() {
                st
            } else {
                Style::new().fg(icon_color)
            };
            let badge_st = if bg.is_some() {
                st
            } else {
                Style::new().fg(MUTED)
            };
            lines.push(Line::from(vec![
                Span::styled(indent, st),
                Span::styled(icon, icon_st),
                Span::styled(text, st),
                Span::styled(" ".repeat(pad), st),
                Span::styled(badge, badge_st),
            ]));
        }
        if self.rows.is_empty() {
            let msg = if flat {
                "no matches"
            } else {
                "no notes yet · n to create one"
            };
            lines.push(Line::styled(msg, Style::new().fg(MUTED)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_editor(&mut self, f: &mut Frame, area: Rect, inner: Rect) {
        if let Some(r) = self.reader.as_mut() {
            let ix = self.core.as_ref().map(|c| &c.index);
            r.layout(inner.width, &|raw| resolves(ix, raw));
        }
        let title = match &self.shown {
            Some(p) => {
                let rel = self
                    .root
                    .as_ref()
                    .and_then(|r| p.strip_prefix(r).ok())
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let focused = self.focus == Focus::Pane;
                let read = self.keys.label("note.read");
                let leave = self.pane_keys.label("pane.leave");
                let editor = self.registry.editor();
                let editor = editor.map(|e| self.registry.manifest(e).title.as_str());
                let who = match (editor, self.reader.is_some(), focused) {
                    (_, true, true) => {
                        let back = self.reader_keys.label("reader.back");
                        format!(" reading · {back} back ")
                    }
                    (Some(c), true, false) => format!(" reader · ⏎ read · {read} {c} "),
                    (None, _, false) => " reader · ⏎ read · e edit in $EDITOR ".to_string(),
                    (Some(c), false, true) => format!(" {c} · editing · {leave} back "),
                    (Some(c), false, false) => format!(" {c} · ⏎ edit · {read} read "),
                    (None, false, true) => String::new(),
                };
                let mut place = rel.trim_end_matches(".md").to_string();
                if let Some(r) = &self.reader {
                    place.push_str(&breadcrumb(r, p));
                }
                // Room inside the corners; the place keeps at least 24 columns before `who`.
                let room = (area.width as usize).saturating_sub(4);
                let who_width = Span::raw(&who).width();
                let who = if room.saturating_sub(who_width) >= 24 {
                    who
                } else {
                    String::new()
                };
                let place = cut_left(&place, room.saturating_sub(Span::raw(&who).width()));
                Line::from(vec![
                    Span::styled(
                        format!(" {place} "),
                        Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(who, Style::new().fg(MUTED)),
                ])
            }
            None => Line::styled(" note ", Style::new().fg(MUTED)),
        };
        let block = self.block(title, Focus::Pane);
        f.render_widget(block, area);
        if let Some(r) = self.reader.as_mut() {
            r.render(inner, f.buffer_mut());
            return;
        }
        if let Some(p) = self.pane.as_mut() {
            p.term.resize(inner.height, inner.width);
            if let Some((x, y)) = p.term.render(f.buffer_mut(), inner)
                && self.focus == Focus::Pane
                && matches!(self.mode, Mode::Normal)
            {
                f.set_cursor_position((x, y));
            }
            return;
        }
        let hint = match self.current().map(|it| it.kind) {
            Some(ItemKind::Folder) => "a folder · → to open it",
            Some(ItemKind::Board) => "a whiteboard · ⏎ to open it",
            _ => "select a note to see it here",
        };
        f.render_widget(
            Paragraph::new(Line::styled(hint, Style::new().fg(MUTED))),
            inner,
        );
    }

    fn draw_backlinks(&self, f: &mut Frame, area: Rect) {
        let n = self.backs.len();
        let title = Line::from(vec![
            Span::styled(
                " Backlinks ",
                Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if n == 0 {
                    "none · b hide ".to_string()
                } else {
                    format!("{n} · b hide ")
                },
                Style::new().fg(MUTED),
            ),
        ]);
        let block = self.block(title, Focus::Backlinks);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if n == 0 {
            let msg = "No other note links here yet.";
            f.render_widget(
                Paragraph::new(Line::styled(msg, Style::new().fg(MUTED))),
                inner,
            );
            return;
        }
        let shown = self.visible_backlinks(inner.height);
        let focused = self.focus == Focus::Backlinks;
        let lines: Vec<Line> = self
            .backs
            .iter()
            .enumerate()
            .skip(shown.start)
            .take(shown.len())
            .map(|(i, b)| {
                let sel = i == self.bsel && focused;
                let name_st = if sel {
                    Style::new()
                        .fg(Color::Black)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD)
                };
                let mut spans = vec![
                    Span::styled(format!(" ← {} ", b.name), name_st),
                    Span::raw(" "),
                ];
                // The line with the link, with light markdown styling.
                let ctx = board::markdown_text(&b.context);
                if let Some(first) = ctx.lines.into_iter().next() {
                    spans.extend(first.spans);
                }
                Line::from(spans)
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_status(&mut self, f: &mut Frame, area: Rect) {
        let mut info = match (self.focus, self.vaults.get(self.vsel)) {
            (Focus::Vaults, Some(v)) => {
                let src = match v.source {
                    "obsidian" => "from Obsidian",
                    "example" => "example vault",
                    "here" => "opened here",
                    _ => "recent",
                };
                Line::from(vec![
                    Span::styled(format!(" {} ", v.path.display()), Style::new().fg(NOTE)),
                    Span::styled(format!(" {src}"), Style::new().fg(MUTED)),
                ])
            }
            _ => match self.current() {
                Some(it) => {
                    let (inc, out) = self.links.get(&it.rel).copied().unwrap_or_default();
                    let extra = if it.kind == ItemKind::Note {
                        format!("   ←{inc} in  {out}→ out")
                    } else {
                        String::new()
                    };
                    Line::from(vec![
                        Span::styled(format!(" {}", it.rel), Style::new().fg(NOTE)),
                        Span::styled(extra, Style::new().fg(ACCENT_SOFT)),
                    ])
                }
                None => Line::raw(""),
            },
        };
        let first = |id| self.keys.keys(id).first();
        let nav = Nav::of(&self.history).hint(first("history.back"), first("history.forward"));
        if !nav.is_empty() {
            info.spans
                .push(Span::styled(format!("   {nav}"), Style::new().fg(MUTED)));
        }
        if self
            .msg
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3))
        {
            self.msg = None;
        }
        let hints = match (self.focus, &self.mode) {
            (_, Mode::Filter) => " type to filter · ↑↓ move · ⏎ keep · Esc clear".to_string(),
            (_, Mode::Hints(_)) => {
                " link hints · type a label to follow its link · Esc cancel".to_string()
            }
            (_, Mode::Peek { .. }) => {
                let k = |id| self.reader_keys.label(id);
                format!(
                    " peek · {} scroll · {} page · {} {} next / previous · {} go there · {} close",
                    k("reader.down"),
                    k("reader.page-down"),
                    k("reader.next-link"),
                    k("reader.previous-link"),
                    k("reader.follow"),
                    k("reader.back"),
                )
            }
            (Focus::Pane, _) if self.reader.is_some() => {
                let k = |id| self.reader_keys.label(id);
                format!(
                    " reading · {} scroll · {} next link · {} follow · {} keys · {} help · {} back to notes",
                    k("reader.down"),
                    k("reader.next-link"),
                    k("reader.follow"),
                    self.keys.label("app.keys"),
                    self.keys.label("app.help"),
                    k("reader.back"),
                )
            }
            (Focus::Pane, _) => {
                let manifest = self.pane.as_ref().map(|p| self.registry.manifest(p.owner));
                let title = manifest.map(|m| format!("in {}", m.title));
                let hints = manifest.iter().flat_map(|m| &m.hints);
                let leave = format!("{} to the menu", self.pane_keys.label("pane.leave"));
                let parts: Vec<String> = title
                    .into_iter()
                    .chain(hints.map(|(key, does)| format!("{key} {does}")))
                    .chain([leave])
                    .collect();
                format!(" {}", parts.join(" · "))
            }
            (Focus::Backlinks, _) => {
                " backlinks · ↑↓ choose · ⏎ open at the link · ← back to notes · b hide".to_string()
            }
            _ => {
                let edit = if self.reader.is_some() {
                    "⏎ read"
                } else {
                    "⏎ edit"
                };
                let k = |id| self.keys.first_label(id);
                format!(
                    " ↑↓ move  ←→ fold  {edit}  {} go to  {} search  {} keys  {} help  {} quit",
                    k("note.switch"),
                    k("search.open"),
                    k("app.keys"),
                    k("app.help"),
                    k("app.quit"),
                )
            }
        };
        let second = match &self.msg {
            Some((m, _)) => Line::styled(format!(" {m}"), Style::new().fg(HIT)),
            None => Line::styled(hints, Style::new().fg(DIM)),
        };
        f.render_widget(Paragraph::new(vec![info, second]), area);
    }
}

/// Whether a wiki or Markdown link written in a note leads to a note (a heading in the same
/// note always does).
fn resolves(ix: Option<&crate::index::Index>, raw: &str) -> bool {
    let link = crate::index::split_link(raw);
    link.note.is_empty() || ix.is_some_and(|ix| ix.resolver.resolve(&link.note).is_some())
}

/// What kind of attachment a link names (`photo.jpg` → `JPG image`); `None` for a note.
fn attachment_label(name: &str) -> Option<String> {
    let kind = vault::attachment_kind(name)?;
    let ext = name.rsplit_once('.')?.1.to_uppercase();
    Some(format!("{ext} {kind}"))
}

/// A `w`×`h` popup under the point `at`, or above it when there is more room there, kept on
/// `screen` and shrunk to fit.
/// The delete prompt's width in `area`.
fn popup_width(area: Rect) -> u16 {
    (area.width * 3 / 4).clamp(50.min(area.width), 120)
}

fn popup_near(at: (u16, u16), w: u16, h: u16, screen: Rect) -> Rect {
    let (x, y) = at;
    let below = screen.bottom().saturating_sub(y + 1);
    let above = y.saturating_sub(screen.y);
    let (top, h) = if below >= h || below >= above {
        (y + 1, h.min(below))
    } else {
        (y - h.min(above), h.min(above))
    };
    let w = w.min(screen.width);
    let left = x.saturating_sub(2).clamp(screen.x, screen.right() - w);
    Rect::new(left, top, w, h)
}

/// ` › Section › Subsection`: the headings the reader is under, without a first heading that
/// only repeats the note's name.
fn breadcrumb(r: &Reader, path: &Path) -> String {
    let name = vault::search_key(&path.file_stem().unwrap_or_default().to_string_lossy());
    let headings = r.headings();
    let section = r.section();
    let skip = section
        .first()
        .is_some_and(|&i| vault::search_key(&headings[i].text) == name);
    let crumbs = section.iter().skip(skip as usize);
    crumbs
        .map(|&i| format!(" › {}", headings[i].text))
        .collect()
}

/// `text` in at most `width` columns, cut from the left with `…`.
fn cut_left(text: &str, width: usize) -> String {
    if Span::raw(text).width() <= width {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = Span::raw(c.to_string()).width();
        if used + w > width {
            break;
        }
        used += w;
        kept.push(c);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn dir_of_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// `text` with `ranges` (char ranges) highlighted, cut to `width` chars around the first match.
fn highlight(
    text: &str,
    ranges: &[(usize, usize)],
    base: Style,
    hl: Style,
    width: usize,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.trim_start().chars().collect();
    let shift = text.chars().count() - chars.len();
    let ranges: Vec<(usize, usize)> = ranges
        .iter()
        .map(|&(a, b)| (a.saturating_sub(shift), b.saturating_sub(shift)))
        .collect();
    // Start a little before the first match if it would be off-screen.
    let first = ranges.first().map(|r| r.0).unwrap_or(0);
    let start = if first + 10 > width {
        first.saturating_sub(width / 3)
    } else {
        0
    };
    let mut spans = Vec::new();
    let mut avail = width;
    if start > 0 {
        spans.push(Span::styled("…", base));
        avail = avail.saturating_sub(1);
    }
    let mut cur = String::new();
    let mut cur_hl = false;
    for (i, ch) in chars.iter().enumerate().skip(start).take(avail) {
        let in_hl = ranges.iter().any(|&(a, b)| i >= a && i < b);
        if in_hl != cur_hl && !cur.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut cur),
                if cur_hl { hl } else { base },
            ));
        }
        cur_hl = in_hl;
        cur.push(*ch);
    }
    if !cur.is_empty() {
        spans.push(Span::styled(cur, if cur_hl { hl } else { base }));
    }
    spans
}

/// The new-note prompt, with the vault's templates under it when it has some.
fn draw_new_note(
    f: &mut Frame,
    area: Rect,
    name: &str,
    templates: &[(String, String)],
    template: Option<usize>,
) {
    if templates.is_empty() {
        prompt(
            f,
            area,
            " new note · ⏎ create · Esc cancel ",
            "name: ",
            name,
        );
        return;
    }
    let title = " new note · Tab template · ⏎ create · Esc cancel ";
    let r = prompt(f, area, title, "name: ", name);
    let room = area.bottom().saturating_sub(r.bottom()) as usize;
    let rows = (templates.len() + 1).min(room.saturating_sub(2));
    let list = Rect::new(r.x, r.bottom(), r.width, rows as u16 + 2);
    let shown = template.map_or(0, |i| i + 1);
    let start = shown.saturating_sub(rows.saturating_sub(1));
    let choices =
        std::iter::once("No template").chain(templates.iter().map(|(_, name)| name.as_str()));
    let lines: Vec<Line> = choices
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(i, t)| {
            let style = if i == shown {
                Style::new().fg(Color::Black).bg(ACCENT)
            } else if i == 0 {
                Style::new().fg(MUTED)
            } else {
                Style::new().fg(NOTE)
            };
            let width = list.width.saturating_sub(2) as usize;
            let mark = if i == shown { "▸" } else { " " };
            Line::styled(
                format!("{mark} {t:<w$}", w = width.saturating_sub(2)),
                style,
            )
        })
        .collect();
    f.render_widget(Clear, list);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT_SOFT))
                .title(" template "),
        ),
        list,
    );
}

/// Draw a one-line prompt in the upper middle of `area`; returns where it went.
fn prompt(f: &mut Frame, area: Rect, title: &str, label: &str, value: &str) -> Rect {
    let w = 60.min(area.width.saturating_sub(4));
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + area.height / 3,
        w,
        3,
    );
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(label.to_string(), Style::new().fg(MUTED)),
            Span::styled(format!("{value}▏"), Style::new().fg(NOTE)),
        ]))
        .block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(title.to_string()),
        ),
        r,
    );
    r
}

/// The centred popup shared by the quick switcher and the command palette; returns its inside.
fn picker_frame(f: &mut Frame, area: Rect, title: &str, hint: &str) -> Rect {
    let w = (area.width * 2 / 3).clamp(40.min(area.width), 100);
    let h = (area.height * 2 / 3).clamp(8.min(area.height), 30);
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + area.height / 6,
        w,
        h,
    );
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::styled(
            title,
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(hint, Style::new().fg(MUTED)));
    let inner = block.inner(r);
    f.render_widget(block, r);
    inner
}

fn draw_help(
    f: &mut Frame,
    area: Rect,
    (menu, keys): (&[Entry<MenuCommand>], &Keymap<MenuCommand>),
    reader_keys: &Keymap<ReaderAction>,
    pane_keys: &Keymap<PaneAction>,
    editor: Option<&Manifest>,
) {
    let enter = match editor {
        Some(m) => format!("go to the note in the reader or {} · open a board", m.title),
        None => "go to the note in the reader · open a board".to_string(),
    };
    let fixed = [
        ("↑↓ / jk", "move through vaults or notes".to_string()),
        (
            "→ / ←",
            "open / close a folder · vaults ↔ notes".to_string(),
        ),
        ("Enter", enter),
    ];
    let row = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("{k:>12}  "), Style::new().fg(ACCENT)),
            Span::styled(d.to_string(), Style::new().fg(NOTE)),
        ])
    };
    let heading = |text: String| {
        Line::from(Span::styled(
            format!("      {text}"),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
    };
    let menu: Vec<Line> = fixed
        .iter()
        .map(|(k, d)| row(k, d))
        .chain(menu.iter().map(|c| row(&keys.label(&c.id), &c.title)))
        .collect();
    let mut right = vec![heading("In the reader".to_string())];
    right.extend(
        commands::READER
            .iter()
            .map(|c| row(&reader_keys.label(c.id), c.title)),
    );
    if let Some(m) = editor {
        right.push(Line::raw(""));
        right.push(heading(format!("In {}", m.title)));
        let leave = commands::PANE.iter();
        right.extend(leave.map(|c| row(&pane_keys.label(c.id), c.title)));
        right.extend(m.hints.iter().map(|(k, d)| row(k, d)));
    }
    // Side by side when there's room, else the reader's and the pane's keys under the menu's.
    let column = 70;
    let (lines, w) = if area.width >= 2 * column + 4 {
        let mut lines = menu;
        for (i, r) in right.into_iter().enumerate() {
            let pad = " ".repeat((column as usize).saturating_sub(lines[i].width()));
            lines[i].spans.push(Span::raw(pad));
            lines[i].spans.extend(r.spans);
        }
        (lines, 2 * column)
    } else {
        let mut lines = menu;
        lines.push(Line::raw(""));
        lines.extend(right);
        (lines, column.min(area.width.saturating_sub(4)))
    };
    let h = (lines.len() as u16 + 2).min(area.height);
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(format!(
                    " cold-lava {} · any key closes ",
                    env!("CARGO_PKG_VERSION")
                )),
        ),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_obsidian_vault_list() {
        let json = r#"{"vaults":{"a1":{"path":"/home/me/Notes","ts":1700000000000,"open":true},
                                 "b2":{"path":"/home/me/Work"}},"frame":"hidden"}"#;
        let mut v = parse_obsidian_vaults(json);
        v.sort();
        assert_eq!(
            v,
            [
                (PathBuf::from("/home/me/Notes"), 1700000000000),
                (PathBuf::from("/home/me/Work"), 0)
            ]
        );
        assert!(parse_obsidian_vaults("not json").is_empty());
    }

    #[test]
    fn tree_sorting_and_folding() {
        let items = scan_tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-vault"));
        let rels: Vec<&str> = items.iter().map(|i| i.rel.as_str()).collect();
        // Folders first, then top-level files; boards are included.
        assert_eq!(rels[0], "Concepts");
        assert!(rels.contains(&"Welcome.canvas"));
        let concepts = rels.iter().position(|r| *r == "Concepts").unwrap();
        let guide = rels.iter().position(|r| *r == "Guide").unwrap();
        assert!(
            rels[concepts + 1].starts_with("Concepts/"),
            "folder contents follow the folder"
        );
        assert!(guide > concepts);
        assert!(!rels.iter().any(|r| r.starts_with(".obsidian")));

        let collapsed = visible(&items, &HashSet::new(), "");
        assert!(collapsed.iter().all(|&i| items[i].depth == 0));
        let open: HashSet<String> = ["Volcanoes".to_string()].into();
        let rows = visible(&items, &open, "");
        assert!(rows.iter().any(|&i| items[i].rel == "Volcanoes/Kīlauea.md"));
        assert!(!rows.iter().any(|&i| items[i].rel.starts_with("Concepts/")));

        let found = visible(&items, &HashSet::new(), "kil");
        assert_eq!(items[found[0]].rel, "Volcanoes/Kīlauea.md");
    }

    #[test]
    fn peek_opens_under_the_link_or_above_it_near_the_bottom() {
        let screen = Rect::new(0, 0, 100, 40);
        assert_eq!(
            popup_near((30, 5), 60, 16, screen),
            Rect::new(28, 6, 60, 16)
        );
        assert_eq!(
            popup_near((90, 35), 60, 16, screen),
            Rect::new(40, 19, 60, 16)
        );
        let small = Rect::new(0, 0, 30, 10);
        let r = popup_near((0, 4), 60, 16, small);
        assert_eq!(r, Rect::new(0, 5, 30, 5), "shrunk to the room below");
    }

    #[test]
    fn attachments_are_named_by_type_and_dotted_note_names_are_notes() {
        assert_eq!(
            attachment_label("kilauea-lava-lake.jpg").as_deref(),
            Some("JPG image")
        );
        assert_eq!(
            attachment_label("Maps/Hawaii.PDF").as_deref(),
            Some("PDF document")
        );
        assert_eq!(attachment_label("Mount St. Helens"), None);
        assert_eq!(attachment_label("Lahar"), None);
    }
}
