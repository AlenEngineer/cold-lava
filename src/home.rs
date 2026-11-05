//! The main menu: vaults on the left, the selected vault's notes in the middle, and the
//! selected note on the right, shown in a real (embedded) Neovim, or as a rendered
//! preview when Neovim isn't installed.

use crate::app::{self, ACCENT, ACCENT_SOFT, DIM, HIT, MUTED, NOTE, Render};
use crate::embed::Embedded;
use crate::{board, graph, term, vault};
use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
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

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(".config")))
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
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
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
    Editor,
}

enum Mode {
    Normal,
    Filter,
    NewNote(String),
    AddVault(String),
    Help,
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
    // Right pane
    editor_cmd: Option<String>,
    editor: Option<Embedded>,
    shown: Option<PathBuf>,
    pending: Option<(PathBuf, Instant)>,
    preview: Vec<String>,
    zoom: bool,
    // UI
    focus: Focus,
    mode: Mode,
    msg: Option<(String, Instant)>,
    areas: [Rect; 3],
    quit: bool,
    fs_events: Option<Receiver<notify::Event>>,
    _watcher: Option<notify::RecommendedWatcher>,
    rescan_at: Option<Instant>,
}

/// Neovim (or Vim) to embed: $VISUAL/$EDITOR if it is one, else `nvim` if it's installed.
fn pick_editor() -> Option<String> {
    let is_vim = |p: &str| {
        let base = Path::new(p)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        matches!(
            base.as_str(),
            "nvim" | "vim" | "vi" | "nvim.exe" | "vim.exe"
        )
    };
    for var in ["COLD_LAVA_EDITOR", "VISUAL", "EDITOR"] {
        if let Ok(v) = std::env::var(var) {
            let prog = v.split_whitespace().next().unwrap_or("").to_string();
            if is_vim(&prog) {
                return Some(prog);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .find(|d| d.join("nvim").is_file())
        .map(|_| "nvim".to_string())
}

/// The command we type into Vim/Neovim to show another note: save edits, then open it.
pub fn open_command(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\'', "''");
    // <C-\><C-n> gets to normal mode from any mode first.
    format!("\x1c\x0e:silent! wall | execute 'edit' fnameescape('{p}')\r")
}

impl Home {
    pub fn new(render: Render, initial: Option<PathBuf>) -> Home {
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
            editor_cmd: pick_editor(),
            editor: None,
            shown: None,
            pending: None,
            preview: Vec::new(),
            zoom: false,
            focus: Focus::Vaults,
            mode: Mode::Normal,
            msg: None,
            areas: [Rect::default(); 3],
            quit: false,
            fs_events: None,
            _watcher: None,
            rescan_at: None,
        };
        if !h.vaults.is_empty() {
            h.open_vault(0);
            if initial.is_some() {
                h.focus = Focus::Notes;
            }
        }
        h
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            let mut changed = false;
            if let Some(ed) = self.editor.as_mut() {
                changed |= ed.pump();
                if ed.exited() {
                    self.editor = None;
                    self.shown = None;
                    if self.focus == Focus::Editor {
                        self.focus = Focus::Notes;
                    }
                    self.flash("Neovim closed. Select a note to open it again");
                }
            }
            changed |= self.load_pending();
            self.check_disk();
            let _ = changed;
            terminal.draw(|f| self.draw(f))?;
            let ms = if self.editor.is_some() || self.pending.is_some() {
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
        if let Some(ed) = self.editor.as_mut() {
            // Save before leaving.
            ed.send(b"\x1c\x0e:silent! wall\r");
            std::thread::sleep(Duration::from_millis(150));
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
        self.expanded.clear();
        self.filter.clear();
        self.nsel = 0;
        self.nscroll = 0;
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
        let (rx, w) = app::watch_vault(&v.path);
        self.fs_events = rx;
        self._watcher = w;
        remember(&v.path);
        self.select_row(0);
    }

    fn reload_tree(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        self.items = scan_tree(&root);
        self.links.clear();
        if let Ok(v) = vault::scan(&root) {
            let opts = graph::BuildOpts {
                unresolved: false,
                tags: false,
                orphans: true,
            };
            let g = graph::build(&v, opts, &HashMap::new());
            for (i, n) in g.nodes.iter().enumerate() {
                if let Some(rel) = &n.rel {
                    self.links
                        .insert(format!("{rel}.md"), (g.links_in[i], g.links_out[i]));
                }
            }
        }
        self.refresh_rows();
    }

    fn refresh_rows(&mut self) {
        let keep = self.current().map(|it| it.rel.clone());
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

    fn load_pending(&mut self) -> bool {
        match &self.pending {
            Some((_, t)) if Instant::now() >= *t => {}
            _ => return false,
        }
        let (path, _) = self.pending.take().unwrap();
        self.show(&path);
        true
    }

    /// Show a note in the right pane (embedded editor, or preview).
    fn show(&mut self, path: &Path) {
        if self.shown.as_deref() == Some(path) {
            return;
        }
        self.shown = Some(path.to_path_buf());
        let Some(cmd) = self.editor_cmd.clone() else {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let mut lines: Vec<String> = text.lines().map(String::from).collect();
            // Hide YAML frontmatter in the preview.
            if lines.first().is_some_and(|l| l.trim_end() == "---")
                && let Some(end) = lines.iter().skip(1).position(|l| l.trim_end() == "---")
            {
                lines.drain(..end + 2);
            }
            self.preview = lines;
            return;
        };
        match self.editor.as_mut() {
            Some(ed) => ed.send(open_command(path).as_bytes()),
            None => {
                let a = self.areas[2];
                let cwd = self.root.clone().unwrap_or_else(|| PathBuf::from("."));
                let args = vec![
                    "-c".to_string(),
                    "set autoread".to_string(),
                    path.to_string_lossy().into_owned(),
                ];
                match Embedded::spawn(&cmd, &args, &cwd, a.height.max(5), a.width.max(20)) {
                    Ok(ed) => self.editor = Some(ed),
                    Err(e) => {
                        self.editor_cmd = None;
                        self.flash(format!(
                            "couldn't start {cmd}: {e}; showing previews instead"
                        ));
                        self.shown = None;
                        self.show(path);
                    }
                }
            }
        }
    }

    fn check_disk(&mut self) {
        if let (Some(rx), Some(root)) = (&self.fs_events, &self.root) {
            let changed = rx.try_iter().any(|ev| {
                ev.paths.iter().any(|p| {
                    app::is_vault_change(root, p) || p.extension().is_some_and(|e| e == "canvas")
                })
            });
            if changed {
                self.rescan_at = Some(Instant::now() + Duration::from_millis(300));
            }
        }
        if self.rescan_at.is_some_and(|t| Instant::now() >= t) {
            self.rescan_at = None;
            self.reload_tree();
            if self.editor_cmd.is_none()
                && let Some(p) = self.shown.take()
            {
                self.show(&p);
            }
        }
    }

    fn flash(&mut self, s: impl Into<String>) {
        self.msg = Some((s.into(), Instant::now()));
    }

    // ---------- input ----------

    fn on_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        // Keys typed into the editor pane go to Neovim, except Ctrl-q (back to the menu).
        if self.focus == Focus::Editor {
            if ctrl && matches!(k.code, KeyCode::Char('q')) {
                self.focus = Focus::Notes;
            } else if let Some(ed) = self.editor.as_mut() {
                ed.send_key(k);
            } else {
                self.focus = Focus::Notes;
            }
            return Ok(());
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
            Mode::NewNote(name) | Mode::AddVault(name) => {
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Backspace => {
                        name.pop();
                    }
                    KeyCode::Char(c) => name.push(c),
                    KeyCode::Enter => {
                        let text = name.trim().to_string();
                        let new_note = matches!(self.mode, Mode::NewNote(_));
                        self.mode = Mode::Normal;
                        if !text.is_empty() {
                            if new_note {
                                self.create_note(&text)?;
                            } else {
                                self.add_vault(&text);
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::Normal => {}
        }

        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Vaults => Focus::Notes,
                    Focus::Notes if self.editor.is_some() => Focus::Editor,
                    _ => Focus::Vaults,
                }
            }
            KeyCode::BackTab => {
                self.focus = if self.focus == Focus::Notes {
                    Focus::Vaults
                } else {
                    Focus::Notes
                }
            }
            KeyCode::Char('/') => {
                self.mode = Mode::Filter;
                self.focus = Focus::Notes;
            }
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.refresh_rows();
            }
            KeyCode::Char('z') => self.zoom = !self.zoom,
            KeyCode::Char('g') => self.open_graph(terminal)?,
            KeyCode::Char('w') => self.open_boards(terminal, None)?,
            KeyCode::Char('n') if self.root.is_some() => self.mode = Mode::NewNote(String::new()),
            KeyCode::Char('a') => self.mode = Mode::AddVault(String::new()),
            KeyCode::Char('r') => {
                self.reload_tree();
                self.flash("rescanned");
            }
            KeyCode::Char('o') => self.open_in_obsidian(),
            KeyCode::Char('e') => self.edit_fullscreen(terminal)?,
            _ => match self.focus {
                Focus::Vaults => self.vault_key(k),
                Focus::Notes => self.notes_key(k, terminal)?,
                Focus::Editor => {}
            },
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
                        if self.editor.is_some() {
                            self.focus = Focus::Editor;
                        } else if k.code == KeyCode::Enter {
                            self.edit_fullscreen(terminal)?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        let [va, na, ea] = self.areas;
        if ea.contains(pos) {
            if let Some(ed) = self.editor.as_mut() {
                if matches!(m.kind, MouseEventKind::Down(_)) {
                    self.focus = Focus::Editor;
                }
                ed.send_mouse(m, m.column - ea.x, m.row - ea.y);
            }
            return;
        }
        let inside = |a: Rect| {
            Rect::new(
                a.x + 1,
                a.y + 1,
                a.width.saturating_sub(2),
                a.height.saturating_sub(2),
            )
            .contains(pos)
        };
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

    fn open_graph(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        match vault::scan(&root) {
            Ok(v) if !v.notes.is_empty() => {
                app::App::new(v, self.render).run(terminal)?;
                terminal.clear()?;
            }
            Ok(_) => self.flash("this vault has no notes yet"),
            Err(e) => self.flash(format!("couldn't read the vault: {e}")),
        }
        Ok(())
    }

    fn open_boards(&mut self, terminal: &mut DefaultTerminal, file: Option<PathBuf>) -> Result<()> {
        let Some(root) = self.root.clone() else {
            return Ok(());
        };
        board::launch(terminal, &root, file, false)?;
        terminal.clear()?;
        self.reload_tree();
        Ok(())
    }

    fn edit_fullscreen(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let (Some(p), Some(root)) = (self.current_path(), self.root.clone()) else {
            return Ok(());
        };
        if self.current().is_some_and(|it| it.kind == ItemKind::Note) {
            if let Some(err) = term::edit_file(terminal, &p, &root)? {
                self.flash(err);
            }
            self.shown = None;
            self.select_row(self.nsel);
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

    fn create_note(&mut self, name: &str) -> Result<()> {
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
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let title = name.trim_end_matches(".md");
            std::fs::write(&path, format!("# {title}\n\n"))?;
            if !folder.is_empty() {
                self.expanded.insert(folder);
            }
            self.flash(format!("created {rel}"));
        }
        // Make sure the new note is visible, even if a filter was active.
        self.filter.clear();
        self.reload_tree();
        if let Some(r) = self.rows.iter().position(|&i| self.items[i].rel == rel) {
            self.nsel = r;
            self.pending = None;
            self.show(&path);
            if self.editor.is_some() {
                self.focus = Focus::Editor;
            }
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

    fn pane(&self, title: Line<'static>, focus: Focus) -> Block<'static> {
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
        let editor_inner = Rect::new(
            ea.x + 1,
            ea.y + 1,
            ea.width.saturating_sub(2),
            ea.height.saturating_sub(2),
        );
        self.areas = [va, na, editor_inner];

        if !self.zoom {
            self.draw_vaults(f, va);
            self.draw_notes(f, na);
        }
        self.draw_editor(f, ea, editor_inner);
        self.draw_status(f, status);
        match &self.mode {
            Mode::Help => draw_help(f, main),
            Mode::NewNote(name) => prompt(
                f,
                main,
                " new note · ⏎ create · Esc cancel ",
                "name: ",
                name,
            ),
            Mode::AddVault(name) => prompt(
                f,
                main,
                " add a vault · ⏎ add · Esc cancel ",
                "folder: ",
                name,
            ),
            _ => {}
        }
    }

    fn draw_vaults(&self, f: &mut Frame, area: Rect) {
        let title = Line::from(vec![
            Span::styled(
                " cold-lava ",
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("vaults ", Style::new().fg(MUTED)),
        ]);
        let block = self.pane(title, Focus::Vaults);
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
        let mut block = self.pane(title, Focus::Notes);
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
        let title = match &self.shown {
            Some(p) => {
                let rel = self
                    .root
                    .as_ref()
                    .and_then(|r| p.strip_prefix(r).ok())
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let who = match (&self.editor_cmd, self.focus == Focus::Editor) {
                    (Some(c), true) => format!(" {c} · editing · Ctrl-q back "),
                    (Some(c), false) => format!(" {c} · ⏎ edit "),
                    (None, _) => " preview · e edit in $EDITOR ".to_string(),
                };
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", rel.trim_end_matches(".md")),
                        Style::new().fg(NOTE).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(who, Style::new().fg(MUTED)),
                ])
            }
            None => Line::styled(" note ", Style::new().fg(MUTED)),
        };
        let block = self.pane(title, Focus::Editor);
        f.render_widget(block, area);
        if let Some(ed) = self.editor.as_mut() {
            ed.resize(inner.height, inner.width);
            if let Some((x, y)) = ed.render(f.buffer_mut(), inner)
                && self.focus == Focus::Editor
                && matches!(self.mode, Mode::Normal)
            {
                f.set_cursor_position((x, y));
            }
            return;
        }
        if self.shown.is_some() && self.editor_cmd.is_none() {
            let text = board::markdown_text(&self.preview.join("\n"));
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        } else {
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
    }

    fn draw_status(&mut self, f: &mut Frame, area: Rect) {
        let info = match (self.focus, self.vaults.get(self.vsel)) {
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
        if self
            .msg
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3))
        {
            self.msg = None;
        }
        let hints = match (self.focus, &self.mode) {
            (_, Mode::Filter) => " type to filter · ↑↓ move · ⏎ keep · Esc clear".to_string(),
            (Focus::Editor, _) => {
                " you're in Neovim: everything you type goes to it · Ctrl-q back to the menu"
                    .to_string()
            }
            _ => {
                let edit = if self.editor_cmd.is_some() {
                    "⏎ edit"
                } else {
                    "⏎ open"
                };
                format!(
                    " ↑↓ move  ←→ fold  {edit}  / filter  g graph  w boards  n new note  a add vault  z zoom  ? help  q quit"
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

fn prompt(f: &mut Frame, area: Rect, title: &str, label: &str, value: &str) {
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
}

fn draw_help(f: &mut Frame, area: Rect) {
    let rows: &[(&str, &str)] = &[
        ("↑↓ / jk", "move through vaults or notes"),
        ("Tab / ←→", "switch between vaults, notes and the editor"),
        ("→ / ←", "open / close a folder"),
        (
            "Enter",
            "edit the note in the embedded Neovim · open a board",
        ),
        ("Ctrl-q", "leave Neovim and return to the menu"),
        ("e", "edit the note in $EDITOR, full screen"),
        ("/", "filter notes by name or path"),
        ("g", "graph view of this vault"),
        ("w", "whiteboards of this vault"),
        ("n", "new note in the selected folder"),
        ("a", "add a vault (any folder of Markdown files)"),
        ("o", "open the selected note in Obsidian"),
        ("z", "zoom the editor to full width"),
        ("r", "rescan the vault"),
        ("q", "quit (notes are saved)"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:>12}  "), Style::new().fg(ACCENT)),
                Span::styled(*d, Style::new().fg(NOTE)),
            ])
        })
        .collect();
    let w = 70.min(area.width.saturating_sub(4));
    let h = (rows.len() as u16 + 3).min(area.height);
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
                .title(" cold-lava · any key closes "),
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
        let items = scan_tree(&Path::new(env!("CARGO_MANIFEST_DIR")).join("example-vault"));
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
    fn vim_open_command_escapes_quotes() {
        let c = open_command(Path::new("/v/It's here.md"));
        assert!(c.contains("fnameescape('/v/It''s here.md')"));
        assert!(c.starts_with("\x1c\x0e"));
    }
}
