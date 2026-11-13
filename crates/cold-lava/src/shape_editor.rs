//! The shape editor (docs/shapes.md, "The shape editor"): makes a shape by drawing it and writes
//! the same `[[shapes]]` TOML people write by hand into the workspace's `.cold-lava/shapes.toml`.
//!
//! The shape being edited is its `Definition` and nothing else (`draft`): vertices and sides,
//! circles, ellipses and rectangles are grips on its parts; guides place points but are never
//! saved; regions are read from what contains a clicked point (`draft::area_at`); the preview is
//! the shape bound to data as a placed one is. Saving rewrites only this shape's block
//! (`shapes::file`), refused if the file changed since it was read.
//!
//! It has no loop: the whiteboard that opened it keeps it as a layer over itself and hands it
//! keys, the mouse and the core's news for the preview (`Editor::open`, `on_key`, `on_mouse`,
//! `noticed`, `heard`, `draw`), so whatever runs the whiteboard runs it too.

mod draft;

use crate::app::{ACCENT, ACCENT_SOFT, DIM, HIT, MUTED, NOTE};
use crate::bind::{Binding, Source};
use crate::commands::{self, KeyHint, Keymap, SHAPE_EDITOR, ShapeEditorAction as Action};
use crate::core::{Core, Event as CoreEvent};
use crate::gateway::News;
use crate::notice::Notice;
use crate::problem::Problem;
use crate::shapes::{self, Definition, PartDef, Pos, Shapes, Xy, file};
use anyhow::Result;
use cold_lava_canvas::snap::{self, Guide};
use cold_lava_canvas::{
    self as canvas, Cell as GridCell, Grid, Mask, Placed, Placement, Point, Res, Viewport,
    draw as ink,
};
use draft::{Grip, GripAt, New, pt, xy};
pub use draft::{Member, merged};
use indexmap::IndexMap;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How near, in world units, a guide or a grip reaches: about a cell.
const REACH: f64 = 2.0;
/// The panel on the right: the shape's data and the preview.
const PANEL: u16 = 44;

/// What the editor opens on.
pub enum Start {
    /// A new shape.
    New,
    /// The workspace's shape with this id, read from its file.
    Edit(String),
    /// A new shape made from this one (the whiteboard's selection).
    From {
        id: String,
        definition: Box<Definition>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Select,
    Path,
    Circle,
    Ellipse,
    Rect,
    Text,
    Region,
    Anchor,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Select => "grab",
            Tool::Path => "vertices",
            Tool::Circle => "circle",
            Tool::Ellipse => "ellipse",
            Tool::Rect => "rectangle",
            Tool::Text => "text",
            Tool::Region => "region",
            Tool::Anchor => "anchor",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tool::Select => "drag a vertex or grip · a side's middle bends it · a part moves",
            Tool::Path => "click each vertex · click the first to close · ⏎ leaves it open",
            Tool::Circle => "drag from the centre out to the radius",
            Tool::Ellipse => "drag from the centre out to a corner",
            Tool::Rect => "drag from corner to corner",
            Tool::Text => "click an anchor or a point, then type the text",
            Tool::Region => "click inside an area to name it",
            Tool::Anchor => "click a vertex or a point to name it",
        }
    }
}

enum Drag {
    /// A grip, grabbed at `from`, `offset` away from where it is.
    Grip {
        part: usize,
        grip: Grip,
        from: Point,
        offset: Point,
        moved: bool,
    },
    Part {
        part: usize,
        from: Point,
        moved: bool,
    },
    /// A circle, an ellipse or a rect being drawn from `from` to `to`.
    Make { from: Xy, to: Xy },
}

/// What a form is for.
enum Ask {
    Shape,
    Shows,
    On,
    Bind,
    Region {
        area: String,
    },
    Anchor {
        at: Xy,
        vertex: Option<(usize, usize)>,
    },
    Text {
        at: Pos,
    },
    Rename {
        part: usize,
    },
}

struct Field {
    label: String,
    value: String,
    /// A choice among these (← → change it) instead of typed text.
    choices: &'static [&'static str],
}

impl Field {
    fn text(label: impl Into<String>, value: impl Into<String>) -> Field {
        Field {
            label: label.into(),
            value: value.into(),
            choices: &[],
        }
    }

    fn choice(label: &str, choices: &'static [&'static str]) -> Field {
        Field {
            label: label.into(),
            value: choices[0].into(),
            choices,
        }
    }
}

struct Form {
    ask: Ask,
    title: String,
    hint: String,
    fields: Vec<Field>,
    pos: usize,
    error: Option<String>,
}

const GESTURES: &[&str] = &["click", "drop", "key", "action"];
const STEPS: &[&str] = &[
    "toggle",
    "set-field value",
    "set-field add",
    "set-field remove",
    "create a note",
    "rebind",
];

/// The shape editor, open on one shape.
pub struct Editor {
    root: PathBuf,
    id: String,
    def: Definition,
    /// The block this shape is in the file, by id; `None` until it is first saved.
    replacing: Option<String>,
    /// The file as last read or written (`None`: there is none), what the next save expects.
    read: Option<Vec<u8>>,
    /// What the file has for this shape, to tell unsaved changes.
    saved: Option<(String, Definition)>,
    undo: Vec<(String, Definition)>,
    loaded: Shapes,
    /// The draft's own mistakes, as the loader words them.
    errors: Vec<String>,
    /// The draft as last drawn without mistakes, unbound.
    good: Option<Arc<canvas::Shape>>,
    tool: Tool,
    drag: Option<Drag>,
    /// The vertices of a path being drawn.
    chain: Vec<Xy>,
    selected: Option<usize>,
    guides: bool,
    guide: Option<Guide>,
    form: Option<Form>,
    sources: IndexMap<String, Source>,
    binding: Option<Binding>,
    /// The draft changed since the preview was bound: it is bound again once the key or the
    /// mouse event that changed it is done.
    unbound: bool,
    msg: Option<Notice>,
    keys: Keymap<Action>,
    which_key: bool,
    canvas: Rect,
    leaving: bool,
    quit: bool,
    wrote: bool,
}

impl Editor {
    /// The editor on `start`, in the workspace at `root`.
    pub fn open(root: &Path, core: &mut Core, start: Start) -> Result<Editor> {
        let mut e = read(root, start)?;
        e.reload();
        e.settle(core);
        if e.def.parts.is_empty() {
            e.tool = Tool::Path;
        }
        Ok(e)
    }

    /// Whether it was left (`q`): the whiteboard closes it then.
    pub fn left(&self) -> bool {
        self.quit
    }

    /// Ends the preview's subscriptions. True when a shape was saved, so the workspace's
    /// shapes are to be read again.
    pub fn close(mut self, core: &mut Core) -> bool {
        if let Some(mut b) = self.binding.take() {
            b.close(core);
        }
        self.wrote
    }

    /// A core event, for the preview's binding.
    pub fn noticed(&mut self, core: &Core, event: &CoreEvent) {
        if let Some(b) = &mut self.binding {
            b.noticed(core, event);
        }
    }

    /// Item news for the canvas's subscriptions, one of which may be the preview's.
    pub fn heard(&mut self, core: &Core, news: &News) {
        if let Some(b) = &mut self.binding {
            b.heard(core, news);
        }
    }

    pub fn on_key(&mut self, k: KeyEvent, core: &mut Core) {
        self.key(k, core);
        self.settle(core);
    }

    pub fn on_mouse(&mut self, m: MouseEvent, core: &mut Core) {
        self.mouse(m);
        self.settle(core);
    }

    /// Binds the preview again when the draft changed.
    fn settle(&mut self, core: &mut Core) {
        if std::mem::take(&mut self.unbound) {
            self.rebind(core);
        }
    }
}

/// The editor's state for `start`, read from the workspace's shapes file.
fn read(root: &Path, start: Start) -> Result<Editor> {
    let path = root.join(shapes::WORKSPACE_FILE);
    let read = match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => anyhow::bail!("{}: {e}", path.display()),
    };
    let text = read
        .as_deref()
        .map(String::from_utf8_lossy)
        .unwrap_or_default()
        .into_owned();
    let (id, def, replacing) = match start {
        Start::New => {
            let taken = shapes::for_workspace(root);
            let id = (1..)
                .map(|n| format!("shape{n}"))
                .find(|id| taken.get(id).is_none())
                .expect("a free id");
            let def: Definition =
                toml::from_str("aspect = 1.0\nparts = []").expect("an empty shape");
            (id, def, None)
        }
        Start::Edit(id) => {
            let def = file::blocks(&text)
                .into_iter()
                .find(|b| b.id.as_deref() == Some(id.as_str()))
                .and_then(|b| definition_of(&text[b.span]))
                .ok_or_else(|| {
                    anyhow::anyhow!("{} has no shape {id} that reads", shapes::WORKSPACE_FILE)
                })?;
            (id.clone(), def, Some(id))
        }
        Start::From { id, definition } => (id, *definition, None),
    };
    let saved = replacing.as_ref().map(|id| (id.clone(), def.clone()));
    Ok(Editor {
        root: root.to_path_buf(),
        id,
        def,
        replacing,
        read,
        saved,
        undo: Vec::new(),
        loaded: Shapes::default(),
        errors: Vec::new(),
        good: None,
        tool: Tool::Select,
        drag: None,
        chain: Vec::new(),
        selected: None,
        guides: true,
        guide: None,
        form: None,
        sources: IndexMap::new(),
        binding: None,
        unbound: false,
        msg: None,
        keys: Keymap::new(SHAPE_EDITOR),
        which_key: false,
        canvas: Rect::default(),
        leaving: false,
        quit: false,
        wrote: false,
    })
}

/// A `[[shapes]]` block's definition, its id aside.
fn definition_of(block: &str) -> Option<Definition> {
    let mut t: toml::Table = toml::from_str(block).ok()?;
    let mut entry = t.remove("shapes")?.as_array()?.first()?.clone();
    entry.as_table_mut()?.remove("id");
    entry.try_into().ok()
}

/// The draft `id`'s own errors among the loader's, each in its own words: the problems about
/// it, whatever the words of the others hold.
fn draft_errors(errors: &[Problem], id: &str) -> Vec<String> {
    (errors.iter())
        .filter(|e| e.is_about(id))
        .map(|e| e.first().to_string())
        .collect()
}

impl Editor {
    // ---------- the draft ----------

    /// Loads the draft as the workspace's shape, to draw and check it; the preview is bound
    /// to it again by `settle`.
    fn reload(&mut self) {
        self.loaded = shapes::with_draft(&self.root, &self.id, &self.def);
        self.errors = draft_errors(&self.loaded.errors, &self.id);
        if self.errors.is_empty()
            && let Some(l) = self.loaded.get(&self.id)
        {
            self.good = Some(Arc::new(l.unbound()));
        } else if self.def.parts.is_empty() {
            self.good = None;
        }
        self.unbound = true;
    }

    fn rebind(&mut self, core: &mut Core) {
        if let Some(mut b) = self.binding.take() {
            b.close(core);
        }
        if self.sources.is_empty() || !self.errors.is_empty() {
            return;
        }
        let sources = (self.sources.iter())
            .filter(|(input, _)| self.def.inputs.contains(input))
            .map(|(i, s)| (i.clone(), s.clone()));
        match Binding::new(core, &self.loaded, &self.id, sources) {
            Ok(b) => self.binding = Some(b),
            Err(e) => self.msg = Some(format!("preview: {e}").into()),
        }
    }

    fn checkpoint(&mut self) {
        self.undo.push((self.id.clone(), self.def.clone()));
        self.leaving = false;
    }

    /// Changes the draft, undoably.
    fn change(&mut self, f: impl FnOnce(&mut Definition)) {
        self.checkpoint();
        f(&mut self.def);
        self.reload();
    }

    /// Whether the draft differs from what the file has for this shape.
    pub fn unsaved(&self) -> bool {
        match &self.saved {
            Some((id, d)) => *id != self.id || *d != self.def,
            None => !self.def.parts.is_empty(),
        }
    }

    fn save(&mut self) {
        if self.def.parts.is_empty() {
            return self.say("nothing to save yet: draw a part first");
        }
        if let Some(e) = self.errors.first() {
            return self.say(format!("not saved: {e}"));
        }
        if !self.unsaved() {
            return self.say("nothing changed since it was saved");
        }
        let saved = file::save(
            &self.root,
            self.read.as_deref(),
            self.replacing.as_deref(),
            &self.id,
            &self.def,
        );
        match saved {
            file::Saved::Written(bytes) => {
                self.read = Some(bytes);
                self.replacing = Some(self.id.clone());
                self.saved = Some((self.id.clone(), self.def.clone()));
                self.wrote = true;
                self.leaving = false;
                self.say(format!("saved {} in {}", self.id, shapes::WORKSPACE_FILE));
            }
            file::Saved::Refused(why) => self.say(why),
        }
    }

    /// Shows `s` in the status line until the next key.
    pub fn say(&mut self, s: impl Into<Notice>) {
        self.msg = Some(s.into());
    }

    // ---------- where things are ----------

    /// The canvas as the canvas engine's grid: a cell is 1×2 world units from its top-left.
    fn view(&self) -> Viewport {
        Viewport::terminal(self.canvas.width, self.canvas.height)
    }

    /// The shape's box in the canvas, as large as fits in its aspect, a cell in from the edges.
    fn placement(&self) -> Placement {
        let world = self.view().world_size() - Point::new(2.0, 4.0);
        let aspect = self.def.aspect.filter(|a| *a > 0.0).unwrap_or(1.0);
        let w = world.x.min(world.y * aspect).max(1.0).floor();
        let h = (w / aspect).floor().max(1.0);
        let at = Point::new(
            1.0 + ((world.x - w) / 2.0).floor(),
            2.0 + ((world.y - h) / 4.0).floor() * 2.0,
        );
        Placement::new(at, Point::new(w, h))
    }

    fn to_world(&self, p: Xy) -> Point {
        self.placement().transform().apply(pt(p))
    }

    fn to_unit(&self, world: Point) -> Xy {
        let back = self
            .placement()
            .transform()
            .inverse()
            .expect("a box of some size");
        xy(back.apply(world))
    }

    /// The world point at the middle of a screen cell, when it is on the canvas.
    fn world_at(&self, col: u16, row: u16) -> Option<Point> {
        let c = self.canvas;
        let on = col >= c.x && row >= c.y && col < c.x + c.width && row < c.y + c.height;
        on.then(|| self.view().cell_center(col - c.x, row - c.y))
    }

    fn cell_of(&self, p: Xy) -> Option<(u16, u16)> {
        let (col, row) = self.view().cell_at(self.to_world(p))?;
        Some((self.canvas.x + col, self.canvas.y + row))
    }

    /// The grip nearest `world`, within reach.
    fn grip_at(&self, world: Point) -> Option<GripAt> {
        draft::grips(&self.def)
            .into_iter()
            .map(|g| ((self.to_world(g.at) - world).length(), g))
            .filter(|(d, _)| *d <= REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, g)| g)
    }

    /// The part under `world`: the topmost the draft has there.
    fn part_at(&self, world: Point) -> Option<usize> {
        let shape = self.good.clone()?;
        let placed = Placed::new(shape, self.placement());
        let hit = canvas::hit(&[placed], world, REACH / 2.0, None)?;
        self.owner(hit.part?)
    }

    /// The draft's part a drawn part comes from: a nested shape draws several.
    fn owner(&self, drawn: usize) -> Option<usize> {
        let mut at = 0;
        for (i, part) in self.def.parts.iter().enumerate() {
            let n = match part {
                PartDef::Shape(n) => {
                    (self.loaded.get(&n.shape)).map_or(0, |l| l.shape.parts().len())
                }
                _ => 1,
            };
            if drawn < at + n {
                return Some(i);
            }
            at += n;
        }
        None
    }

    /// The points guides snap to, in the world, but those of `except`.
    fn points(&self, except: Option<(usize, Grip)>) -> Vec<Point> {
        draft::grips(&self.def)
            .into_iter()
            .filter(|g| !matches!(g.grip, Grip::Side(_) | Grip::Radius))
            .filter(|g| Some((g.part, g.grip)) != except)
            .map(|g| self.to_world(g.at))
            .collect()
    }

    /// The radii of the circles, in world units, but part `except`'s.
    fn radii(&self, except: Option<usize>) -> Vec<f64> {
        let w = self.placement().size.x;
        (self.def.parts.iter().enumerate())
            .filter(|(i, _)| Some(*i) != except)
            .filter_map(|(_, p)| match p {
                PartDef::Circle(c) => Some(c.radius * w),
                _ => None,
            })
            .collect()
    }

    /// `world` placed by the guides (or not, with them off), after the vertices `chain`.
    fn snap(&mut self, world: Point, chain: &[Point], except: Option<(usize, Grip)>) -> Xy {
        if !self.guides {
            self.guide = None;
            return self.to_unit(world);
        }
        let s = snap::vertex(world, chain, &self.points(except), REACH);
        self.guide = s.guide;
        self.to_unit(s.at)
    }

    // ---------- the mouse ----------

    fn mouse(&mut self, m: MouseEvent) {
        if self.form.is_some() {
            return;
        }
        let world = self.world_at(m.column, m.row);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(w) = world {
                    self.press(w);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(w) = world {
                    self.move_to(w);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(w) = world {
                    self.move_to(w);
                    self.release(w);
                }
                self.drag = None;
            }
            _ => {}
        }
    }

    fn press(&mut self, world: Point) {
        self.msg = None;
        self.leaving = false;
        match self.tool {
            Tool::Select => {
                if let Some(g) = self.grip_at(world) {
                    self.selected = Some(g.part);
                    self.drag = Some(Drag::Grip {
                        part: g.part,
                        grip: g.grip,
                        from: world,
                        offset: self.to_world(g.at) - world,
                        moved: false,
                    });
                } else if let Some(part) = self.part_at(world) {
                    self.selected = Some(part);
                    self.drag = Some(Drag::Part {
                        part,
                        from: world,
                        moved: false,
                    });
                } else {
                    self.selected = None;
                }
            }
            Tool::Circle | Tool::Ellipse | Tool::Rect => {
                let from = self.snap(world, &[], None);
                self.drag = Some(Drag::Make { from, to: from });
            }
            _ => {}
        }
    }

    fn move_to(&mut self, world: Point) {
        match self.drag.take() {
            Some(Drag::Grip {
                part,
                grip,
                from,
                offset,
                moved,
            }) => {
                if !moved && world != from {
                    self.checkpoint();
                }
                let moved = moved || world != from;
                if moved {
                    self.drag_grip(part, grip, world + offset);
                    self.reload();
                }
                self.drag = Some(Drag::Grip {
                    part,
                    grip,
                    from,
                    offset,
                    moved,
                });
            }
            Some(Drag::Part { part, from, moved }) => {
                if (world - from).length() > 0.0 {
                    if !moved {
                        self.checkpoint();
                    }
                    let (a, b) = (self.to_unit(from), self.to_unit(world));
                    draft::translate(&mut self.def, part, pt(b) - pt(a));
                    self.reload();
                }
                self.drag = Some(Drag::Part {
                    part,
                    from: world,
                    moved: moved || (world - from).length() > 0.0,
                });
            }
            Some(Drag::Make { from, .. }) => {
                let to = match self.tool {
                    Tool::Circle => {
                        let c = self.to_world(from);
                        let r = (world - c).length();
                        let (r, guide) = match self.guides {
                            true => snap::size(r, &self.radii(None), REACH),
                            false => (r, None),
                        };
                        self.guide = guide;
                        let d = world - c;
                        let d = if d.length() > 0.0 {
                            d * (r / d.length())
                        } else {
                            d
                        };
                        self.to_unit(c + d)
                    }
                    _ => self.snap(world, &[], None),
                };
                self.drag = Some(Drag::Make { from, to });
            }
            None => {}
        }
    }

    /// Moves a grip to `world`, as its guides place it.
    fn drag_grip(&mut self, part: usize, grip: Grip, world: Point) {
        if part >= self.def.parts.len() {
            return;
        }
        let to = match grip {
            Grip::Side(k) => {
                let PartDef::Path(p) = &self.def.parts[part] else {
                    return;
                };
                let v = draft::vertices(p);
                let (a, b) = (self.to_world(v[k]), self.to_world(v[(k + 1) % v.len()]));
                let straight = self.guides && ((a + b) * 0.5 - world).length() <= REACH;
                self.guide = None;
                let to = self.to_unit(world);
                draft::drag(&mut self.def, part, grip, to, straight);
                return;
            }
            Grip::Vertex(k) => {
                let chain: Vec<Point> = match &self.def.parts[part] {
                    PartDef::Path(p) => {
                        let v = draft::vertices(p);
                        let n = v.len();
                        let before = |i: usize| (k + n - i) % n;
                        match (k, p.closed) {
                            (k, false) if k < 2 => Vec::new(),
                            _ if n < 3 => Vec::new(),
                            _ => vec![self.to_world(v[before(2)]), self.to_world(v[before(1)])],
                        }
                    }
                    _ => Vec::new(),
                };
                self.snap(world, &chain, Some((part, grip)))
            }
            Grip::Radius => {
                if let PartDef::Circle(c) = &self.def.parts[part] {
                    let center = self.to_world(c.center);
                    let r = (world - center).length();
                    let (r, guide) = match self.guides {
                        true => snap::size(r, &self.radii(Some(part)), REACH),
                        false => (r, None),
                    };
                    self.guide = guide;
                    self.to_unit(center + Point::new(r, 0.0))
                } else {
                    self.snap(world, &[], Some((part, grip)))
                }
            }
            _ => self.snap(world, &[], Some((part, grip))),
        };
        draft::drag(&mut self.def, part, grip, to, false);
    }

    fn release(&mut self, world: Point) {
        match (self.tool, self.drag.take()) {
            (_, Some(Drag::Make { from, to })) => self.make(from, to),
            (Tool::Path, _) => self.place_vertex(world),
            (Tool::Region, _) => self.ask_region(world),
            (Tool::Anchor, _) => self.ask_anchor(world),
            (Tool::Text, _) => self.ask_text(world),
            _ => {}
        }
    }

    fn make(&mut self, from: Xy, to: Xy) {
        let (a, b) = (self.to_world(from), self.to_world(to));
        if (a - b).length() < 1.0 {
            return self.say("drag further to draw it");
        }
        let size = self.placement().size;
        let new = match self.tool {
            Tool::Circle => {
                let r = (b - a).length();
                match (size.x - size.y).abs() < 1e-9 {
                    true => New::Circle {
                        center: from,
                        radius: r / size.x,
                    },
                    false => New::Ellipse {
                        center: from,
                        radii: [r / size.x, r / size.y],
                    },
                }
            }
            Tool::Ellipse => New::Ellipse {
                center: from,
                radii: [(to[0] - from[0]).abs(), (to[1] - from[1]).abs()],
            },
            _ => New::Rect { from, to },
        };
        let mut part = 0;
        self.change(|d| part = draft::add(d, new));
        self.selected = Some(part);
    }

    fn place_vertex(&mut self, world: Point) {
        let chain: Vec<Point> = self.chain.iter().map(|v| self.to_world(*v)).collect();
        let at = self.snap(world, &chain, None);
        let on_first = (chain.first()).is_some_and(|c| (self.to_world(at) - *c).length() < 1e-6);
        let closes = self.chain.len() >= 3 && self.guide == Some(Guide::Point) && on_first;
        if closes {
            self.finish_path(true);
            return;
        }
        if self.chain.last() == Some(&at) {
            return;
        }
        self.chain.push(at);
    }

    fn finish_path(&mut self, closed: bool) {
        let vertices = std::mem::take(&mut self.chain);
        if vertices.len() < 2 {
            return self.say("a path needs two vertices or more");
        }
        let n = vertices.len();
        let mut part = 0;
        self.change(|d| part = draft::add(d, New::Path { vertices, closed }));
        self.selected = Some(part);
        let shape = if closed { "closed" } else { "open" };
        self.say(format!("{shape} path of {n} vertices · N names it"));
    }

    fn ask_region(&mut self, world: Point) {
        let Some(shape) = self.good.clone() else {
            return self.say("draw an area first");
        };
        let local = pt(self.to_unit(world));
        let Some((area, inside)) = draft::area_at(&shape, local) else {
            return self.say("that point is in no area");
        };
        let name = (self.def.regions.iter())
            .find(|r| r.area == area)
            .map(|r| r.id.clone())
            .unwrap_or_default();
        self.form = Some(Form {
            title: " name the region ".into(),
            hint: format!("in {}: {area}", inside.join(" and ")),
            fields: vec![Field::text("region", name)],
            ask: Ask::Region { area },
            pos: 0,
            error: None,
        });
    }

    fn ask_anchor(&mut self, world: Point) {
        let (at, vertex) = match self.grip_at(world) {
            Some(GripAt {
                part,
                grip: Grip::Vertex(k),
                at,
            }) => (at, Some((part, k))),
            Some(g) if !matches!(g.grip, Grip::Side(_)) => (g.at, None),
            _ => (self.snap(world, &[], None), None),
        };
        self.form = Some(Form {
            title: " name the anchor ".into(),
            hint: match vertex {
                Some(_) => "this vertex; text and lines can attach to it".into(),
                None => format!("the point [{:.2}, {:.2}]", at[0], at[1]),
            },
            fields: vec![Field::text("anchor", "")],
            ask: Ask::Anchor { at, vertex },
            pos: 0,
            error: None,
        });
    }

    /// Anchors of the drawn shape, by name: each part's box (`left.top`) and the named points.
    fn anchors(&self) -> Vec<(String, Point)> {
        let Some(shape) = &self.good else {
            return Vec::new();
        };
        const BOX: [&str; 5] = ["top", "bottom", "left", "right", "center"];
        let mut out: Vec<(String, Point)> = shape.anchors().to_vec();
        for part in shape.parts() {
            let Some(id) = &part.id else { continue };
            if matches!(
                part.kind,
                canvas::Kind::Label { .. } | canvas::Kind::Text { .. }
            ) {
                continue;
            }
            for name in BOX {
                if let Some(p) = part.anchor(name) {
                    out.push((format!("{id}.{name}"), p));
                }
            }
        }
        out
    }

    fn ask_text(&mut self, world: Point) {
        let near = (self.anchors().into_iter())
            .map(|(name, p)| ((self.to_world(xy(p)) - world).length(), name))
            .filter(|(d, _)| *d <= REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let at = match near {
            Some((_, name)) => Pos::Anchor(name),
            None => Pos::Point(self.snap(world, &[], None)),
        };
        let hint = match &at {
            Pos::Anchor(name) => format!("at {name}, on the side it faces"),
            Pos::Point(p) => format!("at [{:.2}, {:.2}]", p[0], p[1]),
        };
        let inputs = match self.def.inputs.first() {
            Some(i) => format!(" · ${{{i}.title}} is an input's title"),
            None => String::new(),
        };
        self.form = Some(Form {
            title: " text ".into(),
            hint: format!("{hint}{inputs}"),
            fields: vec![Field::text("text", "")],
            ask: Ask::Text { at },
            pos: 0,
            error: None,
        });
    }

    // ---------- keys ----------

    fn key(&mut self, k: KeyEvent, core: &mut Core) {
        if self.form.is_some() {
            return self.form_key(k, core);
        }
        if self.which_key {
            self.which_key = false;
            if !self
                .key_hints()
                .iter()
                .any(|h| h.key == commands::Key::from(k))
            {
                return;
            }
        }
        let Some(a) = self.keys.get(k) else { return };
        if a != Action::Quit {
            self.leaving = false;
        }
        self.msg = None;
        if a == Action::Delete {
            self.drag = None;
        }
        let tool = |t| {
            move |e: &mut Editor| {
                e.tool = t;
                e.chain.clear();
                e.guide = None;
            }
        };
        match a {
            Action::Select => tool(Tool::Select)(self),
            Action::Path => tool(Tool::Path)(self),
            Action::Circle => tool(Tool::Circle)(self),
            Action::Ellipse => tool(Tool::Ellipse)(self),
            Action::Rect => tool(Tool::Rect)(self),
            Action::Text => tool(Tool::Text)(self),
            Action::Region => tool(Tool::Region)(self),
            Action::Anchor => tool(Tool::Anchor)(self),
            Action::Rename => match self.selected.and_then(|i| self.def.parts.get(i)?.id()) {
                Some(id) => {
                    let (part, id) = (self.selected.expect("selected"), id.to_string());
                    self.form = Some(Form {
                        title: " rename the part ".into(),
                        hint: "regions, anchors and clicks that name it follow".into(),
                        fields: vec![Field::text("id", id)],
                        ask: Ask::Rename { part },
                        pos: 0,
                        error: None,
                    });
                }
                None => self.say("grab a part with an id first (v, then click it)"),
            },
            Action::Delete => match self.selected.take() {
                Some(part) if part < self.def.parts.len() => {
                    let id = self.def.parts[part].id().map(String::from);
                    self.change(|d| {
                        d.parts.remove(part);
                        if let Some(id) = &id {
                            d.regions.retain(|r| {
                                !r.area
                                    .split(|c: char| {
                                        !c.is_alphanumeric() && c != '-' && c != '_' && c != '.'
                                    })
                                    .any(|w| w == id)
                            });
                        }
                    });
                }
                _ => self.say("grab a part first"),
            },
            Action::Guides => {
                self.guides = !self.guides;
                self.say(match self.guides {
                    true => "guides on",
                    false => "guides off: points go where they are put",
                });
            }
            Action::Shape => self.shape_form(),
            Action::Shows => self.shows_form(),
            Action::On => self.on_form(),
            Action::Bind => self.bind_form(),
            Action::Undo if self.drag.is_some() => self.say("let go of the mouse first"),
            Action::Undo => match self.undo.pop() {
                Some((id, d)) => {
                    (self.id, self.def) = (id, d);
                    self.selected = None;
                    self.reload();
                }
                None => self.say("nothing to undo"),
            },
            Action::Save => self.save(),
            Action::Finish => {
                if self.tool == Tool::Path && !self.chain.is_empty() {
                    self.finish_path(false);
                }
            }
            Action::Escape => {
                if !self.chain.is_empty() {
                    self.chain.clear();
                } else {
                    self.selected = None;
                }
            }
            Action::Keys => self.which_key = true,
            Action::Quit => {
                if self.unsaved() && !self.leaving {
                    self.leaving = true;
                    self.say("unsaved changes: q again leaves without saving them, ctrl+s saves");
                } else {
                    self.quit = true;
                }
            }
        }
    }

    fn key_hints(&self) -> Vec<KeyHint> {
        commands::screen_key_hints(SHAPE_EDITOR, &self.keys, |a| a != Action::Keys, None)
    }

    fn form_key(&mut self, k: KeyEvent, core: &mut Core) {
        let Some(form) = &mut self.form else { return };
        let n = form.fields.len();
        let field = &mut form.fields[form.pos];
        let cycle = |f: &mut Field, by: isize| {
            let at = f.choices.iter().position(|c| *c == f.value).unwrap_or(0) as isize;
            let len = f.choices.len() as isize;
            f.value = f.choices[(at + by).rem_euclid(len) as usize].to_string();
        };
        match k.code {
            KeyCode::Esc => self.form = None,
            KeyCode::Up | KeyCode::BackTab => form.pos = form.pos.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => form.pos = (form.pos + 1).min(n - 1),
            KeyCode::Left if !field.choices.is_empty() => cycle(field, -1),
            KeyCode::Right if !field.choices.is_empty() => cycle(field, 1),
            KeyCode::Char(' ') if !field.choices.is_empty() => cycle(field, 1),
            KeyCode::Backspace if field.choices.is_empty() => {
                field.value.pop();
                form.error = None;
            }
            KeyCode::Char(c) if field.choices.is_empty() => {
                field.value.push(c);
                form.error = None;
            }
            KeyCode::Enter => self.submit(core),
            _ => {}
        }
    }

    // ---------- forms ----------

    fn shape_form(&mut self) {
        let d = &self.def;
        self.form = Some(Form {
            title: " the shape ".into(),
            hint: "inputs are the collections it is bound to, by name: a, b".into(),
            fields: vec![
                Field::text("id", self.id.clone()),
                Field::text("title", d.title.clone()),
                Field::text("inputs", d.inputs.join(", ")),
                Field::text("key", d.key.clone().unwrap_or_default()),
                Field::text(
                    "aspect",
                    d.aspect.map(|a| a.to_string()).unwrap_or_default(),
                ),
            ],
            ask: Ask::Shape,
            pos: 0,
            error: None,
        });
    }

    fn shows_form(&mut self) {
        if self.def.regions.is_empty() {
            return self.say("no regions yet: a, then click inside an area to name one");
        }
        let mut fields = Vec::new();
        for r in &self.def.regions {
            let parts = draft::split_shows(r.shows.as_deref().unwrap_or(""));
            for (i, label) in ["shows", "where", "sort by", "limit"].iter().enumerate() {
                fields.push(Field::text(format!("{} · {label}", r.id), parts[i].clone()));
            }
        }
        let inputs = match self.def.inputs.is_empty() {
            true => "no inputs yet (i)".to_string(),
            false => self.def.inputs.join(", "),
        };
        self.form = Some(Form {
            title: " what each region shows ".into(),
            hint: format!(
                "sets of {inputs} with & | -: a & b · where vei >= 6 · sort by elevation desc · limit 5"
            ),
            fields,
            ask: Ask::Shows,
            pos: 0,
            error: None,
        });
    }

    fn on_form(&mut self) {
        self.form = Some(Form {
            title: " behaviour ".into(),
            hint: "← → choose · a click or drop names regions; the step's item is clicked, dropped or an input".into(),
            fields: vec![
                Field::choice("gesture", GESTURES),
                Field::text("on (region, part, key, or the action's name)", ""),
                Field::text("to (a drop's region, or the action's title)", ""),
                Field::choice("step", STEPS),
                Field::text("item", ""),
                Field::text("field (or the shape to rebind to)", ""),
                Field::text("value (or the note's text, or input=created)", ""),
            ],
            ask: Ask::On,
            pos: 0,
            error: None,
        });
    }

    fn bind_form(&mut self) {
        if self.def.inputs.is_empty() {
            return self.say("no inputs to bind: i names them");
        }
        let fields = (self.def.inputs.iter())
            .map(|i| {
                Field::text(
                    i.clone(),
                    self.sources.get(i).map(Source::short).unwrap_or_default(),
                )
            })
            .collect();
        self.form = Some(Form {
            title: " preview data ".into(),
            hint: "a query: type=volcano tag=eruption title=\"…\" · note:… · items:A, B · text:…"
                .into(),
            fields,
            ask: Ask::Bind,
            pos: 0,
            error: None,
        });
    }

    /// Applies the form, or says in it what's wrong.
    fn submit(&mut self, core: &mut Core) {
        let Some(form) = self.form.take() else { return };
        let values: Vec<String> = form
            .fields
            .iter()
            .map(|f| f.value.trim().to_string())
            .collect();
        let result = self.apply(&form.ask, &values, core);
        if let Err(e) = result {
            self.form = Some(Form {
                error: Some(e),
                ..form
            });
        }
    }

    fn apply(&mut self, ask: &Ask, v: &[String], core: &mut Core) -> Result<(), String> {
        let name = |s: &str| -> Result<String, String> {
            let ok = s.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            match ok {
                true => Ok(s.to_string()),
                false => Err(format!(
                    "{s:?} is not a name: letters, digits, - and _, from a letter"
                )),
            }
        };
        match ask {
            Ask::Region { area } => {
                let id = name(&v[0])?;
                self.change(|d| draft::name_region(d, &id, area));
                self.say(format!("region {id} = {area} · w says what it shows"));
            }
            Ask::Anchor { at, vertex } => {
                let id = name(&v[0])?;
                let mut named = None;
                let at = *at;
                self.change(|d| {
                    named = match vertex {
                        Some((part, k)) => draft::name_vertex(d, *part, *k, &id),
                        None => {
                            d.anchors.insert(id.clone(), at);
                            Some(id.clone())
                        }
                    };
                });
                self.say(format!("anchor {}", named.unwrap_or(id)));
            }
            Ask::Text { at } => {
                if v[0].is_empty() {
                    return Err("type the text".into());
                }
                let new = New::Text {
                    at: at.clone(),
                    value: v[0].clone(),
                };
                self.change(|d| {
                    draft::add(d, new);
                });
            }
            Ask::Rename { part } => {
                let id = name(&v[0])?;
                self.change(|d| draft::rename_part(d, *part, &id));
            }
            Ask::Shape => {
                let id = v[0].clone();
                let ok = id.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
                if !ok {
                    return Err(format!(
                        "{id:?} is not an id: letters, digits, -, _ and ., from a letter"
                    ));
                }
                let inputs: Vec<String> = (v[2].split(',').map(str::trim))
                    .filter(|s| !s.is_empty())
                    .map(name)
                    .collect::<Result<_, _>>()?;
                let aspect = match v[4].as_str() {
                    "" => None,
                    a => Some(
                        a.parse::<f64>()
                            .ok()
                            .filter(|a| a.is_finite() && *a > 0.0)
                            .ok_or("aspect is width over height, a number above 0")?,
                    ),
                };
                self.checkpoint();
                self.id = id;
                self.def.title = v[1].clone();
                self.def.inputs = inputs;
                self.def.key = Some(v[3].clone()).filter(|k| !k.is_empty());
                self.def.aspect = aspect;
                self.reload();
            }
            Ask::Shows => {
                let mut shows = Vec::new();
                for (i, r) in self.def.regions.iter().enumerate() {
                    let old = r.shows.as_deref().unwrap_or("");
                    let mut was = draft::split_shows(old).to_vec();
                    let label = was.pop().unwrap_or_default();
                    let mut fields = v[i * 4..i * 4 + 4].to_vec();
                    if fields == was {
                        // Unchanged: kept as written, spacing inside quotes and all.
                        shows.push(r.shows.clone());
                        continue;
                    }
                    fields.push(label);
                    let s = draft::join_shows(&fields);
                    if let Some(s) = &s
                        && let Err(e) = cold_lava_engine::shows::Shows::parse(s, &self.def.inputs)
                    {
                        return Err(format!("{}: {e}", r.id));
                    }
                    shows.push(s);
                }
                self.change(|d| {
                    for (r, s) in d.regions.iter_mut().zip(shows) {
                        r.shows = s;
                    }
                });
            }
            Ask::On => {
                let b = behaviour(v)?;
                let before = self.errors.len();
                self.change(|d| d.on.push(b));
                if self.errors.len() > before {
                    let e = self.errors.last().cloned().unwrap_or_default();
                    if let Some((id, d)) = self.undo.pop() {
                        (self.id, self.def) = (id, d);
                        self.reload();
                    }
                    return Err(e);
                }
            }
            Ask::Bind => {
                let mut sources = IndexMap::new();
                for (input, text) in self.def.inputs.iter().zip(v) {
                    if !text.is_empty() {
                        let s = Source::parse(text).map_err(|e| format!("{input}: {e}"))?;
                        sources.insert(input.clone(), s);
                    }
                }
                if !self.errors.is_empty() {
                    return Err("the shape has mistakes to fix first (the panel says)".into());
                }
                Binding::new(core, &self.loaded, &self.id, sources.clone())?.close(core);
                self.sources = sources;
                self.unbound = true;
            }
        }
        Ok(())
    }

    // ---------- drawing ----------

    /// Draws it over the whole screen, with the extensions' `ui/status` words (`statuses`) in
    /// its status line.
    pub fn draw(&mut self, f: &mut Frame, statuses: &[Notice]) {
        let area = f.area();
        let panel = PANEL.min(area.width / 2);
        self.canvas = Rect::new(
            area.x,
            area.y + 1,
            area.width - panel,
            area.height.saturating_sub(2),
        );
        let side = Rect::new(
            area.x + area.width - panel,
            area.y + 1,
            panel,
            area.height.saturating_sub(2),
        );
        let state = match (&self.saved, self.unsaved()) {
            (None, _) => "new",
            (Some(_), true) => "changed",
            (Some(_), false) => "saved",
        };
        let title = format!(
            " shape editor · {} ({state}) · {} · {} ",
            self.id,
            self.tool.name(),
            shapes::WORKSPACE_FILE
        );
        f.render_widget(
            Paragraph::new(Line::styled(
                title,
                Style::new().fg(Color::Black).bg(ACCENT),
            )),
            Rect::new(area.x, area.y, area.width, 1),
        );
        self.draw_canvas(f);
        self.draw_panel(f, side);
        let status = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            1,
        );
        let mut line = match &self.msg {
            Some(m) => Line::from(Span::styled(format!(" {m}"), Style::new().fg(HIT))),
            None => {
                let mut spans = vec![Span::styled(
                    format!(" {} · {}", self.tool.name(), self.tool.hint()),
                    Style::new().fg(MUTED),
                )];
                if let Some(g) = &self.guide {
                    spans.push(Span::styled(
                        format!("  ⊹ {}", g.describe()),
                        Style::new().fg(ACCENT),
                    ));
                }
                if !self.guides {
                    spans.push(Span::styled("  guides off", Style::new().fg(MUTED)));
                }
                spans.push(Span::styled("  · , keys", Style::new().fg(DIM)));
                Line::from(spans)
            }
        };
        for text in statuses {
            line.spans.push(Span::styled(
                format!("   {text}"),
                Style::new().fg(ACCENT_SOFT),
            ));
        }
        f.render_widget(Paragraph::new(line), status);
        if let Some(form) = &self.form {
            draw_form(f, self.canvas, form);
        } else if self.which_key {
            crate::whichkey::draw(f, self.canvas, &self.key_hints());
        }
    }

    fn draw_canvas(&self, f: &mut Frame) {
        let c = self.canvas;
        if c.width < 4 || c.height < 4 {
            return;
        }
        let view = self.view();
        let placement = self.placement();
        let corners = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        for (p, ch) in corners.iter().zip(["┌", "┐", "└", "┘"]) {
            if let Some((col, row)) = self.cell_of(*p) {
                put(f, col, row, ch, Style::new().fg(DIM));
            }
        }
        let mut grid = Grid::new(&view);
        if let Some(shape) = &self.good {
            ink::paint(&mut grid, &Placed::new(shape.clone(), placement), &view);
        }
        let mut lines: Vec<Vec<Point>> = Vec::new();
        if !self.chain.is_empty() {
            lines.push(self.chain.iter().map(|v| self.to_world(*v)).collect());
        }
        if let Some(Drag::Make { from, to }) = &self.drag {
            let (a, b) = (self.to_world(*from), self.to_world(*to));
            let r = (b - a).length();
            let ring = |rx: f64, ry: f64| {
                (0..=48)
                    .map(|i| {
                        let t = i as f64 / 48.0 * std::f64::consts::TAU;
                        a + Point::new(t.cos() * rx, t.sin() * ry)
                    })
                    .collect()
            };
            lines.push(match self.tool {
                Tool::Circle => ring(r, r),
                Tool::Ellipse => ring((b.x - a.x).abs(), (b.y - a.y).abs()),
                _ => vec![a, Point::new(b.x, a.y), b, Point::new(a.x, b.y), a],
            });
        }
        let pending = Mask::strokes(&lines, &view, Res::BRAILLE);
        grid.dots(&pending, canvas::Style(12));
        let buf = f.buffer_mut();
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                let cell = grid.cell(col, row);
                if cell == GridCell::default() {
                    continue;
                }
                let b = &mut buf[(c.x + col, c.y + row)];
                if cell.ch != ' ' || cell.fg != canvas::Style::NONE {
                    crate::cells::put_char(b, cell.ch);
                    b.set_fg(match cell.fg {
                        ink::TEXT => NOTE,
                        ink::LABEL => ACCENT_SOFT,
                        canvas::Style(12) => HIT,
                        _ => NOTE,
                    });
                }
                if crate::board::is_tint(cell.bg) {
                    b.set_bg(crate::board::tint(cell.bg));
                }
            }
        }
        for g in draft::grips(&self.def) {
            let Some((col, row)) = self.cell_of(g.at) else {
                continue;
            };
            let on = self.selected == Some(g.part);
            let (ch, colour) = match g.grip {
                Grip::Vertex(_) => ("●", if on { ACCENT } else { ACCENT_SOFT }),
                Grip::Side(_) if self.tool == Tool::Select => ("◦", if on { ACCENT } else { DIM }),
                Grip::Side(_) => continue,
                Grip::Center => ("+", if on { ACCENT } else { MUTED }),
                _ => ("◆", if on { ACCENT } else { MUTED }),
            };
            put(f, col, row, ch, Style::new().fg(colour));
        }
        for v in &self.chain {
            if let Some((col, row)) = self.cell_of(*v) {
                put(f, col, row, "●", Style::new().fg(HIT));
            }
        }
    }

    fn draw_panel(&self, f: &mut Frame, area: Rect) {
        let d = &self.def;
        let head = |s: &str| Line::styled(s.to_string(), Style::new().fg(ACCENT));
        let dim = |s: String| Line::styled(s, Style::new().fg(MUTED));
        let mut lines = vec![Line::styled(
            match d.title.is_empty() {
                true => self.id.clone(),
                false => format!("{} · {}", self.id, d.title),
            },
            Style::new().fg(NOTE),
        )];
        lines.push(dim(match d.inputs.is_empty() {
            true => "inputs: none (i)".into(),
            false => format!("inputs: {}", d.inputs.join(", ")),
        }));
        lines.push(head("parts"));
        for (i, p) in d.parts.iter().enumerate() {
            let what = match p {
                PartDef::Path(path) => {
                    let curves = (path.segments.iter())
                        .filter(|s| !matches!(s, shapes::SegmentDef::Line(_)))
                        .count();
                    let n = draft::vertices(path).len();
                    let closed = if path.closed { "closed" } else { "open" };
                    match curves {
                        0 => format!("path, {n} vertices, {closed}"),
                        c => format!("path, {n} vertices, {c} curved, {closed}"),
                    }
                }
                PartDef::Text(t) => format!("text {:?}", t.value),
                PartDef::Shape(n) => format!("shape {}", n.shape),
                other => other.kind().to_string(),
            };
            let id = p.id().map(|i| format!("{i}: ")).unwrap_or_default();
            let st = match self.selected == Some(i) {
                true => Style::new().fg(Color::Black).bg(ACCENT),
                false => Style::new().fg(NOTE),
            };
            lines.push(Line::styled(format!(" {id}{what}"), st));
        }
        if !d.regions.is_empty() {
            lines.push(head("regions"));
            for r in &d.regions {
                let shows = r
                    .shows
                    .as_deref()
                    .map(|s| format!(" · shows {s}"))
                    .unwrap_or_default();
                lines.push(Line::styled(
                    format!(" {} = {}{shows}", r.id, r.area),
                    Style::new().fg(NOTE),
                ));
            }
        }
        let mut anchors: Vec<String> = d.anchors.keys().cloned().collect();
        for p in &d.parts {
            if let PartDef::Path(path) = p
                && let Some(id) = &path.id
            {
                for s in &path.segments {
                    let name = match s {
                        shapes::SegmentDef::Line(shapes::LineDef::Named(v)) => v.id.as_deref(),
                        shapes::SegmentDef::Curve(c) => c.id.as_deref(),
                        shapes::SegmentDef::Quad(q) => q.id.as_deref(),
                        _ => None,
                    };
                    anchors.extend(name.map(|n| format!("{id}.{n}")));
                }
            }
        }
        if !anchors.is_empty() {
            lines.push(head("anchors"));
            lines.push(Line::styled(
                format!(" {}", anchors.join(", ")),
                Style::new().fg(NOTE),
            ));
        }
        if !d.on.is_empty() {
            lines.push(head("behaviour"));
            for b in &d.on {
                lines.push(Line::styled(
                    format!(" {}", describe(b)),
                    Style::new().fg(NOTE),
                ));
            }
        }
        if d.parts.is_empty() {
            lines.push(dim(
                " none yet: p vertices, c circle, e ellipse, r rectangle".into(),
            ));
        } else {
            for e in &self.errors {
                lines.push(Line::styled(format!("! {e}"), Style::new().fg(HIT)));
            }
        }
        let preview_h = (area.height / 2).min(18);
        let top = Rect::new(area.x + 1, area.y, area.width - 1, area.height - preview_h);
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), top);
        let frame = Rect::new(
            area.x,
            area.y + area.height - preview_h,
            area.width,
            preview_h,
        );
        let bound = match &self.binding {
            Some(_) => " preview · bound (b) ",
            None => " preview · b binds it ",
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(DIM))
            .title(bound);
        let inner = block.inner(frame);
        f.render_widget(Clear, frame);
        f.render_widget(block, frame);
        self.draw_preview(f, inner);
    }

    /// The shape bound to its preview data, as a placed shape draws.
    fn draw_preview(&self, f: &mut Frame, area: Rect) {
        let (shape, contents) = match &self.binding {
            Some(b) => (b.canvas(), b.shown().contents()),
            None => match &self.good {
                Some(s) => (s.clone(), Vec::new()),
                None => return,
            },
        };
        let view = Viewport::terminal(area.width, area.height);
        let world = view.world_size() - Point::new(2.0, 4.0);
        let aspect = self.def.aspect.filter(|a| *a > 0.0).unwrap_or(1.0);
        let w = world.x.min(world.y * aspect);
        let size = Point::new(w, w / aspect);
        let at = Point::new(
            1.0 + (world.x - size.x) / 2.0,
            2.0 + (world.y - size.y) / 2.0,
        );
        let placed = Placed::new(shape, Placement::new(at, size));
        let mut grid = Grid::new(&view);
        ink::paint(&mut grid, &placed, &view);
        ink::paint_contents(&mut grid, &placed, &view, &contents);
        let buf = f.buffer_mut();
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                let cell = grid.cell(col, row);
                if cell == GridCell::default() {
                    continue;
                }
                let b = &mut buf[(area.x + col, area.y + row)];
                crate::cells::put_char(b, cell.ch);
                b.set_fg(match cell.fg {
                    ink::LABEL => ACCENT_SOFT,
                    _ => NOTE,
                });
                if crate::board::is_tint(cell.bg) {
                    b.set_bg(crate::board::tint(cell.bg));
                }
            }
        }
    }
}

/// A behaviour from the form: its gesture, then one step.
fn behaviour(v: &[String]) -> Result<shapes::Behaviour, String> {
    let (gesture, on, to, step, item, field, value) =
        (&v[0], &v[1], &v[2], &v[3], &v[4], &v[5], &v[6]);
    if on.is_empty() {
        return Err("say what it is on: a region, a part, a key or the action's name".into());
    }
    let mut b: shapes::Behaviour =
        toml::from_str("").map_err(|e: toml::de::Error| e.message().to_string())?;
    match gesture.as_str() {
        "click" => b.click = Some(on.clone()),
        "drop" => {
            if to.is_empty() {
                return Err("a drop goes from one region to another: say which in to".into());
            }
            b.drop = Some(shapes::DropDef {
                from: on.clone(),
                to: to.clone(),
            })
        }
        "key" => b.key = Some(on.clone()),
        _ => {
            b.action = Some(on.clone());
            b.title = to.clone();
        }
    }
    let item = match (item.is_empty(), gesture.as_str()) {
        (false, _) => item.clone(),
        (true, "drop") => "dropped".into(),
        _ => "clicked".into(),
    };
    let edit = |set: Option<shapes::SetField>, toggle| shapes::EditStep {
        item: Some(item.clone()),
        set_field: set,
        toggle,
        recipe: None,
        options: IndexMap::new(),
    };
    let set = |value, add, remove| {
        if field.is_empty() {
            return Err("name the field set-field changes".to_string());
        }
        Ok(Some(shapes::SetField {
            field: field.clone(),
            value,
            add,
            remove,
        }))
    };
    let some = || Some(value.clone());
    match step.as_str() {
        "toggle" => b.edit = Some(edit(None, Some(true))),
        "set-field value" => b.edit = Some(edit(set(some(), None, None)?, None)),
        "set-field add" => b.edit = Some(edit(set(None, some(), None)?, None)),
        "set-field remove" => b.edit = Some(edit(set(None, None, some())?, None)),
        "create a note" => {
            b.steps = vec![shapes::Step::Create(shapes::CreateStep {
                text: value.clone(),
                name: None,
            })]
        }
        _ => {
            let inputs = (value.split(',').filter_map(|kv| kv.split_once('=')))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .collect();
            b.steps = vec![shapes::Step::Rebind(shapes::RebindStep {
                shape: field.clone(),
                inputs,
            })]
        }
    }
    Ok(b)
}

/// One line for a behaviour: `click both → toggle clicked`.
fn describe(b: &shapes::Behaviour) -> String {
    let gesture = match (&b.drop, &b.click, &b.key, &b.action) {
        (Some(d), ..) => format!("drop {} → {}", d.from, d.to),
        (_, Some(c), ..) => format!("click {c}"),
        (_, _, Some(k), _) => format!("key {k}"),
        (_, _, _, Some(a)) => format!("action {a}"),
        _ => "?".into(),
    };
    let steps = match (&b.edit, b.steps.len(), b.extension) {
        (Some(_), ..) => "an edit".to_string(),
        (_, _, true) => "its extension".into(),
        (_, 1, _) => "one step".into(),
        (_, n, _) => format!("{n} steps"),
    };
    format!("{gesture}: {steps}")
}

fn put(f: &mut Frame, col: u16, row: u16, s: &str, style: Style) {
    let area = f.area();
    if col < area.x + area.width && row < area.y + area.height {
        crate::cells::put(&mut f.buffer_mut()[(col, row)], s).set_style(style);
    }
}

fn draw_form(f: &mut Frame, area: Rect, form: &Form) {
    let h = (form.fields.len() as u16 + 6).min(area.height.saturating_sub(2));
    let w = 84.min(area.width.saturating_sub(2));
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    let mut lines = vec![
        Line::styled(format!(" {}", form.hint), Style::new().fg(MUTED)),
        Line::raw(""),
    ];
    let rows = h.saturating_sub(5) as usize;
    let start = form.pos.saturating_sub(rows.saturating_sub(1));
    for (i, field) in form.fields.iter().enumerate().skip(start).take(rows) {
        let here = i == form.pos;
        let mark = if here { "▸ " } else { "  " };
        let value = match (field.choices.is_empty(), here) {
            (true, true) => format!("{}▏", field.value),
            (true, false) => field.value.clone(),
            (false, _) => format!("‹ {} ›", field.value),
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {mark}{}: ", field.label), Style::new().fg(ACCENT)),
            Span::styled(value, Style::new().fg(NOTE)),
        ]));
    }
    if let Some(e) = &form.error {
        lines.push(Line::styled(format!(" {e}"), Style::new().fg(HIT)));
    }
    f.render_widget(Clear, r);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(format!("{}· Tab next · ⏎ done · Esc cancel ", form.title));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The draft's errors are the problems about it: not a glyph's whose words hold
    /// `shape x: `, and with no part of a path that holds it before the draft's own words.
    #[test]
    fn the_drafts_errors_are_the_problems_about_it_whatever_the_words_hold() {
        let place = "w/shape x: /.cold-lava/shapes.toml";
        let errors = [
            Problem::line(format!(
                "{place}: [glyphs] shape x: a glyph is one symbol in quotes, like shape x = \
                 \"✓\"; got \"ab\""
            )),
            Problem::line("it nests shape w, which isn't loaded")
                .at(format!("{place}: shape x"))
                .about("x"),
            Problem::line("it nests shape x, which isn't loaded")
                .at(format!("{place}: shape y"))
                .about("y"),
        ];
        assert_eq!(
            draft_errors(&errors, "x"),
            ["it nests shape w, which isn't loaded"]
        );
        assert_eq!(draft_errors(&errors, "z"), Vec::<String>::new());
    }
}
