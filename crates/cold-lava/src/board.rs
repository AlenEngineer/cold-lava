//! A whiteboard for the terminal that reads and writes Obsidian Canvas files (JSON Canvas,
//! https://jsoncanvas.org). Boards made here open in Obsidian, and vice versa.

use crate::app::{ACCENT, ACCENT_SOFT, DIM, HIT, MUTED, NOTE};
use crate::commands::{self, BoardAction, HistoryAction, Key, KeyHint, Keymap};
use crate::history::{Exit, Nav};
use crate::term;
use anyhow::{Context as _, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::canvas::{Canvas, Line as CLine};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

// ---------- JSON Canvas model (unknown fields are preserved) ----------

#[derive(Serialize, Deserialize, Clone, Default)]
struct Doc {
    #[serde(default)]
    nodes: Vec<CNode>,
    #[serde(default)]
    edges: Vec<CEdge>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone)]
struct CNode {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(serialize_with = "round")]
    x: f64,
    #[serde(serialize_with = "round")]
    y: f64,
    #[serde(serialize_with = "round")]
    width: f64,
    #[serde(serialize_with = "round")]
    height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CEdge {
    id: String,
    from_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from_end: Option<String>,
    to_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn round<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_i64(v.round() as i64)
}

fn new_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = t ^ COUNTER
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    format!("{x:016x}")
}

/// Obsidian's colour presets "1".."6", or a hex colour.
fn canvas_color(c: &Option<String>) -> Option<Color> {
    let c = c.as_deref()?;
    Some(match c {
        "1" => Color::Rgb(233, 73, 73),
        "2" => Color::Rgb(233, 151, 63),
        "3" => Color::Rgb(224, 222, 113),
        "4" => Color::Rgb(68, 207, 110),
        "5" => Color::Rgb(83, 223, 221),
        "6" => Color::Rgb(168, 130, 255),
        hex if hex.starts_with('#') && hex.len() == 7 => {
            let v = u32::from_str_radix(&hex[1..], 16).ok()?;
            Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
        }
        _ => return None,
    })
}

const CARD_BORDER: Color = Color::Rgb(110, 110, 125);
const EDGE_COLOR: Color = Color::Rgb(125, 125, 140);
const GRID: Color = Color::Rgb(55, 55, 66);
const GRID_SNAP: f64 = 10.0;
/// Canvas pixels per terminal cell at zoom 1.
const PX_COL: f64 = 10.0;
const PX_ROW: f64 = 20.0;

// ---------- state ----------

enum Mode {
    Normal,
    Edit {
        id: String,
        buf: String,
        is_new: bool,
    },
    Connect {
        from: String,
    },
    PickFile {
        query: String,
        pos: usize,
    },
    Help,
    /// Which key: the whiteboard's commands by group, until a key is typed.
    Keys,
}

enum Drag {
    Pan {
        col: u16,
        row: u16,
    },
    /// Moving a card (and, for groups, the cards inside it).
    Move {
        offsets: Vec<(String, f64, f64)>,
        grab: (f64, f64),
        moved: bool,
    },
    Resize {
        id: String,
        moved: bool,
    },
}

struct Board {
    path: PathBuf,
    vault_root: PathBuf,
    vault_name: String,
    doc: Doc,
    cam: (f64, f64),
    zoom: f64,
    area: Rect,
    sel: Option<String>,
    mode: Mode,
    drag: Option<Drag>,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    msg: Option<(String, Instant)>,
    quit: bool,
    /// The last save was refused (the file on disk changed and doesn't parse): quitting now
    /// would lose edits, so `q` warns once first.
    unsaved: bool,
    last_click: Option<(Instant, u16, u16)>,
    notes: Option<Vec<String>>,
    previews: HashMap<String, Vec<String>>,
    // Sync with the file on disk (Obsidian may edit it while we have it open).
    /// The version last read from or written to disk: the common ancestor for merges.
    base: Doc,
    /// Exact bytes last read or written, to spot outside changes cheaply.
    disk: Vec<u8>,
    last_check: Instant,
    fs_events: Option<Receiver<notify::Event>>,
    _watcher: Option<RecommendedWatcher>,
    keys: Keymap<BoardAction>,
    /// Inside the main menu: where Back and Forward lead, and their keys.
    nav: Option<(Nav, Keymap<HistoryAction>)>,
    exit: Exit,
}

// ---------- entry points ----------

/// Open a board. With no file, show a picker of the vault's `.canvas` files.
pub fn launch(
    terminal: &mut DefaultTerminal,
    vault_root: &Path,
    file: Option<PathBuf>,
    _pixels: bool,
) -> Result<()> {
    let path = match file {
        Some(p) => p,
        None => match pick_board(terminal, vault_root)? {
            Some(p) => p,
            None => return Ok(()),
        },
    };
    open(terminal, vault_root, path, None)?;
    Ok(())
}

/// Runs the board at `path` (created if missing) until it is left. With `nav`, from the main
/// menu, the history keys leave it too.
pub fn open(
    terminal: &mut DefaultTerminal,
    vault_root: &Path,
    path: PathBuf,
    nav: Option<Nav>,
) -> Result<Exit> {
    crate::write::create_if_missing(&path, b"{\n\t\"nodes\":[],\n\t\"edges\":[]\n}\n")?;
    let mut b = Board::open(path, vault_root)?;
    b.nav = nav.map(|nav| (nav, commands::history_keys(&b.keys)));
    b.run(terminal)?;
    Ok(b.exit)
}

pub fn pick_board(terminal: &mut DefaultTerminal, root: &Path) -> Result<Option<PathBuf>> {
    let mut boards: Vec<String> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "canvas"))
        .filter_map(|e| {
            e.path()
                .strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
        })
        .collect();
    boards.sort();
    let mut query = String::new();
    let mut pos = 0usize;
    loop {
        let q = crate::vault::search_key(&query);
        let hits: Vec<&String> = boards
            .iter()
            .filter(|b| crate::vault::search_key(b).contains(&q))
            .collect();
        let new_name = if query.trim().is_empty() {
            "Untitled".to_string()
        } else {
            query.trim().to_string()
        };
        let total = hits.len() + 1;
        pos = pos.min(total - 1);
        terminal.draw(|f| {
            let area = centered(
                f.area(),
                64,
                (total as u16 + 4)
                    .min(f.area().height.saturating_sub(2))
                    .max(6),
            );
            let mut lines = vec![Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
            ])];
            lines.push(Line::raw(""));
            for (i, h) in hits.iter().enumerate() {
                let st = if i == pos {
                    Style::new().fg(Color::Black).bg(ACCENT)
                } else {
                    Style::new().fg(NOTE)
                };
                lines.push(Line::styled(format!("  {h}  "), st));
            }
            let st = if pos == hits.len() {
                Style::new().fg(Color::Black).bg(HIT)
            } else {
                Style::new().fg(HIT)
            };
            lines.push(Line::styled(
                format!("  + new board \"{new_name}.canvas\"  "),
                st,
            ));
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::bordered()
                        .border_type(BorderType::Rounded)
                        .border_style(Style::new().fg(ACCENT))
                        .title(" whiteboards · type to filter · ⏎ open · Esc back "),
                ),
                area,
            );
        })?;
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            match k.code {
                KeyCode::Esc => return Ok(None),
                KeyCode::Up => pos = pos.saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => pos = (pos + 1).min(total - 1),
                KeyCode::Backspace => {
                    query.pop();
                    pos = 0;
                }
                KeyCode::Enter => {
                    let rel = if pos < hits.len() {
                        hits[pos].clone()
                    } else {
                        format!("{new_name}.canvas")
                    };
                    return Ok(Some(root.join(rel)));
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    pos = 0;
                }
                _ => {}
            }
        }
    }
}

impl Board {
    fn open(path: PathBuf, vault_root: &Path) -> Result<Board> {
        let disk = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let doc = parse(&disk, &path)?;
        let base = doc.clone();
        // Watch the folder (not the file): editors replace files by renaming over them.
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let _ = tx.send(ev);
            }
        })
        .ok();
        if let (Some(w), Some(dir)) = (watcher.as_mut(), path.parent())
            && w.watch(dir, RecursiveMode::NonRecursive).is_err()
        {
            watcher = None;
        }
        let vault_root = vault_root
            .canonicalize()
            .unwrap_or_else(|_| vault_root.to_path_buf());
        let vault_name = vault_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut b = Board {
            path,
            vault_root,
            vault_name,
            doc,
            cam: (0.0, 0.0),
            zoom: 1.0,
            area: Rect::default(),
            sel: None,
            mode: Mode::Normal,
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            msg: None,
            quit: false,
            unsaved: false,
            last_click: None,
            notes: None,
            previews: HashMap::new(),
            base,
            disk,
            last_check: Instant::now(),
            fs_events: watcher.is_some().then_some(rx),
            _watcher: watcher,
            keys: Keymap::new(commands::BOARD),
            nav: None,
            exit: Exit::Quit,
        };
        b.fit_pending();
        Ok(b)
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut first = true;
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            if first {
                // Area is known only after the first draw.
                self.fit();
                first = false;
                continue;
            }
            if event::poll(Duration::from_millis(250))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => self.on_key(k, terminal)?,
                    Event::Mouse(m) => self.on_mouse(m),
                    _ => {}
                }
            }
            self.watch_disk();
        }
        Ok(())
    }

    // ---------- live sync with the file on disk ----------

    /// Pick up outside changes: on a file-system event, or every 2 s as a fallback.
    fn watch_disk(&mut self) {
        let mut touched = false;
        if let Some(rx) = &self.fs_events {
            let name = self.path.file_name();
            for ev in rx.try_iter() {
                touched |= ev.paths.iter().any(|p| p.file_name() == name);
            }
        }
        let busy = matches!(self.mode, Mode::Edit { .. }) || self.drag.is_some();
        if busy || !(touched || self.last_check.elapsed() > Duration::from_secs(2)) {
            return;
        }
        self.last_check = Instant::now();
        self.sync_from_disk();
    }

    /// If the file changed outside cold-lava, merge those changes into ours.
    /// Returns true when something was merged in.
    fn sync_from_disk(&mut self) -> bool {
        let Ok(bytes) = std::fs::read(&self.path) else {
            return false;
        };
        if bytes == self.disk {
            return false;
        }
        // A half-written file fails to parse; we'll catch the complete one on the next check.
        let Ok(theirs) = parse(&bytes, &self.path) else {
            return false;
        };
        self.disk = bytes;
        let (Ok(b), Ok(o), Ok(t)) = (
            serde_json::to_value(&self.base),
            serde_json::to_value(&self.doc),
            serde_json::to_value(&theirs),
        ) else {
            return false;
        };
        if t == b {
            return false; // same content, just rewritten
        }
        let (merged, conflicts) = merge3(&b, &o, &t);
        self.base = theirs;
        match serde_json::from_value::<Doc>(merged) {
            Ok(d) => self.doc = d,
            Err(e) => {
                self.flash(format!("couldn't merge outside changes: {e}"));
                return false;
            }
        }
        self.previews.clear();
        if let Some(s) = &self.sel
            && self.node(s).is_none()
        {
            self.sel = None;
        }
        // Write back only if our side added something the file doesn't have yet.
        if serde_json::to_value(&self.doc).ok() != Some(t) {
            self.write_file();
        }
        self.flash(if conflicts > 0 {
            format!("merged changes from disk ({conflicts} conflicting edits: kept yours)")
        } else {
            "merged changes made outside cold-lava".to_string()
        });
        true
    }

    // ---------- persistence ----------

    fn save(&mut self) {
        // Never overwrite edits made elsewhere (e.g. in Obsidian): merge them in first.
        self.sync_from_disk();
        self.write_file();
    }

    fn write_file(&mut self) {
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(b"\t");
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        if let Err(e) = self.doc.serialize(&mut ser) {
            return self.flash(format!("save failed: {e}"));
        }
        buf.push(b'\n');
        if buf == self.disk {
            return;
        }
        // Only over the bytes we last read: a change made since (that the merge above didn't
        // see, or a file that doesn't parse, so it couldn't be merged) is never overwritten.
        match crate::write::replace(&self.path, &self.disk, &buf) {
            Ok(()) => {
                self.base = self.doc.clone();
                self.disk = buf;
                self.unsaved = false;
            }
            Err(crate::write::WriteError::Changed(_)) => {
                self.unsaved = true;
                self.flash(
                    "not saved: the file changed on disk and can't be read yet. Your edits are \
                     kept and saved once it can",
                );
            }
            Err(e) => {
                self.unsaved = true;
                self.flash(format!("save failed: {e}"));
            }
        }
    }

    /// Snapshot for undo; call before every change.
    /// Quit, unless the last save was refused: then warn once, and quit on the next try.
    fn request_quit(&mut self) {
        if self.unsaved {
            self.unsaved = false;
            self.flash("your last edits aren't saved: quit again to leave anyway");
        } else {
            self.quit = true;
        }
    }

    fn checkpoint(&mut self) {
        self.undo.push(self.doc.clone());
        if self.undo.len() > 300 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn commit(&mut self) {
        self.save();
    }

    fn undo(&mut self) {
        match self.undo.pop() {
            Some(d) => {
                self.redo.push(std::mem::replace(&mut self.doc, d));
                self.save();
                self.flash("undone");
            }
            None => self.flash("nothing to undo"),
        }
    }

    fn redo(&mut self) {
        match self.redo.pop() {
            Some(d) => {
                self.undo.push(std::mem::replace(&mut self.doc, d));
                self.save();
                self.flash("redone");
            }
            None => self.flash("nothing to redo"),
        }
    }

    // ---------- geometry ----------

    fn node(&self, id: &str) -> Option<&CNode> {
        self.doc.nodes.iter().find(|n| n.id == id)
    }

    fn node_mut(&mut self, id: &str) -> Option<&mut CNode> {
        self.doc.nodes.iter_mut().find(|n| n.id == id)
    }

    fn to_screen(&self, x: f64, y: f64) -> (f64, f64) {
        let a = self.area;
        (
            a.x as f64 + a.width as f64 / 2.0 + (x - self.cam.0) * self.zoom / PX_COL,
            a.y as f64 + a.height as f64 / 2.0 + (y - self.cam.1) * self.zoom / PX_ROW,
        )
    }

    fn to_world(&self, col: u16, row: u16) -> (f64, f64) {
        let a = self.area;
        (
            self.cam.0
                + (col as f64 + 0.5 - a.x as f64 - a.width as f64 / 2.0) * PX_COL / self.zoom,
            self.cam.1
                + (row as f64 + 0.5 - a.y as f64 - a.height as f64 / 2.0) * PX_ROW / self.zoom,
        )
    }

    /// Card rectangle in absolute cell coordinates: (c0, r0, c1, r1), end-exclusive.
    fn cell_rect(&self, n: &CNode) -> (i32, i32, i32, i32) {
        let (c0, r0) = self.to_screen(n.x, n.y);
        let (c1, r1) = self.to_screen(n.x + n.width, n.y + n.height);
        let (c0, r0) = (c0.round() as i32, r0.round() as i32);
        (
            c0,
            r0,
            (c1.round() as i32).max(c0 + 1),
            (r1.round() as i32).max(r0 + 1),
        )
    }

    fn fit_pending(&mut self) {
        // Rough centre before the first draw; `fit` refines it once the area is known.
        if let Some((x0, y0, x1, y1)) = self.bounds() {
            self.cam = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        }
    }

    fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        let mut it = self.doc.nodes.iter();
        let first = it.next()?;
        let mut b = (
            first.x,
            first.y,
            first.x + first.width,
            first.y + first.height,
        );
        for n in it {
            b.0 = b.0.min(n.x);
            b.1 = b.1.min(n.y);
            b.2 = b.2.max(n.x + n.width);
            b.3 = b.3.max(n.y + n.height);
        }
        Some(b)
    }

    fn fit(&mut self) {
        let Some((x0, y0, x1, y1)) = self.bounds() else {
            self.cam = (0.0, 0.0);
            self.zoom = 1.0;
            return;
        };
        let pad = 60.0;
        let zx = self.area.width as f64 * PX_COL / (x1 - x0 + pad * 2.0);
        let zy = self.area.height as f64 * PX_ROW / (y1 - y0 + pad * 2.0);
        self.zoom = zx.min(zy).clamp(0.15, 1.5);
        self.cam = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    }

    fn ensure_visible(&mut self, id: &str) {
        let Some(n) = self.node(id) else { return };
        let (c0, r0, c1, r1) = self.cell_rect(n);
        let a = self.area;
        let (cx, cy) = (n.x + n.width / 2.0, n.y + n.height / 2.0);
        if c0 < a.x as i32
            || r0 < a.y as i32
            || c1 > (a.x + a.width) as i32
            || r1 > (a.y + a.height) as i32
        {
            self.cam = (cx, cy);
        }
    }

    fn zoom_by(&mut self, factor: f64, anchor: Option<(f64, f64)>) {
        let old = self.zoom;
        self.zoom = (self.zoom * factor).clamp(0.1, 4.0);
        if let Some((wx, wy)) = anchor {
            let k = old / self.zoom;
            self.cam.0 = wx - (wx - self.cam.0) * k;
            self.cam.1 = wy - (wy - self.cam.1) * k;
        }
    }

    fn pan_cells(&mut self, dc: f64, dr: f64) {
        self.cam.0 += dc * PX_COL / self.zoom;
        self.cam.1 += dr * PX_ROW / self.zoom;
    }

    /// Topmost card under a cell. Groups only respond on their frame, like Obsidian.
    fn hit(&self, col: u16, row: u16) -> Option<(String, bool)> {
        let (c, r) = (col as i32, row as i32);
        let inside = |n: &CNode| {
            let (c0, r0, c1, r1) = self.cell_rect(n);
            (c >= c0 && c < c1 && r >= r0 && r < r1).then_some((c0, r0, c1, r1))
        };
        for n in self.draw_order().into_iter().rev() {
            if n.kind == "group" {
                continue;
            }
            if let Some((_, _, c1, r1)) = inside(n) {
                return Some((n.id.clone(), c == c1 - 1 && r == r1 - 1));
            }
        }
        for n in self.doc.nodes.iter().rev().filter(|n| n.kind == "group") {
            if let Some((c0, r0, c1, r1)) = inside(n) {
                let on_frame = r == r0 || r == r1 - 1 || c == c0 || c == c1 - 1;
                if on_frame {
                    return Some((n.id.clone(), c == c1 - 1 && r == r1 - 1));
                }
            }
        }
        None
    }

    /// Groups first, then other cards in file order, with the selection on top.
    fn draw_order(&self) -> Vec<&CNode> {
        let mut v: Vec<&CNode> = self
            .doc
            .nodes
            .iter()
            .filter(|n| n.kind == "group")
            .collect();
        v.extend(
            self.doc
                .nodes
                .iter()
                .filter(|n| n.kind != "group" && Some(&n.id) != self.sel.as_ref()),
        );
        if let Some(s) = self.sel.as_ref().and_then(|s| self.node(s))
            && s.kind != "group"
        {
            v.push(s);
        }
        v
    }

    fn select_dir(&mut self, dx: f64, dy: f64) {
        let Some(sid) = self.sel.clone() else {
            return self.select_first();
        };
        let Some(s) = self.node(&sid) else { return };
        let (sx, sy) = (s.x + s.width / 2.0, s.y + s.height / 2.0);
        let best = self
            .doc
            .nodes
            .iter()
            .filter(|n| n.id != sid)
            .filter_map(|n| {
                let (ox, oy) = (n.x + n.width / 2.0 - sx, n.y + n.height / 2.0 - sy);
                let along = ox * dx + oy * dy;
                if along <= 0.0 {
                    return None;
                }
                let perp = (ox * dy - oy * dx).abs();
                Some((n.id.clone(), along + perp * 2.0))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((id, _)) = best {
            self.sel = Some(id.clone());
            self.ensure_visible(&id);
        }
    }

    fn reading_order(&self) -> Vec<String> {
        let mut v: Vec<&CNode> = self.doc.nodes.iter().collect();
        v.sort_by(|a, b| {
            (a.y, a.x)
                .partial_cmp(&(b.y, b.x))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        v.into_iter().map(|n| n.id.clone()).collect()
    }

    fn select_first(&mut self) {
        if let Some(id) = self.reading_order().first().cloned() {
            self.sel = Some(id.clone());
            self.ensure_visible(&id);
        }
    }

    fn cycle(&mut self, back: bool) {
        let order = self.reading_order();
        if order.is_empty() {
            return;
        }
        let pos = self
            .sel
            .as_ref()
            .and_then(|s| order.iter().position(|x| x == s));
        let next = match (pos, back) {
            (Some(p), false) => (p + 1) % order.len(),
            (Some(p), true) => (p + order.len() - 1) % order.len(),
            (None, _) => 0,
        };
        self.sel = Some(order[next].clone());
        self.ensure_visible(&order[next]);
    }

    // ---------- editing operations ----------

    fn flash(&mut self, s: impl Into<String>) {
        self.msg = Some((s.into(), Instant::now()));
    }

    /// Place a new card: right of the selection, else in the middle of the view.
    fn spot_for_new(&self, w: f64, h: f64) -> (f64, f64) {
        match self.sel.as_ref().and_then(|s| self.node(s)) {
            Some(s) if s.kind != "group" => {
                let mut x = s.x + s.width + 80.0;
                let mut y = s.y;
                // Step down until there's a free spot.
                while self.doc.nodes.iter().any(|n| {
                    n.kind != "group"
                        && x < n.x + n.width
                        && x + w > n.x
                        && y < n.y + n.height
                        && y + h > n.y
                }) {
                    y += h + 40.0;
                    if y > s.y + 4000.0 {
                        x += w + 80.0;
                        y = s.y;
                    }
                }
                (x, y)
            }
            _ => (snap(self.cam.0 - w / 2.0), snap(self.cam.1 - h / 2.0)),
        }
    }

    fn add_node(&mut self, kind: &str, at: Option<(f64, f64)>, w: f64, h: f64) -> String {
        self.checkpoint();
        let (x, y) = at.unwrap_or_else(|| self.spot_for_new(w, h));
        let id = new_id();
        self.doc.nodes.push(CNode {
            id: id.clone(),
            kind: kind.to_string(),
            text: (kind == "text").then(String::new),
            file: None,
            url: None,
            label: (kind == "group").then(|| "Group".to_string()),
            x: snap(x),
            y: snap(y),
            width: w,
            height: h,
            color: None,
            extra: Map::new(),
        });
        id
    }

    fn add_edge(&mut self, from: &str, to: &str) {
        self.doc.edges.push(CEdge {
            id: new_id(),
            from_node: from.to_string(),
            from_side: None,
            from_end: None,
            to_node: to.to_string(),
            to_side: None,
            to_end: None,
            color: None,
            label: None,
            extra: Map::new(),
        });
    }

    fn new_text_card(&mut self, at: Option<(f64, f64)>, connect_from: Option<String>) {
        let id = self.add_node("text", at, 260.0, 100.0);
        if let Some(from) = connect_from {
            self.add_edge(&from, &id);
        }
        self.sel = Some(id.clone());
        self.ensure_visible(&id);
        self.mode = Mode::Edit {
            id,
            buf: String::new(),
            is_new: true,
        };
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.sel.take() else { return };
        self.checkpoint();
        self.doc.nodes.retain(|n| n.id != id);
        self.doc
            .edges
            .retain(|e| e.from_node != id && e.to_node != id);
        self.commit();
        self.flash("deleted");
    }

    fn toggle_edge(&mut self, from: &str, to: &str) {
        if from == to {
            return self.flash("pick a different card");
        }
        self.checkpoint();
        let before = self.doc.edges.len();
        self.doc.edges.retain(|e| {
            !((e.from_node == from && e.to_node == to) || (e.from_node == to && e.to_node == from))
        });
        if self.doc.edges.len() == before {
            self.add_edge(from, to);
            self.flash("connected");
        } else {
            self.flash("arrow removed");
        }
        self.commit();
    }

    fn set_color(&mut self, c: Option<&str>) {
        let Some(id) = self.sel.clone() else { return };
        self.checkpoint();
        if let Some(n) = self.node_mut(&id) {
            n.color = c.map(String::from);
        }
        self.commit();
    }

    fn move_selected(&mut self, dx: f64, dy: f64) {
        let Some(id) = self.sel.clone() else { return };
        self.checkpoint();
        let ids = self.group_members(&id);
        for n in self.doc.nodes.iter_mut().filter(|n| ids.contains(&n.id)) {
            n.x += dx;
            n.y += dy;
        }
        self.commit();
        self.ensure_visible(&id);
    }

    fn resize_selected(&mut self, dw: f64, dh: f64) {
        let Some(id) = self.sel.clone() else { return };
        self.checkpoint();
        if let Some(n) = self.node_mut(&id) {
            n.width = (n.width + dw).max(60.0);
            n.height = (n.height + dh).max(40.0);
        }
        self.commit();
    }

    /// A card id plus, for groups, every card fully inside it (they move together).
    fn group_members(&self, id: &str) -> Vec<String> {
        let mut ids = vec![id.to_string()];
        if let Some(g) = self.node(id).filter(|g| g.kind == "group") {
            for n in &self.doc.nodes {
                if n.id != g.id
                    && n.x >= g.x
                    && n.y >= g.y
                    && n.x + n.width <= g.x + g.width
                    && n.y + n.height <= g.y + g.height
                {
                    ids.push(n.id.clone());
                }
            }
        }
        ids
    }

    fn new_group(&mut self) {
        let (x, y, w, h) = match self.sel.as_ref().and_then(|s| self.node(s)) {
            Some(n) => (n.x - 40.0, n.y - 60.0, n.width + 80.0, n.height + 100.0),
            None => (self.cam.0 - 250.0, self.cam.1 - 150.0, 500.0, 300.0),
        };
        let id = self.add_node("group", Some((x, y)), snap(w), snap(h));
        // Put the group first so it renders beneath its contents in Obsidian too.
        if let Some(pos) = self.doc.nodes.iter().position(|n| n.id == id) {
            let g = self.doc.nodes.remove(pos);
            self.doc.nodes.insert(0, g);
        }
        self.sel = Some(id.clone());
        self.mode = Mode::Edit {
            id,
            buf: "Group".into(),
            is_new: false,
        };
    }

    fn start_edit(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(id) = self.sel.clone() else {
            return Ok(());
        };
        let Some(n) = self.node(&id) else {
            return Ok(());
        };
        match n.kind.as_str() {
            "text" => {
                let buf = n.text.clone().unwrap_or_default();
                self.mode = Mode::Edit {
                    id,
                    buf,
                    is_new: false,
                };
            }
            "group" => {
                let buf = n.label.clone().unwrap_or_default();
                self.mode = Mode::Edit {
                    id,
                    buf,
                    is_new: false,
                };
            }
            "link" => {
                let buf = n.url.clone().unwrap_or_default();
                self.mode = Mode::Edit {
                    id,
                    buf,
                    is_new: false,
                };
            }
            "file" => {
                let rel = n.file.clone().unwrap_or_default();
                let path = self.vault_root.join(&rel);
                if let Some(err) = term::edit_file(terminal, &path, &self.vault_root)? {
                    self.flash(err);
                }
                self.previews.remove(&rel);
            }
            _ => {}
        }
        Ok(())
    }

    /// Edit a text card's markdown in $EDITOR.
    fn edit_external(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some(id) = self.sel.clone() else {
            return Ok(());
        };
        let Some(n) = self.node(&id) else {
            return Ok(());
        };
        if n.kind != "text" {
            return self.start_edit(terminal);
        }
        let tmp = std::env::temp_dir().join(format!("cold-lava-card-{id}.md"));
        std::fs::write(&tmp, n.text.clone().unwrap_or_default())?;
        if let Some(err) = term::edit_file(terminal, &tmp, &self.vault_root)? {
            self.flash(err);
        } else if let Ok(text) = std::fs::read_to_string(&tmp) {
            let text = text.trim_end_matches('\n').to_string();
            self.checkpoint();
            if let Some(n) = self.node_mut(&id) {
                n.text = Some(text);
            }
            self.commit();
        }
        let _ = std::fs::remove_file(tmp);
        Ok(())
    }

    fn finish_edit(&mut self) {
        let Mode::Edit { id, buf, is_new } = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return;
        };
        if is_new && buf.trim().is_empty() {
            // An empty new card is discarded (undo the creation).
            if let Some(d) = self.undo.pop() {
                self.doc = d;
            }
            self.sel = None;
            self.save();
            return;
        }
        if !is_new {
            self.checkpoint();
        }
        if let Some(n) = self.node_mut(&id) {
            match n.kind.as_str() {
                "group" => n.label = Some(buf),
                "link" => n.url = Some(buf),
                _ => {
                    n.text = Some(buf);
                    grow_to_fit(n);
                }
            }
        }
        self.commit();
    }

    fn vault_notes(&mut self) -> &Vec<String> {
        if self.notes.is_none() {
            let root = self.vault_root.clone();
            let mut v: Vec<String> = WalkDir::new(&root)
                .into_iter()
                .filter_entry(|e| {
                    e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.')
                })
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .filter(|e| {
                    let ext = e
                        .path()
                        .extension()
                        .map(|x| x.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    matches!(
                        ext.as_str(),
                        "md" | "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "pdf"
                    )
                })
                .filter_map(|e| {
                    e.path()
                        .strip_prefix(&root)
                        .ok()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                })
                .collect();
            v.sort_by_key(|p| (p.matches('/').count(), p.to_lowercase()));
            self.notes = Some(v);
        }
        self.notes.as_ref().unwrap()
    }

    fn file_hits(&mut self, query: &str) -> Vec<String> {
        let q = crate::vault::search_key(query);
        self.vault_notes()
            .iter()
            .filter(|p| crate::vault::search_key(p).contains(&q))
            .take(200)
            .cloned()
            .collect()
    }

    fn add_file_card(&mut self, rel: String) {
        let id = self.add_node("file", None, 400.0, 400.0);
        if let Some(n) = self.node_mut(&id) {
            n.file = Some(rel);
        }
        self.commit();
        self.sel = Some(id.clone());
        self.ensure_visible(&id);
    }

    fn preview(&mut self, rel: &str) -> Vec<String> {
        if let Some(p) = self.previews.get(rel) {
            return p.clone();
        }
        let path = self.vault_root.join(rel);
        let lines = match std::fs::read_to_string(&path) {
            Ok(t) => {
                let mut lines: Vec<&str> = t.lines().collect();
                if lines.first() == Some(&"---")
                    && let Some(end) = lines.iter().skip(1).position(|l| *l == "---")
                {
                    lines.drain(..end + 2);
                }
                lines.into_iter().take(60).map(String::from).collect()
            }
            Err(_) if path.exists() => vec!["(attachment)".into()],
            Err(_) => vec!["(file not found)".into()],
        };
        self.previews.insert(rel.to_string(), lines.clone());
        lines
    }

    // ---------- input ----------

    fn on_key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
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
            Mode::Edit { buf, .. } => {
                match k.code {
                    KeyCode::Esc => self.finish_edit(),
                    KeyCode::Char('s') if ctrl => self.finish_edit(),
                    KeyCode::Enter => buf.push('\n'),
                    KeyCode::Tab => buf.push_str("    "),
                    KeyCode::Backspace => {
                        buf.pop();
                    }
                    KeyCode::Char(c) => buf.push(c),
                    _ => {}
                }
                return Ok(());
            }
            Mode::PickFile { query, pos } => {
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Up => *pos = pos.saturating_sub(1),
                    KeyCode::Down | KeyCode::Tab => *pos += 1,
                    KeyCode::Backspace => {
                        query.pop();
                        *pos = 0;
                    }
                    KeyCode::Char(c) => {
                        query.push(c);
                        *pos = 0;
                    }
                    KeyCode::Enter => {
                        let (q, p) = (query.clone(), *pos);
                        let hits = self.file_hits(&q);
                        self.mode = Mode::Normal;
                        if let Some(rel) = hits.get(p.min(hits.len().saturating_sub(1))).cloned() {
                            self.add_file_card(rel);
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::Connect { from } => {
                let from = from.clone();
                match k.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.sel = Some(from);
                    }
                    KeyCode::Enter | KeyCode::Char('c') => {
                        if let Some(to) = self.sel.clone() {
                            self.toggle_edge(&from, &to);
                        }
                        self.mode = Mode::Normal;
                    }
                    KeyCode::Left | KeyCode::Char('h') => self.select_dir(-1.0, 0.0),
                    KeyCode::Right | KeyCode::Char('l') => self.select_dir(1.0, 0.0),
                    KeyCode::Up | KeyCode::Char('k') => self.select_dir(0.0, -1.0),
                    KeyCode::Down | KeyCode::Char('j') => self.select_dir(0.0, 1.0),
                    KeyCode::Tab => self.cycle(false),
                    KeyCode::BackTab => self.cycle(true),
                    _ => {}
                }
                return Ok(());
            }
            Mode::Normal | Mode::Keys => {}
        }

        if let Some(a) = self.keys.get(k) {
            return self.run_action(a, terminal);
        }
        if let Some((nav, keys)) = &self.nav
            && let Some(a) = keys.get(k)
        {
            let (exit, to) = match a {
                HistoryAction::Back => (Exit::Back, &nav.back),
                HistoryAction::Forward => (Exit::Forward, &nav.forward),
            };
            if to.is_some() {
                self.request_quit();
                if self.quit {
                    self.exit = exit;
                }
            }
            return Ok(());
        }
        let has_sel = self.sel.is_some();
        match k.code {
            // Resize: Ctrl+arrows, or [ ] { }
            KeyCode::Left if ctrl => self.resize_selected(-20.0, 0.0),
            KeyCode::Right if ctrl => self.resize_selected(20.0, 0.0),
            KeyCode::Up if ctrl => self.resize_selected(0.0, -20.0),
            KeyCode::Down if ctrl => self.resize_selected(0.0, 20.0),
            KeyCode::Char('[') => self.resize_selected(-20.0, 0.0),
            KeyCode::Char(']') => self.resize_selected(20.0, 0.0),
            KeyCode::Char('{') => self.resize_selected(0.0, -20.0),
            KeyCode::Char('}') => self.resize_selected(0.0, 20.0),

            // Move the card (or pan when nothing is selected): Shift+arrows / HJKL
            KeyCode::Left if shift => self.move_or_pan(-20.0, 0.0),
            KeyCode::Right if shift => self.move_or_pan(20.0, 0.0),
            KeyCode::Up if shift => self.move_or_pan(0.0, -20.0),
            KeyCode::Down if shift => self.move_or_pan(0.0, 20.0),
            KeyCode::Char('H') => self.move_or_pan(-20.0, 0.0),
            KeyCode::Char('L') => self.move_or_pan(20.0, 0.0),
            KeyCode::Char('K') => self.move_or_pan(0.0, -20.0),
            KeyCode::Char('J') => self.move_or_pan(0.0, 20.0),

            // Select (or pan when nothing is selected): arrows / hjkl
            KeyCode::Left | KeyCode::Char('h') if has_sel => self.select_dir(-1.0, 0.0),
            KeyCode::Right | KeyCode::Char('l') if has_sel => self.select_dir(1.0, 0.0),
            KeyCode::Up | KeyCode::Char('k') if has_sel => self.select_dir(0.0, -1.0),
            KeyCode::Down | KeyCode::Char('j') if has_sel => self.select_dir(0.0, 1.0),
            KeyCode::Left | KeyCode::Char('h') => self.pan_cells(-6.0, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.pan_cells(6.0, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.pan_cells(0.0, -3.0),
            KeyCode::Down | KeyCode::Char('j') => self.pan_cells(0.0, 3.0),
            KeyCode::Tab => self.cycle(false),
            KeyCode::BackTab => self.cycle(true),
            _ => {}
        }
        Ok(())
    }

    /// The which-key popup's lines: the whiteboard's commands, and Back / Forward.
    fn key_hints(&self) -> Vec<KeyHint> {
        let show = |a| a != BoardAction::Keys;
        commands::screen_key_hints(commands::BOARD, &self.keys, show, self.nav.as_ref())
    }

    fn run_action(&mut self, a: BoardAction, terminal: &mut DefaultTerminal) -> Result<()> {
        match a {
            BoardAction::Quit => self.request_quit(),
            BoardAction::Help => self.mode = Mode::Help,
            BoardAction::Keys => self.mode = Mode::Keys,
            BoardAction::Escape => self.sel = None,
            BoardAction::Undo => self.undo(),
            BoardAction::Redo => self.redo(),
            BoardAction::ZoomIn => self.zoom_by(1.25, None),
            BoardAction::ZoomOut => self.zoom_by(0.8, None),
            BoardAction::Fit => self.fit(),
            BoardAction::NewCard => self.new_text_card(None, None),
            BoardAction::NewLinkedCard => match self.sel.clone() {
                Some(from) => self.new_text_card(None, Some(from)),
                None => self.new_text_card(None, None),
            },
            BoardAction::AddNote => {
                self.mode = Mode::PickFile {
                    query: String::new(),
                    pos: 0,
                }
            }
            BoardAction::NewGroup => self.new_group(),
            BoardAction::Connect => match self.sel.clone() {
                Some(from) => self.mode = Mode::Connect { from },
                None => self.flash("select a card first, then c to draw an arrow from it"),
            },
            BoardAction::Edit => self.start_edit(terminal)?,
            BoardAction::EditExternal => self.edit_external(terminal)?,
            BoardAction::OpenInObsidian => {
                let file = self
                    .sel
                    .as_ref()
                    .and_then(|s| self.node(s))
                    .and_then(|n| n.file.clone());
                match file {
                    Some(rel) => {
                        let ok = term::open_in_obsidian(&self.vault_name, &rel);
                        self.flash(if ok {
                            "opened in Obsidian"
                        } else {
                            "couldn't launch Obsidian"
                        });
                    }
                    None => {
                        let rel = self
                            .path
                            .strip_prefix(&self.vault_root)
                            .unwrap_or(&self.path)
                            .to_string_lossy()
                            .into_owned();
                        let ok = term::open_in_obsidian(&self.vault_name, &rel);
                        self.flash(if ok {
                            "opened board in Obsidian"
                        } else {
                            "couldn't launch Obsidian"
                        });
                    }
                }
            }
            BoardAction::Delete => self.delete_selected(),
            BoardAction::Colour(n) => self.set_color(Some(&n.to_string())),
            BoardAction::ColourClear => self.set_color(None),
            BoardAction::Reload => match std::fs::read(&self.path)
                .map_err(anyhow::Error::from)
                .and_then(|b| Ok((parse(&b, &self.path)?, b)))
            {
                Ok((d, bytes)) => {
                    self.checkpoint();
                    self.base = d.clone();
                    self.disk = bytes;
                    self.doc = d;
                    self.previews.clear();
                    self.notes = None;
                    self.flash("reloaded from disk");
                }
                Err(e) => self.flash(format!("reload failed: {e}")),
            },
        }
        Ok(())
    }

    fn move_or_pan(&mut self, dx: f64, dy: f64) {
        if self.sel.is_some() {
            self.move_selected(dx, dy);
        } else {
            self.pan_cells(dx / 20.0 * 6.0, dy / 20.0 * 3.0);
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if matches!(self.mode, Mode::Help | Mode::Keys | Mode::PickFile { .. }) {
            return;
        }
        if matches!(self.mode, Mode::Edit { .. }) {
            if let MouseEventKind::Down(_) = m.kind {
                self.finish_edit(); // clicking elsewhere commits, like Obsidian
            } else {
                return;
            }
        }
        let a = self.area;
        let in_area =
            m.column >= a.x && m.row >= a.y && m.column < a.x + a.width && m.row < a.y + a.height;
        let world = self.to_world(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollUp if in_area => self.zoom_by(1.15, Some(world)),
            MouseEventKind::ScrollDown if in_area => self.zoom_by(1.0 / 1.15, Some(world)),
            MouseEventKind::Down(MouseButton::Left) if in_area => {
                let double = self.last_click.is_some_and(|(t, c, r)| {
                    t.elapsed() < Duration::from_millis(400)
                        && c.abs_diff(m.column) <= 1
                        && r == m.row
                });
                self.last_click = Some((Instant::now(), m.column, m.row));
                let hit = self.hit(m.column, m.row);
                if let Mode::Connect { from } = &self.mode {
                    let from = from.clone();
                    self.mode = Mode::Normal;
                    if let Some((to, _)) = hit {
                        self.toggle_edge(&from, &to);
                        self.sel = Some(to);
                    }
                    return;
                }
                match hit {
                    Some((id, corner)) => {
                        self.sel = Some(id.clone());
                        if double {
                            let kind = self.node(&id).map(|n| n.kind.clone()).unwrap_or_default();
                            if kind != "file" {
                                let buf = self
                                    .node(&id)
                                    .and_then(|n| match kind.as_str() {
                                        "group" => n.label.clone(),
                                        "link" => n.url.clone(),
                                        _ => n.text.clone(),
                                    })
                                    .unwrap_or_default();
                                self.mode = Mode::Edit {
                                    id,
                                    buf,
                                    is_new: false,
                                };
                            }
                            return;
                        }
                        self.checkpoint();
                        if corner {
                            self.drag = Some(Drag::Resize { id, moved: false });
                        } else {
                            let offsets = self
                                .group_members(&id)
                                .into_iter()
                                .filter_map(|mid| self.node(&mid).map(|n| (mid, n.x, n.y)))
                                .collect();
                            self.drag = Some(Drag::Move {
                                offsets,
                                grab: world,
                                moved: false,
                            });
                        }
                    }
                    None => {
                        self.sel = None;
                        if double {
                            self.new_text_card(Some((world.0 - 130.0, world.1 - 50.0)), None);
                        } else {
                            self.drag = Some(Drag::Pan {
                                col: m.column,
                                row: m.row,
                            });
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match &mut self.drag {
                Some(Drag::Pan { col, row }) => {
                    let (dc, dr) = (*col as f64 - m.column as f64, *row as f64 - m.row as f64);
                    *col = m.column;
                    *row = m.row;
                    self.pan_cells(dc, dr);
                }
                Some(Drag::Move {
                    offsets,
                    grab,
                    moved,
                }) => {
                    let (dx, dy) = (snap(world.0 - grab.0), snap(world.1 - grab.1));
                    *moved = true;
                    let offsets = offsets.clone();
                    for (id, ox, oy) in offsets {
                        if let Some(n) = self.node_mut(&id) {
                            n.x = ox + dx;
                            n.y = oy + dy;
                        }
                    }
                }
                Some(Drag::Resize { id, moved }) => {
                    *moved = true;
                    let id = id.clone();
                    if let Some(n) = self.node_mut(&id) {
                        n.width = snap(world.0 - n.x).max(60.0);
                        n.height = snap(world.1 - n.y).max(40.0);
                    }
                }
                None => {}
            },
            MouseEventKind::Up(_) => match self.drag.take() {
                Some(Drag::Move { moved, .. }) | Some(Drag::Resize { moved, .. }) => {
                    if moved {
                        self.commit();
                    } else {
                        self.undo.pop(); // nothing changed: drop the snapshot
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    // ---------- drawing ----------

    fn draw(&mut self, f: &mut Frame) {
        let [main, status] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(f.area());
        let name = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(DIM))
            .title(Line::from(vec![
                Span::styled(
                    " whiteboard ",
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "{name} · {} cards · {} arrows ",
                        self.doc.nodes.len(),
                        self.doc.edges.len()
                    ),
                    Style::new().fg(MUTED),
                ),
            ]))
            .title_top(
                Line::from(Span::styled(
                    format!(" {:.0}% ", self.zoom * 100.0),
                    Style::new().fg(MUTED),
                ))
                .right_aligned(),
            );
        self.area = block.inner(main);
        f.render_widget(block, main);
        let area = self.area;

        self.draw_grid(f);
        let groups: Vec<CNode> = self
            .doc
            .nodes
            .iter()
            .filter(|n| n.kind == "group")
            .cloned()
            .collect();
        for g in &groups {
            self.draw_card(f, g, area);
        }
        self.draw_edges(f);
        let order: Vec<CNode> = self
            .draw_order()
            .into_iter()
            .filter(|n| n.kind != "group")
            .cloned()
            .collect();
        for n in &order {
            self.draw_card(f, n, area);
        }
        if self.doc.nodes.is_empty() {
            let hint = Paragraph::new(vec![
                Line::styled(
                    "empty board",
                    Style::new().fg(MUTED).add_modifier(Modifier::BOLD),
                ),
                Line::styled(
                    "n new card · f add a note · double-click anywhere · ? help",
                    Style::new().fg(DIM),
                ),
            ])
            .alignment(ratatui::layout::Alignment::Center);
            f.render_widget(
                hint,
                Rect::new(area.x, area.y + area.height / 2, area.width, 2),
            );
        }

        self.draw_status(f, status);
        match &self.mode {
            Mode::Help => draw_help(f, main, &self.keys, self.nav.as_ref().map(|(_, k)| k)),
            Mode::Keys => crate::whichkey::draw(f, main, &self.key_hints()),
            Mode::PickFile { query, pos } => {
                let (q, p) = (query.clone(), *pos);
                self.draw_picker(f, main, &q, p);
            }
            _ => {}
        }
    }

    fn draw_grid(&self, f: &mut Frame) {
        // A dot every 100 canvas px, if that is at least 4 cells apart.
        let step = 100.0;
        if step * self.zoom / PX_COL < 4.0 {
            return;
        }
        let a = self.area;
        let (wx0, wy0) = self.to_world(a.x, a.y);
        let (wx1, wy1) = self.to_world(a.x + a.width, a.y + a.height);
        let buf = f.buffer_mut();
        let mut y = (wy0 / step).floor() * step;
        while y <= wy1 {
            let mut x = (wx0 / step).floor() * step;
            while x <= wx1 {
                let (c, r) = self.to_screen(x, y);
                let (c, r) = (c.round() as i32, r.round() as i32);
                if c >= a.x as i32
                    && r >= a.y as i32
                    && c < (a.x + a.width) as i32
                    && r < (a.y + a.height) as i32
                {
                    let cell = &mut buf[(c as u16, r as u16)];
                    cell.set_char('·');
                    cell.set_fg(GRID);
                }
                x += step;
            }
            y += step;
        }
    }

    fn draw_edges(&self, f: &mut Frame) {
        let a = self.area;
        let hw = a.width as f64 / 2.0 * PX_COL / self.zoom;
        let hh = a.height as f64 / 2.0 * PX_ROW / self.zoom;
        let (x0, x1) = (self.cam.0 - hw, self.cam.0 + hw);
        let (y0, y1) = (self.cam.1 - hh, self.cam.1 + hh);
        let sel = self.sel.clone();
        let connect_from = match &self.mode {
            Mode::Connect { from } => Some(from.clone()),
            _ => None,
        };
        let mut lines: Vec<(f64, f64, f64, f64, Color, bool, bool)> = Vec::new();
        let mut labels: Vec<(f64, f64, String)> = Vec::new();
        for e in &self.doc.edges {
            let (Some(from), Some(to)) = (self.node(&e.from_node), self.node(&e.to_node)) else {
                continue;
            };
            let (fs, ts) = sides(from, to, e.from_side.as_deref(), e.to_side.as_deref());
            let (ax, ay) = side_point(from, fs);
            let (bx, by) = side_point(to, ts);
            let touches = sel
                .as_deref()
                .is_some_and(|s| s == e.from_node || s == e.to_node);
            let color = if touches {
                ACCENT
            } else {
                canvas_color(&e.color).unwrap_or(EDGE_COLOR)
            };
            let arrow_to = e.to_end.as_deref() != Some("none");
            let arrow_from = e.from_end.as_deref() == Some("arrow");
            lines.push((ax, ay, bx, by, color, arrow_to, arrow_from));
            if let Some(l) = e.label.as_ref().filter(|l| !l.is_empty()) {
                labels.push(((ax + bx) / 2.0, (ay + by) / 2.0, l.clone()));
            }
        }
        // Preview arrow while connecting.
        if let (Some(from), Some(to)) = (
            connect_from.as_deref().and_then(|i| self.node(i)),
            sel.as_deref().and_then(|i| self.node(i)),
        ) && from.id != to.id
        {
            let (fs, ts) = sides(from, to, None, None);
            let (ax, ay) = side_point(from, fs);
            let (bx, by) = side_point(to, ts);
            lines.push((ax, ay, bx, by, HIT, true, false));
        }
        let alen = 2.2 * PX_COL / self.zoom;
        let canvas = Canvas::default()
            .marker(Marker::Braille)
            .x_bounds([x0, x1])
            .y_bounds([-y1, -y0])
            .paint(move |ctx| {
                for &(ax, ay, bx, by, color, arrow_to, arrow_from) in &lines {
                    ctx.draw(&CLine::new(ax, -ay, bx, -by, color));
                    let mut head = |tx: f64, ty: f64, fx: f64, fy: f64| {
                        let ang = (fy - ty).atan2(fx - tx);
                        for da in [0.45f64, -0.45] {
                            let (hx, hy) =
                                (tx + alen * (ang + da).cos(), ty + alen * (ang + da).sin());
                            ctx.draw(&CLine::new(tx, -ty, hx, -hy, color));
                        }
                    };
                    if arrow_to {
                        head(bx, by, ax, ay);
                    }
                    if arrow_from {
                        head(ax, ay, bx, by);
                    }
                }
            });
        f.render_widget(canvas, a);
        for (x, y, text) in labels {
            let (c, r) = self.to_screen(x, y);
            let len = text.chars().count() as i32;
            let (c, r) = (c.round() as i32 - len / 2, r.round() as i32);
            put_str(f, a, c, r, &format!(" {text} "), Style::new().fg(MUTED));
        }
    }

    fn draw_card(&mut self, f: &mut Frame, n: &CNode, area: Rect) {
        let (c0, r0, c1, r1) = self.cell_rect(n);
        let (ax0, ay0) = (area.x as i32, area.y as i32);
        let (ax1, ay1) = ((area.x + area.width) as i32, (area.y + area.height) as i32);
        if c1 <= ax0 || r1 <= ay0 || c0 >= ax1 || r0 >= ay1 {
            return;
        }
        let (x0, y0, x1, y1) = (c0.max(ax0), r0.max(ay0), c1.min(ax1), r1.min(ay1));
        let rect = Rect::new(x0 as u16, y0 as u16, (x1 - x0) as u16, (y1 - y0) as u16);

        let selected = self.sel.as_deref() == Some(n.id.as_str());
        let editing = matches!(&self.mode, Mode::Edit { id, .. } if *id == n.id);
        let connecting_from = matches!(&self.mode, Mode::Connect { from } if *from == n.id);
        let base = canvas_color(&n.color).unwrap_or(CARD_BORDER);
        let border_color = if editing || connecting_from {
            HIT
        } else if selected {
            ACCENT
        } else {
            base
        };
        let is_group = n.kind == "group";

        // Too small to draw a box: a coloured blob.
        if c1 - c0 < 4 || r1 - r0 < 2 {
            let buf = f.buffer_mut();
            for y in rect.top()..rect.bottom() {
                for x in rect.left()..rect.right() {
                    let cell = &mut buf[(x, y)];
                    cell.set_char(if is_group { '░' } else { '█' });
                    cell.set_fg(border_color);
                }
            }
            return;
        }

        let mut borders = Borders::NONE;
        if c0 >= ax0 {
            borders |= Borders::LEFT;
        }
        if c1 <= ax1 {
            borders |= Borders::RIGHT;
        }
        if r0 >= ay0 {
            borders |= Borders::TOP;
        }
        if r1 <= ay1 {
            borders |= Borders::BOTTOM;
        }
        let title_style = Style::new()
            .fg(if selected { ACCENT } else { NOTE })
            .add_modifier(Modifier::BOLD);
        let mut block = Block::new()
            .borders(borders)
            .border_type(if selected || editing {
                BorderType::Thick
            } else if is_group {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(Style::new().fg(border_color));

        let edit_buf = match &self.mode {
            Mode::Edit { id, buf, .. } if *id == n.id => Some(buf.clone()),
            _ => None,
        };
        let body: Text = match n.kind.as_str() {
            "group" => {
                let label = edit_buf
                    .map(|b| format!("{b}▏"))
                    .or_else(|| n.label.clone())
                    .unwrap_or_default();
                if r0 >= ay0 {
                    block = block.title(Span::styled(format!(" {label} "), title_style));
                }
                Text::default()
            }
            "file" => {
                let rel = n.file.clone().unwrap_or_default();
                let name = rel
                    .rsplit('/')
                    .next()
                    .unwrap_or(&rel)
                    .trim_end_matches(".md")
                    .to_string();
                if r0 >= ay0 {
                    block = block.title(Span::styled(format!(" ▤ {name} "), title_style));
                }
                let lines = self.preview(&rel);
                markdown_text(&lines.join("\n"))
            }
            "link" => {
                let url = edit_buf
                    .map(|b| format!("{b}▏"))
                    .or_else(|| n.url.clone())
                    .unwrap_or_default();
                if r0 >= ay0 {
                    block = block.title(Span::styled(" ↗ link ", title_style));
                }
                Text::styled(
                    url,
                    Style::new()
                        .fg(Color::Rgb(120, 170, 250))
                        .add_modifier(Modifier::UNDERLINED),
                )
            }
            _ => match edit_buf {
                Some(b) => Text::styled(format!("{b}▏"), Style::new().fg(NOTE)),
                None => markdown_text(n.text.as_deref().unwrap_or("")),
            },
        };

        if !is_group {
            f.render_widget(Clear, rect);
        }
        let inner_w = (c1 - c0 - 2).max(1) as usize;
        let inner_h = (r1 - r0 - 2).max(1) as usize;
        // Scroll: hidden rows above the view, and while editing keep the cursor in sight.
        let mut scroll = (ay0 - (r0 + 1)).max(0) as usize;
        if editing {
            let rows: usize = body
                .lines
                .iter()
                .map(|l| l.width().div_ceil(inner_w).max(1))
                .sum();
            scroll = scroll.max(rows.saturating_sub(inner_h));
        }
        f.render_widget(
            Paragraph::new(body)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((scroll as u16, 0)),
            rect,
        );
        // Resize handle.
        if selected && !is_group && c1 <= ax1 && r1 <= ay1 {
            put_str(f, area, c1 - 1, r1 - 1, "◢", Style::new().fg(ACCENT));
        }
    }

    fn draw_picker(&mut self, f: &mut Frame, area: Rect, query: &str, pos: usize) {
        let hits = self.file_hits(query);
        let h = 16.min(area.height.saturating_sub(2));
        let r = centered(area, 70, h);
        let rows = h.saturating_sub(4) as usize;
        let pos = pos.min(hits.len().saturating_sub(1));
        let start = pos.saturating_sub(rows.saturating_sub(1));
        let mut lines = vec![
            Line::from(vec![
                Span::styled(" ▸ ", Style::new().fg(HIT)),
                Span::styled(format!("{query}▏"), Style::new().fg(NOTE)),
            ]),
            Line::raw(""),
        ];
        for (i, p) in hits.iter().enumerate().skip(start).take(rows) {
            let st = if i == pos {
                Style::new().fg(Color::Black).bg(ACCENT)
            } else {
                Style::new().fg(NOTE)
            };
            lines.push(Line::styled(format!("  {p}  "), st));
        }
        if hits.is_empty() {
            lines.push(Line::styled("  no matching files", Style::new().fg(MUTED)));
        }
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(lines).block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(ACCENT))
                    .title(" add a note card · type to filter · ⏎ add · Esc cancel "),
            ),
            r,
        );
    }

    fn draw_status(&mut self, f: &mut Frame, area: Rect) {
        let mut info = match self.sel.as_ref().and_then(|s| self.node(s)) {
            Some(n) => {
                let links = self
                    .doc
                    .edges
                    .iter()
                    .filter(|e| e.from_node == n.id || e.to_node == n.id)
                    .count();
                let what = match n.kind.as_str() {
                    "file" => format!("note card · {}", n.file.clone().unwrap_or_default()),
                    "group" => format!("group · {}", n.label.clone().unwrap_or_default()),
                    "link" => format!("link card · {}", n.url.clone().unwrap_or_default()),
                    _ => "text card".to_string(),
                };
                Line::from(vec![
                    Span::styled(
                        " ▣ ",
                        Style::new().fg(canvas_color(&n.color).unwrap_or(ACCENT)),
                    ),
                    Span::styled(what, Style::new().fg(NOTE).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!("  {}×{}  {} arrows", n.width as i64, n.height as i64, links),
                        Style::new().fg(MUTED),
                    ),
                ])
            }
            None => Line::styled(
                format!(
                    " {}",
                    self.path
                        .strip_prefix(&self.vault_root)
                        .unwrap_or(&self.path)
                        .display()
                ),
                Style::new().fg(MUTED),
            ),
        };
        if self
            .msg
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3))
        {
            self.msg = None;
        }
        if let Some((nav, keys)) = &self.nav {
            let first = |id| keys.keys(id).first();
            let hint = nav.hint(first("history.back"), first("history.forward"));
            if !hint.is_empty() {
                info.spans
                    .push(Span::styled(format!("   {hint}"), Style::new().fg(MUTED)));
            }
        }
        let second = match (&self.mode, &self.msg) {
            (Mode::Edit { .. }, _) => Line::styled(
                " editing · type text · ⏎ new line · Esc done (saves)",
                Style::new().fg(HIT),
            ),
            (Mode::Connect { .. }, _) => Line::styled(
                " connect · pick the target (arrows/Tab/click) · ⏎ draw arrow (again removes it) · Esc cancel",
                Style::new().fg(HIT),
            ),
            (_, Some((m, _))) => Line::styled(format!(" {m}"), Style::new().fg(HIT)),
            _ => {
                let k = |id| self.keys.first_label(id);
                Line::styled(
                    format!(
                        " {} new  {} connect  {} edit  ⇧+arrows move  {} undo  {} keys  {} help  {} back",
                        k("board.new-card"),
                        k("board.connect"),
                        k("board.edit"),
                        k("board.undo"),
                        k("board.keys"),
                        k("board.help"),
                        k("board.quit"),
                    ),
                    Style::new().fg(DIM),
                )
            }
        };
        f.render_widget(Paragraph::new(vec![info, second]), area);
    }
}

// ---------- helpers ----------

fn parse(bytes: &[u8], path: &Path) -> Result<Doc> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Doc::default());
    }
    serde_json::from_slice(bytes)
        .with_context(|| format!("{} is not a valid canvas file", path.display()))
}

// ---------- three-way merge ----------
//
// `base` is the version both sides started from, `ours` is cold-lava's current board and
// `theirs` is what is now on disk. Cards and arrows are matched by id and merged field by
// field, so Obsidian moving a card while you edit its text keeps both changes. When the same
// field changed on both sides, ours wins (and it is counted as a conflict). An item edited on
// one side and deleted on the other is kept: losing an edit is worse than keeping a card.

fn merge3(base: &Value, ours: &Value, theirs: &Value) -> (Value, usize) {
    let mut conflicts = 0;
    let empty = Map::new();
    let b = base.as_object().unwrap_or(&empty);
    let o = ours.as_object().unwrap_or(&empty);
    let t = theirs.as_object().unwrap_or(&empty);
    let mut out = merge_fields(b, o, t, &["nodes", "edges"], &mut conflicts);
    for key in ["nodes", "edges"] {
        let list = merge_lists(
            b.get(key).unwrap_or(&Value::Null),
            o.get(key).unwrap_or(&Value::Null),
            t.get(key).unwrap_or(&Value::Null),
            &mut conflicts,
        );
        out.insert(key.to_string(), Value::Array(list));
    }
    // Drop arrows whose cards no longer exist.
    let ids: HashSet<String> = out["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| n.get("id")?.as_str().map(String::from))
        .collect();
    if let Some(edges) = out.get_mut("edges").and_then(Value::as_array_mut) {
        edges.retain(|e| {
            let has = |k: &str| {
                e.get(k)
                    .and_then(Value::as_str)
                    .is_some_and(|id| ids.contains(id))
            };
            has("fromNode") && has("toNode")
        });
    }
    (Value::Object(out), conflicts)
}

/// Key-by-key merge of two objects against their common base.
fn merge_fields(
    b: &Map<String, Value>,
    o: &Map<String, Value>,
    t: &Map<String, Value>,
    skip: &[&str],
    conflicts: &mut usize,
) -> Map<String, Value> {
    let mut keys: Vec<&String> = o.keys().collect();
    keys.extend(t.keys().filter(|k| !o.contains_key(*k)));
    keys.extend(
        b.keys()
            .filter(|k| !o.contains_key(*k) && !t.contains_key(*k)),
    );
    let mut out = Map::new();
    for k in keys {
        if skip.contains(&k.as_str()) {
            continue;
        }
        let (bv, ov, tv) = (b.get(k), o.get(k), t.get(k));
        let v = if ov == bv {
            tv
        } else if tv == bv || tv == ov {
            ov
        } else {
            *conflicts += 1;
            ov
        };
        if let Some(v) = v {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn merge_lists(base: &Value, ours: &Value, theirs: &Value, conflicts: &mut usize) -> Vec<Value> {
    let by_id = |v: &Value| -> Vec<(String, Value)> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| Some((x.get("id")?.as_str()?.to_string(), x.clone())))
            .collect()
    };
    let (b, o, t) = (by_id(base), by_id(ours), by_id(theirs));
    let find = |list: &[(String, Value)], id: &str| {
        list.iter().find(|(i, _)| i == id).map(|(_, v)| v.clone())
    };
    // Order: ours first (keeps our stacking), then items only the other side has, in its order.
    let mut ids: Vec<String> = o.iter().map(|(i, _)| i.clone()).collect();
    ids.extend(
        t.iter()
            .map(|(i, _)| i.clone())
            .filter(|i| !o.iter().any(|(j, _)| j == i)),
    );
    let only_base: Vec<String> = b
        .iter()
        .map(|(i, _)| i.clone())
        .filter(|i| !ids.contains(i))
        .collect();
    ids.extend(only_base);
    let mut out = Vec::new();
    for id in ids {
        let (bv, ov, tv) = (find(&b, &id), find(&o, &id), find(&t, &id));
        let merged = match (bv, ov, tv) {
            // Present on both sides now.
            (Some(b), Some(o), Some(t)) => match (b.as_object(), o.as_object(), t.as_object()) {
                (Some(bm), Some(om), Some(tm)) => {
                    Some(Value::Object(merge_fields(bm, om, tm, &[], conflicts)))
                }
                _ => Some(if o == b { t } else { o }),
            },
            (None, Some(o), Some(t)) => {
                if o != t {
                    *conflicts += 1;
                }
                Some(o)
            }
            // Deleted on one side: delete unless the other side changed it.
            (Some(b), Some(o), None) => (o != b).then_some(o),
            (Some(b), None, Some(t)) => (t != b).then_some(t),
            // New on one side only.
            (None, Some(o), None) => Some(o),
            (None, None, Some(t)) => Some(t),
            (_, None, None) => None,
        };
        out.extend(merged);
    }
    out
}

fn snap(v: f64) -> f64 {
    (v / GRID_SNAP).round() * GRID_SNAP
}

/// Make a text card tall enough for its content (never shrinks it).
fn grow_to_fit(n: &mut CNode) {
    let cols = ((n.width / PX_COL) as usize).saturating_sub(2).max(1);
    let rows: usize = n
        .text
        .as_deref()
        .unwrap_or("")
        .lines()
        .map(|l| l.chars().count().div_ceil(cols).max(1))
        .sum();
    let need = (rows as f64 + 2.0) * PX_ROW;
    if need > n.height {
        n.height = snap(need + 10.0);
    }
}

fn sides<'a>(
    from: &CNode,
    to: &CNode,
    fs: Option<&'a str>,
    ts: Option<&'a str>,
) -> (&'a str, &'a str) {
    let dx = (to.x + to.width / 2.0) - (from.x + from.width / 2.0);
    let dy = (to.y + to.height / 2.0) - (from.y + from.height / 2.0);
    let horizontal =
        dx.abs() / (from.width + to.width).max(1.0) > dy.abs() / (from.height + to.height).max(1.0);
    let (auto_f, auto_t) = match (horizontal, dx > 0.0, dy > 0.0) {
        (true, true, _) => ("right", "left"),
        (true, false, _) => ("left", "right"),
        (false, _, true) => ("bottom", "top"),
        (false, _, false) => ("top", "bottom"),
    };
    (fs.unwrap_or(auto_f), ts.unwrap_or(auto_t))
}

fn side_point(n: &CNode, side: &str) -> (f64, f64) {
    match side {
        "top" => (n.x + n.width / 2.0, n.y),
        "bottom" => (n.x + n.width / 2.0, n.y + n.height),
        "left" => (n.x, n.y + n.height / 2.0),
        _ => (n.x + n.width, n.y + n.height / 2.0),
    }
}

/// Light markdown styling: headings, lists, checkboxes, quotes, code, plus inline
/// **bold**, *italic*, `code`, [[wikilinks|aliases]] and [links](url).
pub fn markdown_text(src: &str) -> Text<'static> {
    let note = Style::new().fg(NOTE);
    let mut lines = Vec::new();
    let mut in_code = false;
    for raw in src.lines() {
        let t = raw.trim_start();
        if t.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        let line = if in_code {
            Line::styled(raw.to_string(), Style::new().fg(CODE))
        } else if let Some(h) = t
            .strip_prefix('#')
            .filter(|_| t.trim_start_matches('#').starts_with(' '))
        {
            let heading = Style::new()
                .fg(Color::Rgb(235, 235, 245))
                .add_modifier(Modifier::BOLD);
            Line::from(inline(h.trim_start_matches('#').trim(), heading))
        } else if let Some(rest) = t.strip_prefix("- [ ] ") {
            prefixed(
                Span::styled("☐ ", Style::new().fg(MUTED)),
                inline(rest, note),
            )
        } else if let Some(rest) = t
            .strip_prefix("- [x] ")
            .or_else(|| t.strip_prefix("- [X] "))
        {
            let done = Style::new().fg(MUTED).add_modifier(Modifier::CROSSED_OUT);
            prefixed(
                Span::styled("☑ ", Style::new().fg(ACCENT)),
                inline(rest, done),
            )
        } else if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            let indent = " ".repeat(raw.len() - t.len());
            prefixed(
                Span::styled(format!("{indent}• "), Style::new().fg(ACCENT_SOFT)),
                inline(rest, note),
            )
        } else if let Some(rest) = t.strip_prefix("> ") {
            let quote = Style::new().fg(MUTED).add_modifier(Modifier::ITALIC);
            prefixed(
                Span::styled("▎ ", Style::new().fg(DIM)),
                inline(rest, quote),
            )
        } else {
            Line::from(inline(raw, note))
        };
        lines.push(line);
    }
    Text::from(lines)
}

const CODE: Color = Color::Rgb(160, 200, 160);
const LINK: Color = Color::Rgb(150, 180, 250);

fn prefixed(first: Span<'static>, rest: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![first];
    spans.extend(rest);
    Line::from(spans)
}

/// Inline markdown for one line of text.
fn inline(s: &str, base: Style) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut plain = String::new();
    let mut bold = false;
    let mut italic = false;
    let style = |bold: bool, italic: bool| {
        let mut st = base;
        if bold {
            st = st.add_modifier(Modifier::BOLD);
        }
        if italic {
            st = st.add_modifier(Modifier::ITALIC);
        }
        st
    };
    let mut i = 0;
    let b = s.as_bytes();
    while i < s.len() {
        let rest = &s[i..];
        // Each marker below is ASCII, so byte offsets stay on char boundaries.
        let flush = |plain: &mut String, out: &mut Vec<Span<'static>>, bold: bool, italic: bool| {
            if !plain.is_empty() {
                out.push(Span::styled(std::mem::take(plain), style(bold, italic)));
            }
        };
        if let Some(code) = rest.strip_prefix('`') {
            if let Some(end) = code.find('`') {
                flush(&mut plain, &mut out, bold, italic);
                out.push(Span::styled(code[..end].to_string(), Style::new().fg(CODE)));
                i += end + 2;
                continue;
            }
        } else if rest.starts_with("[[") {
            if let Some(end) = rest.find("]]") {
                flush(&mut plain, &mut out, bold, italic);
                let inner = &rest[2..end];
                let shown = inner.split_once('|').map(|(_, a)| a).unwrap_or(inner);
                let shown = shown.split('#').next().unwrap_or(shown);
                out.push(Span::styled(
                    shown.to_string(),
                    style(bold, italic).fg(LINK),
                ));
                i += end + 2;
                continue;
            }
        } else if rest.starts_with('[') {
            if let Some(close) = rest.find("](")
                && let Some(end) = rest[close..].find(')')
            {
                flush(&mut plain, &mut out, bold, italic);
                let text = &rest[1..close];
                out.push(Span::styled(
                    text.to_string(),
                    style(bold, italic)
                        .fg(LINK)
                        .add_modifier(Modifier::UNDERLINED),
                ));
                i += close + end + 1;
                continue;
            }
        } else if rest.starts_with("**") || rest.starts_with("__") {
            flush(&mut plain, &mut out, bold, italic);
            bold = !bold;
            i += 2;
            continue;
        } else if (b[i] == b'*' || b[i] == b'_')
            && (italic || rest[1..].contains(b[i] as char))
            // `_` inside words (snake_case) is not emphasis.
            && !(b[i] == b'_' && i > 0 && b[i - 1].is_ascii_alphanumeric())
        {
            flush(&mut plain, &mut out, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }
        let ch = rest.chars().next().unwrap();
        plain.push(ch);
        i += ch.len_utf8();
    }
    if !plain.is_empty() {
        out.push(Span::styled(plain, style(bold, italic)));
    }
    out
}

fn put_str(f: &mut Frame, area: Rect, col: i32, row: i32, s: &str, style: Style) {
    if row < area.y as i32 || row >= (area.y + area.height) as i32 {
        return;
    }
    let buf = f.buffer_mut();
    for (k, ch) in s.chars().enumerate() {
        let c = col + k as i32;
        if c >= area.x as i32 && c < (area.x + area.width) as i32 {
            let cell = &mut buf[(c as u16, row as u16)];
            cell.set_char(ch);
            cell.set_style(style);
        }
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn draw_help(
    f: &mut Frame,
    area: Rect,
    keys: &Keymap<BoardAction>,
    history: Option<&Keymap<HistoryAction>>,
) {
    let fixed: &[(&str, &str)] = &[
        (
            "↑↓←→ / hjkl, Tab",
            "select cards (pan when nothing is selected)",
        ),
        (
            "Shift+arrows / HJKL",
            "move the card (groups carry their contents)",
        ),
        ("Ctrl+arrows, [ ] { }", "resize (or drag the ◢ corner)"),
        ("drag · scroll", "move cards, pan · zoom at the cursor"),
    ];
    let commands = commands::BOARD.iter().map(|c| (keys.label(c.id), c.title));
    let rows: Vec<(String, &str)> = fixed
        .iter()
        .map(|&(k, d)| (k.to_string(), d))
        .chain(commands)
        .chain(history.map(commands::history_help).unwrap_or_default())
        .collect();
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:>25}  "), Style::new().fg(ACCENT)),
                Span::styled(*d, Style::new().fg(NOTE)),
            ])
        })
        .collect();
    let r = centered(area, 86, rows.len() as u16 + 3);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(ACCENT))
                .title(
                    " whiteboard keys · saved automatically as Obsidian .canvas · any key closes ",
                ),
        ),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn card(id: &str, x: i64, text: &str) -> Value {
        json!({"id": id, "type": "text", "text": text, "x": x, "y": 0, "width": 250, "height": 60})
    }

    #[test]
    fn never_saves_over_a_file_it_could_not_read() {
        let dir = std::env::temp_dir().join(format!("cold-lava-board-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("B.canvas");
        let canvas = |nodes: Vec<Value>| json!({"nodes": nodes, "edges": []}).to_string();
        std::fs::write(&path, canvas(vec![card("a", 0, "A")])).unwrap();
        let mut b = Board::open(path.clone(), &dir).unwrap();

        // Our edit, while someone else is halfway through writing the file.
        let ours = canvas(vec![card("a", 0, "A"), card("ours", 300, "Ours")]);
        b.doc = parse(ours.as_bytes(), &path).unwrap();
        std::fs::write(&path, "{\"nodes\": [").unwrap();
        b.save();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"nodes\": [");
        assert!(b.unsaved);

        // Quitting now warns once instead of losing the edit.
        b.request_quit();
        assert!(!b.quit);

        // Once the file is whole again, both sides end up in it.
        let theirs = canvas(vec![card("a", 0, "A"), card("theirs", 600, "Theirs")]);
        std::fs::write(&path, theirs).unwrap();
        b.save();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(ids(&saved, "nodes"), ["a", "ours", "theirs"]);
        assert!(!b.unsaved);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn ids(v: &Value, key: &str) -> Vec<String> {
        v[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn merge_keeps_both_sides_field_by_field() {
        let base = json!({"nodes": [card("a", 0, "hi"), card("b", 300, "b")], "edges": []});
        let mut ours = base.clone();
        ours["nodes"][0]["text"] = json!("edited here"); // we edit a's text
        let mut theirs = base.clone();
        theirs["nodes"][0]["x"] = json!(500); // Obsidian moves a
        theirs["nodes"][1]["color"] = json!("4"); // and colours b
        let (m, conflicts) = merge3(&base, &ours, &theirs);
        assert_eq!(conflicts, 0);
        assert_eq!(m["nodes"][0]["text"], "edited here");
        assert_eq!(m["nodes"][0]["x"], 500);
        assert_eq!(m["nodes"][1]["color"], "4");
    }

    #[test]
    fn merge_additions_deletions_and_conflicts() {
        let base = json!({"nodes": [card("a", 0, "a"), card("b", 0, "b"), card("c", 0, "c")],
                          "edges": [{"id": "e", "fromNode": "a", "toNode": "c"}]});
        let mut ours = base.clone();
        ours["nodes"]
            .as_array_mut()
            .unwrap()
            .push(card("mine", 0, "new here"));
        ours["nodes"][1]["text"] = json!("ours wins");
        let mut theirs = base.clone();
        theirs["nodes"].as_array_mut().unwrap().remove(2); // Obsidian deletes c (and so edge e)
        theirs["nodes"]
            .as_array_mut()
            .unwrap()
            .push(card("obs", 0, "new there"));
        theirs["nodes"][1]["text"] = json!("theirs loses");
        let (m, conflicts) = merge3(&base, &ours, &theirs);
        assert_eq!(ids(&m, "nodes"), ["a", "b", "mine", "obs"]);
        assert_eq!(m["nodes"][1]["text"], "ours wins");
        assert_eq!(conflicts, 1);
        assert!(
            ids(&m, "edges").is_empty(),
            "edge to a deleted card is dropped"
        );
    }

    #[test]
    fn merge_keeps_edited_card_deleted_elsewhere() {
        let base = json!({"nodes": [card("a", 0, "a")], "edges": []});
        let mut ours = base.clone();
        ours["nodes"][0]["text"] = json!("still editing");
        let theirs = json!({"nodes": [], "edges": []});
        let (m, _) = merge3(&base, &ours, &theirs);
        assert_eq!(ids(&m, "nodes"), ["a"]);
        // ...but an untouched card deleted elsewhere goes away.
        let (m, _) = merge3(&base, &base, &theirs);
        assert!(ids(&m, "nodes").is_empty());
    }

    #[test]
    fn merge_top_level_fields() {
        let base = json!({"nodes": [], "edges": [], "plugin": 1});
        let theirs = json!({"nodes": [], "edges": [], "plugin": 2, "added": true});
        let (m, _) = merge3(&base, &base, &theirs);
        assert_eq!(m["plugin"], 2);
        assert_eq!(m["added"], true);
    }

    #[test]
    fn canvas_round_trip_preserves_unknown_fields_and_integers() {
        let src = r##"{"nodes":[{"id":"a","type":"text","text":"x","x":10.4,"y":-20,"width":250,"height":60,"color":"#ff0000","futureField":{"k":[1,2]}}],
                      "edges":[{"id":"e","fromNode":"a","toNode":"a","toEnd":"none","styleAttributes":{}}],"meta":"keep"}"##;
        let doc = parse(src.as_bytes(), Path::new("t.canvas")).unwrap();
        let out = serde_json::to_value(&doc).unwrap();
        assert_eq!(out["nodes"][0]["x"], 10); // coordinates are written as integers
        assert_eq!(out["nodes"][0]["futureField"]["k"][1], 2);
        assert_eq!(out["edges"][0]["styleAttributes"], json!({}));
        assert_eq!(out["meta"], "keep");
        assert!(
            out["nodes"][0].get("file").is_none(),
            "absent optional fields stay absent"
        );
        assert_eq!(
            canvas_color(&Some("#ff0000".into())),
            Some(Color::Rgb(255, 0, 0))
        );
    }

    #[test]
    fn inline_markdown() {
        let text = |spans: Vec<Span>| {
            spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<Vec<_>>()
        };
        let base = Style::new();
        assert_eq!(
            text(inline("a **b** *c* `d` [[e|E]] [[f#g]] [h](u)", base)),
            ["a ", "b", " ", "c", " ", "d", " ", "E", " ", "f", " ", "h"]
        );
        let spans = inline("**bold**", base);
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        // Lone markers and snake_case stay as text.
        assert_eq!(
            text(inline("2 * 3 and snake_case_name", base)).concat(),
            "2 * 3 and snake_case_name"
        );
        assert_eq!(text(inline("Hawaiʻi `x`", base)), ["Hawaiʻi ", "x"]);
    }

    #[test]
    fn empty_file_is_an_empty_board() {
        let d = parse(b"  \n", Path::new("t.canvas")).unwrap();
        assert!(d.nodes.is_empty() && d.edges.is_empty());
        assert!(parse(b"{not json", Path::new("t.canvas")).is_err());
    }

    #[test]
    fn arrows_attach_to_facing_sides() {
        let n = |x: f64, y: f64| CNode {
            id: String::new(),
            kind: "text".into(),
            text: None,
            file: None,
            url: None,
            label: None,
            x,
            y,
            width: 100.0,
            height: 50.0,
            color: None,
            extra: Map::new(),
        };
        assert_eq!(
            sides(&n(0.0, 0.0), &n(400.0, 0.0), None, None),
            ("right", "left")
        );
        assert_eq!(
            sides(&n(0.0, 0.0), &n(0.0, 400.0), None, None),
            ("bottom", "top")
        );
        assert_eq!(
            sides(&n(0.0, 0.0), &n(400.0, 0.0), Some("top"), None),
            ("top", "left")
        );
    }
}
