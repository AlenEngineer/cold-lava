//! A whiteboard for the terminal that reads and writes Obsidian Canvas files (JSON Canvas,
//! https://jsoncanvas.org). Boards made here open in Obsidian, and vice versa.

use crate::app::{ACCENT, ACCENT_SOFT, DIM, HIT, MUTED, NOTE};
use crate::commands::{self, BoardAction, HistoryAction, Key, KeyHint, Keymap};
use crate::history::{Exit, Nav};
use crate::looks;
use crate::term;
use crate::types::Colour;
use anyhow::{Context as _, Result};
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
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

mod live;
mod placed;

use crate::act::{self, Acted, Answered, Done, Gesture};
use crate::bind::{Binding, Source};
use crate::core::{Core, Event as CoreEvent};
use crate::ext::{Effect, ExtId, Registry};
use crate::gateway::News;
use cold_lava_canvas::{Cell as GridCell, Handle, Point, Style as Token, Viewport, draw as ink};
use live::Live;

// The JSON Canvas model (unknown fields are preserved) is the engine's, shared with the
// canvas owner.
use crate::json_canvas::{CEdge, CNode, Doc};

/// How long a message stays in the status line.
const MESSAGE_SHOWS: Duration = Duration::from_secs(3);

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
        "1" => looks::colour(Colour::Red),
        "2" => looks::colour(Colour::Orange),
        "3" => looks::colour(Colour::Yellow),
        "4" => looks::colour(Colour::Green),
        "5" => looks::colour(Colour::Cyan),
        "6" => looks::colour(Colour::Purple),
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
    /// Picking a shape to place.
    Palette {
        query: String,
        pos: usize,
    },
    /// Binding a shape's inputs, each a source in the short form (`type=volcano tag=x`,
    /// `note:…`, `items:…`, `text:…`): a new shape when `node` is `None`.
    Bind {
        node: Option<String>,
        shape: String,
        fields: Vec<(String, String)>,
        pos: usize,
        error: Option<String>,
    },
    /// The selected shape's actions.
    Actions {
        id: String,
        pos: usize,
    },
    /// Which shape the shape editor opens on: a new one or one of the workspace's.
    PickShape {
        query: String,
        pos: usize,
    },
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
    /// Stretching a card from its corner, or a shape from one of its corners.
    Resize {
        id: String,
        handle: Handle,
        moved: bool,
    },
    /// Turning a shape by its grip.
    Turn {
        id: String,
        moved: bool,
    },
    /// A member dragged out of a region of a shape that takes drops from it.
    Member {
        id: String,
        from: String,
        item: String,
        moved: bool,
    },
}

/// What a click grabbed on a card or shape.
#[derive(Clone, Copy, PartialEq)]
enum Grab {
    Body,
    Corner(Handle),
    Turn,
}

/// An open whiteboard. The workspace's core and the extensions stay the menu's, handed to each
/// call that needs them: placed shapes' bindings follow the core.
pub struct Board {
    live: Live,
    /// The shape editor (`S`), a layer over the board while it is open: it gets the keys and
    /// the mouse and is drawn instead, and the board under it keeps being ticked.
    editor: Option<Box<crate::shape_editor::Editor>>,
    /// Extensions asked about their shapes. On its own the board polls them; in the menu, the
    /// menu polls every extension.
    asking: Vec<ExtId>,
    /// What extensions asked for (`ui/notify`, `ui/status`, opening something) inside the
    /// board's calls to them about their shapes, for whoever runs the board to do
    /// (`take_asked`).
    asked: Vec<(ExtId, Effect)>,
    /// A gesture handed to the extension that ships its shape: the node, and the gesture.
    gesturing: Option<(String, Gesture)>,
    /// Where the mouse went down on a shape, for a click when it comes up without moving.
    press: Option<(String, u16, u16)>,
    path: PathBuf,
    workspace_root: PathBuf,
    workspace_name: String,
    doc: Doc,
    cam: (f64, f64),
    zoom: f64,
    area: Rect,
    sel: Option<String>,
    mode: Mode,
    drag: Option<Drag>,
    undo: Vec<Doc>,
    redo: Vec<Doc>,
    /// What the second status line says for 3 seconds, and since when it has shown: `None`
    /// while an alert or a mode's hint has the line, under which it waits (`draw_status`).
    msg: Option<(String, Option<Instant>)>,
    /// The ids another program repeated: the name each later node of one goes by here (its
    /// place, `parse`) to the id it has in the file, which `as_written` gives it back. Never
    /// forgotten while the board is open, so an undo finds its node's id again.
    twins: HashMap<String, String>,
    /// How many refused requests the message tells of (`refused`), 0 when it is about
    /// something else.
    refusals: usize,
    /// Something the user must see, shown until the next key.
    alert: Option<String>,
    /// Left, and how: `exit` says where to.
    quit: bool,
    exit: Exit,
    /// The area is known only after the first draw, which fits the board to it.
    fitted: bool,
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
    /// The file as the core names it in its events (workspace-relative), when it is one the
    /// core watches: the board has no watcher of its own.
    rel: Option<String>,
    /// The core said the file changed, and it hasn't been read since.
    stale: bool,
    last_check: Instant,
    keys: Keymap<BoardAction>,
    /// Inside the main menu: where Back and Forward lead, and their keys.
    nav: Option<(Nav, Keymap<HistoryAction>)>,
}

// ---------- entry points ----------

/// Open a board. With no file, show a picker of the workspace's `.canvas` files.
pub fn launch(
    terminal: &mut DefaultTerminal,
    workspace_root: &Path,
    file: Option<PathBuf>,
    registry: &mut Registry,
) -> Result<()> {
    let path = match file {
        Some(p) => p,
        None => {
            let mut picker = Picker::new(workspace_root);
            let picked = loop {
                terminal.draw(|f| picker.draw(f))?;
                if let Event::Key(k) = event::read()?
                    && k.kind == KeyEventKind::Press
                    && let Some(picked) = picker.on_key(k)
                {
                    break picked;
                }
            };
            match picked {
                Some(p) => p,
                None => return Ok(()),
            }
        }
    };
    let mut core = crate::owned::open(workspace_root, registry)?;
    let mut b = Board::open(workspace_root, path, None)?;
    let ran = b.run(terminal, &mut core, registry);
    b.close(&mut core);
    ran
}

/// The board picker (`w`): the workspace's `.canvas` files, filtered as you type, and a new
/// board named by what was typed. It has no loop: whoever shows it hands it keys and draws it.
pub struct Picker {
    root: PathBuf,
    boards: Vec<String>,
    query: String,
    /// The chosen row: one of the boards the query keeps, or the new board after them.
    pos: usize,
}

impl Picker {
    pub fn new(root: &Path) -> Picker {
        let mut boards: Vec<String> = WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "canvas")
            })
            .filter_map(|e| {
                e.path()
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
            })
            .collect();
        boards.sort();
        Picker {
            root: root.to_path_buf(),
            boards,
            query: String::new(),
            pos: 0,
        }
    }

    /// The boards the query keeps.
    fn hits(&self) -> Vec<&String> {
        let q = crate::workspace::search_key(&self.query);
        (self.boards.iter())
            .filter(|b| crate::workspace::search_key(b).contains(&q))
            .collect()
    }

    /// The name of the new board: what was typed, or "Untitled".
    fn new_name(&self) -> &str {
        match self.query.trim() {
            "" => "Untitled",
            typed => typed,
        }
    }

    /// A key. `Some` once the picker is done: the board to open, or `None` when it was left.
    pub fn on_key(&mut self, k: KeyEvent) -> Option<Option<PathBuf>> {
        match k.code {
            KeyCode::Esc => return Some(None),
            KeyCode::Up => self.pos = self.pos.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => self.pos = (self.pos + 1).min(self.hits().len()),
            KeyCode::Backspace => {
                self.query.pop();
                self.pos = 0;
            }
            KeyCode::Enter => {
                let rel = match self.hits().get(self.pos) {
                    Some(hit) => hit.to_string(),
                    None => format!("{}.canvas", self.new_name()),
                };
                return Some(Some(self.root.join(rel)));
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.pos = 0;
            }
            _ => {}
        }
        None
    }

    /// Draws it in the middle of the screen, over what is drawn there.
    pub fn draw(&self, f: &mut Frame) {
        let hits = self.hits();
        let total = hits.len() + 1;
        let area = centered(
            f.area(),
            64,
            (total as u16 + 4)
                .min(f.area().height.saturating_sub(2))
                .max(6),
        );
        let mut lines = vec![Line::from(vec![
            Span::styled(" ▸ ", Style::new().fg(HIT)),
            Span::styled(format!("{}▏", self.query), Style::new().fg(NOTE)),
        ])];
        lines.push(Line::raw(""));
        for (i, h) in hits.iter().enumerate() {
            let st = if i == self.pos {
                Style::new().fg(Color::Black).bg(ACCENT)
            } else {
                Style::new().fg(NOTE)
            };
            lines.push(Line::styled(format!("  {h}  "), st));
        }
        let st = if self.pos == hits.len() {
            Style::new().fg(Color::Black).bg(HIT)
        } else {
            Style::new().fg(HIT)
        };
        lines.push(Line::styled(
            format!("  + new board \"{}.canvas\"  ", self.new_name()),
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
    }
}

impl Board {
    /// The board at `path`, created if missing, with `node` selected and in view when it is
    /// one of its nodes.
    pub fn open(workspace_root: &Path, path: PathBuf, node: Option<String>) -> Result<Board> {
        crate::write::create_if_missing(&path, b"{\n\t\"nodes\":[],\n\t\"edges\":[]\n}\n")?;
        let disk = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let (doc, twins) = parse(&disk, &path)?;
        let base = doc.clone();
        let workspace_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let rel = path.canonicalize().ok().and_then(|p| {
            let rel = p.strip_prefix(&workspace_root).ok()?;
            Some(rel.to_string_lossy().replace('\\', "/"))
        });
        let workspace_name = workspace_root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let live = Live::new(crate::shapes::for_workspace(&workspace_root));
        let sel = node.filter(|id| doc.nodes.iter().any(|n| n.id == *id));
        Ok(Board {
            live,
            editor: None,
            asking: Vec::new(),
            asked: Vec::new(),
            gesturing: None,
            press: None,
            path,
            workspace_root,
            workspace_name,
            doc,
            cam: (0.0, 0.0),
            zoom: 1.0,
            area: Rect::default(),
            sel,
            mode: Mode::Normal,
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            msg: None,
            twins: twins.into_iter().collect(),
            refusals: 0,
            alert: None,
            quit: false,
            exit: Exit::Quit,
            fitted: false,
            unsaved: false,
            last_click: None,
            notes: None,
            previews: HashMap::new(),
            base,
            disk,
            rel,
            stale: false,
            last_check: Instant::now(),
            keys: Keymap::new(commands::BOARD),
            nav: None,
        })
    }

    /// Runs the board on its own (`cold-lava board`) until it is left: its own loop, which
    /// polls the core and the extensions that ship its shapes.
    fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        core: &mut Core,
        registry: &mut Registry,
    ) -> Result<()> {
        let mut statuses = Vec::new();
        while !self.quit {
            let events = core.poll();
            let news = core.take_item_news();
            self.noticed(core, &events);
            for (owner, news) in &news {
                self.heard(core, owner, news);
            }
            for ext in self.asking.clone() {
                let effects = registry.call(ext, Some(&mut *core), |e, host| e.poll(host));
                self.asked.extend(effects.into_iter().map(|e| (ext, e)));
            }
            self.tick(core, registry);
            self.asked_alone(registry, &mut statuses);
            let words: Vec<String> = statuses.iter().map(|(_, text)| text.clone()).collect();
            terminal.draw(|f| self.draw(f, &words))?;
            if event::poll(Duration::from_millis(250))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        self.on_key(k, terminal, core, registry)?
                    }
                    Event::Mouse(m) => self.on_mouse(m, core, registry),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// On its own, does what extensions asked for in the board's calls to them. No menu is
    /// behind it: a notification shows, each extension's status words are kept in `statuses`
    /// for the status line, and what only the menu can do (open a note, show a view, run a
    /// command) is refused with a notice.
    fn asked_alone(&mut self, registry: &Registry, statuses: &mut Vec<(ExtId, String)>) {
        for (ext, effect) in self.take_asked() {
            let wanted = match effect {
                Effect::Notify { message } => {
                    self.flash(message);
                    continue;
                }
                Effect::Status { text } => {
                    statuses.retain(|(e, _)| *e != ext);
                    statuses.extend(text.map(|text| (ext, text)));
                    continue;
                }
                Effect::OpenNote { note, .. } => format!("open {}", note.display()),
                Effect::OpenView { .. } | Effect::ShowView { .. } => "show a view".to_string(),
                Effect::RunCommand { id, .. } => format!("run {id}"),
                _ => continue,
            };
            let name = &registry.manifest(ext).name;
            self.refused(format!(
                "{name} wanted to {wanted}: cold-lava board shows the whiteboard only"
            ));
        }
    }

    /// Stops following the workspace: the placed shapes' subscriptions end.
    pub fn close(mut self, core: &mut Core) {
        if let Some(editor) = self.editor.take() {
            editor.close(core);
        }
        self.live.close(core);
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The selected node's id.
    pub fn selected(&self) -> Option<&str> {
        self.sel.as_deref()
    }

    /// What closing the board now would lose, in words for the status line: a draft in the
    /// shape editor, the text of a card being typed, edits a refused save left unsaved.
    pub fn unsaved_work(&self) -> Option<&'static str> {
        if self.editor.as_ref().is_some_and(|e| e.unsaved()) {
            Some("the shape editor has unsaved changes")
        } else if matches!(self.mode, Mode::Edit { .. }) {
            Some("a card is being edited")
        } else if self.unsaved {
            Some("the whiteboard's last edits aren't saved")
        } else {
            None
        }
    }

    /// Where Back and Forward lead from here, for the history keys and the status line.
    pub fn navigate(&mut self, nav: Nav) {
        self.nav = Some((nav, commands::history_keys(&self.keys)));
    }

    /// How the board was left, once it was: the history keys, `q`, or a card's note.
    pub fn left(&self) -> Option<Exit> {
        self.quit.then(|| self.exit.clone())
    }

    /// What extensions asked for in the board's calls to them since the last time, in order:
    /// the menu does it as it does after any call to an extension.
    pub fn take_asked(&mut self) -> Vec<(ExtId, Effect)> {
        std::mem::take(&mut self.asked)
    }

    // ---------- placed shapes, kept up with the workspace ----------

    /// The core's events: for the placed shapes' bindings, and a change to the board's own
    /// file, which the next tick reads.
    pub fn noticed(&mut self, core: &Core, events: &[CoreEvent]) {
        for e in events {
            self.live.noticed(core, e);
            if let Some(editor) = &mut self.editor {
                editor.noticed(core, e);
            }
            self.stale |= match e {
                CoreEvent::BoardChanged(rel)
                | CoreEvent::OwnedChanged(rel)
                | CoreEvent::FileChanged(rel) => Some(rel) == self.rel.as_ref(),
                CoreEvent::Rescanned => true,
                _ => false,
            };
            if let CoreEvent::ItemEdited { note, by, what, .. } = e {
                let name = note.rsplit('/').next().unwrap_or(note);
                self.flash(format!("{by}: {what} on {name}"));
            }
        }
    }

    /// Item news for `owner`'s subscriptions: true when it was the canvas's, as which the
    /// placed shapes and the shape editor's preview are subscribed.
    pub fn heard(&mut self, core: &Core, owner: &str, news: &News) -> bool {
        let mine = owner == crate::bind::OWNER;
        if mine {
            self.live.heard(core, news);
            if let Some(editor) = &mut self.editor {
                editor.heard(core, news);
            }
        }
        mine
    }

    /// Once a frame: binds shapes that are new or changed, asks the extensions that ship
    /// shapes what they show and hands their answers over, and picks up outside changes to the
    /// file.
    pub fn tick(&mut self, core: &mut Core, registry: &mut Registry) {
        for problem in self.live.sync(core, &self.doc.nodes) {
            self.flash(problem);
        }
        for (name, ask) in self.live.take_asks() {
            let Some(ext) = registry.by_name(&name) else {
                continue;
            };
            let effects = registry.ask_shape(ext, Some(&mut *core), ask);
            self.asked.extend(effects.into_iter().map(|e| (ext, e)));
            if !self.asking.contains(&ext) {
                self.asking.push(ext);
            }
        }
        for answer in core.take_shape_answers() {
            let placed = answer.ask.placed();
            let mine = (self.gesturing.as_ref())
                .is_some_and(|(id, _)| self.live.placed_id(id) == Some(placed));
            match (mine, answer.answer) {
                (true, Ok(Answered::Gesture { steps })) => {
                    let (id, gesture) = self.gesturing.take().expect("checked");
                    self.run_steps(&id, &gesture, &steps, core, registry);
                }
                (true, Err(why)) => {
                    self.gesturing = None;
                    self.flash(format!("{}: {why}", answer.by));
                }
                (_, a) => self.live.answered(crate::act::ShapeAnswer {
                    answer: a,
                    ..answer
                }),
            }
        }
        self.watch_disk();
    }

    /// Runs `gesture` on the shaped node `id`: its steps go through the workspace gateway, as
    /// the canvas for the user's shapes and as the extension that ships the shape for its own.
    fn gesture(&mut self, id: &str, gesture: Gesture, core: &mut Core, registry: &mut Registry) {
        let (Some(binding), Some(placed)) = (self.live.binding(id), self.live.placed_id(id)) else {
            return;
        };
        let acted = match binding.extension() {
            None => binding.act(&mut act::Direct::user(core), &gesture, placed),
            Some(name) => match registry.by_name(name) {
                Some(ext) => {
                    let mut out = None;
                    let gw = |_: &mut dyn crate::ext::Extension,
                              host: &mut crate::ext::Host<'_>| {
                        out = Some(binding.act(host, &gesture, placed))
                    };
                    let effects = registry.call(ext, Some(&mut *core), gw);
                    self.asked.extend(effects.into_iter().map(|e| (ext, e)));
                    out.expect("called")
                }
                None => return self.flash(format!("{name} isn't installed")),
            },
        };
        match acted {
            Ok(Acted::Ran(done)) => self.done(id, done),
            Ok(Acted::Ask(ask)) => {
                let Some(ext) = binding.extension().and_then(|n| registry.by_name(n)) else {
                    return;
                };
                let effects = registry.ask_shape(ext, Some(&mut *core), ask);
                self.asked.extend(effects.into_iter().map(|e| (ext, e)));
                if !self.asking.contains(&ext) {
                    self.asking.push(ext);
                }
                self.gesturing = Some((id.to_string(), gesture));
            }
            Err(e) => self.flash(e.to_string()),
        }
    }

    /// The steps an extension answered for a gesture on its shape, run as that extension.
    fn run_steps(
        &mut self,
        id: &str,
        gesture: &Gesture,
        steps: &[crate::shapes::Step],
        core: &mut Core,
        registry: &mut Registry,
    ) {
        let Some(binding) = self.live.binding(id) else {
            return;
        };
        let Some(ext) = binding.extension().and_then(|n| registry.by_name(n)) else {
            return;
        };
        let mut out = None;
        let effects = registry.call(ext, Some(&mut *core), |_, host| {
            out = Some(binding.run(host, gesture, steps))
        });
        self.asked.extend(effects.into_iter().map(|e| (ext, e)));
        match out.expect("called") {
            Ok(done) => self.done(id, done),
            Err(e) => self.flash(e.to_string()),
        }
    }

    /// What a gesture's steps did: notes made are said, and a rebind makes the node the new
    /// shape with the new inputs, in the same place and size.
    fn done(&mut self, id: &str, done: Done) {
        let mut said: Vec<String> = (done.steps.iter())
            .filter_map(|d| match d {
                act::Did::Created { path } => Some(format!("created {path}")),
                act::Did::Recipe { title } => Some(format!("ran {title}")),
                act::Did::Edited { .. } => None,
            })
            .collect();
        if let Some(to) = done.rebind {
            let Some(shape) = self.live.shapes.get(&to.shape) else {
                return self.flash(format!("no shape is called {}", to.shape));
            };
            let (inputs, title) = (
                shape.definition.inputs.clone(),
                shape.definition.title.clone(),
            );
            self.checkpoint();
            if let Some(n) = self.node_mut(id)
                && let Some(Ok(stored)) = placed::Stored::of(n)
            {
                let at = stored.placement(n);
                placed::place(n, &to.shape, &inputs, &title, &to.inputs, at);
            }
            self.commit();
            said.push(format!("now a {}", to.shape));
        }
        if !said.is_empty() {
            self.flash(said.join(" · "));
        }
    }

    /// A click at a cell of the shaped node `id`: the click its shape declares for the member,
    /// region or part there, if any.
    fn click(&mut self, id: &str, col: u16, row: u16, core: &mut Core, registry: &mut Registry) {
        let Some(binding) = self.live.binding(id) else {
            return;
        };
        let a = self.area;
        let member = (self.live).member_at(
            id,
            i32::from(col) - i32::from(a.x),
            i32::from(row) - i32::from(a.y),
        );
        let world = self.to_world(col, row);
        let tolerance = PX_COL / self.zoom / placed::UNIT;
        let hit = self
            .node(id)
            .and_then(|n| self.live.hit(n, world, tolerance, false));
        let placed = self.node(id).and_then(|n| self.live.placed(n));
        let d = &binding.shape().definition;
        let mut on: Vec<(String, Option<String>)> = Vec::new();
        if let Some(m) = &member {
            on.push((m.region.clone(), Some(m.item.clone())));
        }
        if let Some(r) = hit.and_then(|h| d.regions.get(h.region?)) {
            on.push((r.id.clone(), None));
        }
        if let (Some(p), Some(h)) = (&placed, hit)
            && let Some(part) = h.part.and_then(|i| p.shape.parts()[i].id.clone())
        {
            on.push((part, None));
        }
        let found =
            (on.into_iter()).find(|(o, _)| d.on.iter().any(|b| b.click.as_ref() == Some(o)));
        if let Some((on, item)) = found {
            self.gesture(id, Gesture::Click { on, item }, core, registry);
        }
    }

    /// The member under a cell of the shaped node `id`, when its region is where a drop starts.
    fn draggable(&self, id: &str, col: u16, row: u16) -> Option<live::Member> {
        let a = self.area;
        let m = (self.live).member_at(
            id,
            i32::from(col) - i32::from(a.x),
            i32::from(row) - i32::from(a.y),
        )?;
        let d = &self.live.binding(id)?.shape().definition;
        let starts =
            |b: &crate::shapes::Behaviour| b.drop.as_ref().is_some_and(|d| d.from == m.region);
        d.on.iter().any(starts).then_some(m)
    }

    /// Enter or a double-click on a shaped node: a sticky's text is edited in place, a card
    /// opens its note, any other shape's inputs are bound again.
    fn open_shape(&mut self, id: &str) -> bool {
        let Some(n) = self.node(id) else { return false };
        let Some(Ok(stored)) = placed::Stored::of(n) else {
            return false;
        };
        match (n.kind.as_str(), &stored.node) {
            ("text", Some(_)) => {
                let buf = n.text.clone().unwrap_or_default();
                let id = id.to_string();
                self.mode = Mode::Edit {
                    id,
                    buf,
                    is_new: false,
                };
            }
            ("file", Some(_)) if self.nav.is_some() => {
                let path = self.workspace_root.join(n.file.clone().unwrap_or_default());
                self.request_quit();
                if self.quit {
                    self.exit = Exit::Open(path);
                }
            }
            ("file", Some(_)) => return false,
            _ => self.bind_form(Some(id.to_string()), stored.shape.clone()),
        }
        true
    }

    /// The form that binds `shape`'s inputs: filled in from the node's sources when it has one.
    fn bind_form(&mut self, node: Option<String>, shape: String) {
        let Some(loaded) = self.live.shapes.get(&shape) else {
            return self.flash(format!("no shape is called {shape}"));
        };
        let sources = (node.as_ref())
            .and_then(|id| self.node(id))
            .and_then(|n| Some(placed::Stored::of(n)?.ok()?.sources(n)))
            .unwrap_or_default();
        let fields = (loaded.definition.inputs.iter())
            .map(|i| {
                (
                    i.clone(),
                    sources.get(i).map(Source::short).unwrap_or_default(),
                )
            })
            .collect();
        self.mode = Mode::Bind {
            node,
            shape,
            fields,
            pos: 0,
            error: None,
        };
    }

    /// Places or binds again what the form says, or says what's wrong with it.
    fn submit_bind(&mut self, core: &mut Core) {
        let Mode::Bind {
            node,
            shape,
            fields,
            ..
        } = &self.mode
        else {
            return;
        };
        let (node, shape) = (node.clone(), shape.clone());
        let mut sources = indexmap::IndexMap::new();
        let mut wrong = None;
        for (input, text) in fields {
            if text.trim().is_empty() {
                continue;
            }
            match Source::parse(text) {
                Ok(s) => drop(sources.insert(input.clone(), s)),
                Err(e) => wrong = Some(format!("{input}: {e}")),
            }
        }
        let tried = match wrong {
            Some(e) => Err(e),
            None => Binding::new(core, &self.live.shapes, &shape, sources.clone()),
        };
        let error = match tried {
            Ok(mut b) => {
                b.close(core);
                None
            }
            Err(e) => Some(e),
        };
        if let Some(e) = error {
            if let Mode::Bind { error, .. } = &mut self.mode {
                *error = Some(e);
            }
            return;
        }
        self.mode = Mode::Normal;
        let Some(loaded) = self.live.shapes.get(&shape) else {
            return;
        };
        let (inputs, title) = (
            loaded.definition.inputs.clone(),
            loaded.definition.title.clone(),
        );
        let aspect = loaded.definition.aspect.unwrap_or(1.5);
        let id = match node {
            Some(id) => {
                self.checkpoint();
                id
            }
            None => {
                // About 1200 square world units, the shape's own width over height.
                let (w, h) = ((1200.0 * aspect).sqrt(), (1200.0 / aspect).sqrt());
                let (w, h) = (snap(w * placed::UNIT), snap(h * placed::UNIT));
                // Beside the selection, else right of everything, so it covers nothing.
                let at = match (&self.sel, self.bounds()) {
                    (None, Some((_, y0, x1, _))) => Some((x1 + 80.0, y0)),
                    _ => None,
                };
                self.add_node("text", at, w, h)
            }
        };
        if let Some(n) = self.node_mut(&id) {
            let at = match placed::Stored::of(n) {
                Some(Ok(stored)) => stored.placement(n),
                _ => cold_lava_canvas::Placement::new(
                    Point::new(n.x, n.y) * (1.0 / placed::UNIT),
                    Point::new(n.width, n.height) * (1.0 / placed::UNIT),
                ),
            };
            placed::place(n, &shape, &inputs, &title, &sources, at);
        }
        self.commit();
        self.sel = Some(id.clone());
        self.ensure_visible(&id);
    }

    /// The shapes the palette lists for `query`: id, title and where each comes from.
    fn palette(&self, query: &str) -> Vec<(String, String)> {
        let q = crate::workspace::search_key(query);
        (self.live.shapes.shapes.iter())
            .filter(|s| {
                let words = format!("{} {}", s.id, s.definition.title);
                crate::workspace::search_key(&words).contains(&q)
            })
            .map(|s| {
                let title = match s.definition.title.is_empty() {
                    true => String::new(),
                    false => format!("  {}", s.definition.title),
                };
                (s.id.clone(), format!("{}{title}  · {}", s.id, s.origin))
            })
            .collect()
    }

    /// What the shape editor can open for `query`: a new shape, then the workspace's own (only
    /// they are in the file it writes).
    fn editable(&self, query: &str) -> Vec<(Option<String>, String)> {
        let q = crate::workspace::search_key(query);
        let own = (self.live.shapes.shapes.iter())
            .filter(|s| matches!(s.origin, crate::shapes::Origin::Workspace { .. }))
            .filter(|s| {
                let words = format!("{} {}", s.id, s.definition.title);
                crate::workspace::search_key(&words).contains(&q)
            })
            .map(|s| {
                (
                    Some(s.id.clone()),
                    format!("edit {}  {}", s.id, s.definition.title),
                )
            });
        let new = "+ a new shape".to_string();
        let new = crate::workspace::search_key(&new)
            .contains(&q)
            .then_some((None, new));
        new.into_iter().chain(own).collect()
    }

    /// The shape editor on the selected placed shape (a copy to make your own) or the placed
    /// shapes in the selected group (one shape of all their parts), else a new or a workspace
    /// shape to pick.
    fn make_shape(&mut self, core: &mut Core) {
        let Some(sel) = self.sel.clone() else {
            self.mode = Mode::PickShape {
                query: String::new(),
                pos: 0,
            };
            return;
        };
        let members: Vec<String> = (self.group_members(&sel).into_iter())
            .filter(|id| self.live.is_shape(id))
            .collect();
        let loaded = crate::shapes::for_workspace(&self.workspace_root);
        let taken = |id: &str| loaded.get(id).is_some();
        let start = match members.as_slice() {
            [] => {
                self.mode = Mode::PickShape {
                    query: String::new(),
                    pos: 0,
                };
                return;
            }
            [one] if *one == sel => {
                let Some(shape) = (self.node(one))
                    .and_then(|n| Some(placed::Stored::of(n)?.ok()?.shape))
                    .and_then(|s| self.live.shapes.get(&s))
                else {
                    self.flash("this shape didn't load, so there is nothing to start from");
                    return;
                };
                let id = (1..)
                    .map(|n| match n {
                        1 => format!("{}-copy", shape.id),
                        n => format!("{}-copy{n}", shape.id),
                    })
                    .find(|id| !taken(id))
                    .expect("a free id");
                let mut definition = shape.definition.clone();
                definition.title = format!("{} (copy)", definition.title).trim().to_string();
                crate::shape_editor::Start::From {
                    id,
                    definition: Box::new(definition),
                }
            }
            ids => {
                let mut built = Vec::new();
                for id in ids {
                    let Some(n) = self.node(id) else { continue };
                    let Some(Ok(stored)) = placed::Stored::of(n) else {
                        continue;
                    };
                    let Some(loaded) = self.live.shapes.get(&stored.shape) else {
                        continue;
                    };
                    built.push((
                        loaded.definition.clone(),
                        loaded.shape.clone(),
                        stored.placement(n),
                    ));
                }
                let members: Vec<crate::shape_editor::Member> = (built.iter())
                    .map(|(definition, shape, at)| crate::shape_editor::Member {
                        definition,
                        shape,
                        at: *at,
                    })
                    .collect();
                let (definition, left_out) = crate::shape_editor::merged(&members);
                if !left_out.is_empty() {
                    self.flash(format!("left out: {}", left_out.join("; ")));
                }
                let id = (1..)
                    .map(|n| format!("drawing{n}"))
                    .find(|id| !taken(id))
                    .expect("a free id");
                crate::shape_editor::Start::From {
                    id,
                    definition: Box::new(definition),
                }
            }
        };
        self.open_editor(start, core);
    }

    /// Opens the shape editor over the board, on the workspace's core.
    fn open_editor(&mut self, start: crate::shape_editor::Start, core: &mut Core) {
        match crate::shape_editor::Editor::open(&self.workspace_root, core, start) {
            Ok(editor) => self.editor = Some(Box::new(editor)),
            Err(e) => self.flash(e.to_string()),
        }
    }

    /// Back from the shape editor. When it saved, the workspace's shapes are read again and
    /// every placed shape is bound anew, so an extension's answer about a binding from before
    /// names none (`live`).
    fn close_editor(&mut self, core: &mut Core) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        if editor.close(core) {
            self.live.close(core);
            self.live = Live::new(crate::shapes::for_workspace(&self.workspace_root));
            self.flash("shapes read again: s places the one you made");
        }
    }

    /// The selected shape's actions: each name and how its shape titles it.
    fn actions(&self, id: &str) -> Vec<(String, String)> {
        let Some(b) = self.live.binding(id) else {
            return Vec::new();
        };
        (b.shape().definition.on.iter())
            .filter_map(|o| {
                let name = o.action.clone()?;
                let title = if o.title.is_empty() {
                    name.clone()
                } else {
                    o.title.clone()
                };
                Some((name, title))
            })
            .collect()
    }

    // ---------- live sync with the file on disk ----------

    /// Pick up outside changes: once the core says the file changed (`noticed`), and every 2 s
    /// for a file the core doesn't watch (outside the workspace, or in a hidden folder). A
    /// card being typed or dragged comes first: the change waits, and saving merges it.
    fn watch_disk(&mut self) {
        let busy = matches!(self.mode, Mode::Edit { .. }) || self.drag.is_some();
        if busy || !(self.stale || self.last_check.elapsed() > Duration::from_secs(2)) {
            return;
        }
        self.stale = false;
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
        let Ok((theirs, twins)) = parse(&bytes, &self.path) else {
            return false;
        };
        self.twins.extend(twins);
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
        let (mut merged, conflicts) = merge3(&b, &o, &t);
        // A program that rewrote the file without cold-lava's fields would turn placed shapes
        // into plain cards: keep ours, and say so where it can't be missed.
        let lost = placed::lost(&b, &t);
        placed::restore(&mut merged, &o, &lost);
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
        if !lost.is_empty() {
            let n = lost.len();
            let s = if n == 1 { "" } else { "s" };
            self.alert = Some(format!(
                "⚠ the canvas changed outside and lost cold-lava's fields on {n} shape{s}; \
                 they are kept here and saved back (any key to dismiss)"
            ));
        }
        true
    }

    // ---------- persistence ----------

    fn save(&mut self) {
        // Never overwrite edits made elsewhere (e.g. in Obsidian): merge them in first.
        self.sync_from_disk();
        self.write_file();
    }

    fn write_file(&mut self) {
        placed::mark_version(&mut self.doc.extra, &self.doc.nodes);
        let buf = match crate::json_canvas::to_bytes(&as_written(&self.doc, &self.twins)) {
            Ok(buf) => buf,
            Err(e) => return self.flash(format!("save failed: {e}")),
        };
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

    /// Topmost card under a cell, and what of it. Groups only respond on their frame, like
    /// Obsidian; a placed shape where its parts, regions or box are, and its grips once selected.
    fn hit(&self, col: u16, row: u16) -> Option<(String, Grab)> {
        let (c, r) = (col as i32, row as i32);
        let inside = |n: &CNode| {
            let (c0, r0, c1, r1) = self.cell_rect(n);
            (c >= c0 && c < c1 && r >= r0 && r < r1).then_some((c0, r0, c1, r1))
        };
        let corner = |c1: i32, r1: i32| match c == c1 - 1 && r == r1 - 1 {
            true => Grab::Corner(Handle::BottomRight),
            false => Grab::Body,
        };
        let world = self.to_world(col, row);
        let tolerance = PX_ROW / self.zoom / placed::UNIT;
        for n in self.draw_order().into_iter().rev() {
            if n.kind == "group" {
                continue;
            }
            if self.live.is_shape(&n.id) {
                let selected = self.sel.as_deref() == Some(n.id.as_str());
                if let Some(h) = self.live.hit(n, world, tolerance, selected) {
                    let grab = match h.handle {
                        Some(Handle::Rotate) => Grab::Turn,
                        Some(handle) => Grab::Corner(handle),
                        None => Grab::Body,
                    };
                    return Some((n.id.clone(), grab));
                }
                continue;
            }
            if let Some((_, _, c1, r1)) = inside(n) {
                return Some((n.id.clone(), corner(c1, r1)));
            }
        }
        for n in self.doc.nodes.iter().rev().filter(|n| n.kind == "group") {
            if let Some((c0, r0, c1, r1)) = inside(n) {
                let on_frame = r == r0 || r == r1 - 1 || c == c0 || c == c1 - 1;
                if on_frame {
                    return Some((n.id.clone(), corner(c1, r1)));
                }
            }
        }
        None
    }

    /// The board's area in the canvas engine's world units, for painting shapes.
    fn view(&self) -> Viewport {
        let a = self.area;
        let cell = Point::new(PX_COL / self.zoom, PX_ROW / self.zoom) * (1.0 / placed::UNIT);
        let half = Point::new(f64::from(a.width) * cell.x, f64::from(a.height) * cell.y) * 0.5;
        Viewport {
            cols: a.width,
            rows: a.height,
            origin: Point::new(self.cam.0, self.cam.1) * (1.0 / placed::UNIT) - half,
            cell,
        }
    }

    /// Turns the selected shape `by` degrees clockwise.
    fn turn_selected(&mut self, by: f64) {
        let Some(id) = self.sel.clone() else { return };
        let Some(Some(Ok(stored))) = self.node(&id).map(placed::Stored::of) else {
            return self.flash("only a placed shape turns");
        };
        self.checkpoint();
        if let Some(n) = self.node_mut(&id) {
            let p = stored.placement(n);
            placed::replace(n, p.rotated(p.rotation + by));
        }
        self.commit();
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

    pub fn flash(&mut self, s: impl Into<String>) {
        let s = s.into();
        // The shape editor in front shows it now, in the board's place.
        let shown = self.editor.as_mut().map(|editor| {
            editor.say(s.clone());
            Instant::now()
        });
        self.msg = Some((s, shown));
        self.refusals = 0;
    }

    /// Says that something asked for was not done, so that the board keeps what it holds
    /// (`notice`). The user is told of every such request: a notice that comes while the last
    /// one still shows, or waits to, takes its place and says how many there were.
    pub fn refused(&mut self, notice: impl Into<String>) {
        let notice = notice.into();
        let unread = (self.msg.as_ref())
            .is_some_and(|(_, shown)| shown.is_none_or(|at| at.elapsed() <= MESSAGE_SHOWS));
        let n = match unread {
            true => self.refusals + 1,
            false => 1,
        };
        self.flash(match n {
            1 => notice,
            n => format!("{notice} ({n} requests refused)"),
        });
        self.refusals = n;
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
        // Not an id the file may still hold: a twin's, whose first node was deleted here.
        let mut id = new_id();
        while self.node(&id).is_some() || self.twins.values().any(|theirs| *theirs == id) {
            id = new_id();
        }
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
        if self.open_shape(&id) {
            return Ok(());
        }
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
                let path = self.workspace_root.join(&rel);
                if let Some(err) = term::edit_file(terminal, &path, &self.workspace_root)? {
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
        if let Some(err) = term::edit_file(terminal, &tmp, &self.workspace_root)? {
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
                    if !n.extra.contains_key(placed::FIELD) {
                        grow_to_fit(n);
                    }
                }
            }
        }
        self.commit();
    }

    fn workspace_notes(&mut self) -> &Vec<String> {
        if self.notes.is_none() {
            let root = self.workspace_root.clone();
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
        let q = crate::workspace::search_key(query);
        self.workspace_notes()
            .iter()
            .filter(|p| crate::workspace::search_key(p).contains(&q))
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
        let path = self.workspace_root.join(rel);
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

    pub fn on_key(
        &mut self,
        k: KeyEvent,
        terminal: &mut DefaultTerminal,
        core: &mut Core,
        registry: &mut Registry,
    ) -> Result<()> {
        if let Some(editor) = &mut self.editor {
            editor.on_key(k, core);
            if editor.left() {
                self.close_editor(core);
            }
            return Ok(());
        }
        if self.alert.take().is_some() {
            return Ok(());
        }
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
            Mode::Palette { query, pos } => {
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
                        let hits = self.palette(&q);
                        self.mode = Mode::Normal;
                        if let Some((id, _)) = hits.get(p.min(hits.len().saturating_sub(1))) {
                            self.bind_form(None, id.clone());
                            if let Mode::Bind { fields, .. } = &self.mode
                                && fields.is_empty()
                            {
                                self.submit_bind(core);
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::Bind {
                fields, pos, error, ..
            } => {
                let field = fields.get_mut(*pos).map(|(_, text)| text);
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Up | KeyCode::BackTab => *pos = pos.saturating_sub(1),
                    KeyCode::Down | KeyCode::Tab => {
                        *pos = (*pos + 1).min(fields.len().saturating_sub(1))
                    }
                    KeyCode::Backspace => {
                        field.map(String::pop);
                        *error = None;
                    }
                    KeyCode::Char(c) => {
                        if let Some(f) = field {
                            f.push(c);
                        }
                        *error = None;
                    }
                    KeyCode::Enter => self.submit_bind(core),
                    _ => {}
                }
                return Ok(());
            }
            Mode::PickShape { query, pos } => {
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
                        let picks = self.editable(&q);
                        self.mode = Mode::Normal;
                        if let Some((id, _)) = picks.get(p.min(picks.len().saturating_sub(1))) {
                            let start = match id {
                                None => crate::shape_editor::Start::New,
                                Some(id) => crate::shape_editor::Start::Edit(id.clone()),
                            };
                            self.open_editor(start, core);
                        }
                    }
                    _ => {}
                }
                return Ok(());
            }
            Mode::Actions { id, pos } => {
                let (id, p) = (id.clone(), *pos);
                let actions = self.actions(&id);
                match k.code {
                    KeyCode::Esc => self.mode = Mode::Normal,
                    KeyCode::Up => {
                        self.mode = Mode::Actions {
                            id,
                            pos: p.saturating_sub(1),
                        }
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        let pos = (p + 1).min(actions.len().saturating_sub(1));
                        self.mode = Mode::Actions { id, pos };
                    }
                    KeyCode::Enter => {
                        self.mode = Mode::Normal;
                        if let Some((name, _)) = actions.get(p) {
                            self.gesture(
                                &id,
                                Gesture::Action { name: name.clone() },
                                core,
                                registry,
                            );
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

        if let Some(id) = self.sel.clone()
            && let Some(b) = self.live.binding(&id)
        {
            let key = Key::from(k);
            let declared = (b.shape().definition.on.iter())
                .filter_map(|o| o.key.as_deref())
                .find(|s| Key::parse(s) == Some(key))
                .map(String::from);
            if let Some(key) = declared {
                self.gesture(&id, Gesture::Key { key }, core, registry);
                return Ok(());
            }
        }
        if let Some(a) = self.keys.get(k) {
            return self.run_action(a, terminal, core);
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

    fn run_action(
        &mut self,
        a: BoardAction,
        terminal: &mut DefaultTerminal,
        core: &mut Core,
    ) -> Result<()> {
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
                        let ok = term::open_in_obsidian(&self.workspace_name, &rel);
                        self.flash(if ok {
                            "opened in Obsidian"
                        } else {
                            "couldn't launch Obsidian"
                        });
                    }
                    None => {
                        let rel = self
                            .path
                            .strip_prefix(&self.workspace_root)
                            .unwrap_or(&self.path)
                            .to_string_lossy()
                            .into_owned();
                        let ok = term::open_in_obsidian(&self.workspace_name, &rel);
                        self.flash(if ok {
                            "opened board in Obsidian"
                        } else {
                            "couldn't launch Obsidian"
                        });
                    }
                }
            }
            BoardAction::Turn => self.turn_selected(15.0),
            BoardAction::PlaceShape => {
                self.mode = Mode::Palette {
                    query: String::new(),
                    pos: 0,
                }
            }
            BoardAction::Bind => match self.sel.clone() {
                Some(id) if self.live.is_shape(&id) => {
                    let shape =
                        (self.node(&id)).and_then(|n| Some(placed::Stored::of(n)?.ok()?.shape));
                    if let Some(shape) = shape {
                        self.bind_form(Some(id), shape);
                    }
                }
                _ => self.flash("select a placed shape first, then b to bind it"),
            },
            BoardAction::ShapeActions => match self.sel.clone() {
                Some(id) if !self.actions(&id).is_empty() => {
                    self.mode = Mode::Actions { id, pos: 0 }
                }
                Some(id) if self.live.is_shape(&id) => self.flash("this shape has no actions"),
                _ => self.flash("select a placed shape first, then x for what it can do"),
            },
            BoardAction::MakeShape => self.make_shape(core),
            BoardAction::Delete => self.delete_selected(),
            BoardAction::Colour(n) => self.set_color(Some(&n.to_string())),
            BoardAction::ColourClear => self.set_color(None),
            BoardAction::Reload => match std::fs::read(&self.path)
                .map_err(anyhow::Error::from)
                .and_then(|b| Ok((parse(&b, &self.path)?, b)))
            {
                Ok(((d, twins), bytes)) => {
                    self.checkpoint();
                    self.twins.extend(twins);
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

    pub fn on_mouse(&mut self, m: MouseEvent, core: &mut Core, registry: &mut Registry) {
        if let Some(editor) = &mut self.editor {
            return editor.on_mouse(m, core);
        }
        if matches!(
            self.mode,
            Mode::Help
                | Mode::Keys
                | Mode::PickFile { .. }
                | Mode::Palette { .. }
                | Mode::Bind { .. }
                | Mode::Actions { .. }
                | Mode::PickShape { .. }
        ) {
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
                    Some((id, grab)) => {
                        self.sel = Some(id.clone());
                        if double && self.live.is_shape(&id) {
                            self.drag = None;
                            self.open_shape(&id);
                            return;
                        }
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
                        if let Grab::Corner(handle) = grab {
                            self.drag = Some(Drag::Resize {
                                id,
                                handle,
                                moved: false,
                            });
                        } else if grab == Grab::Turn {
                            self.drag = Some(Drag::Turn { id, moved: false });
                        } else if let Some(m) = self.draggable(&id, m.column, m.row) {
                            self.drag = Some(Drag::Member {
                                id,
                                from: m.region,
                                item: m.item,
                                moved: false,
                            });
                        } else {
                            if self.live.is_shape(&id) {
                                self.press = Some((id.clone(), m.column, m.row));
                            }
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
                Some(Drag::Resize { id, handle, moved }) => {
                    *moved = true;
                    let (id, handle) = (id.clone(), *handle);
                    let to = Point::new(world.0, world.1) * (1.0 / placed::UNIT);
                    if let Some(n) = self.node_mut(&id) {
                        match placed::Stored::of(n) {
                            Some(Ok(stored)) => {
                                let min = Point::new(4.0, 2.0);
                                let p = live::stretch(stored.placement(n), handle, to, min);
                                placed::replace(n, p);
                            }
                            _ => {
                                n.width = snap(world.0 - n.x).max(60.0);
                                n.height = snap(world.1 - n.y).max(40.0);
                            }
                        }
                    }
                }
                Some(Drag::Member { moved, .. }) => *moved = true,
                Some(Drag::Turn { id, moved }) => {
                    *moved = true;
                    let id = id.clone();
                    let to = Point::new(world.0, world.1) * (1.0 / placed::UNIT);
                    if let Some(n) = self.node_mut(&id)
                        && let Some(Ok(stored)) = placed::Stored::of(n)
                    {
                        let p = stored.placement(n);
                        placed::replace(n, p.rotated(live::turn_towards(p, to, 15.0)));
                    }
                }
                None => {}
            },
            MouseEventKind::Up(_) => {
                let press = self.press.take();
                if let Some(Drag::Member {
                    id,
                    from,
                    item,
                    moved,
                }) = &self.drag
                {
                    let (id, from, item, moved) = (id.clone(), from.clone(), item.clone(), *moved);
                    self.drag = None;
                    self.undo.pop();
                    if !moved {
                        return self.click(&id, m.column, m.row, core, registry);
                    }
                    let tolerance = PX_COL / self.zoom / placed::UNIT;
                    let hit = self
                        .node(&id)
                        .and_then(|n| self.live.hit(n, world, tolerance, false));
                    let to = (self.live.binding(&id))
                        .zip(hit.and_then(|h| h.region))
                        .and_then(|(b, r)| b.shape().definition.regions.get(r))
                        .map(|r| r.id.clone());
                    match to {
                        Some(to) if to != from => {
                            self.gesture(&id, Gesture::Drop { from, to, item }, core, registry)
                        }
                        _ => self.flash("dropped outside another region: nothing to do"),
                    }
                    return;
                }
                if let Some(Drag::Move { moved: false, .. }) = &self.drag
                    && let Some((id, col, row)) = press
                {
                    self.drag = None;
                    self.undo.pop();
                    return self.click(&id, col, row, core, registry);
                }
                if let Some(
                    Drag::Move { moved, .. }
                    | Drag::Resize { moved, .. }
                    | Drag::Turn { moved, .. },
                ) = self.drag.take()
                {
                    if moved {
                        self.commit();
                    } else {
                        self.undo.pop(); // nothing changed: drop the snapshot
                    }
                }
            }
            _ => {}
        }
    }

    // ---------- drawing ----------

    pub fn draw(&mut self, f: &mut Frame, statuses: &[String]) {
        if let Some(editor) = &mut self.editor {
            return editor.draw(f, statuses);
        }
        let [main, status] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(f.area());
        self.area = Block::bordered().inner(main);
        if !self.fitted {
            self.fitted = true;
            self.fit();
            if let Some(id) = self.sel.clone() {
                self.ensure_visible(&id);
            }
        }
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
            let editing = matches!(&self.mode, Mode::Edit { id, .. } if *id == n.id);
            match self.live.is_shape(&n.id) && !editing {
                true => self.draw_shape(f, n),
                false => self.draw_card(f, n, area),
            }
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

        self.draw_status(f, status, statuses);
        match &self.mode {
            Mode::Help => draw_help(f, main, &self.keys, self.nav.as_ref().map(|(_, k)| k)),
            Mode::Keys => crate::whichkey::draw(f, main, &self.key_hints()),
            Mode::PickFile { query, pos } => {
                let (q, p) = (query.clone(), *pos);
                self.draw_picker(f, main, &q, p);
            }
            Mode::Palette { query, pos } => {
                let lines = self.palette(query).into_iter().map(|(_, l)| l).collect();
                let title = " place a shape · type to filter · ⏎ pick · Esc cancel ";
                draw_choices(f, main, title, Some(query), lines, *pos);
            }
            Mode::PickShape { query, pos } => {
                let lines = self.editable(query).into_iter().map(|(_, l)| l).collect();
                let title = " shape editor · type to filter · ⏎ open · Esc cancel ";
                draw_choices(f, main, title, Some(query), lines, *pos);
            }
            Mode::Actions { id, pos } => {
                let lines = self.actions(id).into_iter().map(|(_, t)| t).collect();
                let title = " what the shape does · ⏎ do it · Esc cancel ";
                draw_choices(f, main, title, None, lines, *pos);
            }
            Mode::Bind {
                shape,
                fields,
                pos,
                error,
                ..
            } => draw_bind(f, main, shape, fields, *pos, error.as_deref()),
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

    /// A placed shape, as the canvas engine paints it: outlines in braille, regions tinted,
    /// text, labels and what each region shows; its grips when selected.
    fn draw_shape(&mut self, f: &mut Frame, n: &CNode) {
        let view = self.view();
        let selected = self.sel.as_deref() == Some(n.id.as_str());
        let outline = match selected {
            true => ACCENT,
            false => canvas_color(&n.color).unwrap_or(CARD_BORDER),
        };
        let a = self.area;
        let Some(p) = self.live.paint(n, &view) else {
            return;
        };
        let buf = f.buffer_mut();
        for row in 0..p.grid.rows() {
            for col in 0..p.grid.cols() {
                let c = p.grid.cell(col, row);
                if c == GridCell::default() {
                    continue;
                }
                let at = (a.x + p.col as u16 + col, a.y + p.row as u16 + row);
                let cell = &mut buf[at];
                if c.ch != ' ' || c.fg != Token::NONE {
                    cell.set_char(c.ch);
                    cell.set_fg(match c.fg {
                        ink::TEXT => NOTE,
                        ink::LABEL => ACCENT_SOFT,
                        _ => outline,
                    });
                }
                if live::is_region_tint(c.bg) {
                    cell.set_bg(tint(c.bg));
                }
            }
        }
        if selected && let Some(placed) = self.live.placed(n) {
            for (handle, at) in placed
                .placement
                .handles(cold_lava_canvas::hit::ROTATE_REACH)
            {
                let at = at * placed::UNIT;
                let (c, r) = self.to_screen(at.x, at.y);
                let (c, r) = (c.floor() as i32, r.floor() as i32);
                // A grip over a label leaves the label readable; it still answers the mouse.
                let under = (c >= 0 && r >= 0)
                    .then(|| f.buffer_mut().cell((c as u16, r as u16)))
                    .flatten();
                if under.is_some_and(|cell| cell.symbol().chars().any(char::is_alphanumeric)) {
                    continue;
                }
                let glyph = if handle == Handle::Rotate {
                    "↻"
                } else {
                    "◆"
                };
                put_str(f, a, c, r, glyph, Style::new().fg(ACCENT));
            }
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

    fn draw_status(&mut self, f: &mut Frame, area: Rect, statuses: &[String]) {
        let mut info = match self.sel.as_ref().and_then(|s| self.node(s)) {
            Some(n) => {
                let links = self
                    .doc
                    .edges
                    .iter()
                    .filter(|e| e.from_node == n.id || e.to_node == n.id)
                    .count();
                let shape = self.live.binding(&n.id).map(|b| b.shape());
                let turned = match placed::Stored::of(n) {
                    Some(Ok(s)) if s.rotation != 0.0 => format!("  turned {}°", s.rotation),
                    _ => String::new(),
                };
                let what = match n.kind.as_str() {
                    _ if let Some(shape) = shape => {
                        format!("shape {} · {}", shape.id, shape.definition.title)
                    }
                    _ if let Some(why) = self.live.problem(&n.id) => format!("shape · {why}"),
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
                        format!(
                            "  {}×{}{turned}  {} arrows",
                            n.width as i64, n.height as i64, links
                        ),
                        Style::new().fg(MUTED),
                    ),
                ])
            }
            None => Line::styled(
                format!(
                    " {}",
                    self.path
                        .strip_prefix(&self.workspace_root)
                        .unwrap_or(&self.path)
                        .display()
                ),
                Style::new().fg(MUTED),
            ),
        };
        // The second line is one of: an alert, the mode's hint, a message, the keys. A message
        // made under an alert or a hint waits: its time starts when it shows.
        let hint = match self.mode {
            Mode::Edit { .. } => Some(" editing · type text · ⏎ new line · Esc done (saves)"),
            Mode::Bind { .. } => {
                Some(" bind · type a source for each input · Tab next · ⏎ place · Esc cancel")
            }
            Mode::Connect { .. } => Some(
                " connect · pick the target (arrows/Tab/click) · ⏎ draw arrow (again removes it) · Esc cancel",
            ),
            _ => None,
        };
        if hint.is_none()
            && self.alert.is_none()
            && let Some((_, shown)) = &mut self.msg
            && shown.is_none()
        {
            *shown = Some(Instant::now());
        }
        if (self.msg.as_ref())
            .is_some_and(|(_, shown)| shown.is_some_and(|at| at.elapsed() > MESSAGE_SHOWS))
        {
            self.msg = None;
        }
        for text in statuses {
            info.spans.push(Span::styled(
                format!("   {text}"),
                Style::new().fg(ACCENT_SOFT),
            ));
        }
        if let Some((nav, keys)) = &self.nav {
            let first = |id| keys.keys(id).first();
            let hint = nav.hint(first("history.back"), first("history.forward"));
            if !hint.is_empty() {
                info.spans
                    .push(Span::styled(format!("   {hint}"), Style::new().fg(MUTED)));
            }
        }
        let second = match (&self.alert, hint, &self.msg) {
            (Some(alert), ..) => {
                Line::styled(format!(" {alert}"), Style::new().fg(Color::Black).bg(HIT))
            }
            (None, Some(hint), _) => Line::styled(hint, Style::new().fg(HIT)),
            (None, None, Some((m, _))) => Line::styled(format!(" {m}"), Style::new().fg(HIT)),
            (None, None, None) => {
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

/// The canvas in a file's bytes. Here every node goes by the canvas owner's place for it, so
/// that the selection, an edit and a placed shape's binding each mean one node. That is its
/// id, unless another program repeated one: then a later node of that id has a name of the
/// board's own, which must never reach the file. Those come back too, each with the id its
/// node has in the file, for `as_written`. An arrow names an id, so it is the first node's.
fn parse(bytes: &[u8], path: &Path) -> Result<(Doc, Vec<(String, String)>)> {
    let mut doc = crate::json_canvas::parse(bytes)
        .with_context(|| format!("{} is not a valid canvas file", path.display()))?;
    let places = crate::owner::canvas::places(doc.nodes.iter().map(|n| n.id.as_str()));
    let mut twins = Vec::new();
    for (n, place) in doc.nodes.iter_mut().zip(places) {
        if place != n.id {
            twins.push((place.clone(), std::mem::replace(&mut n.id, place)));
        }
    }
    Ok((doc, twins))
}

/// The canvas as its file has it: every node under the id it was read with, and an arrow
/// drawn here to a node in `twins` naming that id, as the file's own arrows do.
fn as_written(doc: &Doc, twins: &HashMap<String, String>) -> Doc {
    let mut doc = doc.clone();
    let theirs = |name: &mut String| {
        if let Some(id) = twins.get(name.as_str()) {
            *name = id.clone();
        }
    };
    doc.nodes.iter_mut().for_each(|n| theirs(&mut n.id));
    for e in &mut doc.edges {
        theirs(&mut e.from_node);
        theirs(&mut e.to_node);
    }
    doc
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

/// A region's background: region `i` is `Token(i + 1)`, each a dim colour of its own.
/// Whether a cell's background is a region's tint.
pub(crate) fn is_tint(t: Token) -> bool {
    live::is_region_tint(t)
}

pub(crate) fn tint(t: Token) -> Color {
    const TINTS: [Color; 8] = [
        Color::Rgb(52, 40, 78),
        Color::Rgb(34, 62, 66),
        Color::Rgb(74, 52, 36),
        Color::Rgb(40, 66, 44),
        Color::Rgb(72, 38, 52),
        Color::Rgb(38, 48, 78),
        Color::Rgb(66, 64, 34),
        Color::Rgb(56, 56, 64),
    ];
    TINTS[usize::from(t.0.saturating_sub(1)) % TINTS.len()]
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

/// A popup list to pick from, with what is typed to filter it on top when there is a `query`.
fn draw_choices(
    f: &mut Frame,
    area: Rect,
    title: &str,
    query: Option<&str>,
    items: Vec<String>,
    pos: usize,
) {
    let h = 16.min(area.height.saturating_sub(2));
    let r = centered(area, 72, h);
    let rows = h.saturating_sub(4) as usize;
    let pos = pos.min(items.len().saturating_sub(1));
    let start = pos.saturating_sub(rows.saturating_sub(1));
    let mut lines = Vec::new();
    if let Some(q) = query {
        lines.push(Line::from(vec![
            Span::styled(" ▸ ", Style::new().fg(HIT)),
            Span::styled(format!("{q}▏"), Style::new().fg(NOTE)),
        ]));
        lines.push(Line::raw(""));
    }
    for (i, item) in items.iter().enumerate().skip(start).take(rows) {
        let st = match i == pos {
            true => Style::new().fg(Color::Black).bg(ACCENT),
            false => Style::new().fg(NOTE),
        };
        lines.push(Line::styled(format!("  {item}  "), st));
    }
    if items.is_empty() {
        lines.push(Line::styled("  nothing here", Style::new().fg(MUTED)));
    }
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(title.to_string());
    f.render_widget(Paragraph::new(lines).block(block), r);
}

/// The form that binds a shape's inputs: one line per input, the one being typed in marked.
fn draw_bind(
    f: &mut Frame,
    area: Rect,
    shape: &str,
    fields: &[(String, String)],
    pos: usize,
    error: Option<&str>,
) {
    let h = (fields.len() as u16 + 8).min(area.height.saturating_sub(2));
    let r = centered(area, 90, h);
    let mut lines = vec![Line::styled(
        " a query: type=volcano tag=eruption folder=Volcanoes title=\"…\" where vei >= 6",
        Style::new().fg(MUTED),
    )];
    lines.push(Line::styled(
        " or note:Volcanoes/Krakatoa · items:Fuji, Krakatoa · text:… · empty leaves it unbound",
        Style::new().fg(MUTED),
    ));
    lines.push(Line::raw(""));
    for (i, (input, text)) in fields.iter().enumerate() {
        let here = i == pos;
        let mark = if here { "▸ " } else { "  " };
        let cursor = if here { "▏" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(format!(" {mark}{input}: "), Style::new().fg(ACCENT)),
            Span::styled(format!("{text}{cursor}"), Style::new().fg(NOTE)),
        ]));
    }
    if let Some(e) = error {
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!(" {e}"), Style::new().fg(HIT)));
    }
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(format!(
            " bind {shape} · Tab next input · ⏎ done · Esc cancel "
        ));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        r,
    );
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
        let mut b = Board::open(&dir, path.clone(), None).unwrap();

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

    /// A workspace folder with `B.canvas` in it, and its core.
    fn workspace(name: &str, nodes: Vec<Value>) -> (PathBuf, PathBuf, Core) {
        let dir = std::env::temp_dir().join(format!("cold-lava-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("B.canvas");
        std::fs::write(&path, canvas(nodes)).unwrap();
        let core = Core::open(&dir).unwrap();
        (dir, path, core)
    }

    fn canvas(nodes: Vec<Value>) -> String {
        json!({"nodes": nodes, "edges": []}).to_string()
    }

    fn on_disk(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn an_outside_change_the_core_reports_is_merged_on_the_next_tick() {
        let (dir, path, mut core) = workspace("board-core-event", vec![card("a", 0, "A")]);
        let mut registry = Registry::empty();
        let mut b = Board::open(&dir, path.clone(), None).unwrap();
        b.doc.nodes[0].text = Some("A, ours".into());

        // Two outside changes before one tick: the file as it is now is what merges.
        let changed = [CoreEvent::BoardChanged("B.canvas".into())];
        std::fs::write(&path, canvas(vec![card("a", 0, "A"), card("c", 300, "C")])).unwrap();
        b.noticed(&core, &changed);
        let theirs = vec![card("a", 0, "A"), card("c", 300, "C"), card("d", 600, "D")];
        std::fs::write(&path, canvas(theirs)).unwrap();
        b.noticed(&core, &changed);
        b.tick(&mut core, &mut registry);

        let saved = on_disk(&path);
        assert_eq!(ids(&saved, "nodes"), ["a", "c", "d"]);
        assert_eq!(saved["nodes"][0]["text"], "A, ours", "our edit is kept");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_outside_change_waits_for_the_card_being_edited_and_a_save_of_its_own_is_not_one() {
        let cards = vec![card("a", 0, "Alpha"), card("b", 300, "Bravo")];
        let (dir, path, mut core) = workspace("board-core-edit", cards.clone());
        let mut registry = Registry::empty();
        let mut b = Board::open(&dir, path.clone(), None).unwrap();
        let changed = [CoreEvent::BoardChanged("B.canvas".into())];

        b.mode = Mode::Edit {
            id: "b".into(),
            buf: "Bravo, checked".into(),
            is_new: false,
        };
        let theirs = [cards, vec![card("c", 600, "Charlie")]].concat();
        std::fs::write(&path, canvas(theirs)).unwrap();
        b.noticed(&core, &changed);
        b.tick(&mut core, &mut registry);
        assert_eq!(b.doc.nodes.len(), 2, "not under a card that is being typed");
        b.finish_edit();
        let saved = on_disk(&path);
        assert_eq!(ids(&saved, "nodes"), ["a", "b", "c"]);
        assert_eq!(saved["nodes"][1]["text"], "Bravo, checked");

        // The core reports the board's own save as well: there is nothing to merge in it.
        let (text, undo) = (std::fs::read(&path).unwrap(), b.undo.len());
        b.msg = None;
        b.noticed(&core, &changed);
        b.tick(&mut core, &mut registry);
        assert_eq!(b.msg.as_ref().map(|(m, _)| m.as_str()), None);
        assert_eq!(std::fs::read(&path).unwrap(), text);
        assert_eq!((b.doc.nodes.len(), b.undo.len()), (3, undo));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// An extension that ships the shape `chatty.box` and has things to say when asked what
    /// its region shows. A `pushy` one also asks for a note and a command, unasked.
    struct Chatty {
        pushy: bool,
    }

    impl crate::ext::Extension for Chatty {
        fn initialize(&mut self) -> crate::ext::Manifest {
            let manifest = json!({"name": "chatty", "title": "Chatty", "roles": [], "hints": [],
                "commands": []});
            serde_json::from_value(manifest).unwrap()
        }
        fn pane(&mut self, _: crate::ext::PaneNews, _: &mut crate::ext::Host<'_>) {}
        fn editor(&mut self, _: crate::ext::EditorRequest, _: &mut crate::ext::Host<'_>) {}
        fn shape(&mut self, ask: act::Ask, host: &mut crate::ext::Host<'_>) {
            host.notify("chatty counted");
            host.status(Some("chatty: 1".into()));
            if self.pushy {
                host.open_note("Krakatoa.md".into(), None, None);
                host.run_command("search.open", Value::Null);
            }
            host.shape_answered(&ask, Err("nothing to show".into()));
        }
    }

    /// A workspace whose board has one `chatty.box` placed on it, not ticked yet: the folder,
    /// its core, the registry with Chatty in it, Chatty's id, the board.
    fn chatty_board(name: &str, pushy: bool) -> (PathBuf, Core, Registry, ExtId, Board) {
        use crate::shapes::{Layer, load};
        let (dir, path, _) = workspace(name, Vec::new());
        std::fs::write(dir.join("Krakatoa.md"), "# Krakatoa\n").unwrap();
        let core = Core::open(&dir).unwrap();
        let registry = Registry::new(vec![Box::new(Chatty { pushy })]);
        let chatty = registry.by_name("chatty").unwrap();
        let shape: toml::Table = toml::from_str(
            r#"
            inputs = ["a"]
            parts = [{ rect = { id = "box", from = [0, 0], to = [1, 1] } }]
            regions = [{ id = "count", area = "box", extension = true }]
            "#,
        )
        .unwrap();
        let mut b = Board::open(&dir, path, None).unwrap();
        b.live = Live::new(load([
            Layer::built_in(),
            Layer::extension("chatty", [("chatty.box".to_string(), shape)]),
        ]));
        let mut node: CNode = serde_json::from_value(json!(
            {"id": "n", "type": "text", "x": 0, "y": 0, "width": 200, "height": 100}
        ))
        .unwrap();
        let sources: indexmap::IndexMap<String, Source> =
            [("a".to_string(), Source::Items(vec!["Krakatoa".into()]))].into();
        let at = cold_lava_canvas::Placement::new(Point::new(0.0, 0.0), Point::new(20.0, 10.0));
        let inputs = ["a".to_string()];
        placed::place(&mut node, "chatty.box", &inputs, "Chatty", &sources, at);
        b.doc.nodes.push(node);
        (dir, core, registry, chatty, b)
    }

    #[test]
    fn what_an_extension_asks_for_when_asked_about_its_shape_is_kept_for_the_menu() {
        let (dir, mut core, mut registry, chatty, mut b) = chatty_board("board-asked", false);
        b.tick(&mut core, &mut registry);
        assert_eq!(b.msg.as_ref().map(|(m, _)| m.as_str()), None, "it bound");
        let said = Effect::Notify {
            message: "chatty counted".into(),
        };
        let status = Effect::Status {
            text: Some("chatty: 1".into()),
        };
        assert_eq!(b.take_asked(), [(chatty, said), (chatty, status)]);
        b.tick(&mut core, &mut registry);
        assert!(b.take_asked().is_empty(), "asked once, said once");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `cold-lava board`: no menu is behind the board to open a note or run a command in.
    #[test]
    fn on_its_own_the_board_keeps_status_words_and_refuses_what_needs_the_menu() {
        let (dir, mut core, mut registry, chatty, mut b) = chatty_board("board-alone", true);
        let mut statuses = Vec::new();
        b.tick(&mut core, &mut registry);
        b.asked_alone(&registry, &mut statuses);
        assert_eq!(statuses, [(chatty, "chatty: 1".to_string())]);
        assert_eq!(
            b.msg.as_ref().map(|(m, _)| m.as_str()),
            Some(
                "chatty wanted to run search.open: cold-lava board shows the whiteboard only \
                 (2 requests refused)"
            ),
            "the note and the command, both refused"
        );
        assert!(b.left().is_none(), "the board stays");
        assert_eq!(b.doc.nodes.len(), 1);

        let words = |text: Option<&str>| Effect::Status {
            text: text.map(String::from),
        };
        b.asked.push((chatty, words(Some("chatty: 2"))));
        b.asked_alone(&registry, &mut statuses);
        assert_eq!(statuses, [(chatty, "chatty: 2".to_string())]);
        b.asked.push((chatty, words(None)));
        b.asked_alone(&registry, &mut statuses);
        assert!(statuses.is_empty(), "{statuses:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The board's two status lines, as drawn.
    fn status_lines(b: &mut Board) -> [String; 2] {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        t.draw(|f| b.draw(f, &[])).unwrap();
        let buffer = t.backend().buffer();
        let line = |y: u16| {
            let cells = (0..120).filter_map(|x| buffer.cell((x, y)));
            let text: String = cells.map(|c| c.symbol()).collect();
            text.trim_end().to_string()
        };
        [line(28), line(29)]
    }

    #[test]
    fn a_notice_made_while_a_card_is_typed_waits_for_the_edit_to_end() {
        let (dir, path, _core) = workspace("board-notice", vec![card("a", 0, "Alpha")]);
        let mut b = Board::open(&dir, path, None).unwrap();
        b.mode = Mode::Edit {
            id: "a".into(),
            buf: "Alpha, typed".into(),
            is_new: false,
        };
        b.refused("pushy wanted to open Lava.md: not while a card is being edited");
        b.refused("pushy wanted to run search.open: not while a card is being edited");
        let [_, second] = status_lines(&mut b);
        assert!(second.contains("Esc done (saves)"), "the hint: {second}");
        let waits = |b: &Board| b.msg.as_ref().map(|(_, shown)| shown.is_none());
        assert_eq!(waits(&b), Some(true), "its 3 seconds haven't begun");

        b.finish_edit();
        let [_, second] = status_lines(&mut b);
        assert_eq!(
            second,
            " pushy wanted to run search.open: not while a card is being edited \
             (2 requests refused)"
        );
        assert_eq!(waits(&b), Some(false), "they begin once it shows");

        // A request refused after that notice had its time is counted from one again.
        if let Some(long_ago) = Instant::now().checked_sub(MESSAGE_SHOWS * 2) {
            b.msg = Some(("pushy wanted to open Lava.md".into(), Some(long_ago)));
            b.refused("pushy wanted to open Ash.md");
            let said = b.msg.as_ref().map(|(m, _)| m.as_str());
            assert_eq!(said, Some("pushy wanted to open Ash.md"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Two cards of one id, `First` and `Second`, a card `t`, and an arrow from the id to `t`,
    /// written as the board writes a canvas.
    fn twins_canvas() -> String {
        let doc: Doc = serde_json::from_value(json!({
            "nodes": [card("s", 0, "First"), card("s", 300, "Second"), card("t", 600, "Third")],
            "edges": [{"id": "e", "fromNode": "s", "toNode": "t"}],
        }))
        .unwrap();
        String::from_utf8(crate::json_canvas::to_bytes(&doc).unwrap()).unwrap()
    }

    /// The board on `twins_canvas`, with the node named `sel` selected.
    fn twins_board(name: &str, sel: &str) -> (PathBuf, PathBuf, Board) {
        let (dir, path, _core) = workspace(name, Vec::new());
        std::fs::write(&path, twins_canvas()).unwrap();
        let b = Board::open(&dir, path.clone(), Some(sel.into())).unwrap();
        assert_eq!(b.selected(), Some(sel));
        (dir, path, b)
    }

    /// The saved canvas, which must never hold the board's own name for a repeated id.
    fn saved_twins(path: &Path) -> Value {
        let text = std::fs::read_to_string(path).unwrap();
        let marked = text.contains('\u{1f}') || text.to_lowercase().contains("\\u001f");
        assert!(
            !marked,
            "the board's name for a node is in the file: {text}"
        );
        serde_json::from_str(&text).unwrap()
    }

    fn texts(v: &Value) -> Vec<&str> {
        let nodes = v["nodes"].as_array().unwrap().iter();
        nodes.map(|n| n["text"].as_str().unwrap()).collect()
    }

    const SECOND: &str = "s\u{1f}1";

    /// Another program's canvas may repeat an id. The board names such nodes by the canvas
    /// owner's places, so the place of an item opens the board at its card, and saves them
    /// under the ids they had.
    #[test]
    fn a_card_that_repeats_an_id_is_selected_and_edited_by_its_place() {
        let (dir, path, mut b) = twins_board("board-twins", SECOND);
        let selected = b.selected().and_then(|id| b.node(id));
        assert_eq!(selected.and_then(|n| n.text.as_deref()), Some("Second"));
        b.mode = Mode::Edit {
            id: SECOND.into(),
            buf: "Second, edited".into(),
            is_new: false,
        };
        b.finish_edit();
        let saved = saved_twins(&path);
        assert_eq!(ids(&saved, "nodes"), ["s", "s", "t"]);
        assert_eq!(texts(&saved), ["First", "Second, edited", "Third"]);
        assert!(!b.sync_from_disk(), "its own save read back is nothing new");
        assert_eq!(b.selected(), Some(SECOND));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn moving_a_card_that_repeats_an_id_changes_only_where_it_is() {
        let (dir, path, mut b) = twins_board("board-twins-move", SECOND);
        b.move_selected(40.0, 20.0);
        let (before, after) = (twins_canvas(), std::fs::read_to_string(&path).unwrap());
        assert_eq!(before.lines().count(), after.lines().count(), "{after}");
        let changed: Vec<(&str, &str)> = (before.lines().zip(after.lines()))
            .filter(|(was, is)| was != is)
            .map(|(was, is)| (was.trim(), is.trim()))
            .collect();
        let fields: Vec<&str> = (changed.iter())
            .map(|(was, _)| was.split(':').next().unwrap_or_default())
            .collect();
        assert_eq!(fields, ["\"x\"", "\"y\""], "{changed:?}");
        let saved = saved_twins(&path);
        assert_eq!(ids(&saved, "nodes"), ["s", "s", "t"]);
        let second = &saved["nodes"][1];
        assert_eq!(
            (second["x"].as_f64(), second["y"].as_f64()),
            (Some(340.0), Some(20.0))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The first of two cards of one id is deleted: the other is written with the id it was
    /// read with, and undo brings the first back under the same id.
    #[test]
    fn deleting_a_twin_leaves_the_other_its_id_and_undo_brings_it_back() {
        let (dir, path, mut b) = twins_board("board-twins-delete", "s");
        b.delete_selected();
        let saved = saved_twins(&path);
        assert_eq!(ids(&saved, "nodes"), ["s", "t"]);
        assert_eq!(texts(&saved), ["Second", "Third"]);
        assert_eq!(
            saved["edges"],
            json!([]),
            "its arrow went with it, as drawn"
        );

        b.undo();
        let saved = saved_twins(&path);
        assert_eq!(ids(&saved, "nodes"), ["s", "s", "t"]);
        assert_eq!(texts(&saved), ["First", "Second", "Third"]);
        assert_eq!(saved["edges"][0]["fromNode"], "s");
        b.redo();
        assert_eq!(ids(&saved_twins(&path), "nodes"), ["s", "t"]);

        // A card made now gets an id of its own, and the twin keeps the file's.
        let made = b.add_node("text", None, 200.0, 100.0);
        b.commit();
        assert_ne!(made, "s");
        assert_eq!(ids(&saved_twins(&path), "nodes"), ["s", "t", made.as_str()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// An arrow in the file names an id, so with two cards of that id it is the first one's:
    /// drawn from it, deleted with it, and written back as it was while the other card is
    /// changed or deleted. An arrow drawn here from the second card names the same id.
    #[test]
    fn an_arrow_to_a_repeated_id_is_the_first_cards() {
        let (dir, path, mut b) = twins_board("board-twins-arrow", SECOND);
        let from = b.node(&b.doc.edges[0].from_node);
        assert_eq!(from.and_then(|n| n.text.as_deref()), Some("First"));

        b.toggle_edge(SECOND, "t");
        let saved = saved_twins(&path);
        let ends = |v: &Value| -> Vec<(String, String)> {
            let edges = v["edges"].as_array().unwrap().iter();
            let end = |e: &Value, key: &str| e[key].as_str().unwrap().to_string();
            edges
                .map(|e| (end(e, "fromNode"), end(e, "toNode")))
                .collect()
        };
        let arrow = ("s".to_string(), "t".to_string());
        assert_eq!(ends(&saved), [arrow.clone(), arrow.clone()]);
        assert_eq!(
            saved["edges"][0]["id"], "e",
            "the file's own arrow, untouched"
        );

        b.sel = Some(SECOND.into());
        b.delete_selected();
        let saved = saved_twins(&path);
        assert_eq!(ids(&saved, "nodes"), ["s", "t"]);
        assert_eq!(texts(&saved), ["First", "Third"]);
        assert_eq!(ends(&saved), [arrow], "the first card's arrow stays");
        assert_eq!(saved["edges"][0]["id"], "e");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Another program adds a card, ahead of the twins, while the second twin is typed.
    #[test]
    fn twins_survive_an_outside_change_merged_into_an_edit() {
        let (dir, path, mut b) = twins_board("board-twins-merge", SECOND);
        let mut core = Core::open(&dir).unwrap();
        let mut registry = Registry::empty();
        b.mode = Mode::Edit {
            id: SECOND.into(),
            buf: "Second, edited".into(),
            is_new: false,
        };
        let mut theirs: Value = serde_json::from_str(&twins_canvas()).unwrap();
        let nodes = theirs["nodes"].as_array_mut().unwrap();
        nodes.insert(0, card("c", 900, "Theirs"));
        std::fs::write(&path, theirs.to_string()).unwrap();
        b.noticed(&core, &[CoreEvent::BoardChanged("B.canvas".into())]);
        b.tick(&mut core, &mut registry);
        b.finish_edit();

        let saved = saved_twins(&path);
        let mut cards: Vec<(String, &str)> = (ids(&saved, "nodes").into_iter())
            .zip(texts(&saved))
            .collect();
        cards.sort();
        let is = |id: &str, text: &'static str| (id.to_string(), text);
        assert_eq!(
            cards,
            [
                is("c", "Theirs"),
                is("s", "First"),
                is("s", "Second, edited"),
                is("t", "Third")
            ]
        );
        assert_eq!(saved["edges"][0]["fromNode"], "s");
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
