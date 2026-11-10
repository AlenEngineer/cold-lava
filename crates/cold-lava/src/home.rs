//! The main menu: workspaces on the left, the selected workspace's notes in the middle, and the
//! selected note on the right, in the reader or in the editor extension's pane (a real,
//! embedded Neovim when it's installed).

use crate::app::{ACCENT, ACCENT_SOFT, DIM, GraphScreen, HIT, MUTED, NOTE, Render, Then};
use crate::bookmarks::{self, Entry as Bookmark, Target};
use crate::commands::{
    self, Entry, HistoryAction, Key, KeyHint, Keymap, MenuAction, MenuCommand, PaneAction,
    ReaderAction,
};
use crate::core::{Core, Event as CoreEvent, Jump};
use crate::embed::Embedded;
use crate::ext::view::{Happened, ViewId, ViewNews};
use crate::ext::{
    AGENDA, CommandRequest, EditorRequest, Effect, ExtId, Extension, FileNews, FilePreview, Host,
    Manifest, PaneId, PaneNews, PaneSpec, READER, Registry, TABLE,
};
use crate::frontmatter::{self, TagEdit};
use crate::gateway::{Answer, Edit, Op, Reason, Ticket};
use crate::hints::{Hints, Typed};
use crate::history::{Exit, History, Nav, Place};
use crate::index::Backlink;
use crate::looks::{self, Looks};
use crate::markdown::{self, Flavour};
use crate::paths::{self, config_dir, home_dir};
use crate::previews::{self, Previews};
use crate::reader::{Reader, Spot};
use crate::recipe_view::{self, RecipeView};
use crate::resolver::dir_of;
use crate::scene_view;
use crate::tidy_view::{self, TidyView};
use crate::trash::{self, TrashOption, Trashed, Went};
use crate::type_table;
use crate::views::{self, Views};
use crate::{agenda, board, files, items, templates, term, whichkey, workspace};
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
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

// ---------- workspace discovery ----------

#[derive(Clone)]
pub struct WorkspaceEntry {
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

fn recent_dir() -> Option<PathBuf> {
    config_dir().map(|c| c.join("cold-lava"))
}

/// The list of opened workspaces, `{"workspaces": [{"path", "opened"}]}`.
const RECENT_FILE: &str = "workspaces.json";
/// Where 0.3.0 and earlier kept that list, as `{"vaults": [...]}`: read while `RECENT_FILE`
/// doesn't exist, and left in place.
const OLD_RECENT_FILE: &str = "vaults.json";

fn load_recent(dir: &Path) -> Vec<(PathBuf, u64)> {
    let read = |file: &str| std::fs::read_to_string(dir.join(file)).ok();
    let Some((text, key)) = read(RECENT_FILE)
        .map(|t| (t, "workspaces"))
        .or_else(|| read(OLD_RECENT_FILE).map(|t| (t, "vaults")))
    else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    v.get(key)
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

fn remember(dir: &Path, path: &Path) {
    let mut list = load_recent(dir);
    list.retain(|(p, _)| p != path);
    list.insert(0, (path.to_path_buf(), now_ms()));
    list.truncate(50);
    let json = serde_json::json!({
        "workspaces": list.iter().map(|(p, t)| serde_json::json!({"path": p, "opened": t})).collect::<Vec<_>>()
    });
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(
        dir.join(RECENT_FILE),
        serde_json::to_string_pretty(&json).unwrap_or_default(),
    );
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn looks_like_workspace(p: &Path) -> bool {
    p.join(".obsidian").is_dir()
        || std::fs::read_dir(p)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .any(|e| e.path().extension().is_some_and(|x| x == "md"))
            })
            .unwrap_or(false)
}

/// Every workspace we know about, most recently used first, without duplicates.
pub fn discover(extra: &[PathBuf]) -> Vec<WorkspaceEntry> {
    let mut found: Vec<(PathBuf, u64, &'static str)> = Vec::new();
    for p in extra {
        found.push((p.clone(), u64::MAX, "here"));
    }
    for (p, t) in recent_dir().map(|d| load_recent(&d)).unwrap_or_default() {
        found.push((p, t, "recent"));
    }
    for cfg in obsidian_configs() {
        if let Ok(text) = std::fs::read_to_string(&cfg) {
            for (p, t) in parse_obsidian_vaults(&text) {
                found.push((p, t, "obsidian"));
            }
        }
    }
    // The bundled example workspace, when run from the repository.
    for base in [
        std::env::current_dir().ok(),
        Some(PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))),
    ]
    .into_iter()
    .flatten()
    {
        let ex = base.join("example-workspace");
        if ex.is_dir() {
            found.push((ex, 1, "example"));
        }
    }
    let mut seen = HashSet::new();
    let mut out: Vec<WorkspaceEntry> = Vec::new();
    for (p, ts, source) in found {
        let Ok(canon) = p.canonicalize() else {
            continue;
        };
        if !canon.is_dir() {
            continue;
        }
        // The bundled example workspace only if no workspace of that name is listed already.
        if source == "example" && out.iter().any(|e| e.name == "example-workspace") {
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
            out.push(WorkspaceEntry {
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
    /// A file of a kind an extension owns, which opens it.
    File,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub kind: ItemKind,
}

/// All folders, notes and boards in a workspace, and the files of the kinds `owned` says an
/// extension opens (by extension), as a depth-first sorted list (folders before files,
/// case-insensitive).
pub fn scan_tree(root: &Path, owned: impl Fn(&str) -> bool) -> Vec<Item> {
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
                    x if owned(&x.to_ascii_lowercase()) => ItemKind::File,
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
                ItemKind::Folder | ItemKind::File => file,
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
        let q = workspace::search_key(filter);
        let mut hits: Vec<(usize, bool)> = items
            .iter()
            .enumerate()
            .filter(|(_, it)| {
                it.kind != ItemKind::Folder && workspace::search_key(&it.rel).contains(&q)
            })
            .map(|(i, it)| (i, workspace::search_key(&it.name).starts_with(&q)))
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
    Workspaces,
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
    /// In a file another owner reads: the owner's place for the link, which opens it there.
    place: Option<String>,
    context: String,
}

enum Mode {
    Normal,
    Filter,
    /// Typing a new note's name; `templates` are the workspace's (workspace-relative without `.md`, and
    /// the name shown), and `template` is the chosen one's place in them, `None` for none.
    NewNote {
        name: String,
        templates: Vec<(String, String)>,
        template: Option<usize>,
    },
    AddWorkspace(String),
    /// Typing a new folder's name.
    NewFolder(String),
    Help,
    /// Which key: the commands this screen has, by group, until a key is typed.
    Keys,
    /// Quick switcher: jump to a note by (fuzzy) name, or with `merging` pick the note to merge
    /// that one into.
    Switcher {
        query: String,
        sel: usize,
        merging: Option<PathBuf>,
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
    /// Typing a note's or folder's new name (or the folder to move it into).
    Rename {
        path: PathBuf,
        input: String,
        moving: bool,
    },
    /// Typing the folder to move the marked notes and folders into.
    MoveMarked(String),
    /// Typing a tag to add to the marked notes (or the selected one); `-tag` removes it. `all`
    /// is the workspace's tags, and `sel` the one Tab completed to among those matching `typed`.
    Tag {
        typed: String,
        all: Vec<String>,
        sel: Option<usize>,
    },
    /// Showing what a rename will change before doing it.
    ConfirmRename {
        plan: Box<crate::rename::Plan>,
        scroll: usize,
        /// It moves the marked items, so the marks are done with afterwards.
        marked: bool,
        /// Confirmed, and waiting for the owners of files other owners read to give their new
        /// texts (`Core::relink`); nothing has moved yet.
        waiting: Option<Ticket>,
    },
    /// Showing what merging a note into another will change before doing it.
    ConfirmMerge {
        plan: Box<crate::merge::Merge>,
        scroll: usize,
        /// Confirmed, and waiting for owners, as a rename's.
        waiting: Option<Ticket>,
    },
    /// Asking before a note or folder goes to the trash.
    ConfirmDelete(Box<PendingDelete>),
    /// Undoing a delete whose old paths are taken now: put those back under free names? Each
    /// item with where it goes back to.
    ConfirmRestore(Vec<(Trashed, PathBuf)>),
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
    /// The workspace's bookmarks (Obsidian's); `sel` is a row of `bookmark_rows`.
    Bookmarks {
        query: String,
        sel: usize,
    },
    /// What nothing uses: orphan notes, unused attachments, empty folders, unresolved links.
    Tidy(Box<TidyView>),
    /// `recipe.run`: picking a recipe, filling its options, or looking at its plan.
    Recipe(Box<RecipeView>),
    /// The graph view, `Home::graph`, over the whole screen.
    Graph,
    /// A view an extension opened, kept in `Home::views`. `backlinks`: keys go to the backlinks
    /// pane under it, the backlinks of the file it shows, instead of to the view.
    View {
        owner: ExtId,
        view: ViewId,
        backlinks: bool,
    },
}

/// A note, board or folder about to go to the trash, as the user was shown it.
struct Doomed {
    path: PathBuf,
    /// Workspace-relative, without `.md`.
    rel: String,
    contents: trash::Contents,
}

/// A delete waiting for a yes: what the user is deciding on, and what links to it.
struct PendingDelete {
    /// One item, or the marked ones.
    doomed: Vec<Doomed>,
    /// For a folder, or several items: how many notes go.
    notes: Option<usize>,
    /// Links from outside to them (or to the notes in the folders).
    backlinks: Vec<BackRow>,
    /// Which trash it goes to, as the prompt says it.
    goes: String,
    /// It changed on disk after the first prompt, so this is the question again.
    changed: bool,
    scroll: usize,
    /// Asked from the Tidy view, which comes back afterwards.
    tidy: Option<Box<TidyView>>,
}

/// Something `note.undo-delete` can put back.
enum Undo {
    /// A delete: one item, or the marked ones.
    Delete(Vec<Trashed>),
    Merge(Box<crate::merge::Merged>),
    Recipe(Box<crate::recipe::Applied>),
}

/// Deletes, merges and recipes that `note.undo-delete` can still put back.
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
        /// Whether it is in the workspace, so following it opens it.
        found: bool,
    },
}

/// Where a wiki or Markdown link written in a note leads.
enum Destination {
    /// A note, at a heading or block when the link names one.
    Note(PathBuf, Option<Jump>),
    /// No note by this name yet: following the link creates it.
    NewNote(String),
    /// A file that isn't a note (`map.pdf`, a canvas), and where it is if it's in the workspace.
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
    workspaces: Vec<WorkspaceEntry>,
    vsel: usize,
    // The open workspace
    root: Option<PathBuf>,
    items: Vec<Item>,
    expanded: HashSet<String>,
    rows: Vec<usize>,
    nsel: usize,
    nscroll: usize,
    filter: String,
    links: HashMap<String, (usize, usize)>,
    /// The open workspace: index, file watching and change events.
    core: Option<Core>,
    backs: Vec<BackRow>,
    /// The file `backs` are the backlinks of (`backlinks_target`).
    backs_of: Option<PathBuf>,
    bsel: usize,
    show_backlinks: bool,
    keys: Keymap<MenuCommand>,
    reader_keys: Keymap<ReaderAction>,
    pane_keys: Keymap<PaneAction>,
    /// Recently shown notes, most recent first (the quick switcher's starting list).
    mru: Vec<PathBuf>,
    /// Back and forward, within the open workspace.
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
    /// Each extension's own part of the status line (`ui/status`).
    statuses: Vec<(ExtId, String)>,
    /// The views extensions opened and haven't closed, shown or not.
    views: Views,
    /// The workspace's graph, once opened: kept while the workspace is, shown or not.
    graph: Option<GraphScreen>,
    /// The graph showed in the last frame (its pixels go when it stops showing).
    graph_shown: bool,
    /// Opened as `cold-lava graph`: leaving the graph quits.
    graph_alone: bool,
    /// What owners answered to `file/preview`, for the embeds the reader draws.
    previews: Previews,
    /// The picture placed with Kitty graphics: its view and the cells it covers. And the one
    /// the frame just drawn wants there.
    picture: Option<(ExtId, ViewId, Rect)>,
    picture_wanted: Option<(ExtId, ViewId, Rect)>,
    /// Reading an extension's edits back during `call`: calls meanwhile leave theirs for the
    /// next frame.
    catching_up: bool,
    shown: Option<PathBuf>,
    pending: Option<(PathBuf, Instant)>,
    /// What was deleted or merged this session, most recent last.
    undo: Vec<Undo>,
    /// Marked tree rows (workspace-relative, as `Item::rel`), for moving, deleting and tagging
    /// several at once.
    marks: BTreeSet<String>,
    /// Bookmark groups folded in the bookmarks popup (as `Bookmark::at`).
    folded: HashSet<Vec<usize>>,
    /// A screen a link led to, gone to once the key that followed it is handled.
    go_to: Option<Place>,
    /// `Some` while the pane shows the reader: always without an editor, else after
    /// `note.read`.
    reader: Option<Reader>,
    /// The open workspace's Markdown flavour.
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
        let workspaces = discover(&extra);
        let mut h = Home {
            render,
            workspaces,
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
            backs_of: None,
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
            statuses: Vec::new(),
            views: Views::new(match render {
                Render::Braille => false,
                Render::Auto => crate::kitty::supported(),
                Render::Pixels => true,
            }),
            previews: Previews::default(),
            graph: None,
            graph_shown: false,
            graph_alone: false,
            picture: None,
            picture_wanted: None,
            catching_up: false,
            shown: None,
            pending: None,
            undo: Vec::new(),
            marks: BTreeSet::new(),
            folded: HashSet::new(),
            go_to: None,
            reader: None,
            flavour: markdown::flavour_for(Path::new("")),
            zoom: false,
            focus: Focus::Workspaces,
            mode: Mode::Normal,
            msg: None,
            areas: [Rect::default(); 4],
            quit: false,
        };
        if !h.workspaces.is_empty() {
            h.open_workspace(0);
            if initial.is_some() {
                h.focus = Focus::Notes;
            }
        }
        let config = crate::config::get();
        match h.registry.mistakes() {
            [] => {}
            [why] => h.flash(why.clone()),
            many => h.flash(format!(
                "{} extensions didn't load: run cold-lava ext list to see why",
                many.len()
            )),
        }
        match (config.errors.len(), h.registry.refused()) {
            (0, []) if !config.warnings.is_empty() => {
                h.flash("config.toml uses an old command name: run cold-lava keys to see it")
            }
            (0, []) => {}
            (0, [why]) => h.flash(why.clone()),
            (0, refused) => h.flash(format!(
                "{} extension commands, keys or file types were left out: run cold-lava keys to see why",
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
            self.poll_extensions();
            self.pump_pane();
            self.run_requested(terminal)?;
            if let Some(place) = self.go_to.take() {
                self.go_from_here(place, terminal)?;
            }
            self.load_pending();
            self.check_disk();
            self.views.settle();
            self.ask_previews();
            if self.backlinks_target() != self.backs_of {
                self.bsel = 0;
                self.refresh_backlinks();
            }
            let graph_wait = self.tick_graph();
            self.picture_wanted = None;
            terminal.draw(|f| self.draw(f))?;
            self.place_picture()?;
            if let Some(g) = self.graph.as_mut().filter(|_| self.graph_shown) {
                term::write_raw(&g.view.take_image());
            }
            let ms = if self.pane.is_some() || self.pending.is_some() {
                15
            } else if self.registry.has_processes() {
                50
            } else {
                250
            };
            let wait = graph_wait.map_or(Duration::from_millis(ms), |w| {
                w.min(Duration::from_millis(ms))
            });
            if event::poll(wait)? {
                loop {
                    match event::read()? {
                        Event::Key(k) if k.kind == KeyEventKind::Press => {
                            self.on_key(k, terminal)?
                        }
                        Event::Mouse(m) => self.on_mouse(m),
                        Event::Resize(..) => {
                            if let Some(g) = self.graph.as_mut() {
                                g.view.resized();
                            }
                        }
                        _ => {}
                    }
                    if self.quit || !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }
        }
        self.hide_picture()?;
        self.hide_graph();
        self.save_editor();
        self.registry.shutdown();
        Ok(())
    }

    /// The graph's frame, when it is in front: where Back leads, a step of its layout. How long
    /// it may wait for input. Its pixels go when it stops being in front.
    fn tick_graph(&mut self) -> Option<Duration> {
        let shown = matches!(self.mode, Mode::Graph);
        if self.graph_shown && !shown {
            self.hide_graph();
        }
        self.graph_shown = shown;
        let (Some(g), Some(core)) = (self.graph.as_mut(), self.core.as_ref()) else {
            return None;
        };
        if !shown {
            return None;
        }
        g.view.navigate(Nav::of(&self.history));
        Some(g.tick(core))
    }

    /// Remove the graph's pixels, before something else takes the terminal.
    fn hide_graph(&mut self) {
        if let Some(g) = self.graph.as_mut() {
            term::write_raw(&g.view.hide());
        }
    }

    /// Answer what extensions in processes asked since the last frame.
    fn poll_extensions(&mut self) {
        for ext in self.registry.ids() {
            self.call(ext, |e, host| e.poll(host));
        }
        let registry = &self.registry;
        self.views.retain_owners(|e| registry.running(e));
        self.forget_closed_views();
    }

    /// Back and the screen no longer lead to views that closed.
    fn forget_closed_views(&mut self) {
        let views = &self.views;
        (self.history).retain(|p| match p {
            Place::View { owner, view, .. } => views.has(*owner, *view),
            _ => true,
        });
        if let Mode::View { owner, view, .. } = self.mode
            && !self.views.has(owner, view)
        {
            self.mode = Mode::Normal;
        }
    }

    /// Tell every extension which workspace is open, or only the ones that `stopped`, to start
    /// them again. Their views belonged to the workspace before.
    fn tell_workspace(&mut self, stopped: bool) {
        let root = self.root.clone();
        for ext in self.registry.ids() {
            if stopped && self.registry.stopped(ext).is_none() {
                continue;
            }
            self.statuses.retain(|(e, _)| *e != ext);
            self.views.retain_owners(|e| e != ext);
            self.call(ext, |e, host| e.workspace(root.as_deref(), host));
        }
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

    /// Call an extension, then do what it asked for. When it edited items, the menu reads them
    /// back first, as after its own screens' edits, so every view shows the edit and the
    /// extension's message about it comes after the attribution.
    fn call(&mut self, ext: ExtId, f: impl FnOnce(&mut dyn Extension, &mut Host<'_>)) {
        let effects = self.registry.call(ext, self.core.as_mut(), f);
        if !self.catching_up
            && let Some(core) = self.core.as_mut()
            && core.has_unseen()
        {
            self.catching_up = true;
            let events = core.poll();
            self.sync_with_core(&events);
            self.catching_up = false;
        }
        for effect in effects {
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
            Effect::Status { text } => {
                let at = self.statuses.iter().position(|(e, _)| *e == ext);
                match (at, text) {
                    (Some(i), Some(text)) => self.statuses[i].1 = text,
                    (None, Some(text)) => self.statuses.push((ext, text)),
                    (Some(i), None) => {
                        self.statuses.remove(i);
                    }
                    (None, None) => {}
                }
            }
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
            Effect::OpenView {
                view,
                content,
                select,
                file,
            } => self.open_view(ext, view, content, select, file),
            Effect::UpdateView {
                view,
                content,
                select,
            } => {
                self.views.update(ext, view, content, select);
            }
            Effect::CloseView { view } => {
                self.views.close(ext, view);
                self.forget_closed_views();
            }
            Effect::ShowView { view } => self.show_view(ext, view),
            Effect::OpenNote { note, line, item } => self.open_for(ext, &note, line, item),
            Effect::Previewed { path, rev, content } => {
                if self.previews.answer(&path, &rev, content)
                    && let Some(r) = self.reader.as_mut()
                {
                    r.invalidate();
                }
            }
        }
    }

    /// Show an extension's new view in place of what showed, which Back returns to.
    fn open_view(
        &mut self,
        ext: ExtId,
        view: ViewId,
        content: crate::ext::view::Content,
        select: Option<String>,
        file: Option<String>,
    ) {
        if let Some(oldest) = self.views.open(ext, view, content, select, file) {
            self.forget_closed_views();
            let closed = ViewNews::Closed { view: oldest };
            self.call(ext, |e, host| e.view(closed, host));
        }
        self.show_view(ext, view);
    }

    /// Bring one of an extension's open views to the front, in place of what showed, which
    /// Back returns to. Nothing when it isn't open or is in front already.
    fn show_view(&mut self, ext: ExtId, view: ViewId) {
        let shown =
            matches!(self.mode, Mode::View { owner, view: v, .. } if (owner, v) == (ext, view));
        if shown || !self.views.has(ext, view) {
            return;
        }
        if let Some(here) = self.here() {
            self.history.visit(here);
        }
        if self.focus == Focus::Pane {
            self.focus = Focus::Notes;
        }
        self.mode = Mode::View {
            owner: ext,
            view,
            backlinks: false,
        };
    }

    /// `workspace/open`: the note at `note` (inside the workspace, absolute or relative to it),
    /// at `line`, from where you are.
    fn open_for(&mut self, ext: ExtId, note: &Path, line: Option<usize>, item: Option<String>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let owned = self.core.as_ref().and_then(|c| {
            let rel = note.strip_prefix(c.root()).unwrap_or(note);
            let rel = rel.to_string_lossy().replace('\\', "/");
            (c.index.workspace.owned.iter()).find(|f| f.rel == rel)
        });
        if let Some(f) = owned {
            let place = item.and_then(|id| f.places.get(&id).cloned());
            let path = f.path.clone();
            self.mode = Mode::Normal;
            return self.open_file_at(&path, place);
        }
        let path = root.join(note);
        let inside = match (path.canonicalize(), root.canonicalize()) {
            (Ok(p), Ok(r)) => p.starts_with(r) && p.is_file(),
            _ => false,
        };
        if !inside {
            let name = &self.registry.manifest(ext).name;
            let note = note.display();
            return self.flash(format!(
                "{name} asked to open {note}, which isn't in the workspace"
            ));
        }
        let from = self.here();
        self.mode = Mode::Normal;
        self.open_found(from, &path, line.map(Jump::Line));
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

    // ---------- workspaces & notes ----------

    fn open_workspace(&mut self, i: usize) {
        let Some(v) = self.workspaces.get(i).cloned() else {
            return;
        };
        self.vsel = i;
        self.root = Some(v.path.clone());
        self.views.root = Some(v.path.clone());
        self.flavour = markdown::flavour_for(&v.path);
        self.expanded.clear();
        self.filter.clear();
        self.marks.clear();
        self.folded.clear();
        self.nsel = 0;
        self.nscroll = 0;
        self.core = None; // a different workspace: start a fresh core
        self.graph = None;
        self.history.clear();
        self.reload_tree();
        // Start with top-level folders open if the workspace is small enough to take in at once.
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
        if let Some(dir) = recent_dir() {
            remember(&dir, &v.path);
        }
        self.tell_workspace(false);
        self.set_owners();
        self.select_row(0);
    }

    fn reload_tree(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.set_items(self.scan_tree(&root));
        self.links.clear();
        match self.core.as_mut() {
            Some(core) => core.rescan_now(),
            None => self.core = Core::open(&root).ok(),
        }
        if let Some(ix) = self.core.as_ref().map(|c| &c.index) {
            for (i, n) in ix.workspace.notes.iter().enumerate() {
                self.links.insert(format!("{}.md", n.rel), ix.counts(i));
            }
        }
        self.refresh_rows();
        self.refresh_backlinks();
    }

    /// The file whose backlinks the pane lists: the one the view in front shows, else the note
    /// in the editor pane. A view of no file has none.
    fn backlinks_target(&self) -> Option<PathBuf> {
        match self.mode {
            Mode::View { owner, view, .. } => {
                let file = self.views.get(owner, view)?.file.as_ref()?;
                Some(self.root.as_ref()?.join(file))
            }
            _ => self.shown.clone(),
        }
    }

    /// Backlinks of the note in the editor pane, or of the file the view in front shows: a
    /// note or a file another owner reads.
    fn refresh_backlinks(&mut self) {
        self.backs.clear();
        self.backs_of = self.backlinks_target();
        let (Some(ix), Some(of)) = (self.core.as_ref().map(|c| &c.index), &self.backs_of) else {
            return;
        };
        let Some(backs) = ix.backlinks_of(of) else {
            return;
        };
        self.backs = backs
            .iter()
            .map(|b| BackRow {
                name: ix.name(b.from).to_string(),
                path: ix.path_of(b.from),
                line: b.line,
                place: b.place.clone(),
                context: b.context.clone(),
            })
            .collect();
        self.bsel = self.bsel.min(self.backs.len().saturating_sub(1));
    }

    /// Where a link written in `from` leads, resolved the way Obsidian resolves it.
    fn destination(&self, from: &Path, raw: &str) -> Option<Destination> {
        let link = crate::resolver::split_link(raw);
        let ix = &self.core.as_ref()?.index;
        let jump = link
            .heading
            .map(Jump::Heading)
            .or(link.block.map(Jump::Block));
        if link.note.is_empty() {
            return Some(Destination::Note(from.to_path_buf(), jump));
        }
        let dir = ix
            .by_path(from)
            .map_or("", |i| dir_of(&ix.workspace.notes[i].rel));
        if let Some(i) = ix.resolver.resolve(dir, &link.note) {
            return Some(Destination::Note(ix.path(i), jump));
        }
        let owned = self.registry.owner_of(&link.note).is_some();
        if owned || workspace::attachment_kind(&link.note).is_some() {
            let path = crate::resolver::file(ix, &link.note, ix.by_path(from));
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
                if jump.is_none() && self.shower(&path).is_some() {
                    return self.open_file(&path);
                }
                self.leave(here, &path, jump.as_ref());
                self.show_at(&path, jump);
                self.select_path(&path);
            }
            Destination::NewNote(name) => {
                let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
                    return;
                };
                let path = crate::resolver::new_note_path(ix, &name, ix.by_path(from));
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
                self.flash(format!("{name} isn't in the workspace"))
            }
            Destination::Attachment {
                path: Some(path), ..
            } if self.registry.owner_of(&path.to_string_lossy()).is_some() => {
                self.open_file(&path);
            }
            Destination::Attachment {
                path: Some(path), ..
            } if workspace::attachment_kind(&path.to_string_lossy()) == Some("whiteboard") => {
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
        if let Mode::View { owner, view, .. } = self.mode {
            let label = self.registry.manifest(owner).name.clone();
            return Some(Place::View { owner, view, label });
        }
        if let (Mode::Graph, Some(g)) = (&self.mode, &self.graph) {
            let selected = g.view.selected().map(|n| n.id.clone());
            return Some(Place::Graph { selected });
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
                Place::Graph { selected } => {
                    self.show_graph(selected)?;
                    None
                }
                Place::Board { path } => {
                    self.mode = Mode::Normal;
                    self.run_board(terminal, path)?
                }
                Place::View { owner, view, .. } => {
                    if self.views.has(owner, view) {
                        if self.focus == Focus::Pane {
                            self.focus = Focus::Notes;
                        }
                        self.mode = Mode::View {
                            owner,
                            view,
                            backlinks: false,
                        };
                    }
                    None
                }
            };
        }
        Ok(())
    }

    /// Where leaving a board leads. Quitting goes to the menu, so Back comes back.
    fn left(&mut self, here: Place, exit: Exit) -> Option<Place> {
        match exit {
            Exit::Quit => {
                self.history.visit(here);
                None
            }
            Exit::Back => self.history.back(Some(here)),
            Exit::Forward => self.history.forward(Some(here)),
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
        let rels: HashSet<&str> = items.iter().map(|it| it.rel.as_str()).collect();
        self.marks.retain(|r| rels.contains(r.as_str()));
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
                self.reader = Some(self.shown_reader());
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

    /// After an edit through the gateway: read `note` again (the edit may have found the core
    /// behind the disk), then everything the core has to say, so every view shows the edit, or
    /// what refused it, before the next frame.
    fn catch_up(&mut self, note: &Path) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        let mut events = core.note_changed_now(note);
        events.extend(core.poll());
        if !events.is_empty() {
            self.sync_with_core(&events);
        }
    }

    /// Hand the item subscriptions' news to the extensions that hold them. A subscription
    /// nobody holds any more is stopped.
    fn deliver_item_news(&mut self) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        for (owner, news) in core.take_item_news() {
            let Some(core) = self.core.as_mut() else {
                return;
            };
            let sub = news.subscription();
            match self.registry.by_name(&owner) {
                Some(ext) => self.call(ext, |e, host| e.items(news, host)),
                None => drop(core.unsubscribe_items(&owner, sub)),
            }
        }
    }

    /// Bring the tree, link counts, backlinks and reader up to date with the core's index.
    fn sync_with_core(&mut self, events: &[CoreEvent]) {
        let Some(root) = self.root.clone() else {
            return;
        };
        if let Some(g) = self.graph.as_mut() {
            g.follow(events, &root);
        }
        let tree_changed = events.iter().any(|e| match e {
            CoreEvent::FileChanged(rel) => self.registry.owner_of(rel).is_some(),
            CoreEvent::NoteChanged(_)
            | CoreEvent::OwnedChanged(_)
            | CoreEvent::OwnerTrouble { .. }
            | CoreEvent::GraphSettingsChanged
            | CoreEvent::BookmarksChanged
            | CoreEvent::ItemEdited { .. } => false,
            _ => true,
        });
        if tree_changed {
            self.set_items(self.scan_tree(&root));
        }
        self.links.clear();
        if let Some(ix) = self.core.as_ref().map(|c| &c.index) {
            for (i, n) in ix.workspace.notes.iter().enumerate() {
                self.links.insert(format!("{}.md", n.rel), ix.counts(i));
            }
        }
        self.refresh_rows();
        self.refresh_backlinks();
        if let (Mode::Tidy(view), Some(core)) = (&mut self.mode, &self.core) {
            view.update(crate::tidy::report(&core.index));
        }
        self.deliver_item_news();
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
            CoreEvent::BoardChanged(_)
            | CoreEvent::FileChanged(_)
            | CoreEvent::OwnedChanged(_)
            | CoreEvent::OwnerTrouble { .. }
            | CoreEvent::GraphSettingsChanged
            | CoreEvent::BookmarksChanged
            | CoreEvent::ItemEdited { .. } => false,
        });
        if reread {
            self.reload_reader();
        } else if let Some(r) = self.reader.as_mut() {
            r.invalidate();
        }
        for e in events {
            match e {
                CoreEvent::ItemEdited { note, by, what, .. } => {
                    let name = note.rsplit('/').next().unwrap_or(note);
                    self.flash(format!("{by}: {what} on {name}"));
                }
                CoreEvent::OwnerTrouble {
                    owner,
                    path,
                    message,
                } => self.flash(format!("{owner} couldn't read {path}: {message}")),
                _ => {}
            }
        }
        self.tell_owners(&root, events);
        self.ask_owners();
        self.deliver_edited();
        self.deliver_relinked();
    }

    /// Hand the answers to `items/edit` that came later (edits of files other owners read) to
    /// whoever asked: an extension, or one of the menu's screens, whose status line says it.
    fn deliver_edited(&mut self) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        for (by, ticket, answer) in core.take_edited() {
            match (self.registry.by_name(&by), answer) {
                (Some(ext), answer) => self.call(ext, |e, host| e.edited(ticket, answer, host)),
                (None, Ok(_)) => {}
                (None, Err(e)) => self.flash(format!("not saved: {e}")),
            }
        }
    }

    /// The tree of the workspace at `root`, with the files extensions open.
    fn scan_tree(&self, root: &Path) -> Vec<Item> {
        scan_tree(root, |x| self.registry.owner(x).is_some())
    }

    /// `file/changed` to the owner of each file that changed on disk.
    fn tell_owners(&mut self, root: &Path, events: &[CoreEvent]) {
        for e in events {
            let CoreEvent::FileChanged(rel) = e else {
                continue;
            };
            // Its preview, in the note shown, is asked for again.
            if self.previews.has(rel)
                && let Some(r) = self.reader.as_mut()
            {
                r.invalidate();
            }
            let Some((owner, _)) = self.registry.owner_of(rel) else {
                continue;
            };
            let news = FileNews::Changed {
                path: rel.clone(),
                rev: files::rev_at(root, rel),
            };
            self.call(owner, |e, host| e.file(news, host));
        }
    }

    /// Remove the picture shown with Kitty graphics, before another screen takes the terminal.
    fn hide_picture(&mut self) -> Result<()> {
        self.picture_wanted = None;
        self.place_picture()
    }

    /// Show the picture of the view in front with Kitty graphics, under the text, or remove
    /// the one shown when that view is gone, moved or out of sight.
    fn place_picture(&mut self) -> Result<()> {
        if self.picture == self.picture_wanted {
            return Ok(());
        }
        let mut out = Vec::new();
        if self.picture.take().is_some() {
            out.extend(crate::kitty::delete(crate::kitty::PICTURE));
        }
        if let Some((owner, view, at)) = self.picture_wanted {
            let png = self.views.get(owner, view).and_then(|v| v.picture()?.png());
            if let Some(png) = png {
                out.extend(crate::kitty::place_png(png, at, crate::kitty::PICTURE));
                self.picture = self.picture_wanted;
            }
        }
        let mut stdout = std::io::stdout();
        std::io::Write::write_all(&mut stdout, &out)?;
        std::io::Write::flush(&mut stdout)?;
        Ok(())
    }

    /// Ask owners for the previews the reader wanted while drawing (`file/preview`).
    fn ask_previews(&mut self) {
        for (path, rev, width) in self.previews.take_wanted() {
            let Some((owner, _)) = self.registry.owner_of(&path) else {
                continue;
            };
            let preview = FilePreview {
                path,
                rev,
                width,
                height: previews::HEIGHT,
            };
            self.call(owner, |e, host| e.preview_file(preview, host));
        }
    }

    /// What an embed in the note at `from` shows under its name: its owner's preview, when an
    /// extension owns the file it names (`file/preview`); `None` leaves its name alone.
    fn embed_lines(
        &self,
        previews: &mut Previews,
        from: Option<&Path>,
        target: &str,
        width: usize,
    ) -> Option<Vec<Line<'static>>> {
        let ix = &self.core.as_ref()?.index;
        let link = crate::resolver::split_link(target);
        let path = crate::resolver::file(ix, &link.note, from.and_then(|p| ix.by_path(p)))?;
        let rel = path.strip_prefix(&ix.workspace.root).ok()?;
        let rel = rel.to_string_lossy().replace('\\', "/");
        let root = &ix.workspace.root;
        let Some((owner, _)) = self.registry.owner_of(&rel) else {
            // A picture nobody owns: its kind and size, and a PNG in half blocks.
            workspace::attachment_kind(&rel).filter(|k| *k == "image")?;
            let image = crate::image::Image {
                title: rel.clone(),
                source: crate::image::Source::Path(rel),
            };
            let content = crate::ext::view::Content::Image(image);
            let lines = views::preview(&content, Some(root), width, previews::HEIGHT as usize);
            return Some(lines.into_iter().skip(1).collect());
        };
        let rev = files::rev_at(root, &rel)?;
        let name = &self.registry.manifest(owner).name;
        Some(previews.lines(&rel, &rev, name, width as u16, Some(root)))
    }

    /// Hand the file at `path` to the extension that opens its kind (`file/open`), which shows
    /// it in a view.
    fn open_file(&mut self, path: &Path) {
        self.open_file_at(path, None);
    }

    /// `open_file`, at the owner's `place` for an item or a link in it. A note goes to the
    /// extension that shows it, if one does.
    fn open_file_at(&mut self, path: &Path, place: Option<String>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        let to =
            (self.registry.owner_of(&rel).map(|(owner, _)| owner)).or_else(|| self.shower(path));
        if let Some(to) = to {
            let news = FileNews::Open { path: rel, place };
            self.call(to, |e, host| e.file(news, host));
        }
    }

    /// The extension that shows the note at `path` instead of the reader (`[[shows]]`): the
    /// most specific claim the note meets, of an extension that hasn't stopped.
    fn shower(&self, path: &Path) -> Option<ExtId> {
        let ix = &self.core.as_ref()?.index;
        let note = &ix.workspace.notes[ix.by_path(path)?];
        let rel = format!("{}.md", note.rel);
        let (ext, _) = self.registry.shower_of(&rel, &note.text, &note.tags)?;
        self.registry.stopped(ext).is_none().then_some(ext)
    }

    /// The note the view in front shows, when an extension shows one (`[[shows]]`).
    fn viewed_note(&self) -> Option<PathBuf> {
        let Mode::View { owner, view, .. } = self.mode else {
            return None;
        };
        let file = self.views.get(owner, view)?.file.as_deref()?;
        let path = self.root.as_ref()?.join(file);
        let ix = &self.core.as_ref()?.index;
        ix.by_path(&path).map(|_| path)
    }

    /// Tell the core who owns which kind of file, so their files' items and links join it
    /// (from the cache, else by asking the owners).
    fn set_owners(&mut self) {
        let kinds = self.registry.owned_kinds();
        if let Some(core) = self.core.as_mut() {
            let cache = paths::cache_dir().map(|d| d.join("cold-lava/owners"));
            core.set_owners(kinds, cache.as_deref());
        }
        self.ask_owners();
    }

    /// Hand each owner the `file/read` and `file/edit` requests the core has for it.
    fn ask_owners(&mut self) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        let (reads, edits) = (core.take_reads(), core.take_file_edits());
        for (owner, read) in reads {
            match self.registry.by_name(&owner) {
                Some(ext) => self.call(ext, |e, host| e.read_file(read, host)),
                None => drop(read),
            }
        }
        for (owner, edit) in edits {
            match (self.registry.by_name(&owner), self.core.as_mut()) {
                (Some(ext), _) => self.call(ext, |e, host| e.edit_file(edit, host)),
                (None, Some(core)) => {
                    let why = Err(format!("{owner} isn't loaded"));
                    core.file_edited(&owner, &edit.path, &edit.rev, why);
                }
                (None, None) => {}
            }
        }
    }

    /// The shown note, parsed.
    fn read_shown(&self) -> (markdown::Doc, Looks) {
        match &self.shown {
            Some(p) => self.read_note(p),
            None => (markdown::parse("", &self.flavour), Looks::default()),
        }
    }

    fn shown_reader(&self) -> Reader {
        let (doc, looks) = self.read_shown();
        Reader::new(doc, looks)
    }

    /// The note parsed, with its typed values' looks: from the items the core has for it when
    /// they were read from this text, else read now.
    fn read_note(&self, path: &Path) -> (markdown::Doc, Looks) {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return (markdown::parse(&text, &self.flavour), Looks::default());
        };
        let workspace = &ix.workspace;
        let settings = workspace.item_settings();
        let read;
        let items = match ix.by_path(path).map(|i| &workspace.notes[i]) {
            Some(note) if note.text == text => workspace.items(note),
            _ => {
                let rel = path.strip_prefix(&workspace.root).unwrap_or(path);
                read = items::extract(&rel.to_string_lossy().replace('\\', "/"), &text, settings);
                &read
            }
        };
        looks::read(&text, &self.flavour, items, &settings.types)
    }

    /// Read the shown note again, keeping the reader's place.
    fn reload_reader(&mut self) {
        let (doc, looks) = self.read_shown();
        if let Some(r) = self.reader.as_mut() {
            r.set_doc(doc, looks);
        }
    }

    fn flash(&mut self, s: impl Into<String>) {
        let s = s.into();
        if let (Mode::Graph, Some(g)) = (&self.mode, self.graph.as_mut()) {
            g.view.flash(s.clone());
        }
        self.msg = Some((s, Instant::now()));
    }

    // ---------- input ----------

    fn on_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        if matches!(self.mode, Mode::Graph) {
            return self.graph_key(k, terminal);
        }
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
        if matches!(self.mode, Mode::Bookmarks { .. }) {
            self.bookmarks_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Tidy(_)) {
            self.tidy_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::View { .. }) {
            return self.view_key(k, terminal);
        }
        if matches!(self.mode, Mode::Palette { .. }) {
            return self.palette_key(k, terminal);
        }
        if matches!(self.mode, Mode::Search { .. }) {
            self.search_key(k);
            return Ok(());
        }
        if matches!(
            self.mode,
            Mode::Rename { .. } | Mode::ConfirmRename { .. } | Mode::MoveMarked(_)
        ) {
            self.rename_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Tag { .. }) {
            self.tag_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::ConfirmDelete(_) | Mode::ConfirmRestore(_)) {
            self.delete_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::ConfirmMerge { .. }) {
            self.merge_key(k);
            return Ok(());
        }
        if matches!(self.mode, Mode::Recipe(_)) {
            self.recipe_key(k);
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
            Mode::NewFolder(name) => {
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
                            self.create_folder(&text);
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::AddWorkspace(name) => {
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
                            self.add_workspace(&text);
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
            | Mode::MoveMarked(_)
            | Mode::Tag { .. }
            | Mode::ConfirmRename { .. }
            | Mode::ConfirmDelete(_)
            | Mode::ConfirmMerge { .. }
            | Mode::ConfirmRestore(_)
            | Mode::Hints(_)
            | Mode::Outline { .. }
            | Mode::Bookmarks { .. }
            | Mode::Tidy(_)
            | Mode::Recipe(_)
            | Mode::View { .. }
            | Mode::Graph
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
            Focus::Workspaces => self.workspace_key(k),
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
            MenuCommand::Core(MenuAction::ClearFilter) => {
                !self.filter.is_empty() || !self.marks.is_empty()
            }
            MenuCommand::Core(
                MenuAction::NewNote
                | MenuAction::NewFolder
                | MenuAction::Delete
                | MenuAction::Tag
                | MenuAction::Duplicate,
            ) => self.root.is_some(),
            MenuCommand::Core(MenuAction::Merge | MenuAction::Recipe) => self.core.is_some(),
            MenuCommand::Core(MenuAction::Mark) => {
                self.focus == Focus::Notes && self.current().is_some()
            }
            MenuCommand::Core(MenuAction::UndoDelete) => !self.undo.is_empty(),
            MenuCommand::Core(MenuAction::RestartExtensions) => {
                self.root.is_some()
                    && self
                        .registry
                        .ids()
                        .any(|e| self.registry.stopped(e).is_some())
            }
            MenuCommand::Ext(ext, _) => self.registry.running(ext),
            MenuCommand::Core(
                MenuAction::Bookmarks
                | MenuAction::Bookmark
                | MenuAction::Tidy
                | MenuAction::Table
                | MenuAction::Agenda,
            ) => self.core.is_some(),
            MenuCommand::Core(MenuAction::BookmarkSearch) => {
                matches!(self.mode, Mode::Search { .. })
            }
            MenuCommand::Core(MenuAction::Read) => {
                (self.running_editor().is_some() && self.shown.is_some())
                    || self.viewed_note().is_some()
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
            ReaderAction::Tick => self.tick_in_reader(),
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

    /// Tick the reader's task (`Reader::task_line`) as the core last read it.
    fn tick_in_reader(&mut self) {
        let line = self.reader.as_ref().and_then(Reader::task_line);
        let (Some(line), Some(path), Some(root)) = (line, self.shown.clone(), self.root.clone())
        else {
            return self.flash("no task on screen");
        };
        let task = self.core.as_ref().and_then(|core| {
            let note = &core.index.workspace.notes[core.index.by_path(&path)?];
            let raw = note.text.lines().nth(line - 1)?;
            let settings = &core.index.workspace.item_settings().tasks;
            (core.index.workspace.items(note).iter())
                .filter(|i| i.line == line)
                .find_map(|i| {
                    let task = crate::tasks::Task::of(i, raw, settings)?;
                    Some((i.id.clone(), i.rev.clone(), task))
                })
        });
        let Some((id, rev, task)) = task else {
            return self.flash(format!("line {line} is no task"));
        };
        let rel = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy();
        self.tick(READER, &id, &rev, &rel.replace('\\', "/"), &task.text);
    }

    /// Tick the task `id` in `note` (workspace-relative, with `.md`) through the workspace
    /// gateway, on behalf of the screen `by`: the way the Tasks plugin does, refused if it is no
    /// longer the version `rev`. Every view shows the change, or what refused it, on the next
    /// frame. Returns the line the ticked task is on now. `text` is its words, for the status
    /// line.
    fn tick(
        &mut self,
        by: &'static str,
        id: &str,
        rev: &str,
        note: &str,
        text: &str,
    ) -> Option<usize> {
        let path = self.root.as_ref()?.join(note);
        let name = note.trim_end_matches(".md");
        let core = self.core.as_mut()?;
        let edit = Edit {
            id: id.to_string(),
            rev: rev.to_string(),
            op: Op::Toggle,
        };
        let (msg, at) = match self.registry.screen(by, core).edit_item(&edit) {
            Ok(Answer::Later(_)) => (format!("ticking: {}", agenda::links_as_text(text)), None),
            Ok(Answer::Now(e)) => {
                let settings = &core.index.workspace.item_settings().tasks;
                let status = match &e.item.data {
                    items::Data::Typed(t) => (t.fields.get("status"))
                        .and_then(|s| s.as_str()?.chars().next())
                        .map(|c| settings.status_type(c).agenda_name()),
                    _ => None,
                };
                let what = match status {
                    Some("done") => "done",
                    Some("cancelled") => "cancelled",
                    Some("in_progress") => "in progress",
                    Some("todo") => "open again",
                    _ => "ticked",
                };
                let mut msg = format!("{what}: {}", agenda::links_as_text(text));
                if e.next.is_some() {
                    msg.push_str(" · next occurrence added");
                }
                if let Some(w) = &e.warning {
                    msg = format!("{msg} · {w}");
                }
                (msg, Some(e.item.line))
            }
            Err(e) if e.reason == Reason::Changed => (
                format!("not ticked: {name} changed since it was read · read again"),
                None,
            ),
            Err(e) => (format!("not ticked: {e}"), None),
        };
        self.catch_up(&path);
        self.flash(msg);
        at
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
                let b = self.backs.get(self.bsel).filter(|b| b.place.is_none());
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
                    kind: (self.registry.owner_of(&name).map(|(_, t)| t.title.clone()))
                        .or_else(|| attachment_label(&name))
                        .unwrap_or_default(),
                    found: path.is_some(),
                    name,
                },
            },
        )
    }

    fn peek_note(&self, path: PathBuf, jump: Option<Jump>) -> Peek {
        let (doc, looks) = self.read_note(&path);
        let mut reader = Reader::new(doc, looks);
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
                    Focus::Workspaces => Focus::Notes,
                    Focus::Notes if self.pane.is_some() || self.reader.is_some() => Focus::Pane,
                    Focus::Notes | Focus::Pane if has_backs => Focus::Backlinks,
                    _ => Focus::Workspaces,
                }
            }
            MenuAction::Palette => {
                self.mode = Mode::Palette {
                    query: String::new(),
                    sel: 0,
                }
            }
            MenuAction::Switcher => self.open_switcher(),
            MenuAction::Bookmarks => self.open_bookmarks(),
            MenuAction::Tidy => self.open_tidy(),
            MenuAction::Table => self.hand_to(TABLE, type_table::OPEN),
            MenuAction::Agenda => self.hand_to(AGENDA, agenda::OPEN),
            MenuAction::Bookmark => match self.bookmark_target() {
                Some(target) => self.toggle_bookmark(target),
                None => self.flash("select a note or folder to bookmark"),
            },
            MenuAction::BookmarkSearch => self.bookmark_search(),
            MenuAction::Rename => self.start_rename(false),
            MenuAction::Move if !self.marks.is_empty() => {
                self.mode = Mode::MoveMarked(String::new())
            }
            MenuAction::Move => self.start_rename(true),
            MenuAction::Delete => self.start_delete(false),
            MenuAction::UndoDelete => self.undo_delete(),
            MenuAction::Duplicate => self.duplicate(),
            MenuAction::Merge => self.start_merge(),
            MenuAction::Recipe => self.mode = Mode::Recipe(Box::new(RecipeView::new())),
            MenuAction::Search => self.open_search(),
            MenuAction::ToggleBacklinks => {
                self.show_backlinks = !self.show_backlinks;
                if !self.show_backlinks && self.focus == Focus::Backlinks {
                    self.focus = Focus::Notes;
                }
            }
            MenuAction::FocusPrevious => {
                self.focus = if self.focus == Focus::Notes {
                    Focus::Workspaces
                } else {
                    Focus::Notes
                }
            }
            MenuAction::Filter => {
                self.mode = Mode::Filter;
                self.focus = Focus::Notes;
            }
            MenuAction::ClearFilter if !self.marks.is_empty() => {
                self.marks.clear();
                self.flash("marks cleared");
            }
            MenuAction::ClearFilter => {
                self.filter.clear();
                self.refresh_rows();
            }
            MenuAction::Mark => self.toggle_mark(),
            MenuAction::Tag => self.start_tag(),
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
            MenuAction::AddWorkspace => self.mode = Mode::AddWorkspace(String::new()),
            MenuAction::NewFolder => self.mode = Mode::NewFolder(String::new()),
            MenuAction::RestartExtensions => {
                self.tell_workspace(true);
                self.set_owners();
            }
            MenuAction::Rescan => {
                if let Some(core) = self.core.as_mut() {
                    core.rescan(); // applied on the next poll, as a Rescanned event
                }
                self.flash("rescanning");
            }
            MenuAction::OpenInObsidian => self.open_in_obsidian(),
            MenuAction::EditFullscreen => self.edit_fullscreen(terminal)?,
            MenuAction::Read if self.viewed_note().is_some() => {
                let path = self.viewed_note().unwrap_or_default();
                if let Some(here) = self.here() {
                    self.history.visit(here);
                }
                self.reader = None;
                self.return_to_note(&path, None);
                if self.reader.is_none() {
                    self.reader = Some(self.shown_reader());
                    self.focus = Focus::Pane;
                }
            }
            MenuAction::Read => {
                if self.reader.take().is_none() {
                    self.save_editor();
                    self.reader = Some(self.shown_reader());
                }
            }
        }
        Ok(())
    }

    fn workspace_key(&mut self, k: KeyEvent) {
        let n = self.workspaces.len();
        if n == 0 {
            return;
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.open_workspace((self.vsel + 1).min(n - 1)),
            KeyCode::Up | KeyCode::Char('k') => self.open_workspace(self.vsel.saturating_sub(1)),
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
                    self.focus = Focus::Workspaces;
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
                    ItemKind::File => {
                        if let Some(p) = self.current_path() {
                            self.open_file(&p);
                        }
                    }
                    ItemKind::Note => {
                        let Some(p) = self.current_path() else {
                            return Ok(());
                        };
                        self.pending = None;
                        if k.code == KeyCode::Enter && self.shower(&p).is_some() {
                            self.open_file(&p);
                            return Ok(());
                        }
                        self.show(&p);
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
            merging: None,
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
                let p = &ix.workspace.notes[h.note].path;
                self.mru.iter().position(|m| m == p).unwrap_or(usize::MAX)
            };
            hits.sort_by(|a, b| {
                rank(a).cmp(&rank(b)).then_with(|| {
                    ix.workspace.notes[a.note]
                        .rel
                        .to_lowercase()
                        .cmp(&ix.workspace.notes[b.note].rel.to_lowercase())
                })
            });
        }
        hits
    }

    /// The switcher's list, without the note being merged when it picks where to merge it.
    fn switcher_targets(
        &self,
        query: &str,
        merging: Option<&Path>,
    ) -> Vec<crate::search::SwitchHit> {
        let mut hits = self.switcher_hits(query);
        if let (Some(source), Some(ix)) = (merging, self.core.as_ref().map(|c| &c.index)) {
            hits.retain(|h| ix.workspace.notes[h.note].path != source);
        }
        hits
    }

    /// Files of the kinds extensions open, which the switcher lists after the notes: (tree
    /// item, match), best first, by name, else by path.
    fn switcher_files(&self, query: &str) -> Vec<(usize, crate::search::Fuzzy)> {
        let mut hits: Vec<_> = (self.items.iter().enumerate())
            .filter(|(_, it)| it.kind == ItemKind::File)
            .filter_map(|(i, it)| {
                let by_path = || {
                    let f = crate::search::fuzzy(query, &it.rel)?;
                    Some(crate::search::Fuzzy {
                        score: f.score - 10,
                        positions: Vec::new(),
                    })
                };
                Some((i, crate::search::fuzzy(query, &it.name).or_else(by_path)?))
            })
            .collect();
        hits.sort_by_key(|(i, f)| {
            (
                std::cmp::Reverse(f.score),
                self.items[*i].rel.to_lowercase(),
            )
        });
        hits
    }

    /// Whether the switcher offers "create note" (a query that isn't exactly an existing name).
    fn offers_create(&self, query: &str) -> bool {
        let q = workspace::search_key(query.trim());
        let file = |it: &Item| it.kind == ItemKind::File && workspace::search_key(&it.name) == q;
        !q.is_empty()
            && !self.items.iter().any(file)
            && self.core.as_ref().map(|c| &c.index).is_some_and(|ix| {
                !ix.workspace.notes.iter().any(|n| {
                    workspace::search_key(&n.name) == q
                        || n.aliases.iter().any(|a| workspace::search_key(a) == q)
                })
            })
    }

    fn switcher_key(&mut self, k: KeyEvent) {
        let Mode::Switcher {
            query,
            sel,
            merging,
        } = &mut self.mode
        else {
            return;
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let (mut q, mut s, merging) = (query.clone(), *sel, merging.clone());
        if let Some(source) = merging.as_deref()
            && k.code == KeyCode::Enter
        {
            let hits = self.switcher_targets(&q, Some(source));
            self.mode = Mode::Normal;
            self.back_to_editor = false;
            let ix = self.core.as_ref().map(|c| &c.index);
            return match hits.get(s).zip(ix).map(|(h, ix)| ix.path(h.note)) {
                Some(target) => self.plan_merge(source, &target),
                None => self.flash("merge cancelled: no note picked"),
            };
        }
        let create = merging.is_none() && self.offers_create(&q);
        let files = match merging {
            None => self.switcher_files(&q).len(),
            Some(_) => 0,
        };
        let total = self.switcher_targets(&q, merging.as_deref()).len() + files + create as usize;
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
                let file = s.checked_sub(hits.len()).and_then(|f| {
                    let (item, _) = self.switcher_files(&q).get(f).cloned()?;
                    self.root.as_ref().map(|r| r.join(&self.items[item].rel))
                });
                self.mode = Mode::Normal;
                let from = self.here();
                if let Some(path) = file {
                    self.open_file(&path);
                } else if let Some(h) = hits.get(s) {
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
                        crate::resolver::new_note_path(ix, q.trim(), from)
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
        self.mode = Mode::Switcher {
            query: q,
            sel: s,
            merging,
        };
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

    // ---------- bookmarks ----------

    fn open_bookmarks(&mut self) {
        match self.core.as_ref().map(Core::bookmarks) {
            Some(Ok(_)) => {}
            Some(Err(e)) => return self.flash(format!("can't read the bookmarks: {e}")),
            None => return,
        }
        self.back_to_editor = self.focus == Focus::Pane;
        if self.focus == Focus::Pane {
            self.focus = Focus::Notes;
        }
        self.mode = Mode::Bookmarks {
            query: String::new(),
            sel: 0,
        };
    }

    /// The bookmarks popup's rows: every bookmark outside folded groups, or with a query the
    /// ones (not groups) whose name matches, with the match.
    fn bookmark_rows(&self, query: &str) -> Vec<(Bookmark, Option<crate::search::Fuzzy>)> {
        let Some(Ok(b)) = self.core.as_ref().map(Core::bookmarks) else {
            return Vec::new();
        };
        let all = b.entries().into_iter();
        if query.trim().is_empty() {
            let folded = |e: &Bookmark| (1..e.at.len()).any(|n| self.folded.contains(&e.at[..n]));
            return all.filter(|e| !folded(e)).map(|e| (e, None)).collect();
        }
        all.filter(|e| e.target != Target::Group)
            .filter_map(|e| {
                let m = crate::search::fuzzy(query, &e.label())?;
                Some((e, Some(m)))
            })
            .collect()
    }

    /// Whether what a bookmark points at is in the workspace.
    fn bookmark_found(&self, b: &Bookmark) -> bool {
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        match &b.target {
            Target::File { path, .. } => root.join(path).is_file(),
            Target::Folder(path) => root.join(path).is_dir(),
            _ => true,
        }
    }

    fn bookmarks_key(&mut self, k: KeyEvent) {
        let Mode::Bookmarks { query, sel } = &self.mode else {
            return;
        };
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let (mut q, mut s) = (query.clone(), *sel);
        let rows = self.bookmark_rows(&q);
        let last = rows.len().saturating_sub(1);
        s = s.min(last);
        let selected = rows.get(s).map(|(b, _)| b.clone());
        let group = selected.as_ref().filter(|b| b.target == Target::Group);
        match k.code {
            KeyCode::Esc => return self.close_overlay(),
            KeyCode::Down | KeyCode::Tab => s = (s + 1).min(last),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => s = (s + 1).min(last),
            KeyCode::Up | KeyCode::BackTab => s = s.saturating_sub(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => s = s.saturating_sub(1),
            KeyCode::Right if group.is_some() => {
                self.folded.remove(&group.unwrap().at);
            }
            KeyCode::Left if group.is_some_and(|g| !self.folded.contains(&g.at)) => {
                self.folded.insert(group.unwrap().at.clone());
            }
            // Out of a group, onto it.
            KeyCode::Left if q.is_empty() => {
                if let Some(b) = selected.as_ref().filter(|b| b.at.len() > 1) {
                    let parent = &b.at[..b.at.len() - 1];
                    s = rows.iter().position(|(r, _)| r.at == parent).unwrap_or(s);
                }
            }
            KeyCode::Delete => {
                if let Some(b) = selected {
                    self.remove_bookmark(&b);
                }
            }
            KeyCode::Backspace => {
                q.pop();
                s = 0;
            }
            KeyCode::Char('u') if ctrl => {
                q.clear();
                s = 0;
            }
            KeyCode::Enter => {
                let Some(b) = selected else { return };
                if b.target == Target::Group {
                    if !self.folded.remove(&b.at) {
                        self.folded.insert(b.at.clone());
                    }
                } else {
                    return self.open_bookmark(b);
                }
            }
            KeyCode::Char(c) if !ctrl => {
                q.push(c);
                s = 0;
            }
            _ => {}
        }
        self.mode = Mode::Bookmarks { query: q, sel: s };
    }

    /// Go where a bookmark points, like Obsidian: a note (at its heading or block), a folder in
    /// the tree, a search, the graph, a board, a link in the browser.
    fn open_bookmark(&mut self, b: Bookmark) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let from = self.here();
        self.mode = Mode::Normal;
        self.back_to_editor = false;
        if !self.bookmark_found(&b) {
            return self.flash(format!("{} is missing", b.label()));
        }
        match b.target {
            Target::File { path, .. } if self.registry.owner_of(&path).is_some() => {
                self.open_file(&root.join(&path));
            }
            Target::File { path, subpath } => {
                let p = root.join(&path);
                match workspace::attachment_kind(&path) {
                    None => self.open_found(from, &p, subpath.as_deref().and_then(bookmarks::jump)),
                    Some("whiteboard") => self.go_to = Some(Place::Board { path: p }),
                    Some(_) if term::open_url(&p.to_string_lossy()) => {
                        self.flash(format!("opened {path}"))
                    }
                    Some(_) => self.flash(format!("couldn't open {path}")),
                }
            }
            Target::Folder(path) => {
                self.filter.clear();
                self.expanded.insert(path.clone());
                self.select_path(&root.join(&path));
                self.focus = Focus::Notes;
            }
            Target::Search(query) => {
                if let Some(from) = from {
                    self.history.visit(from);
                }
                self.search_for(query);
            }
            Target::Graph => self.go_to = Some(Place::Graph { selected: None }),
            Target::Url(url) => {
                if !term::open_url(&url) {
                    self.flash(format!("couldn't open {url}"));
                }
            }
            Target::Group => {}
            Target::Other(kind) => self.flash(format!("cold-lava can't open {kind} bookmarks yet")),
        }
    }

    /// What `note.bookmark` bookmarks: the tree's selection when it has the focus, else the
    /// note shown, at the heading the reader is in.
    fn bookmark_target(&self) -> Option<Target> {
        let root = self.root.as_ref()?;
        if self.focus == Focus::Notes {
            let it = self.current()?;
            return Some(match it.kind {
                ItemKind::Folder => Target::Folder(it.rel.clone()),
                ItemKind::Note | ItemKind::Board | ItemKind::File => Target::File {
                    path: it.rel.clone(),
                    subpath: None,
                },
            });
        }
        let shown = self.shown.as_ref()?;
        let reading = self.reader.as_ref().filter(|_| self.focus == Focus::Pane);
        let heading = reading.and_then(|r| Some(&r.headings()[*crumbs(r, shown).last()?]));
        Some(Target::File {
            path: (shown.strip_prefix(root).ok()?.to_string_lossy()).replace('\\', "/"),
            subpath: heading.map(|h| format!("#{}", h.text)),
        })
    }

    fn toggle_bookmark(&mut self, target: Target) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let label = target.label();
        let done = bookmarks::edit(&root, |b| b.toggle(target.clone(), bookmarks::now_ms()));
        if let Some(core) = self.core.as_mut() {
            core.reload_bookmarks();
        }
        self.flash(match done {
            Ok(true) => format!(
                "bookmarked {label} · {} bookmarks",
                self.keys.label("bookmarks.open")
            ),
            Ok(false) => format!("removed the bookmark {label}"),
            Err(e) => format!("couldn't change the bookmarks: {e:#}"),
        });
    }

    fn remove_bookmark(&mut self, b: &Bookmark) {
        let Some(root) = self.root.clone() else {
            return;
        };
        // By what it points at, not where: Obsidian may have moved things around since.
        let done = bookmarks::edit(&root, |all| {
            match all.entries().iter().find(|e| {
                e.target == b.target
                    && e.title == b.title
                    && (e.target != Target::Group || e.at == b.at)
            }) {
                Some(e) => {
                    all.remove(&e.at);
                    true
                }
                None => false,
            }
        });
        if let Some(core) = self.core.as_mut() {
            core.reload_bookmarks();
        }
        match done {
            Ok(true) => self.flash(format!("removed the bookmark {}", b.label())),
            Ok(false) => self.flash("that bookmark is already gone"),
            Err(e) => self.flash(format!("couldn't change the bookmarks: {e:#}")),
        }
    }

    /// Bookmark (or unbookmark) the open search's query.
    fn bookmark_search(&mut self) {
        let Mode::Search { query, .. } = &self.mode else {
            return self.flash("open a search to bookmark it");
        };
        let query = query.trim().to_string();
        if query.is_empty() {
            return self.flash("type a search to bookmark it");
        }
        self.toggle_bookmark(Target::Search(query));
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
        if self.keys.get(k) == Some(MenuCommand::Core(MenuAction::BookmarkSearch)) {
            return self.bookmark_search();
        }
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

    /// The folder selected in the tree, when the tree has the focus.
    fn selected_folder(&self) -> Option<PathBuf> {
        match (self.focus, self.current()) {
            (Focus::Notes, Some(it)) if it.kind == ItemKind::Folder => self.current_path(),
            _ => None,
        }
    }

    fn start_rename(&mut self, moving: bool) {
        let target = self.selected_folder().or_else(|| self.rename_target());
        let (Some(path), Some(root)) = (target, self.root.clone()) else {
            return self.flash("select a note or folder to rename or move");
        };
        let rel = path
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
            path,
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
                path,
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
                    let (path, input, moving) = (path.clone(), input.clone(), *moving);
                    self.mode = Mode::Normal;
                    self.plan_rename(&path, &input, moving);
                }
                _ => {}
            },
            Mode::MoveMarked(input) => match k.code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => input.clear(),
                KeyCode::Char(_) if k.modifiers.contains(KeyModifiers::CONTROL) => {}
                KeyCode::Char(c) => input.push(c),
                KeyCode::Enter => {
                    let input = std::mem::take(input);
                    self.mode = Mode::Normal;
                    self.plan_move_marked(&input);
                }
                _ => {}
            },
            Mode::ConfirmRename {
                scroll, waiting, ..
            } => match k.code {
                KeyCode::Esc | KeyCode::Char('n') => {
                    if let (Some(t), Some(core)) = (*waiting, self.core.as_mut()) {
                        core.cancel_relink(t);
                    }
                    self.mode = Mode::Normal;
                    self.flash("rename cancelled");
                }
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::Enter | KeyCode::Char('y') if waiting.is_none() => self.confirm_rename(),
                _ => {}
            },
            _ => {}
        }
    }

    fn plan_rename(&mut self, path: &Path, input: &str, moving: bool) {
        // A folder typed without a trailing slash still means "move into it".
        let input =
            if moving && !input.ends_with('/') && !input.ends_with(".md") && !input.is_empty() {
                format!("{input}/")
            } else if moving && input.is_empty() {
                return self.flash("type a folder, e.g. Archive/ (or / for the workspace root)");
            } else {
                input.to_string()
            };
        self.save_editor();
        self.reload_tree();
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let plan = if path.is_dir() {
            let from = path
                .strip_prefix(&ix.workspace.root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let name = from.rsplit('/').next().unwrap_or(&from).to_string();
            // Moving to the root: "/" means "no folder".
            let target = if input == "/" {
                Ok(name)
            } else {
                crate::rename::folder_target(&from, &input)
            };
            target.and_then(|to| crate::rename::plan_folder(ix, &from, &to))
        } else {
            let Some(i) = ix.by_path(path) else {
                return self.flash("that note is no longer there");
            };
            let from = format!("{}.md", ix.workspace.notes[i].rel);
            let target = if input == "/" {
                Ok(format!("{}.md", ix.workspace.notes[i].name))
            } else {
                crate::rename::target_rel(&from, &input)
            };
            target.and_then(|to| crate::rename::plan(ix, i, &to))
        };
        match plan {
            Ok(plan) => {
                self.mode = Mode::ConfirmRename {
                    plan: Box::new(plan),
                    scroll: 0,
                    marked: false,
                    waiting: None,
                }
            }
            Err(e) => self.flash(format!("can't rename: {e}")),
        }
    }

    /// Plan moving every marked note and folder into the folder typed (`/` is the workspace's root).
    fn plan_move_marked(&mut self, input: &str) {
        let folder = input.trim();
        if folder.is_empty() {
            return self.flash("type a folder, e.g. Archive/ (or / for the workspace root)");
        }
        self.save_editor();
        self.reload_tree();
        let items: Vec<String> = self.marks.iter().cloned().collect();
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        if items.is_empty() {
            return self.flash("the marked notes are no longer there");
        }
        match crate::rename::plan_into(ix, &items, folder) {
            Ok(plan) => {
                self.mode = Mode::ConfirmRename {
                    plan: Box::new(plan),
                    scroll: 0,
                    marked: true,
                    waiting: None,
                }
            }
            Err(e) => self.flash(format!("can't move: {e}")),
        }
    }

    // ---------- marks & tags ----------

    /// Mark the selected row, or unmark it, and go to the next one.
    fn toggle_mark(&mut self) {
        let Some(rel) = self.current().map(|it| it.rel.clone()) else {
            return;
        };
        if !self.marks.remove(&rel) {
            self.marks.insert(rel);
        }
        self.select_row(self.nsel + 1);
    }

    /// The marked rows that aren't inside a marked folder (they go with it).
    fn marked_tops(&self) -> Vec<String> {
        let inside = |r: &str| {
            (self.marks.iter()).any(|f| {
                r.len() > f.len() && r.starts_with(f.as_str()) && r[f.len()..].starts_with('/')
            })
        };
        self.marks.iter().filter(|r| !inside(r)).cloned().collect()
    }

    fn start_tag(&mut self) {
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let all: BTreeSet<String> = (ix.workspace.notes.iter())
            .flat_map(|n| n.tags.iter().cloned())
            .collect();
        self.mode = Mode::Tag {
            typed: String::new(),
            all: all.into_iter().collect(),
            sel: None,
        };
    }

    fn tag_key(&mut self, k: KeyEvent) {
        let Mode::Tag { typed, all, sel } = &mut self.mode else {
            return;
        };
        let shown = tag_shown(typed, all, *sel);
        let matches = tag_matches(typed, all).len();
        match k.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Tab | KeyCode::Down if matches > 0 => {
                *sel = Some(sel.map_or(0, |i| (i + 1) % matches));
            }
            KeyCode::BackTab | KeyCode::Up if matches > 0 => {
                *sel = Some(sel.map_or(matches - 1, |i| (i + matches - 1) % matches));
            }
            KeyCode::Backspace => {
                *typed = shown;
                *sel = None;
                typed.pop();
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                typed.clear();
                *sel = None;
            }
            KeyCode::Char(_) if k.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(c) => {
                *typed = shown;
                *sel = None;
                typed.push(c);
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.apply_tag(&shown);
            }
            _ => {}
        }
    }

    /// Add `input`'s tag to the marked notes' frontmatter (or the selected note's), or remove it
    /// for `-tag`. A note that changed since it was read, or whose tags can't be edited safely,
    /// is left alone and named.
    fn apply_tag(&mut self, input: &str) {
        let input = input.trim();
        let (edit, tag) = match input.strip_prefix('-') {
            Some(t) => (TagEdit::Remove, t.trim().trim_start_matches('#')),
            None => (TagEdit::Add, input.trim_start_matches('#')),
        };
        if !frontmatter::is_tag(tag) {
            return self.flash(format!(
                "\"{tag}\" isn't a tag: letters, digits, _ - and /, not only digits"
            ));
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let (notes, skipped): (Vec<String>, Vec<String>) = match self.marks.is_empty() {
            false => self.marks.iter().cloned().partition(|r| r.ends_with(".md")),
            true => match self.rename_target() {
                Some(p) => (
                    vec![
                        p.strip_prefix(&root)
                            .unwrap_or(&p)
                            .to_string_lossy()
                            .replace('\\', "/"),
                    ],
                    Vec::new(),
                ),
                None => return self.flash("select a note to tag"),
            },
        };
        if notes.is_empty() {
            return self.flash("only notes have tags: mark some notes");
        }
        self.save_editor();
        let style = match self.core.as_ref() {
            Some(core) => core.index.workspace.style(),
            None => crate::style::Style::of_root(&root),
        };
        let (mut changed, mut unchanged, mut refused) = (Vec::new(), 0, Vec::new());
        for rel in &notes {
            let path = root.join(rel);
            let name = rel
                .rsplit('/')
                .next()
                .unwrap_or(rel)
                .trim_end_matches(".md");
            let result = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(
                    |text| match frontmatter::edit_tag(&text, tag, edit, &style)? {
                        Some(new) => crate::write::replace(&path, text.as_bytes(), new.as_bytes())
                            .map(|()| true)
                            .map_err(|e| e.to_string()),
                        None => Ok(false),
                    },
                );
            match result {
                Ok(true) => changed.push(path),
                Ok(false) => unchanged += 1,
                Err(why) => refused.push(format!("{name}: {why}")),
            }
        }
        self.reload_tree();
        if let Some(shown) = self.shown.clone().filter(|s| changed.contains(s)) {
            match self.running_editor() {
                Some(editor) => {
                    let request = EditorRequest::Open {
                        note: shown,
                        jump: None,
                    };
                    self.call(editor, |e, host| e.editor(request, host));
                }
                None => self.reload_reader(),
            }
        }
        let mut msg = match (edit, changed.len()) {
            (TagEdit::Add, n) => format!("tagged {} #{tag}", plural(n, "note")),
            (TagEdit::Remove, n) => format!("removed #{tag} from {}", plural(n, "note")),
        };
        if unchanged > 0 {
            msg.push_str(&match edit {
                TagEdit::Add => format!(" · {unchanged} had it already"),
                TagEdit::Remove => format!(" · {unchanged} didn't have it"),
            });
        }
        if !skipped.is_empty() {
            msg.push_str(&format!(" · {} skipped (not notes)", skipped.len()));
        }
        if !refused.is_empty() {
            msg.push_str(&format!(" · left alone: {}", refused.join("; ")));
        }
        self.flash(msg);
    }

    /// The rename shown was confirmed: ask the owners of the files other owners read in it for
    /// their new texts first (`deliver_relinked` makes it once they have), else make it now.
    fn confirm_rename(&mut self) {
        let Mode::ConfirmRename {
            plan,
            marked,
            waiting,
            ..
        } = &mut self.mode
        else {
            return;
        };
        let Some(core) = self.core.as_mut() else {
            return;
        };
        match core.relink(&plan.edits) {
            Ok(Some(ticket)) => {
                *waiting = Some(ticket);
                self.ask_owners();
            }
            Ok(None) => {
                let (plan, marked) = ((**plan).clone(), *marked);
                self.mode = Mode::Normal;
                self.apply_rename(plan, marked);
            }
            Err(why) => {
                self.mode = Mode::Normal;
                self.flash(format!("rename cancelled, nothing was changed: {why}"));
            }
        }
    }

    /// Make the rename or merge whose owners have given their files' new texts, or say why one
    /// didn't.
    fn deliver_relinked(&mut self) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        for (ticket, answer) in core.take_relinked() {
            let ours = match &self.mode {
                Mode::ConfirmRename { waiting, .. } | Mode::ConfirmMerge { waiting, .. } => {
                    *waiting == Some(ticket)
                }
                Mode::Recipe(view) => {
                    matches!(view.step, recipe_view::Step::Plan { waiting, .. } if waiting == Some(ticket))
                }
                _ => false,
            };
            if !ours {
                continue;
            }
            match (std::mem::replace(&mut self.mode, Mode::Normal), answer) {
                (
                    Mode::ConfirmRename {
                        mut plan, marked, ..
                    },
                    Ok(answers),
                ) => {
                    crate::rename::answered(&mut plan.edits, answers);
                    self.apply_rename(*plan, marked);
                }
                (Mode::ConfirmMerge { mut plan, .. }, Ok(answers)) => {
                    crate::rename::answered(&mut plan.edits, answers);
                    self.apply_merge(*plan);
                }
                (Mode::ConfirmMerge { .. }, Err(why)) => {
                    self.flash(format!("merge cancelled, nothing was changed: {why}"))
                }
                (Mode::Recipe(view), answer) => {
                    let recipe_view::Step::Plan { mut plan, .. } = view.step else {
                        continue;
                    };
                    match answer {
                        Ok(answers) => {
                            crate::rename::answered(&mut plan.edits, answers);
                            self.apply_recipe(*plan);
                        }
                        Err(why) => {
                            self.flash(format!("recipe cancelled, nothing was changed: {why}"))
                        }
                    }
                }
                (_, Err(why)) => {
                    self.flash(format!("rename cancelled, nothing was changed: {why}"))
                }
                (_, Ok(_)) => {}
            }
        }
    }

    fn apply_rename(&mut self, plan: crate::rename::Plan, marked: bool) {
        if let Err(e) = crate::rename::apply(&plan) {
            self.reload_tree();
            return self.flash(format!("rename failed: {e}"));
        }
        if let Some(core) = self.core.as_mut() {
            core.relinked(&plan.edits, |p| plan.moved(p));
        }
        if let Some(editor) = self.running_editor() {
            for (from, to) in plan.file_moves() {
                if from.extension().is_some_and(|e| e == "md") {
                    let request = EditorRequest::Renamed { from, to };
                    self.call(editor, |e, host| e.editor(request, host));
                }
            }
        }
        self.shown = self.shown.as_deref().map(|p| plan.moved(p));
        if let Some((p, _)) = self.pending.as_mut() {
            *p = plan.moved(p);
        }
        for p in &mut self.mru {
            *p = plan.moved(p);
        }
        for m in &plan.moves {
            self.history.moved(&m.from_path, &m.to_path);
            if m.folder {
                let under = |r: &str| r == m.from || r.starts_with(&format!("{}/", m.from));
                let expanded: Vec<String> =
                    self.expanded.iter().filter(|r| under(r)).cloned().collect();
                for r in expanded {
                    self.expanded.remove(&r);
                    self.expanded
                        .insert(format!("{}{}", m.to, &r[m.from.len()..]));
                }
            }
        }
        if marked {
            self.marks.clear();
        } else if let Some(root) = self.root.clone() {
            self.marks = (self.marks.iter())
                .map(|r| {
                    let to = plan.moved(&root.join(r));
                    let to = to.strip_prefix(&root).unwrap_or(&to);
                    to.to_string_lossy().replace('\\', "/")
                })
                .collect();
        }
        self.reload_tree();
        if let Some(first) = plan.moves.first() {
            self.select_path(&first.to_path);
        }
        let links = plan.links();
        let moved_own = plan
            .edits
            .iter()
            .filter(|e| plan.moved(&e.path) != e.path)
            .count();
        let files = plan.files() - moved_own;
        let (verb, to) = match plan.moves.as_slice() {
            [m] => {
                let moved = dir_of_rel(&m.from) != dir_of_rel(&m.to);
                let verb = match (m.folder, moved) {
                    (false, true) => "moved",
                    (false, false) => "renamed",
                    (true, true) => "moved the folder",
                    (true, false) => "renamed the folder",
                };
                (verb.to_string(), m.to.clone())
            }
            ms => (
                format!("moved {}", plural(ms.len(), "item")),
                destination(ms),
            ),
        };
        let mut msg = if links == 0 {
            format!("{verb} to {to} (no links needed updating)")
        } else {
            format!(
                "{verb} to {to} · {} updated in {}",
                plural(links, "link"),
                plural(files.max(1), "file")
            )
        };
        if let Some(b) = &plan.bookmarks {
            msg.push_str(&format!(" · {} followed", plural(b.links, "bookmark")));
        }
        self.flash(msg);
    }

    fn draw_confirm_rename(&self, f: &mut Frame, area: Rect) {
        let Mode::ConfirmRename {
            plan,
            scroll,
            waiting,
            ..
        } = &self.mode
        else {
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
        let (title, hint) = match plan.moves.len() {
            1 => (" Rename ", " ⏎ rename · Esc cancel · ↑↓ scroll "),
            _ => (" Move ", " ⏎ move · Esc cancel · ↑↓ scroll "),
        };
        let waiting_hint;
        let hint = match waiting {
            Some(_) => {
                waiting_hint = waiting_for(&plan.edits);
                waiting_hint.as_str()
            }
            None => hint,
        };
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
        let arrow = |m: &crate::rename::Move| {
            let slash = if m.folder { "/" } else { "" };
            Line::from(vec![
                Span::styled(format!(" {}{slash}", m.from), Style::new().fg(MUTED)),
                Span::styled("  →  ", Style::new().fg(ACCENT)),
                Span::styled(
                    format!("{}{slash}", m.to),
                    Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                ),
            ])
        };
        let notes = plan.moving.iter().filter(|r| r.ends_with(".md")).count();
        let others = plan.moving.len() - notes;
        let folders = plan.moves.iter().any(|m| m.folder);
        let mut lines = Vec::new();
        let mut detail = Vec::new();
        match plan.moves.as_slice() {
            [m] => {
                lines.push(arrow(m));
                if m.folder {
                    lines.push(Line::styled(
                        match others {
                            0 => format!(" {} move with it.", plural(notes, "note")),
                            n => format!(
                                " {} and {} move with it.",
                                plural(notes, "note"),
                                plural(n, "other file")
                            ),
                        },
                        Style::new().fg(NOTE),
                    ));
                }
            }
            ms => {
                let into = match dir_of_rel(&ms[0].to) {
                    "" => "the workspace's root".to_string(),
                    d => format!("{d}/"),
                };
                let mut what = format!(" Move {} into {into}", plural(ms.len(), "item"));
                if folders {
                    what.push_str(&format!(": {} in all", plural(notes, "note")));
                }
                lines.push(Line::styled(
                    what,
                    Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                ));
                detail.extend(ms.iter().map(arrow));
                detail.push(Line::raw(""));
            }
        }
        lines.extend([Line::styled(
            match plan.links() {
                0 if folders || plan.moves.len() > 1 => " No links need changing.".to_string(),
                0 => " No links point to it, so only the file moves.".to_string(),
                n => format!(
                    " {} in {} will be updated:",
                    plural(n, "link"),
                    plural(plan.files(), "file")
                ),
            },
            Style::new().fg(NOTE),
        )]);
        if let Some(b) = &plan.bookmarks {
            lines.push(Line::styled(
                format!(" {} will follow it.", plural(b.links, "bookmark")),
                Style::new().fg(NOTE),
            ));
        }
        for w in &plan.warnings {
            lines.push(Line::styled(format!(" ! {w}"), Style::new().fg(HIT)));
        }
        lines.push(Line::raw(""));
        for e in &plan.edits {
            detail.extend(link_changes(e, inner.width));
        }
        let room = (inner.height as usize).saturating_sub(lines.len());
        let start = (*scroll).min(detail.len().saturating_sub(room));
        lines.extend(detail.into_iter().skip(start).take(room));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    // ---------- duplicate & merge ----------

    /// Copy the selected note or whiteboard to `Name 1` beside it and open the copy.
    fn duplicate(&mut self) {
        let Ok(path) = self.delete_target() else {
            return self.flash("select a note or whiteboard to duplicate");
        };
        if path.is_dir() {
            return self.flash("only notes and whiteboards can be duplicated");
        }
        if self.shown.as_deref() == Some(path.as_path()) {
            self.save_editor();
        }
        let copy = match crate::merge::duplicate(&path) {
            Ok(copy) => copy,
            Err(e) => return self.flash(format!("can't duplicate: {e}")),
        };
        self.reload_tree();
        if copy.extension().is_some_and(|e| e == "md") {
            let from = self.here();
            self.open_found(from, &copy, None);
        } else {
            self.select_path(&copy);
        }
        let name = copy.file_stem().unwrap_or_default().to_string_lossy();
        let rename = self.keys.first_label("note.rename");
        self.flash(format!("duplicated as {name} · {rename} rename"));
    }

    /// Pick the note to merge the selected one into, in the quick switcher.
    fn start_merge(&mut self) {
        let Some(source) = self.rename_target() else {
            return self.flash("select a note to merge into another");
        };
        self.open_switcher();
        if let Mode::Switcher { merging, .. } = &mut self.mode {
            *merging = Some(source);
        }
    }

    fn plan_merge(&mut self, source: &Path, target: &Path) {
        self.save_editor();
        self.reload_tree();
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let (Some(s), Some(t)) = (ix.by_path(source), ix.by_path(target)) else {
            return self.flash("that note is no longer there");
        };
        match crate::merge::plan(ix, s, t) {
            Ok(plan) => {
                self.mode = Mode::ConfirmMerge {
                    plan: Box::new(plan),
                    scroll: 0,
                    waiting: None,
                }
            }
            Err(e) => self.flash(format!("can't merge: {e}")),
        }
    }

    fn merge_key(&mut self, k: KeyEvent) {
        let Mode::ConfirmMerge {
            plan,
            scroll,
            waiting,
        } = &mut self.mode
        else {
            return;
        };
        match k.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                if let (Some(t), Some(core)) = (*waiting, self.core.as_mut()) {
                    core.cancel_relink(t);
                }
                self.mode = Mode::Normal;
                self.flash("merge cancelled");
            }
            KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
            KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('y') if waiting.is_none() => {
                let Some(core) = self.core.as_mut() else {
                    return;
                };
                match core.relink(&plan.edits) {
                    Ok(Some(ticket)) => {
                        *waiting = Some(ticket);
                        self.ask_owners();
                    }
                    Ok(None) => {
                        let plan = (**plan).clone();
                        self.mode = Mode::Normal;
                        self.apply_merge(plan);
                    }
                    Err(why) => {
                        self.mode = Mode::Normal;
                        self.flash(format!("merge cancelled, nothing was changed: {why}"));
                    }
                }
            }
            _ => {}
        }
    }

    fn apply_merge(&mut self, plan: crate::merge::Merge) {
        let done = match crate::merge::apply(&plan, trash::system_bin().as_ref()) {
            Ok(done) => done,
            Err(e) => {
                self.reload_tree();
                return self.flash(format!("merge failed: {e:#}"));
            }
        };
        if let Some(core) = self.core.as_mut() {
            core.relinked(&plan.edits, Path::to_path_buf);
        }
        let (source, target) = (&plan.source_path, &plan.target_path);
        if let Some(editor) = self.running_editor() {
            let request = EditorRequest::Deleted {
                note: source.clone(),
            };
            self.call(editor, |e, host| e.editor(request, host));
        }
        let was_shown = self.shown.as_ref() == Some(source);
        if was_shown {
            self.shown = None;
        }
        if self.pending.as_ref().is_some_and(|(p, _)| p == source) {
            self.pending = None;
        }
        for p in &mut self.mru {
            if p == source {
                *p = target.clone();
            }
        }
        let mut seen = HashSet::new();
        self.mru.retain(|p| seen.insert(p.clone()));
        self.history.moved(source, target);
        self.marks.remove(&plan.source);
        self.reload_tree();
        if self.shown.as_ref() == Some(target) {
            // The editor reloads it itself: wiping the source checks every buffer's file.
            if self.running_editor().is_none() {
                self.reload_reader();
            }
            self.select_path(target);
        } else {
            let from = self.here();
            self.open_found(from, target, None);
        }
        let name = |rel: &str| {
            let rel = rel.trim_end_matches(".md");
            rel.rsplit('/').next().unwrap_or(rel).to_string()
        };
        let (s, t) = (name(&plan.source), name(&plan.target));
        let undo = self.keys.first_label("note.undo-delete");
        let mut msg = format!("merged {s} into {t}");
        if plan.links() > 0 {
            msg.push_str(&format!(
                " · {} now lead there",
                plural(plan.links(), "link")
            ));
        }
        self.flash(format!("{msg} · {undo} undo"));
        self.push_undo(Undo::Merge(Box::new(done)));
    }

    fn undo_merge(&mut self, m: crate::merge::Merged) {
        self.save_editor();
        if let Err(e) = crate::merge::undo(&m) {
            self.flash(format!("{e:#}"));
            return self.push_undo(Undo::Merge(Box::new(m)));
        }
        self.reload_tree();
        if self.shown.as_ref() == Some(&m.target_path) && self.running_editor().is_none() {
            self.reload_reader();
        }
        let from = self.here();
        self.open_found(from, &m.source_path, None);
        let root = self.root.clone().unwrap_or_default();
        let rel = |p: &Path| p.strip_prefix(&root).unwrap_or(p).display().to_string();
        self.flash(format!(
            "merge undone: {} is back and {} is as it was",
            rel(&m.source_path),
            rel(&m.target_path)
        ));
    }

    // ---------- recipes ----------

    fn recipe_key(&mut self, k: KeyEvent) {
        let Mode::Recipe(mut view) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        match view.key(k) {
            recipe_view::Action::Stay => self.mode = Mode::Recipe(view),
            recipe_view::Action::Close(waiting) => {
                if let (Some(t), Some(core)) = (waiting, self.core.as_mut()) {
                    core.cancel_relink(t);
                }
                if matches!(view.step, recipe_view::Step::Plan { .. }) {
                    self.flash("recipe cancelled, nothing was changed");
                }
            }
            recipe_view::Action::Plan(recipe, options) => {
                self.save_editor();
                self.reload_tree();
                let Some(core) = self.core.as_ref() else {
                    return;
                };
                match crate::recipe::plan(&core.index, recipe, options) {
                    Ok(plan) => {
                        view.step = recipe_view::Step::Plan {
                            plan: Box::new(plan),
                            scroll: 0,
                            waiting: None,
                        }
                    }
                    Err(e) => view.refused(format!("{e:#}")),
                }
                self.mode = Mode::Recipe(view);
            }
            recipe_view::Action::Apply => {
                let recipe_view::Step::Plan { plan, waiting, .. } = &mut view.step else {
                    return;
                };
                let Some(core) = self.core.as_mut() else {
                    return;
                };
                match core.relink(&plan.edits) {
                    Ok(Some(ticket)) => {
                        *waiting = Some(ticket);
                        self.mode = Mode::Recipe(view);
                        self.ask_owners();
                    }
                    Ok(None) => {
                        let plan = (**plan).clone();
                        self.apply_recipe(plan);
                    }
                    Err(why) => {
                        self.flash(format!("recipe cancelled, nothing was changed: {why}"));
                    }
                }
            }
        }
    }

    fn apply_recipe(&mut self, plan: crate::recipe::Plan) {
        let done = match crate::recipe::apply(&plan) {
            Ok(done) => done,
            Err(e) => {
                self.reload_tree();
                return self.flash(format!("recipe failed: {e:#}"));
            }
        };
        if let Some(core) = self.core.as_mut() {
            core.relinked(&plan.edits, Path::to_path_buf);
        }
        self.reload_tree();
        if self.running_editor().is_none() {
            self.reload_reader();
        }
        let undo = self.keys.first_label("note.undo-delete");
        self.flash(format!(
            "{}: changed {} · {undo} undo",
            plan.title(),
            plan.what()
        ));
        self.push_undo(Undo::Recipe(Box::new(done)));
    }

    fn undo_recipe(&mut self, r: crate::recipe::Applied) {
        self.save_editor();
        if let Err(e) = crate::recipe::undo(&r) {
            self.flash(format!("{e:#}"));
            return self.push_undo(Undo::Recipe(Box::new(r)));
        }
        self.reload_tree();
        if self.running_editor().is_none() {
            self.reload_reader();
        }
        self.flash(format!(
            "recipe undone: {} · {} as they were",
            r.title,
            plural(r.files, "file")
        ));
    }

    fn draw_confirm_merge(&self, f: &mut Frame, area: Rect) {
        let Mode::ConfirmMerge {
            plan,
            scroll,
            waiting,
        } = &self.mode
        else {
            return;
        };
        let hint = match waiting {
            Some(_) => waiting_for(&plan.edits),
            None => " ⏎ merge · Esc cancel · ↑↓ scroll ".to_string(),
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
                " Merge ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::styled(hint, Style::new().fg(MUTED)));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let name = |rel: &str| {
            let rel = rel.trim_end_matches(".md");
            rel.rsplit('/').next().unwrap_or(rel).to_string()
        };
        let (s, t) = (name(&plan.source), name(&plan.target));
        let text = Style::new().fg(NOTE);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(
                    format!(" {}", plan.source.trim_end_matches(".md")),
                    Style::new().fg(MUTED),
                ),
                Span::styled("  into  ", Style::new().fg(ACCENT)),
                Span::styled(
                    plan.target.trim_end_matches(".md").to_string(),
                    text.add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::styled(
                match plan.appended {
                    0 => format!(" {s} has no text to add; it goes to the trash."),
                    n => format!(
                        " Its text ({}) goes at the end of {t}; {s} goes to the trash.",
                        plural(n, "line")
                    ),
                },
                text,
            ),
        ];
        if plan.own_links > 0 {
            lines.push(Line::styled(
                format!(
                    " {} in its text will be rewritten to work from {t}.",
                    plural(plan.own_links, "link")
                ),
                text,
            ));
        }
        for p in &plan.properties {
            lines.push(match p {
                crate::merge::Property::Added(key) => {
                    Line::styled(format!(" + {key}: copied from {s}"), text)
                }
                crate::merge::Property::Joined { key, items } => {
                    Line::styled(format!(" + {key}: {}", items.join(", ")), text)
                }
                crate::merge::Property::Alias(alias) => Line::styled(
                    format!(" + aliases: {alias}, so searching for it still finds {t}"),
                    text,
                ),
                crate::merge::Property::Kept { key, source } => Line::styled(
                    format!(" ! {key}: {t}'s value stays ({s} had {source})"),
                    Style::new().fg(HIT),
                ),
            });
        }
        lines.push(Line::styled(
            match plan.links() {
                0 => format!(" Nothing else links to {s}."),
                n => format!(
                    " {} in {} will lead to {t}:",
                    plural(n, "link"),
                    plural(
                        plan.edits.len() + (plan.target_edit.links > 0) as usize,
                        "file"
                    )
                ),
            },
            text,
        ));
        if let Some(b) = &plan.bookmarks {
            lines.push(Line::styled(
                format!(" {} will follow it.", plural(b.links, "bookmark")),
                text,
            ));
        }
        for w in &plan.warnings {
            lines.push(Line::styled(format!(" ! {w}"), Style::new().fg(HIT)));
        }
        let undo = self.keys.first_label("note.undo-delete");
        lines.push(Line::styled(
            format!(" {undo} undoes the whole merge."),
            Style::new().fg(MUTED),
        ));
        lines.push(Line::raw(""));
        let mut detail = Vec::new();
        let target = Some(&plan.target_edit).filter(|e| e.links > 0);
        for e in plan.edits.iter().chain(target) {
            detail.extend(link_changes(e, inner.width));
        }
        let room = (inner.height as usize).saturating_sub(lines.len());
        let start = (*scroll).min(detail.len().saturating_sub(room));
        lines.extend(detail.into_iter().skip(start).take(room));
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    // ---------- tidy ----------

    fn open_tidy(&mut self) {
        let Some(core) = self.core.as_ref() else {
            return;
        };
        let view = TidyView::new(crate::tidy::report(&core.index));
        if self.focus == Focus::Pane {
            self.focus = Focus::Notes;
        }
        self.mode = Mode::Tidy(Box::new(view));
    }

    /// Show the Tidy view again after the workspace changed under it: read the workspace now rather
    /// than when the watcher reports it, so what was just deleted or created is gone at once.
    fn show_tidy(&mut self, view: Box<TidyView>) {
        let Some(core) = self.core.as_mut() else {
            return;
        };
        core.rescan_now();
        self.mode = Mode::Tidy(view);
        self.sync_with_core(&[CoreEvent::Rescanned]);
    }

    fn tidy_key(&mut self, k: KeyEvent) {
        let Mode::Tidy(mut view) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        if self.keys.get(k) == Some(MenuCommand::Core(MenuAction::UndoDelete)) {
            self.undo_delete();
            // Asking where to put things back takes over; the view is closed then.
            if matches!(self.mode, Mode::Normal) {
                self.show_tidy(view);
            }
            return;
        }
        let root = self.root.clone().unwrap_or_default();
        let here = self.here();
        match view.key(k) {
            tidy_view::Action::Stay => self.mode = Mode::Tidy(view),
            tidy_view::Action::Close => {}
            tidy_view::Action::OpenNote(rel, line) => {
                self.open_found(here, &root.join(rel), line.map(Jump::Line));
            }
            tidy_view::Action::OpenFile(rel) => {
                self.mode = Mode::Tidy(view);
                match term::open_url(&root.join(&rel).to_string_lossy()) {
                    true => self.flash(format!("opened {rel}")),
                    false => self.flash(format!("couldn't open {rel}")),
                }
            }
            tidy_view::Action::OpenFolder(rel) => {
                self.expanded.insert(rel.clone());
                self.refresh_rows();
                self.select_path(&root.join(rel));
                self.focus = Focus::Notes;
            }
            tidy_view::Action::Delete(rels) => {
                let paths = rels.iter().map(|r| root.join(r)).collect();
                self.ask_delete(paths, false, Some(view));
            }
            tidy_view::Action::Create { target, from } => {
                let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
                    return;
                };
                let path = crate::resolver::new_note_path(ix, &target, ix.by_rel(&from));
                let created = crate::write::create_if_missing(&path, b"");
                self.reload_tree();
                self.show_tidy(view);
                let rel = path.strip_prefix(&root).unwrap_or(&path).display();
                match created {
                    Ok(_) => self.flash(format!("created {rel}")),
                    Err(e) => self.flash(format!("couldn't create {rel}: {e}")),
                }
            }
        }
    }

    fn draw_tidy(&self, f: &mut Frame, area: Rect, view: &TidyView) {
        let undo = self.keys.first_label("note.undo-delete");
        let hint = format!(
            " ⏎ open · space mark · d delete · {undo} undo · n create note · a all no-backlinks · Esc close "
        );
        let title = match view.marks() {
            0 => " Tidy ".to_string(),
            n => format!(" Tidy · {n} marked "),
        };
        let inner = picker_frame(f, area, &title, &hint);
        let lines = view.lines(inner.width as usize, inner.height as usize);
        f.render_widget(Paragraph::new(lines), inner);
    }

    // ---------- extensions' views ----------

    /// A core command that a built-in extension carries out, handed to it as `command/run`.
    fn hand_to(&mut self, name: &str, command: &str) {
        if let Some(ext) = self.registry.by_name(name) {
            let request = CommandRequest::Run {
                id: command.into(),
                args: Value::Null,
            };
            self.call(ext, |e, host| e.command(request, host));
        }
    }

    /// A key in an extension's view: the view's own, else Back and Forward, else nothing.
    /// What the user did goes to the extension as `view/event`, and Esc or `q` close it.
    fn view_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let Mode::View {
            owner,
            view,
            backlinks: backs,
        } = self.mode
        else {
            return Ok(());
        };
        let Some(open) = self.views.get_mut(owner, view) else {
            self.mode = Mode::Normal;
            return Ok(());
        };
        let of_file = open.file.is_some();
        let pane = of_file && self.show_backlinks;
        let menu = match backs || !open.takes(&k) {
            true => self.keys.get(k),
            false => None,
        };
        let done = match menu {
            Some(MenuCommand::Core(MenuAction::History(h))) => {
                let here = self.here();
                let to = match h {
                    HistoryAction::Back => self.history.back(here),
                    HistoryAction::Forward => self.history.forward(here),
                };
                if let Some(place) = to {
                    self.mode = Mode::Normal;
                    self.go(place, terminal)?;
                }
                true
            }
            // Between the view and the backlinks of the file it shows.
            Some(MenuCommand::Core(MenuAction::FocusNext | MenuAction::FocusPrevious))
                if backs || (pane && !self.backs.is_empty()) =>
            {
                self.view_backlinks(!backs);
                true
            }
            Some(MenuCommand::Core(MenuAction::ToggleBacklinks)) if of_file => {
                self.show_backlinks = !self.show_backlinks;
                self.view_backlinks(false);
                true
            }
            // A note an extension shows, in the plain reader.
            Some(MenuCommand::Core(MenuAction::Read)) if self.viewed_note().is_some() => {
                self.run_action(MenuAction::Read, terminal)?;
                true
            }
            _ if backs => {
                match k.code {
                    KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => self.view_backlinks(false),
                    _ => self.backlinks_key(k),
                }
                true
            }
            _ => false,
        };
        if done {
            return Ok(());
        }
        let Some(open) = self.views.get_mut(owner, view) else {
            return Ok(());
        };
        match open.key(k) {
            views::Action::Stay => {}
            views::Action::Say(s) => self.flash(s),
            views::Action::Close => {
                self.views.close(owner, view);
                self.forget_closed_views();
                let closed = ViewNews::Closed { view };
                self.call(owner, |e, host| e.view(closed, host));
            }
            views::Action::Send(event) => {
                let news = ViewNews::Event(Happened { view, event });
                self.call(owner, |e, host| e.view(news, host));
            }
        }
        Ok(())
    }

    /// Keys go to the backlinks pane under the view in front (`true`), or to the view.
    fn view_backlinks(&mut self, on: bool) {
        if let Mode::View { backlinks, .. } = &mut self.mode {
            *backlinks = on;
        }
    }

    /// Whether keys go to the backlinks pane: the view's, or the reader's.
    fn in_backlinks(&self) -> bool {
        match self.mode {
            Mode::View { backlinks, .. } => backlinks,
            _ => self.focus == Focus::Backlinks,
        }
    }

    // ---------- delete & undo ----------

    /// The note, board or folder to delete: the tree's selection, else the note in the pane.
    fn delete_target(&self) -> Result<PathBuf, &'static str> {
        match (self.focus, self.current()) {
            (Focus::Notes, Some(_)) => self.current_path().ok_or("select a note to delete"),
            _ => self.rename_target().ok_or("select a note to delete"),
        }
    }

    /// Ask before deleting the selected item, or the marked ones, showing the notes whose links
    /// will break. `changed`: asking again because one changed on disk after the first time.
    fn start_delete(&mut self, changed: bool) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let tidy = match &mut self.mode {
            Mode::ConfirmDelete(p) => p.tidy.take(),
            _ => None,
        };
        let paths: Vec<PathBuf> = match (&self.mode, self.delete_target()) {
            (Mode::ConfirmDelete(p), _) => p.doomed.iter().map(|d| d.path.clone()).collect(),
            _ if !self.marks.is_empty() => {
                (self.marked_tops().iter()).map(|r| root.join(r)).collect()
            }
            (_, Ok(p)) => vec![p],
            (_, Err(why)) => return self.flash(why),
        };
        self.ask_delete(paths, changed, tidy);
    }

    /// Ask before `paths` go to the trash. `tidy`: asked from the Tidy view, shown again after.
    fn ask_delete(&mut self, paths: Vec<PathBuf>, changed: bool, tidy: Option<Box<TidyView>>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.mode = Mode::Normal;
        let doomed_path = |p: &Path| paths.iter().any(|d| p.starts_with(d));
        if self.shown.as_deref().is_some_and(doomed_path) {
            self.save_editor();
        }
        let mut doomed = Vec::new();
        for path in &paths {
            let rel = path
                .strip_prefix(&root)
                .map(|r| r.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let Ok(contents) = trash::Contents::read(path) else {
                self.reload_tree();
                if let Some(view) = tidy {
                    self.show_tidy(view);
                }
                return self.flash(format!("{rel} is no longer there"));
            };
            let rel = rel.trim_end_matches(".md").to_string();
            doomed.push(Doomed {
                path: path.clone(),
                rel,
                contents,
            });
        }
        // The notes that go, and the links to them from notes that stay.
        let mut backlinks = Vec::new();
        let mut notes = 0;
        if let Some(ix) = self.core.as_ref().map(|c| &c.index) {
            let inside = |i: usize| doomed_path(&ix.workspace.notes[i].path);
            for i in (0..ix.workspace.notes.len()).filter(|&i| inside(i)) {
                notes += 1;
                let stays = |b: &&Backlink| !doomed_path(&ix.path_of(b.from));
                backlinks.extend(ix.backlinks(i).iter().filter(stays).map(|b| BackRow {
                    name: ix.name(b.from).to_string(),
                    path: ix.path_of(b.from),
                    line: b.line,
                    place: b.place.clone(),
                    context: b.context.clone(),
                }));
            }
        }
        // A line linking to two notes that go is one row.
        backlinks.sort_by_cached_key(|b| (b.name.to_lowercase(), b.path.clone(), b.line));
        backlinks.dedup_by(|a, b| a.path == b.path && a.line == b.line);
        let notes = (doomed.len() > 1 || paths[0].is_dir()).then_some(notes);
        let goes = match TrashOption::of_workspace(&root) {
            TrashOption::System if trash::system_bin().is_some() => "the system trash.",
            TrashOption::System | TrashOption::Local => "the workspace's .trash folder.",
            TrashOption::Permanent => {
                ".trash: this workspace deletes permanently in Obsidian, cold-lava never does."
            }
        };
        self.mode = Mode::ConfirmDelete(Box::new(PendingDelete {
            doomed,
            notes,
            backlinks,
            goes: goes.to_string(),
            changed,
            scroll: 0,
            tidy,
        }));
    }

    fn delete_key(&mut self, k: KeyEvent) {
        let yes = matches!(k.code, KeyCode::Enter | KeyCode::Char('y'));
        let no = matches!(k.code, KeyCode::Esc | KeyCode::Char('n'));
        match &mut self.mode {
            Mode::ConfirmDelete(p) => match k.code {
                _ if no => {
                    let tidy = p.tidy.take();
                    self.mode = Mode::Normal;
                    if let Some(view) = tidy {
                        self.show_tidy(view);
                    }
                    self.flash("delete cancelled");
                }
                KeyCode::Down | KeyCode::Char('j') => p.scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => p.scroll = p.scroll.saturating_sub(1),
                _ if yes => self.apply_delete(),
                _ => {}
            },
            Mode::ConfirmRestore(_) if yes || no => {
                let Mode::ConfirmRestore(batch) = std::mem::replace(&mut self.mode, Mode::Normal)
                else {
                    return;
                };
                if yes {
                    self.restore(batch);
                } else {
                    let batch = batch.into_iter().map(|(t, _)| t).collect();
                    self.undo.push(Undo::Delete(batch));
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
        // Everything is checked before anything goes, so a change means nothing went.
        if (p.doomed.iter())
            .any(|d| trash::Contents::read(&d.path).ok() != Some(d.contents.clone()))
        {
            return self.start_delete(true);
        }
        let system = trash::system_bin();
        let mut batch = Vec::new();
        let mut failed = None;
        for d in &p.doomed {
            match trash::delete(&root, &d.path, &d.contents, system.as_ref()) {
                Ok(t) => batch.push(t),
                Err(e) => {
                    failed = Some(format!("couldn't delete {}: {e}", d.rel));
                    break;
                }
            }
        }
        let Mode::ConfirmDelete(p) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        let gone: Vec<&Doomed> = p.doomed.iter().take(batch.len()).collect();
        let went = |path: &Path| gone.iter().any(|d| path.starts_with(&d.path));
        let was_shown = self.shown.as_deref().is_some_and(went);
        if let Some(editor) = self.running_editor() {
            let notes: Vec<PathBuf> = match self.core.as_ref() {
                Some(core) => (core.index.workspace.notes.iter())
                    .filter(|n| went(&n.path))
                    .map(|n| n.path.clone())
                    .collect(),
                None => self
                    .shown
                    .clone()
                    .filter(|_| was_shown)
                    .into_iter()
                    .collect(),
            };
            for note in notes {
                let request = EditorRequest::Deleted { note };
                self.call(editor, |e, host| e.editor(request, host));
            }
        }
        if was_shown {
            self.shown = None;
            self.pending = None;
        }
        self.mru.retain(|m| !went(m));
        for d in &gone {
            self.history.forget(&d.path);
        }
        if gone.len() == p.doomed.len() && p.tidy.is_none() {
            self.marks.clear();
        }
        self.reload_tree();
        if was_shown {
            match (self.current().map(|it| it.kind), self.current_path()) {
                (Some(ItemKind::Note), Some(next)) => self.show(&next),
                _ if self.reader.is_some() => self.reload_reader(),
                _ => {}
            }
        }
        let undo = self.keys.first_label("note.undo-delete");
        let name = match gone.as_slice() {
            [d] => d.rel.rsplit('/').next().unwrap_or(&d.rel).to_string(),
            ds => plural(ds.len(), "item"),
        };
        let local = batch.iter().any(|t| t.went == Went::WorkspaceTrash);
        let msg = match (local, TrashOption::of_workspace(&root)) {
            (true, TrashOption::Permanent) => format!(
                "moved {name} to .trash: this workspace deletes permanently in Obsidian, cold-lava never does · {undo} undo"
            ),
            (true, TrashOption::Local) => format!("deleted {name} (to .trash) · {undo} undo"),
            (true, TrashOption::System) => format!(
                "deleted {name} (to .trash: the system trash couldn't take it) · {undo} undo"
            ),
            (false, _) => format!("deleted {name} · {undo} undo"),
        };
        self.flash(match failed {
            Some(why) if batch.is_empty() => why,
            Some(why) => format!("{msg} · {why}"),
            None => msg,
        });
        if !batch.is_empty() {
            self.push_undo(Undo::Delete(batch));
        }
        if let Some(view) = p.tidy {
            self.show_tidy(view);
        }
    }

    fn push_undo(&mut self, u: Undo) {
        self.undo.push(u);
        if self.undo.len() > UNDO_DELETES {
            self.undo.remove(0);
        }
    }

    /// Undo the last merge, or put the last delete back (asking about free names for items whose
    /// paths are taken now).
    fn undo_delete(&mut self) {
        let batch = match self.undo.pop() {
            None => return self.flash("nothing to undo"),
            Some(Undo::Merge(m)) => return self.undo_merge(*m),
            Some(Undo::Recipe(r)) => return self.undo_recipe(*r),
            Some(Undo::Delete(batch)) => batch,
        };
        let taken = batch.iter().any(|t| t.from.exists());
        let batch: Vec<(Trashed, PathBuf)> = (batch.into_iter())
            .map(|t| {
                let to = trash::free_path(&t.from);
                (t, to)
            })
            .collect();
        if taken {
            self.mode = Mode::ConfirmRestore(batch);
            return;
        }
        self.restore(batch);
    }

    fn restore(&mut self, batch: Vec<(Trashed, PathBuf)>) {
        let mut back = Vec::new();
        let mut failed = Vec::new();
        for (t, to) in batch {
            match trash::restore(&t, &to) {
                Ok(()) => back.push(to),
                Err(e) => {
                    failed.push(format!("couldn't put {} back: {e}", t.from.display()));
                    self.undo.push(Undo::Delete(vec![t]));
                }
            }
        }
        self.reload_tree();
        let root = self.root.clone().unwrap_or_default();
        for to in back.iter().filter(|to| to.is_dir()) {
            if let Ok(rel) = to.strip_prefix(&root) {
                self.expanded
                    .insert(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        if let Some(first) = back.first() {
            self.select_path(first);
        }
        if let [to] = back.as_slice()
            && to.extension().is_some_and(|e| e == "md")
        {
            let to = to.clone();
            self.pending = None;
            self.show(&to);
        }
        let msg = match back.as_slice() {
            [] => String::new(),
            [to] => format!(
                "restored {}",
                to.strip_prefix(&root).unwrap_or(to).display()
            ),
            all => format!("restored {}", plural(all.len(), "item")),
        };
        let parts: Vec<String> = [msg]
            .into_iter()
            .chain(failed)
            .filter(|m| !m.is_empty())
            .collect();
        self.flash(parts.join(" · "));
    }

    fn draw_confirm_delete(&self, f: &mut Frame, area: Rect) {
        let (title, hint, lines, detail, scroll) = match &self.mode {
            Mode::ConfirmDelete(p) => {
                let several = p.doomed.len() > 1;
                let mut lines = vec![match p.doomed.as_slice() {
                    [d] => {
                        let slash = if p.notes.is_some() { "/" } else { "" };
                        Line::from(vec![
                            Span::styled(" Delete ", Style::new().fg(NOTE)),
                            Span::styled(
                                format!("{}{slash}", d.rel),
                                Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled("?", Style::new().fg(NOTE)),
                        ])
                    }
                    ds => Line::styled(
                        format!(" Delete {} marked items?", ds.len()),
                        Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                    ),
                }];
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
                let note = p.doomed[0].path.extension().is_some_and(|e| e == "md");
                match p.notes {
                    Some(n) if several => lines.push(Line::styled(
                        format!(" {} in all.", plural(n, "note")),
                        Style::new().fg(NOTE),
                    )),
                    Some(n) => lines.push(Line::styled(
                        format!(
                            " It holds {}, and everything else in it.",
                            plural(n, "note")
                        ),
                        Style::new().fg(NOTE),
                    )),
                    None => {}
                }
                let them = if several { "them" } else { "it" };
                lines.push(Line::styled(
                    match names.len() {
                        0 if p.notes.is_some() => format!(" Nothing outside {them} links to them."),
                        n if p.notes.is_some() => format!(
                            " {} from outside: {}. Their links will become unresolved.",
                            if n == 1 {
                                "1 note links to them".to_string()
                            } else {
                                format!("{n} notes link to them")
                            },
                            names.join(", ")
                        ),
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
                let who = match p.doomed.as_slice() {
                    [d] => format!("{} goes", d.rel.rsplit('/').next().unwrap_or(&d.rel)),
                    _ => "They go".to_string(),
                };
                lines.push(Line::styled(
                    format!(" {who} to {}", p.goes),
                    Style::new().fg(MUTED),
                ));
                lines.push(Line::raw(""));
                let inner_w = popup_width(area).saturating_sub(2) as usize;
                let mut detail: Vec<Line> = Vec::new();
                if several {
                    detail.extend(p.doomed.iter().map(|d| {
                        let slash = if d.path.is_dir() { "/" } else { "" };
                        Line::styled(format!("   ✕ {}{slash}", d.rel), Style::new().fg(NOTE))
                    }));
                    detail.push(Line::raw(""));
                }
                detail.extend(p.backlinks.iter().map(|b| {
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
                }));
                (
                    " Delete ",
                    " y delete · n cancel · ↑↓ scroll ",
                    lines,
                    detail,
                    p.scroll,
                )
            }
            Mode::ConfirmRestore(batch) => {
                let file = |p: &Path| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                let taken: Vec<&(Trashed, PathBuf)> =
                    batch.iter().filter(|(t, _)| t.from.exists()).collect();
                let lines = match taken.as_slice() {
                    [(t, to)] if batch.len() == 1 => vec![
                        Line::styled(
                            format!(" {} exists now.", file(&t.from)),
                            Style::new().fg(NOTE),
                        ),
                        Line::styled(
                            format!(" Put the deleted one back as {}?", file(to)),
                            Style::new().fg(NOTE),
                        ),
                    ],
                    _ => vec![
                        Line::styled(
                            format!(
                                " {} of the {} exist now.",
                                taken.len(),
                                plural(batch.len(), "deleted item")
                            ),
                            Style::new().fg(NOTE),
                        ),
                        Line::styled(
                            format!(
                                " Put them back as {}?",
                                (taken.iter().map(|(_, to)| file(to)))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            Style::new().fg(NOTE),
                        ),
                    ],
                };
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

    /// Open a note picked in the switcher or search, and give the editor focus. A note an
    /// extension shows goes to it, unless `jump` points into its text.
    fn open_found(&mut self, from: Option<Place>, path: &Path, jump: Option<Jump>) {
        if jump.is_none() && self.shower(path).is_some() {
            self.pending = None;
            return self.open_file(path);
        }
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
        if let Some(place) = b.place.clone() {
            let path = b.path.clone();
            return self.open_file_at(&path, Some(place));
        }
        let (path, line) = (b.path.clone(), b.line);
        let from = self.here();
        self.leave(from, &path, Some(&Jump::Line(line)));
        self.mode = Mode::Normal;
        self.pending = None;
        self.show_at(&path, Some(Jump::Line(line)));
        self.select_path(&path);
        self.focus_pane();
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if let (Mode::Graph, Some(g)) = (&self.mode, self.graph.as_mut()) {
            return g.view.mouse(m);
        }
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
                    match self.mode {
                        Mode::View { .. } => self.view_backlinks(true),
                        _ => self.focus = Focus::Backlinks,
                    }
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
                    self.focus = Focus::Workspaces;
                    let i = (m.row - va.y - 1) as usize;
                    if i < self.workspaces.len() && i != self.vsel {
                        self.open_workspace(i);
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

    /// The graph in front, with `selected` selected (the node's id), made the first time.
    fn show_graph(&mut self, selected: Option<String>) -> Result<()> {
        if (self.core.as_ref()).is_none_or(|c| c.index.workspace.notes.is_empty()) {
            self.flash("this workspace has no notes yet");
            return Ok(());
        }
        self.hide_picture()?;
        let Some(core) = self.core.as_ref() else {
            return Ok(());
        };
        let render = self.render;
        let g = self
            .graph
            .get_or_insert_with(|| GraphScreen::new(core, render));
        g.view.select_id(selected.as_deref());
        self.mode = Mode::Graph;
        Ok(())
    }

    /// A key in the graph: the view's own, else what its command means here.
    fn graph_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(g) = self.graph.as_mut() else {
            self.mode = Mode::Normal;
            return Ok(());
        };
        match g.view.key(k) {
            scene_view::Action::Stay => {}
            scene_view::Action::Leave if self.graph_alone => self.quit = true,
            scene_view::Action::Leave => {
                if let Some(here) = self.here() {
                    self.history.visit(here);
                }
                self.mode = Mode::Normal;
            }
            scene_view::Action::History(h) => self.travel(h, terminal)?,
            scene_view::Action::Command(a) => {
                let Some(core) = self.core.as_mut() else {
                    return Ok(());
                };
                match g.command(a, core)? {
                    Then::Nothing => {}
                    Then::Edit(path) => {
                        term::write_raw(&g.view.hide());
                        match term::edit_file(terminal, &path, core.root())? {
                            None => core.note_changed(&path), // its links may have changed
                            Some(err) => g.view.flash(err),
                        }
                    }
                    Then::Boards => {
                        term::write_raw(&g.view.hide());
                        let root = core.root().to_path_buf();
                        match board::pick_board(terminal, &root)? {
                            Some(path) => self.go_from_here(Place::Board { path }, terminal)?,
                            None => terminal.clear()?,
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `cold-lava graph`: the graph in front, and leaving it quits.
    pub fn open_graph_alone(&mut self) -> Result<()> {
        self.graph_alone = true;
        self.show_graph(None)
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
        self.hide_picture()?;
        let exit = board::open(terminal, &root, path.clone(), Some(Nav::of(&self.history)))?;
        terminal.clear()?;
        self.reload_tree();
        Ok(self.left(Place::Board { path }, exit))
    }

    fn edit_fullscreen(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.hide_picture()?;
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
        let (Some(it), Some(v)) = (self.current().cloned(), self.workspaces.get(self.vsel)) else {
            return;
        };
        let ok = term::open_in_obsidian(&v.name, &it.rel);
        self.flash(if ok {
            "opened in Obsidian"
        } else {
            "couldn't launch Obsidian"
        });
    }

    /// The workspace's templates, from the folders its Templates settings name.
    fn templates(&self) -> Vec<(String, String)> {
        let (Some(root), Some(core)) = (&self.root, &self.core) else {
            return Vec::new();
        };
        let settings = templates::Settings::load(root);
        let mut found: Vec<(String, String)> = core
            .index
            .workspace
            .notes
            .iter()
            .filter(|n| settings.is_template(&n.rel))
            .map(|n| (n.rel.clone(), settings.name(&n.rel).to_string()))
            .collect();
        found.sort_by_key(|(_, name)| workspace::search_key(name));
        found
    }

    /// Create the note `name` in the selected folder, from `template` (workspace-relative, without
    /// `.md`) if one is given, and open it.
    fn create_note(&mut self, name: &str, template: Option<&str>) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        let folder = self.selected_dir();
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

    /// Where new notes and folders go: the selected folder, or the selected note's.
    fn selected_dir(&self) -> String {
        match self.current() {
            Some(it) if it.kind == ItemKind::Folder => it.rel.clone(),
            Some(it) => dir_of_rel(&it.rel).to_string(),
            None => String::new(),
        }
    }

    fn create_folder(&mut self, name: &str) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let parent = self.selected_dir();
        let name = name.trim_matches('/');
        let rel = if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}/{name}")
        };
        let made = crate::rename::check_path(&rel)
            .and_then(|()| Ok(crate::write::create_dir(&root.join(&rel))?));
        if let Err(e) = made {
            return self.flash(format!("can't create {rel}/: {e}"));
        }
        self.flash(format!("created {rel}/"));
        self.filter.clear();
        self.reload_tree();
        self.select_path(&root.join(&rel));
    }

    fn add_workspace(&mut self, text: &str) {
        let p = match text.strip_prefix("~/") {
            Some(rest) => home_dir()
                .map(|h| h.join(rest))
                .unwrap_or_else(|| PathBuf::from(text)),
            None => PathBuf::from(text),
        };
        match p.canonicalize() {
            Ok(c) if c.is_dir() => {
                if !looks_like_workspace(&c) {
                    self.flash("added (no notes in that folder yet)");
                }
                if let Some(dir) = recent_dir() {
                    remember(&dir, &c);
                }
                self.workspaces = discover(&[]);
                if let Some(i) = self.workspaces.iter().position(|v| v.path == c) {
                    self.open_workspace(i);
                    self.focus = Focus::Notes;
                }
            }
            _ => self.flash(format!("not a folder: {text}")),
        }
    }

    // ---------- drawing ----------

    /// `area` with the backlinks pane under it, when it has one (`has`) and it isn't hidden.
    fn backlinks_split(&self, area: Rect, has: bool) -> (Rect, Rect) {
        if !(self.show_backlinks && has && area.height > 12) {
            return (area, Rect::default());
        }
        let want = if self.backs.is_empty() {
            3
        } else {
            self.backs.len() as u16 + 2
        };
        let h = want.min(area.height / 3).max(3);
        let [e, b] = Layout::vertical([Constraint::Min(5), Constraint::Length(h)]).areas(area);
        (e, b)
    }

    fn block(&self, title: Line<'static>, focus: Focus) -> Block<'static> {
        self.frame(title, self.focus == focus)
    }

    fn frame(&self, title: Line<'static>, active: bool) -> Block<'static> {
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
        if let (Mode::Graph, Some(g)) = (&self.mode, self.graph.as_mut()) {
            let statuses: Vec<String> = self.statuses.iter().map(|(_, t)| t.clone()).collect();
            return g.view.draw(f, f.area(), &statuses);
        }
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
        let (ea, ba) = self.backlinks_split(ea, self.shown.is_some());
        let editor_inner = inner(ea);
        self.areas = [va, na, editor_inner, ba];

        if !self.zoom {
            self.draw_workspaces(f, va);
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
        if matches!(self.mode, Mode::Bookmarks { .. }) {
            self.draw_bookmarks(f, main);
        }
        if let Mode::Tidy(view) = &self.mode {
            self.draw_tidy(f, main, view);
        }
        if let Mode::Recipe(view) = &self.mode {
            let undo = self.keys.first_label("note.undo-delete");
            let inner = picker_frame(f, main, &view.title(), &view.hint(&undo));
            let lines = view.lines(inner.width as usize, inner.height as usize);
            f.render_widget(Paragraph::new(lines), inner);
        }
        if let Mode::View { owner, view, .. } = self.mode {
            let of_file = self
                .views
                .get(owner, view)
                .is_some_and(|v| v.file.is_some());
            let (va, ba) = self.backlinks_split(main, of_file);
            self.areas[3] = ba;
            if let Some(open) = self.views.get_mut(owner, view) {
                let body = draw_view(f, va, open);
                let png = open.picture().and_then(|p| p.png()).is_some();
                if png && self.views.pixels {
                    self.picture_wanted = Some((owner, view, crate::image::pixels_area(body)));
                }
            }
            if ba.height > 0 {
                f.render_widget(Clear, ba);
                self.draw_backlinks(f, ba);
            }
        }
        if matches!(self.mode, Mode::Search { .. }) {
            self.draw_search(f, main);
        }
        if matches!(self.mode, Mode::ConfirmRename { .. }) {
            self.draw_confirm_rename(f, main);
        }
        if matches!(self.mode, Mode::ConfirmDelete(_) | Mode::ConfirmRestore(_)) {
            self.draw_confirm_delete(f, main);
        }
        if matches!(self.mode, Mode::ConfirmMerge { .. }) {
            self.draw_confirm_merge(f, main);
        }
        if let Mode::Rename { input, moving, .. } = &self.mode {
            let (title, label) = if *moving {
                (" move to folder · ⏎ preview · Esc cancel ", "folder: ")
            } else {
                (" rename · ⏎ preview · Esc cancel ", "new name: ")
            };
            prompt(f, main, title, label, input);
        }
        if let Mode::MoveMarked(input) = &self.mode {
            let title = format!(
                " move {} marked into folder · ⏎ preview · Esc cancel ",
                self.marks.len()
            );
            prompt(f, main, &title, "folder: ", input);
        }
        if let Mode::Tag { typed, all, sel } = &self.mode {
            draw_tag(f, main, typed, all, *sel, self.marks.len());
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
            Mode::NewFolder(name) => {
                prompt(
                    f,
                    main,
                    " new folder · ⏎ create · Esc cancel ",
                    "folder name: ",
                    name,
                );
            }
            Mode::AddWorkspace(name) => {
                prompt(
                    f,
                    main,
                    " add a workspace · ⏎ add · Esc cancel ",
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
                reader.layout(room, &|raw| resolves(ix, Some(path), raw), &|_, _| None);
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
                            false => format!("{name} isn't in the workspace"),
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
        let Mode::Switcher {
            query,
            sel,
            merging,
        } = &self.mode
        else {
            return;
        };
        let Some(ix) = self.core.as_ref().map(|c| &c.index) else {
            return;
        };
        let inner = match merging.as_deref().and_then(|p| ix.by_path(p)) {
            Some(i) => picker_frame(
                f,
                area,
                &format!(" Merge {} into… ", ix.workspace.notes[i].name),
                " ↑↓ choose · ⏎ preview the merge · Esc cancel ",
            ),
            None => picker_frame(f, area, " Go to note ", " ↑↓ choose · ⏎ open · Esc close "),
        };
        let hits = self.switcher_targets(query, merging.as_deref());
        let files = match merging {
            None => self.switcher_files(query),
            Some(_) => Vec::new(),
        };
        let create = merging.is_none() && self.offers_create(query);
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(
                    if query.is_empty() {
                        "  type a name (letters in order are enough)".to_string()
                    } else {
                        format!("  {} notes", hits.len() + files.len())
                    },
                    Style::new().fg(MUTED),
                ),
            ]),
            Line::raw(""),
        ];
        let rows = inner.height.saturating_sub(2) as usize;
        let total = hits.len() + files.len() + create as usize;
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
            if i == hits.len() + files.len() {
                lines.push(Line::styled(
                    format!(" + Create note \"{}\"", query.trim()),
                    if selected { base } else { Style::new().fg(HIT) },
                ));
                continue;
            }
            let (label, positions, extra, right) = match hits.get(i) {
                Some(hit) => {
                    let note = &ix.workspace.notes[hit.note];
                    let label = hit.alias.clone().unwrap_or_else(|| note.name.clone());
                    let extra = match hit.alias {
                        Some(_) => format!("  → {}", note.name),
                        None => String::new(),
                    };
                    let folder = note.rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                    let (inc, _) = ix.counts(hit.note);
                    let right = format!("{folder}  ←{inc} ");
                    (label, &hit.positions, extra, right)
                }
                None => {
                    let (item, fuzzy) = &files[i - hits.len()];
                    let it = &self.items[*item];
                    let folder = it.rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                    let kind = self
                        .registry
                        .owner_of(&it.rel)
                        .map(|(_, t)| t.title.as_str());
                    let right = format!("{folder}  {} ", kind.unwrap_or_default());
                    (it.name.clone(), &fuzzy.positions, String::new(), right)
                }
            };
            let mut spans = vec![Span::styled(" ", base)];
            for (ci, ch) in label.chars().enumerate() {
                spans.push(Span::styled(
                    ch.to_string(),
                    if positions.contains(&ci) { hl } else { base },
                ));
            }
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

    fn draw_bookmarks(&self, f: &mut Frame, area: Rect) {
        let Mode::Bookmarks { query, sel } = &self.mode else {
            return;
        };
        let hint = " ↑↓ choose · ⏎ open · ←→ fold · Del remove · Esc close ";
        let inner = picker_frame(f, area, " Bookmarks ", hint);
        let rows = self.bookmark_rows(query);
        let count = match query.is_empty() {
            true => (self.core.as_ref().and_then(|c| c.bookmarks().ok())).map_or(0, |b| {
                b.entries()
                    .iter()
                    .filter(|e| e.target != Target::Group)
                    .count()
            }),
            false => rows.len(),
        };
        let summary = match (query.is_empty(), count) {
            (true, 0) => format!(
                "  no bookmarks yet · {} bookmarks a note",
                self.keys.label("note.bookmark")
            ),
            (true, _) => format!("  {} · type to filter", plural(count, "bookmark")),
            (false, _) => format!("  {count} match"),
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
                Span::styled(summary, Style::new().fg(MUTED)),
            ]),
            Line::raw(""),
        ];
        let height = inner.height.saturating_sub(2) as usize;
        let sel = (*sel).min(rows.len().saturating_sub(1));
        let start = sel.saturating_sub(height.saturating_sub(1));
        let width = inner.width as usize;
        for (row, (b, m)) in rows.iter().enumerate().skip(start).take(height) {
            let selected = row == sel;
            let found = self.bookmark_found(b);
            let icon = match &b.target {
                Target::Group if self.folded.contains(&b.at) => "▸ ",
                Target::Group => "▾ ",
                Target::File {
                    subpath: Some(_), ..
                } => "§ ",
                Target::File { .. } => "• ",
                Target::Folder(_) => "▤ ",
                Target::Search(_) => "⌕ ",
                Target::Graph => "◎ ",
                Target::Url(_) => "↗ ",
                Target::Other(_) => "? ",
            };
            let base = match (selected, found) {
                (true, _) => Style::new().fg(Color::Black).bg(ACCENT),
                (false, false) => Style::new().fg(DIM),
                (false, true) if b.target == Target::Group => {
                    Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD)
                }
                (false, true) => Style::new().fg(NOTE),
            };
            let hl = match selected {
                true => base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                false => Style::new().fg(HIT).add_modifier(Modifier::BOLD),
            };
            let indent = match query.is_empty() {
                true => "  ".repeat(b.depth()),
                false => String::new(),
            };
            let mut spans = vec![Span::styled(format!(" {indent}{icon}"), base)];
            let label = b.label();
            for (ci, ch) in label.chars().enumerate() {
                let on = m.as_ref().is_some_and(|m| m.positions.contains(&ci));
                spans.push(Span::styled(ch.to_string(), if on { hl } else { base }));
            }
            let right = match &b.target {
                _ if !found => "missing ".to_string(),
                Target::File { path, .. } => format!("{} ", dir_of_rel(path)),
                Target::Search(_) => "search ".to_string(),
                Target::Graph => "graph ".to_string(),
                Target::Url(_) => "link ".to_string(),
                Target::Folder(_) | Target::Group | Target::Other(_) => String::new(),
            };
            let used: usize = spans.iter().map(Span::width).sum();
            let pad = width.saturating_sub(used + right.chars().count());
            let dim = if selected {
                base
            } else {
                Style::new().fg(MUTED)
            };
            spans.push(Span::styled(" ".repeat(pad), base));
            spans.push(Span::styled(right, dim));
            lines.push(Line::from(spans));
        }
        if rows.is_empty() && !query.is_empty() {
            lines.push(Line::styled(" no bookmarks match", Style::new().fg(MUTED)));
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
                format!(
                    " ↑↓ choose · ⏎ open at the match · {} bookmark · Esc close ",
                    self.keys.label("search.bookmark")
                ),
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
                    let note = &ix.workspace.notes[res.note];
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

    fn draw_workspaces(&self, f: &mut Frame, area: Rect) {
        let title = Line::from(vec![
            Span::styled(
                " cold-lava ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("workspaces ", Style::new().fg(MUTED)),
        ]);
        let block = self.block(title, Focus::Workspaces);
        let inner = block.inner(area);
        f.render_widget(block, area);
        if self.workspaces.is_empty() {
            f.render_widget(
                Paragraph::new("No workspaces found.\n\nPress a to add a folder.")
                    .style(Style::new().fg(MUTED))
                    .wrap(Wrap { trim: false }),
                inner,
            );
            return;
        }
        let w = inner.width as usize;
        let lines: Vec<Line> = self
            .workspaces
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
                let st = if sel && self.focus == Focus::Workspaces {
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
            .workspaces
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
        let bookmarked = (self.core.as_ref())
            .and_then(|c| c.bookmarks().ok())
            .map(|b| b.paths())
            .unwrap_or_default();
        let mut lines = Vec::new();
        for (r, &i) in self.rows.iter().enumerate().skip(self.nscroll).take(h) {
            let it = &self.items[i];
            let sel = r == self.nsel;
            let indent = if flat {
                String::new()
            } else {
                "  ".repeat(it.depth)
            };
            let marked = self.marks.contains(&it.rel);
            let (icon, icon_color) = match it.kind {
                _ if marked => ("✓ ", HIT),
                ItemKind::Folder if self.expanded.contains(&it.rel) => ("▾ ", MUTED),
                ItemKind::Folder => ("▸ ", MUTED),
                ItemKind::Board => ("▦ ", Color::Rgb(83, 223, 221)),
                ItemKind::File => ("▤ ", Color::Rgb(83, 223, 221)),
                ItemKind::Note => ("• ", DIM),
            };
            let label = if flat {
                it.rel.trim_end_matches(".md").to_string()
            } else {
                it.name.clone()
            };
            let links = match (it.kind, self.links.get(&it.rel)) {
                (ItemKind::Note, Some(&(inc, _))) if inc > 0 => format!("←{inc}"),
                _ => String::new(),
            };
            let badge = match bookmarked.contains(&it.rel) {
                true if links.is_empty() => "★".to_string(),
                true => format!("★ {links}"),
                false => links,
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
            } else if marked {
                (HIT, None)
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
        if let Some(mut r) = self.reader.take() {
            let ix = self.core.as_ref().map(|c| &c.index);
            let shown = self.shown.as_deref();
            let previews = RefCell::new(std::mem::take(&mut self.previews));
            let embed = |target: &str, width: usize| {
                self.embed_lines(&mut previews.borrow_mut(), shown, target, width)
            };
            r.layout(inner.width, &|raw| resolves(ix, shown, raw), &embed);
            self.previews = previews.into_inner();
            self.reader = Some(r);
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
                let shower = (!focused).then(|| self.shower(p)).flatten();
                let who = match shower.map(|e| self.registry.manifest(e)) {
                    Some(m) => format!(" reader · ⏎ open in {} · → read ", m.name),
                    None => who,
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
        let owned = (self.current())
            .filter(|it| it.kind == ItemKind::File)
            .and_then(|it| self.registry.owner_of(&it.rel));
        let hint = match (self.current().map(|it| it.kind), owned) {
            (Some(ItemKind::Folder), _) => "a folder · → to open it".to_string(),
            (Some(ItemKind::Board), _) => "a whiteboard · ⏎ to open it".to_string(),
            (_, Some((owner, kind))) => {
                let name = &self.registry.manifest(owner).name;
                format!("{} · ⏎ to open it in {name}", kind.title)
            }
            _ => "select a note to see it here".to_string(),
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
        let focused = self.in_backlinks();
        let block = self.frame(title, focused);
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
        let mut info = match (self.focus, self.workspaces.get(self.vsel)) {
            (Focus::Workspaces, Some(v)) => {
                let src = match v.source {
                    "obsidian" => "from Obsidian",
                    "example" => "example workspace",
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
        for (_, text) in &self.statuses {
            info.spans.push(Span::styled(
                format!("   {text}"),
                Style::new().fg(ACCENT_SOFT),
            ));
        }
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
            (
                _,
                Mode::View {
                    backlinks: true, ..
                },
            ) => " backlinks · ↑↓ choose · ⏎ open at the link · ← back to the view · b hide"
                .to_string(),
            (Focus::Backlinks, _) => {
                " backlinks · ↑↓ choose · ⏎ open at the link · ← back to notes · b hide".to_string()
            }
            _ if !self.marks.is_empty() => {
                let k = |id| self.keys.first_label(id);
                format!(
                    " {} marked · {} move · {} delete · {} tag · {} clear",
                    self.marks.len(),
                    k("note.move"),
                    k("note.delete"),
                    k("note.tag"),
                    k("filter.clear"),
                )
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

/// Whether a wiki or Markdown link written in the note at `from` leads to a note (a heading in
/// the same note always does).
fn resolves(ix: Option<&crate::index::Index>, from: Option<&Path>, raw: &str) -> bool {
    let link = crate::resolver::split_link(raw);
    link.note.is_empty()
        || ix.is_some_and(|ix| {
            let note = from.and_then(|p| ix.by_path(p));
            let dir = note.map_or("", |i| dir_of(&ix.workspace.notes[i].rel));
            ix.resolver.resolve(dir, &link.note).is_some()
        })
}

/// What kind of attachment a link names (`photo.jpg` → `JPG image`); `None` for a note.
fn attachment_label(name: &str) -> Option<String> {
    let kind = workspace::attachment_kind(name)?;
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
    let headings = r.headings();
    (crumbs(r, path).into_iter())
        .map(|i| format!(" › {}", headings[i].text))
        .collect()
}

/// The headings the reader is under (as indices into its headings), less a first one that
/// only repeats the note's name.
fn crumbs(r: &Reader, path: &Path) -> Vec<usize> {
    let name = workspace::search_key(&path.file_stem().unwrap_or_default().to_string_lossy());
    let headings = r.headings();
    let mut section = r.section();
    if section
        .first()
        .is_some_and(|&i| workspace::search_key(&headings[i].text) == name)
    {
        section.remove(0);
    }
    section
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

/// A file's rewritten links in a preview: its name, then the first few changed lines.
/// The hint of a rename or merge dialog waiting for the owners of the files in `edits`.
fn waiting_for(edits: &[crate::rename::FileEdit]) -> String {
    let mut owners: Vec<&str> = (edits.iter())
        .filter_map(|e| Some(e.owned.as_ref()?.owner.as_str()))
        .collect();
    owners.sort();
    owners.dedup();
    format!(
        " waiting for {} to update its files · Esc cancel ",
        owners.join(", ")
    )
}

fn link_changes(e: &crate::rename::FileEdit, width: u16) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![
        Span::styled(
            format!(" ▤ {}", e.file.trim_end_matches(".md")),
            Style::new().fg(ACCENT_SOFT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} link{}", e.links, if e.links == 1 { "" } else { "s" }),
            Style::new().fg(DIM),
        ),
    ])];
    for c in e.changes.iter().take(3) {
        let mut t: String = (c.after.trim().chars())
            .take(width.saturating_sub(10) as usize)
            .collect();
        if c.after.trim().chars().count() > t.chars().count() {
            t.push('…');
        }
        let line = match c.line {
            0 => String::new(),
            n => n.to_string(),
        };
        out.push(Line::from(vec![
            Span::styled(format!("   {line:>4}  "), Style::new().fg(DIM)),
            Span::styled(t, Style::new().fg(NOTE)),
        ]));
    }
    if e.changes.len() > 3 {
        out.push(Line::styled(
            format!("         … {} more lines", e.changes.len() - 3),
            Style::new().fg(DIM),
        ));
    }
    out
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The workspace's tags that start with what was typed (after a `-` and a `#`).
fn tag_matches<'a>(typed: &str, all: &'a [String]) -> Vec<&'a String> {
    let q = typed
        .trim_start_matches('-')
        .trim_start_matches('#')
        .to_lowercase();
    all.iter().filter(|t| t.starts_with(&q)).collect()
}

/// What the tag prompt holds: what was typed, or the tag Tab completed it to (keeping a `-`).
fn tag_shown(typed: &str, all: &[String], sel: Option<usize>) -> String {
    match sel.and_then(|i| tag_matches(typed, all).get(i).copied()) {
        Some(t) if typed.starts_with('-') => format!("-{t}"),
        Some(t) => t.clone(),
        None => typed.to_string(),
    }
}

/// The tag prompt, with the workspace's tags that match under it.
fn draw_tag(
    f: &mut Frame,
    area: Rect,
    typed: &str,
    all: &[String],
    sel: Option<usize>,
    marked: usize,
) {
    let whom = match marked {
        0 => "the note".to_string(),
        n => plural(n, "marked note"),
    };
    let title = format!(" tag {whom} · -tag removes · Tab · ⏎ ");
    let r = prompt(f, area, &title, "tag: #", &tag_shown(typed, all, sel));
    let hits = tag_matches(typed, all);
    let room = area.bottom().saturating_sub(r.bottom()) as usize;
    let rows = hits.len().min(8).min(room.saturating_sub(2));
    if rows == 0 {
        return;
    }
    let start = sel.map_or(0, |i| (i + 1).saturating_sub(rows));
    let list = Rect::new(r.x, r.bottom(), r.width, rows as u16 + 2);
    let width = list.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = (hits.iter().enumerate().skip(start).take(rows))
        .map(|(i, t)| {
            let on = Some(i) == sel;
            let style = match on {
                true => Style::new().fg(Color::Black).bg(ACCENT),
                false => Style::new().fg(NOTE),
            };
            let mark = if on { "▸" } else { " " };
            Line::styled(
                format!("{mark} #{t:<w$}", w = width.saturating_sub(3)),
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
                .title(format!(" {} ", plural(hits.len(), "tag"))),
        ),
        list,
    );
}

/// The folder several moves go into, as the user typed it: `Archive`, or the workspace's root.
fn destination(moves: &[crate::rename::Move]) -> String {
    match moves.first().map(|m| dir_of_rel(&m.to)) {
        Some("") | None => "the workspace's root".to_string(),
        Some(d) => d.to_string(),
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

/// The new-note prompt, with the workspace's templates under it when it has some.
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

/// An extension's view over the whole main area, its filter on the bottom line.
/// Draws the view in `area` and returns where its rows went.
fn draw_view(f: &mut Frame, area: Rect, v: &mut views::Open) -> Rect {
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::styled(
            format!(" {} ", v.title()),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(v.hint(), Style::new().fg(MUTED)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [body, status] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let lines = v.lines(body.width as usize, body.height as usize);
    f.render_widget(Paragraph::new(lines), body);
    f.render_widget(Paragraph::new(v.status()), status);
    body
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
        ("↑↓ / jk", "move through workspaces or notes".to_string()),
        (
            "→ / ←",
            "open / close a folder · workspaces ↔ notes".to_string(),
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
                .title(format!(" cold-lava {} · any key closes ", crate::VERSION)),
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
    fn the_list_of_workspaces_carries_over_from_vaults_json() {
        let dir = std::env::temp_dir().join(format!("cold-lava-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = r#"{"vaults": [{"path": "/home/me/Notes", "opened": 5}]}"#;
        std::fs::write(dir.join("vaults.json"), old).unwrap();
        assert_eq!(load_recent(&dir), [(PathBuf::from("/home/me/Notes"), 5)]);

        remember(&dir, Path::new("/home/me/Work"));
        let paths: Vec<PathBuf> = load_recent(&dir).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            paths,
            ["/home/me/Work", "/home/me/Notes"].map(PathBuf::from)
        );
        let new = std::fs::read_to_string(dir.join("workspaces.json")).unwrap();
        assert!(new.contains("\"workspaces\""), "{new}");
        assert_eq!(
            std::fs::read_to_string(dir.join("vaults.json")).unwrap(),
            old
        );

        std::fs::write(dir.join("vaults.json"), r#"{"vaults": []}"#).unwrap();
        assert_eq!(
            load_recent(&dir).len(),
            2,
            "the new file wins once it exists"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tree_sorting_and_folding() {
        let items = scan_tree(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace"),
            |_| false,
        );
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
