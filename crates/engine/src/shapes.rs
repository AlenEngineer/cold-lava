//! Shape definitions: `[[shapes]]` tables that say how something is drawn on the canvas, declared
//! in TOML by cold-lava, an extension, you or a workspace (docs/shapes.md, "Shape definitions").
//! This module reads and checks them, and builds each into a `cold_lava_canvas::Shape`.
//!
//! The built-in shapes are written in the same language (`shapes/built-in.toml`) and loaded first,
//! so nothing here knows about a card or a Venn: only the primitives, anchors, nesting and the
//! checks. Origins load in the order types do (built in < extension < config < workspace); a later
//! definition of an id replaces the earlier one, and a broken one is dropped with its error while
//! the rest load.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use canvas::{Kind, Part, Point, Rect, Side};
use cold_lava_canvas as canvas;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::problem::Problem;
pub use crate::types::Origin;
use crate::types::is_name;

pub mod file;
mod steps;

pub use steps::{Steps, Way};

/// The only `version` a shapes file can have today.
pub const VERSION: i64 = 1;

/// Where a workspace keeps its own shapes.
pub const WORKSPACE_FILE: &str = ".cold-lava/shapes.toml";

/// The shapes and glyphs cold-lava ships, in the language anyone's shapes are written in.
pub const BUILT_IN: &str = include_str!("../shapes/built-in.toml");

/// A point in the shape's unit box, `[x, y]`, 0..1 on each axis.
pub type Xy = [f64; 2];

fn point([x, y]: Xy) -> Point {
    Point::new(x, y)
}

/// One `[[shapes]]` entry, its `id` aside.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The collections a placed shape is bound to (step 4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    /// What makes two items the same across inputs; the item itself without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Width over height, in world units (a terminal cell is 1×2), when first placed or previewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect: Option<f64>,
    pub parts: Vec<PartDef>,
    /// Named points of its own: `tip = [0.9, 0.5]`.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub anchors: IndexMap<String, Xy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<RegionDef>,
    /// Behaviour: gestures and actions, each a list of gateway steps (run in step 4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<Behaviour>,
}

/// A part as written: `{ circle = { id = "left", center = [0.38, 0.5], radius = 0.32 } }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PartDef {
    Rect(RectDef),
    Circle(CircleDef),
    Ellipse(EllipseDef),
    Line(StrokeDef),
    Arrow(StrokeDef),
    Path(PathDef),
    Text(TextDef),
    /// Another shape, by id, drawn in the box `from`..`to`; its parts are `<id>.<part>`.
    Shape(NestedDef),
}

impl PartDef {
    /// The id a part gives itself; a nested shape's parts are `<its id>.<part>`.
    pub fn id(&self) -> Option<&str> {
        match self {
            PartDef::Rect(RectDef { id, .. })
            | PartDef::Circle(CircleDef { id, .. })
            | PartDef::Ellipse(EllipseDef { id, .. })
            | PartDef::Line(StrokeDef { id, .. })
            | PartDef::Arrow(StrokeDef { id, .. })
            | PartDef::Path(PathDef { id, .. })
            | PartDef::Text(TextDef { id, .. }) => id.as_deref(),
            PartDef::Shape(_) => None,
        }
    }

    /// What it is, as written: `circle`, `text`, `shape`.
    pub fn kind(&self) -> &'static str {
        match self {
            PartDef::Rect(_) => "rect",
            PartDef::Circle(_) => "circle",
            PartDef::Ellipse(_) => "ellipse",
            PartDef::Line(_) => "line",
            PartDef::Arrow(_) => "arrow",
            PartDef::Path(_) => "path",
            PartDef::Text(_) => "text",
            PartDef::Shape(_) => "shape",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RectDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub from: Xy,
    pub to: Xy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircleDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub center: Xy,
    pub radius: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EllipseDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub center: Xy,
    pub radii: Xy,
}

/// A line or an arrow; each end a point or an anchor of an earlier part (`left.right`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrokeDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub from: Pos,
    pub to: Pos,
}

/// A path: `points` joined by straight lines, or a `start` and `segments` (lines and curves).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Xy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Xy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<SegmentDef>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
}

/// The most points a path may have once its curves are flattened (each `curve` and `quad` is
/// `canvas::part::CUBIC_STEPS` points, each `line` and each of `points` one). Sized with the
/// canvas bench: a full-screen scene of 200 shapes, 50 of them paths this size, draws within a
/// 60 Hz frame (docs/shapes.md, "Path size").
pub const PATH_LIMIT: usize = 2048;

/// One step of a path from where the last one ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SegmentDef {
    /// `{ line = [x, y] }`, or `{ line = { to = [x, y], id = "tip" } }` to name its end.
    Line(LineDef),
    /// A cubic Bézier: `{ curve = { c1 = [x, y], c2 = [x, y], to = [x, y] } }`.
    Curve(CurveDef),
    /// A quadratic Bézier, drawn as the cubic it equals: `{ quad = { c = [x, y], to = [x, y] } }`.
    Quad(QuadDef),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    untagged,
    expecting = "a line is a point, line = [x, y], or line = { to = [x, y], id = \"name\" }"
)]
pub enum LineDef {
    To(Xy),
    Named(Vertex),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vertex {
    pub to: Xy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurveDef {
    pub c1: Xy,
    pub c2: Xy,
    pub to: Xy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuadDef {
    pub c: Xy,
    pub to: Xy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl SegmentDef {
    /// Where it ends, and the name given to that vertex.
    fn end(&self) -> (Xy, Option<&str>) {
        match self {
            SegmentDef::Line(LineDef::To(to)) => (*to, None),
            SegmentDef::Line(LineDef::Named(v)) => (v.to, v.id.as_deref()),
            SegmentDef::Curve(c) => (c.to, c.id.as_deref()),
            SegmentDef::Quad(q) => (q.to, q.id.as_deref()),
        }
    }

    /// The canvas segment, starting at `from` (a quadratic's control point becomes two).
    fn canvas(&self, from: Point) -> canvas::Segment {
        match self {
            SegmentDef::Line(_) => canvas::Segment::Line(point(self.end().0)),
            SegmentDef::Curve(c) => canvas::Segment::Cubic {
                c1: point(c.c1),
                c2: point(c.c2),
                to: point(c.to),
            },
            SegmentDef::Quad(q) => {
                let (c, to) = (point(q.c), point(q.to));
                canvas::Segment::Cubic {
                    c1: from + (c - from) * (2.0 / 3.0),
                    c2: to + (c - to) * (2.0 / 3.0),
                    to,
                }
            }
        }
    }

    /// Points it adds once flattened.
    fn size(&self) -> usize {
        match self {
            SegmentDef::Line(_) => 1,
            SegmentDef::Curve(_) | SegmentDef::Quad(_) => canvas::part::CUBIC_STEPS,
        }
    }
}

impl PathDef {
    /// Its vertices as written: the start or the points, then each segment's end.
    fn vertices(&self) -> usize {
        self.points.len() + usize::from(self.start.is_some()) + self.segments.len()
    }

    /// Its points once flattened.
    fn size(&self) -> usize {
        self.points.len()
            + usize::from(self.start.is_some())
            + self.segments.iter().map(SegmentDef::size).sum::<usize>()
    }

    /// The vertices given an `id`, with where they are.
    fn named(&self) -> impl Iterator<Item = (&str, Xy)> {
        (self.segments.iter()).filter_map(|s| match s.end() {
            (to, Some(id)) => Some((id, to)),
            _ => None,
        })
    }

    fn build(&self) -> canvas::Path {
        match self.start {
            None => canvas::Path::polyline(
                self.points.iter().copied().map(point).collect(),
                self.closed,
            ),
            Some(start) => {
                let mut from = point(start);
                let segments: Vec<canvas::Segment> = (self.segments.iter())
                    .map(|s| {
                        let seg = s.canvas(from);
                        from = point(s.end().0);
                        seg
                    })
                    .collect();
                canvas::Path::new(point(start), &segments, self.closed)
            }
        }
    }
}

/// Text: a label beside a point (`at`), or a box it is wrapped in (`from`, `to`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Pos>,
    /// Which side of `at`; by default the side its anchor faces (`left.top`: above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<SideDef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Xy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Xy>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideDef {
    Above,
    Below,
    Left,
    Right,
    Center,
}

impl From<SideDef> for Side {
    fn from(s: SideDef) -> Side {
        match s {
            SideDef::Above => Side::Above,
            SideDef::Below => Side::Below,
            SideDef::Left => Side::Left,
            SideDef::Right => Side::Right,
            SideDef::Center => Side::Center,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NestedDef {
    pub id: String,
    #[serde(rename = "use")]
    pub shape: String,
    pub from: Xy,
    pub to: Xy,
}

/// A point, or the name of an anchor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Pos {
    Point(Xy),
    Anchor(String),
}

/// A named area (`area`, over the parts) and what it shows (`shows`, over the inputs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionDef {
    pub id: String,
    pub area: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shows: Option<String>,
    /// What it shows is computed by the extension that ships the shape (`shape/region`),
    /// instead of `shows`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub extension: bool,
}

/// One gesture or action and the steps it runs. Exactly one of `drop`, `click`, `key` and
/// `action`; the steps are `do`, or `edit` alone as its shorthand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Behaviour {
    /// An item dragged from one region to another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop: Option<DropDef>,
    /// A click on a region or a part, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click: Option<String>,
    /// A key pressed while the shape is selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// A named action the shape offers (in its menu and the palette): `make-a-note`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// How menus name the action.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit: Option<EditStep>,
    #[serde(default, rename = "do", skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    /// The extension that ships the shape decides the steps (`shape/gesture`), instead of
    /// `do` or `edit`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub extension: bool,
}

impl Behaviour {
    /// Its steps as written: its one `edit`, then its `do`. A loaded entry has only one of
    /// the two.
    pub fn written(&self) -> Vec<Step> {
        let edit = self.edit.iter().cloned().map(Step::Edit);
        edit.chain(self.steps.iter().cloned()).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropDef {
    pub from: String,
    pub to: String,
}

/// A generic step on the workspace gateway; nothing in it is particular to one shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    /// `items/edit` on an item (`dropped`, `clicked`).
    Edit(EditStep),
    /// `workspace/create`: a new note with `text`, where Obsidian puts new notes; it is `created`
    /// in later steps.
    Create(CreateStep),
    /// This placed shape becomes another shape, same place and size, with these inputs.
    Rebind(RebindStep),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct EditStep {
    /// The item `set-field` and `toggle` change: `dropped`, `clicked`, or an input with one
    /// member. A recipe changes the whole workspace and names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_field: Option<SetField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// A recipe's options, by name; placeholders are filled.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub options: IndexMap<String, String>,
}

/// `set-field`: exactly one of `value`, `add` and `remove`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetField {
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateStep {
    pub text: String,
    /// The new note's name; by default its text's first line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebindStep {
    pub shape: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub inputs: IndexMap<String, String>,
}

/// A loaded shape: its definition as written and the canvas shape built from it.
#[derive(Clone, Debug, Serialize)]
pub struct Loaded {
    pub id: String,
    pub origin: Origin,
    #[serde(flatten)]
    pub definition: Definition,
    #[serde(skip)]
    pub shape: Arc<canvas::Shape>,
}

impl Loaded {
    /// The shape with nothing bound: `${a.title}` shows as `a.title`.
    pub fn unbound(&self) -> canvas::Shape {
        self.shape.map_text(|v| fill(v, |name| name.to_string()))
    }
}

/// A named symbol that draws like a letter: `:check:` in any text is `✓`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Glyph {
    pub name: String,
    pub symbol: char,
    pub origin: Origin,
}

/// Every loaded shape and glyph, and what was wrong with the rest.
#[derive(Debug, Default, Serialize)]
pub struct Shapes {
    /// In load order; one replaced by a later origin takes the later one's place.
    pub shapes: Vec<Loaded>,
    pub glyphs: Vec<Glyph>,
    /// One problem per mistake: where, which shape (what it is about), what. The shape is
    /// dropped.
    pub errors: Vec<Problem>,
    /// Things worth knowing that aren't mistakes, such as a shape replacing an earlier one.
    pub notes: Vec<String>,
}

impl Shapes {
    pub fn get(&self, id: &str) -> Option<&Loaded> {
        self.shapes.iter().find(|s| s.id == id)
    }
}

/// Definitions and glyphs from one origin, read but not yet checked.
#[derive(Clone, Debug)]
pub struct Layer {
    origin: Origin,
    definitions: Vec<(String, Result<Definition, Problem>)>,
    glyphs: Vec<(String, Result<char, String>)>,
    /// Problems with the file as a whole, or with an entry that has no id.
    errors: Vec<Problem>,
}

impl Layer {
    /// An extension's `[[shapes]]`, each by its id (`<extension>.<name>`; the frontend leaves out
    /// the ones outside its namespace) and the rest of its table.
    pub fn extension(name: &str, shapes: impl IntoIterator<Item = (String, toml::Table)>) -> Layer {
        Layer {
            origin: Origin::Extension {
                name: name.to_string(),
            },
            definitions: (shapes.into_iter())
                .map(|(id, table)| (id, parse(toml::Value::Table(table))))
                .collect(),
            glyphs: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// config.toml's `[[shapes]]` and `[glyphs]`, which the frontend reads and hands over.
    pub fn config(path: &Path, shapes: Vec<toml::Table>, glyphs: toml::Table) -> Layer {
        let mut layer = Layer::empty(Origin::Config {
            path: path.to_path_buf(),
        });
        layer.read(shapes, glyphs);
        layer
    }

    /// The workspace's `.cold-lava/shapes.toml`; `None` when it has none.
    pub fn workspace(root: &Path) -> Option<Layer> {
        let path = root.join(WORKSPACE_FILE);
        let text = match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            r => r.map_err(|e| e.to_string()),
        };
        Some(Layer::from_file(Origin::Workspace { path }, text))
    }

    pub fn built_in() -> Layer {
        Layer::from_file(Origin::BuiltIn, Ok(BUILT_IN.to_string()))
    }

    fn empty(origin: Origin) -> Layer {
        Layer {
            origin,
            definitions: Vec::new(),
            glyphs: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// A shapes file: `version = 1`, then `[[shapes]]` tables and a `[glyphs]` table.
    fn from_file(origin: Origin, text: Result<String, String>) -> Layer {
        let place = origin.place();
        let mut layer = Layer::empty(origin);
        let read = text
            .map_err(Problem::line)
            .and_then(|t| toml::from_str(&t).map_err(|e| Problem::toml_file(&e, &t)));
        let mut file: toml::Table = match read {
            Ok(f) => f,
            Err(e) => {
                layer.errors.push(e.unreadable(&place));
                return layer;
            }
        };
        match file.remove("version") {
            Some(toml::Value::Integer(VERSION)) => {}
            Some(v) => {
                layer.errors.push(Problem::line(format!(
                    "{place}: version = {v} is not one this cold-lava reads (it reads version = {VERSION}); the file is skipped"
                )));
                return layer;
            }
            None => {
                layer.errors.push(Problem::line(format!(
                    "{place}: no version: start the file with version = {VERSION}; the file is skipped"
                )));
                return layer;
            }
        }
        let shapes = match file.remove("shapes") {
            None => Vec::new(),
            Some(toml::Value::Array(entries)) => (entries.into_iter())
                .filter_map(|e| match e {
                    toml::Value::Table(t) => Some(t),
                    _ => {
                        layer.errors.push(Problem::line(format!(
                            "{place}: shapes: each entry is a [[shapes]] table"
                        )));
                        None
                    }
                })
                .collect(),
            Some(_) => {
                layer.errors.push(Problem::line(format!(
                    "{place}: shapes: write each shape as a [[shapes]] table with its id"
                )));
                Vec::new()
            }
        };
        let glyphs = match file.remove("glyphs") {
            None => toml::Table::new(),
            Some(toml::Value::Table(t)) => t,
            Some(_) => {
                layer.errors.push(Problem::line(format!(
                    "{place}: glyphs: expected a [glyphs] table of name = \"symbol\""
                )));
                toml::Table::new()
            }
        };
        for key in file.keys() {
            layer.errors.push(Problem::line(format!(
                "{place}: {key:?} is not a setting (only version, [[shapes]] and [glyphs])"
            )));
        }
        layer.read(shapes, glyphs);
        layer
    }

    fn read(&mut self, shapes: Vec<toml::Table>, glyphs: toml::Table) {
        let place = self.origin.place();
        for (i, mut table) in shapes.into_iter().enumerate() {
            match table.remove("id") {
                Some(toml::Value::String(id)) => {
                    self.definitions
                        .push((id, parse(toml::Value::Table(table))));
                }
                _ => self.errors.push(Problem::line(format!(
                    "{place}: shape {} has no id: give it one, like id = \"lane\"",
                    i + 1
                ))),
            }
        }
        for (name, symbol) in glyphs {
            let parsed = match symbol.as_str().map(|s| (s, s.chars().count())) {
                Some((s, 1)) if !s.chars().any(char::is_control) => {
                    Ok(s.chars().next().expect("one char"))
                }
                _ => Err(format!(
                    "a glyph is one symbol in quotes, like {name} = \"✓\"; got {symbol}"
                )),
            };
            self.glyphs.push((name, parsed));
        }
    }
}

fn parse(value: toml::Value) -> Result<Definition, Problem> {
    (value.try_into::<Definition>()).map_err(|e| Problem::toml(&e))
}

static CONFIGURED: OnceLock<Vec<Layer>> = OnceLock::new();

/// The layers every workspace gets after the built-in one: extensions' shapes and config.toml's,
/// which the frontend reads and sets once at startup.
pub fn set_configured(layers: Vec<Layer>) {
    let _ = CONFIGURED.set(layers);
}

/// The shapes a workspace draws with: built in, the configured layers, then its own file.
pub fn for_workspace(root: &Path) -> Shapes {
    let configured = CONFIGURED.get().cloned().unwrap_or_default();
    load(
        std::iter::once(Layer::built_in())
            .chain(configured)
            .chain(Layer::workspace(root)),
    )
}

/// The shapes a workspace draws with, and `d` as the workspace's shape `id`, last: what the shape
/// editor draws, binds and checks before it saves. Its mistakes are the errors about `id`.
pub fn with_draft(root: &Path, id: &str, d: &Definition) -> Shapes {
    let configured = CONFIGURED.get().cloned().unwrap_or_default();
    let mut draft = Layer::empty(Origin::Workspace {
        path: root.join(WORKSPACE_FILE),
    });
    draft.definitions.push((id.to_string(), Ok(d.clone())));
    load(
        std::iter::once(Layer::built_in())
            .chain(configured)
            .chain(Layer::workspace(root))
            .chain(std::iter::once(draft)),
    )
}

/// A problem with the shape `id` from `place`: said there, and about the shape, so the shape
/// editor finds its draft's. Every problem with one shape is made here.
fn shape_problem(place: &str, id: &str, why: Problem) -> Problem {
    why.at(format!("{place}: shape {id}")).about(id)
}

/// Loads layers in order. A later definition of an id or a glyph replaces an earlier one, and
/// says so in a note. Broken definitions are dropped, and so are the shapes that need them (by
/// nesting or `rebind`), until every kept shape builds.
pub fn load(layers: impl IntoIterator<Item = Layer>) -> Shapes {
    let mut errors = Vec::new();
    let mut notes = Vec::new();
    let mut kept: IndexMap<String, (Origin, Definition)> = IndexMap::new();
    let mut glyphs: IndexMap<String, Glyph> = IndexMap::new();
    for layer in layers {
        errors.extend(layer.errors);
        let place = layer.origin.place();
        for (name, parsed) in layer.glyphs {
            match parsed {
                Err(e) => errors.push(Problem::line(format!("{place}: [glyphs] {name}: {e}"))),
                Ok(_) if !is_name(&name, false) => errors.push(Problem::line(format!(
                    "{place}: [glyphs] {name:?}: a glyph's name is letters, digits, - and _"
                ))),
                Ok(symbol) => {
                    if let Some(old) = glyphs.shift_remove(&name) {
                        notes.push(format!(
                            "{place}: glyph {name} replaces the one from {}",
                            old.origin
                        ));
                    }
                    let origin = layer.origin.clone();
                    glyphs.insert(
                        name.clone(),
                        Glyph {
                            name,
                            symbol,
                            origin,
                        },
                    );
                }
            }
        }
        for (id, parsed) in layer.definitions {
            let problems = match &parsed {
                Err(e) => vec![e.clone()],
                Ok(d) => {
                    let mut p = check(&id, d);
                    let asks =
                        d.regions.iter().any(|r| r.extension) || d.on.iter().any(|b| b.extension);
                    if asks && !matches!(layer.origin, Origin::Extension { .. }) {
                        p.push(
                            "extension = true hands a region or a gesture to the extension that ships the shape, and this one comes from none".into(),
                        );
                    }
                    p.into_iter().map(Problem::line).collect()
                }
            };
            match parsed {
                Ok(d) if problems.is_empty() => {
                    if let Some((old, _)) = kept.shift_remove(&id) {
                        notes.push(format!("{place}: shape {id} replaces the one from {old}"));
                    }
                    kept.insert(id, (layer.origin.clone(), d));
                }
                _ => errors.extend((problems.into_iter()).map(|p| shape_problem(&place, &id, p))),
            }
        }
    }
    let symbols: HashMap<&str, char> = glyphs
        .values()
        .map(|g| (g.name.as_str(), g.symbol))
        .collect();
    // Build every kept shape; drop the ones that fail and build again, as a shape that nests or
    // rebinds to a dropped one fails too.
    let built = loop {
        let mut built: HashMap<String, Result<Arc<canvas::Shape>, String>> = HashMap::new();
        for id in kept.keys() {
            build(id, &kept, &symbols, &mut built, &mut Vec::new());
        }
        let mut failed: Vec<(String, Problem)> = Vec::new();
        for (id, (_, d)) in &kept {
            let problem = match &built[id] {
                Err(e) => Some(Problem::line(e.clone())),
                Ok(_) => (d.on.iter())
                    .flat_map(|b| {
                        steps::unknown_rebinds(&b.written(), &|to: &str| kept.contains_key(to))
                    })
                    .next(),
            };
            failed.extend(problem.map(|p| (id.clone(), p)));
        }
        if failed.is_empty() {
            break built;
        }
        for (id, problem) in failed {
            if let Some((origin, _)) = kept.shift_remove(&id) {
                errors.push(shape_problem(&origin.place(), &id, problem));
            }
        }
    };
    let shapes = (kept.into_iter())
        .map(|(id, (origin, definition))| {
            let shape = built[&id].clone().expect("only shapes that built are kept");
            Loaded {
                id,
                origin,
                definition,
                shape,
            }
        })
        .collect();
    Shapes {
        shapes,
        glyphs: glyphs.into_values().collect(),
        errors,
        notes,
    }
}

/// Builds `id` (and first the shapes it nests) into `built`. `path` is the nesting being built,
/// to catch a cycle.
fn build(
    id: &str,
    kept: &IndexMap<String, (Origin, Definition)>,
    glyphs: &HashMap<&str, char>,
    built: &mut HashMap<String, Result<Arc<canvas::Shape>, String>>,
    path: &mut Vec<String>,
) {
    if built.contains_key(id) {
        return;
    }
    let (_, d) = &kept[id];
    path.push(id.to_string());
    for nested in d.parts.iter().filter_map(|p| match p {
        PartDef::Shape(n) => Some(n.shape.as_str()),
        _ => None,
    }) {
        if let Some(at) = path.iter().position(|p| p == nested) {
            let mut cycle = path[at..].to_vec();
            cycle.push(nested.to_string());
            let e = format!("nested shapes make a cycle: {}", cycle.join(" → "));
            for member in &path[at..] {
                built.insert(member.clone(), Err(e.clone()));
            }
        } else if kept.contains_key(nested) {
            build(nested, kept, glyphs, built, path);
        }
    }
    path.pop();
    if built.contains_key(id) {
        return;
    }
    let result =
        assemble(d, built).map(|shape| Arc::new(shape.map_text(|v| with_glyphs(v, glyphs))));
    built.insert(id.to_string(), result);
}

/// The canvas shape of a definition whose nested shapes are built.
fn assemble(
    d: &Definition,
    built: &HashMap<String, Result<Arc<canvas::Shape>, String>>,
) -> Result<canvas::Shape, String> {
    let mut parts: Vec<Part> = Vec::new();
    let mut anchors: Vec<(String, Point)> = (d.anchors.iter())
        .map(|(name, xy)| (name.clone(), point(*xy)))
        .collect();
    let resolve = |parts: &[Part], anchors: &[(String, Point)], pos: &Pos| match pos {
        Pos::Point(xy) => Ok(point(*xy)),
        Pos::Anchor(name) => canvas::anchor_in(parts, anchors, name).ok_or_else(|| {
            format!("no anchor is called `{name}` (parts can only name the parts before them)")
        }),
    };
    for part in &d.parts {
        let new = |id: &Option<String>, kind| Part::new(id.as_deref(), kind);
        match part {
            PartDef::Rect(r) => parts.push(new(&r.id, Kind::Rect(rect(r.from, r.to)))),
            PartDef::Circle(c) => parts.push(new(
                &c.id,
                Kind::Circle {
                    center: point(c.center),
                    radius: c.radius,
                },
            )),
            PartDef::Ellipse(e) => parts.push(new(
                &e.id,
                Kind::Ellipse {
                    center: point(e.center),
                    radii: point(e.radii),
                },
            )),
            PartDef::Line(s) | PartDef::Arrow(s) => {
                let from = resolve(&parts, &anchors, &s.from)?;
                let to = resolve(&parts, &anchors, &s.to)?;
                let kind = match part {
                    PartDef::Line(_) => Kind::Line { from, to },
                    _ => Kind::Arrow { from, to },
                };
                parts.push(new(&s.id, kind));
            }
            PartDef::Path(p) => {
                parts.push(new(&p.id, Kind::Path(p.build())));
                if let Some(path) = &p.id {
                    anchors.extend(p.named().map(|(v, xy)| (format!("{path}.{v}"), point(xy))));
                }
            }
            PartDef::Text(t) => {
                let value = t.value.clone();
                let kind = match (&t.at, t.from, t.to) {
                    (Some(at), None, None) => Kind::Label {
                        at: resolve(&parts, &anchors, at)?,
                        side: match (t.side, at) {
                            (Some(s), _) => s.into(),
                            (None, Pos::Anchor(name)) => Side::outside(name),
                            (None, Pos::Point(_)) => Side::Center,
                        },
                        value,
                    },
                    (None, Some(from), Some(to)) => Kind::Text {
                        bounds: rect(from, to),
                        value,
                    },
                    _ => unreachable!("check() lets through only `at`, or `from` and `to`"),
                };
                parts.push(new(&t.id, kind));
            }
            PartDef::Shape(n) => {
                let inner = match built.get(&n.shape) {
                    Some(Ok(s)) => s,
                    Some(Err(_)) => {
                        return Err(format!("it nests shape {}, which didn't load", n.shape));
                    }
                    None => return Err(format!("it nests shape {}, which isn't loaded", n.shape)),
                };
                let (from, to) = (point(n.from), point(n.to));
                let map = |p: Point| {
                    Point::new(
                        from.x + p.x * (to.x - from.x),
                        from.y + p.y * (to.y - from.y),
                    )
                };
                let (sx, sy) = ((to.x - from.x).abs(), (to.y - from.y).abs());
                for p in inner.parts() {
                    let kind = match &p.kind {
                        Kind::Rect(r) => Kind::Rect(rect_of(map(r.min), map(r.max))),
                        Kind::Circle { center, radius } if (sx - sy).abs() < 1e-12 => {
                            Kind::Circle {
                                center: map(*center),
                                radius: radius * sx,
                            }
                        }
                        Kind::Circle { center, radius } => Kind::Ellipse {
                            center: map(*center),
                            radii: Point::new(radius * sx, radius * sy),
                        },
                        Kind::Ellipse { center, radii } => Kind::Ellipse {
                            center: map(*center),
                            radii: Point::new(radii.x * sx, radii.y * sy),
                        },
                        Kind::Line { from, to } => Kind::Line {
                            from: map(*from),
                            to: map(*to),
                        },
                        Kind::Arrow { from, to } => Kind::Arrow {
                            from: map(*from),
                            to: map(*to),
                        },
                        Kind::Path(path) => Kind::Path(canvas::Path::polyline(
                            path.points().iter().copied().map(map).collect(),
                            path.is_closed(),
                        )),
                        Kind::Text { bounds, value } => Kind::Text {
                            bounds: rect_of(map(bounds.min), map(bounds.max)),
                            value: value.clone(),
                        },
                        Kind::Label { at, side, value } => Kind::Label {
                            at: map(*at),
                            side: *side,
                            value: value.clone(),
                        },
                    };
                    let id = p.id.as_ref().map(|inner| format!("{}.{inner}", n.id));
                    parts.push(Part { id, kind });
                }
                anchors.extend(
                    (inner.anchors().iter()).map(|(name, p)| (format!("{}.{name}", n.id), map(*p))),
                );
                let unit = rect_of(from, to);
                anchors.push((n.id.clone(), unit.anchor("center").expect("a box anchor")));
                for name in BOX_ANCHORS {
                    let at = unit.anchor(name).expect("a box anchor");
                    anchors.push((format!("{}.{name}", n.id), at));
                }
            }
        }
    }
    let regions: Vec<(&str, &str)> = (d.regions.iter())
        .map(|r| (r.id.as_str(), r.area.as_str()))
        .collect();
    canvas::Shape::new(parts, &regions)
        .and_then(|s| s.with_anchors(anchors))
        .map_err(|e| e.to_string())
}

const BOX_ANCHORS: [&str; 9] = [
    "center",
    "top",
    "bottom",
    "left",
    "right",
    "top-left",
    "top-right",
    "bottom-left",
    "bottom-right",
];

fn rect(a: Xy, b: Xy) -> Rect {
    rect_of(point(a), point(b))
}

fn rect_of(a: Point, b: Point) -> Rect {
    Rect::around([a, b]).expect("two points")
}

/// What a definition gets wrong on its own, without the other shapes: one line each.
fn check(id: &str, d: &Definition) -> Vec<String> {
    let mut p = Vec::new();
    if !is_name(id, true) {
        p.push(format!(
            "{id:?} is not an id: letters, digits, -, _ and ., starting with a letter"
        ));
    }
    if d.parts.is_empty() {
        p.push("it has no parts".into());
    }
    if let Some(a) = d.aspect
        && !(a.is_finite() && a > 0.0)
    {
        p.push(format!("aspect = {a}: it is width over height, above 0"));
    }
    if d.key.as_deref() == Some("") {
        p.push("key is empty: name a field, or leave it out".into());
    }
    let mut inputs = HashSet::new();
    for input in &d.inputs {
        if !is_name(input, false) {
            p.push(format!("input {input:?} is not a name"));
        } else if !inputs.insert(input.as_str()) {
            p.push(format!("two inputs are called {input}"));
        }
    }
    for (i, part) in d.parts.iter().enumerate() {
        let at = format!("part {}", i + 1);
        match part {
            PartDef::Circle(c) if !positive(c.radius) => {
                p.push(format!("{at}: a circle's radius is above 0"))
            }
            PartDef::Ellipse(e) if !(positive(e.radii[0]) && positive(e.radii[1])) => {
                p.push(format!("{at}: an ellipse's radii are above 0"))
            }
            PartDef::Path(path) => p.extend(check_path(path).map(|e| format!("{at}: {e}"))),
            PartDef::Text(t) => {
                if !matches!(
                    (&t.at, t.from, t.to),
                    (Some(_), None, None) | (None, Some(_), Some(_))
                ) {
                    p.push(format!(
                        "{at}: a text has `at` (a label beside a point) or `from` and `to` (a box), not both"
                    ));
                }
                if t.side.is_some() && t.at.is_none() {
                    p.push(format!("{at}: `side` is for a text `at` a point"));
                }
                for name in placeholders(&t.value) {
                    let input = name.split('.').next().unwrap_or(name);
                    if !inputs.contains(input) {
                        p.push(format!(
                            "{at}: `${{{name}}}`: {input} is not one of its inputs"
                        ));
                    }
                }
            }
            PartDef::Shape(n) if !is_name(&n.id, false) => p.push(format!(
                "{at}: a nested shape's id {:?} is not a name",
                n.id
            )),
            _ => {}
        }
        if let Some(id) = part.id()
            && !is_name(id, false)
        {
            p.push(format!("{at}: id {id:?} is not a name"));
        }
    }
    let mut regions = HashSet::new();
    for r in &d.regions {
        regions.insert(r.id.as_str());
        if let Some(shows) = &r.shows
            && let Err(e) = crate::shows::Shows::parse(shows, &d.inputs)
        {
            p.push(format!("region {}: shows {shows:?}: {e}", r.id));
        }
        if r.extension && r.shows.is_some() {
            p.push(format!(
                "region {}: its extension computes what it shows, so it has no shows",
                r.id
            ));
        }
    }
    let part_ids: HashSet<&str> = d.parts.iter().filter_map(PartDef::id).collect();
    let mut actions = HashSet::new();
    for (i, b) in d.on.iter().enumerate() {
        let at = format!("on {}", i + 1);
        let gestures = [
            b.drop.is_some(),
            b.click.is_some(),
            b.key.is_some(),
            b.action.is_some(),
        ];
        if gestures.iter().filter(|g| **g).count() != 1 {
            p.push(format!(
                "{at}: say what starts it with exactly one of drop, click, key and action"
            ));
        }
        if let Some(drop) = &b.drop {
            for r in [&drop.from, &drop.to] {
                if !regions.contains(r.as_str()) {
                    p.push(format!("{at}: drop: no region is called {r}"));
                }
            }
        }
        if let Some(c) = &b.click
            && !regions.contains(c.as_str())
            && !part_ids.contains(c.as_str())
        {
            p.push(format!("{at}: click: no region or part is called {c}"));
        }
        if let Some(a) = &b.action
            && (!is_name(a, false) || !actions.insert(a.as_str()))
        {
            p.push(format!(
                "{at}: action {a:?} must be a name, and only one action may have it"
            ));
        }
        let ways = [b.edit.is_some(), !b.steps.is_empty(), b.extension];
        if ways.iter().filter(|w| **w).count() != 1 {
            p.push(format!(
                "{at}: give its steps as do = [ … ], or one edit = {{ … }}, or extension = true"
            ));
        }
        let wrong = steps::edits(&b.written());
        p.extend(wrong.into_iter().map(|e| format!("{at}: {}", e.one_line())));
    }
    p
}

/// What a path gets wrong: its form, its size and the names of its vertices.
fn check_path(path: &PathDef) -> impl Iterator<Item = String> {
    let mut p = Vec::new();
    match (path.points.is_empty(), path.start, path.segments.is_empty()) {
        (false, Some(_), _) | (false, _, false) => p.push(
            "a path is points = [ … ], or a start and segments = [ … ], not both".to_string(),
        ),
        (_, None, false) => {
            p.push("a path's segments go on from a start = [x, y]: give it one".into())
        }
        _ if path.vertices() < 2 => p.push("a path needs two points or more".into()),
        _ => {}
    }
    let size = path.size();
    if size > PATH_LIMIT {
        p.push(format!(
            "this path is {size} points once its curves are flattened (a curve is {}), and a path may be at most {PATH_LIMIT}, so a screen of shapes still draws within a frame: simplify it or split it into several paths",
            canvas::part::CUBIC_STEPS
        ));
    }
    let mut seen = HashSet::new();
    for (name, _) in path.named() {
        if !is_name(name, false) {
            p.push(format!("vertex id {name:?} is not a name"));
        } else if name == "start" || name == "end" || !seen.insert(name) {
            p.push(format!(
                "two vertices are called {name} (start and end are the path's own)"
            ));
        }
    }
    if !seen.is_empty() && path.id.is_none() {
        p.push("its vertices have ids but the path has none: give it an id, and a vertex `tip` is the anchor `<path>.tip`".into());
    }
    p.into_iter()
}

/// Above 0, and not NaN.
fn positive(x: f64) -> bool {
    x > 0.0
}

/// The names in `${…}`.
fn placeholders(text: &str) -> impl Iterator<Item = &str> {
    text.split("${")
        .skip(1)
        .filter_map(|s| s.split_once('}').map(|(n, _)| n))
}

/// `text` with each `${name}` replaced by `value(name)`.
pub fn fill(text: &str, value: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str(&value(&rest[start + 2..start + len]));
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out
}

/// `text` with each `:name:` of a known glyph replaced by its symbol; other colons stay.
fn with_glyphs(text: &str, glyphs: &HashMap<&str, char>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(':') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after
            .find(':')
            .and_then(|end| Some((end, glyphs.get(&after[..end])?)))
        {
            Some((end, symbol)) => {
                out.push(*symbol);
                rest = &after[end + 1..];
            }
            None => {
                out.push(':');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workspace(text: &str) -> Layer {
        let path = PathBuf::from("/w/.cold-lava/shapes.toml");
        Layer::from_file(Origin::Workspace { path }, Ok(text.to_string()))
    }

    fn config(text: &str) -> Layer {
        let mut file: toml::Table = toml::from_str(text).unwrap();
        let shapes = match file.remove("shapes") {
            Some(toml::Value::Array(a)) => a
                .into_iter()
                .map(|v| v.as_table().unwrap().clone())
                .collect(),
            _ => Vec::new(),
        };
        let glyphs = match file.remove("glyphs") {
            Some(toml::Value::Table(t)) => t,
            _ => toml::Table::new(),
        };
        Layer::config(Path::new("/c/config.toml"), shapes, glyphs)
    }

    /// The errors of a workspace file holding `shapes`, after the built-in layer.
    fn errors_of(shapes: &str) -> Vec<String> {
        let text = format!("version = 1\n{shapes}");
        said(&load([Layer::built_in(), workspace(&text)]))
    }

    /// Each error as it is said.
    fn said(s: &Shapes) -> Vec<String> {
        s.errors.iter().map(Problem::one_line).collect()
    }

    fn ids(s: &Shapes) -> Vec<&str> {
        s.shapes.iter().map(|l| l.id.as_str()).collect()
    }

    #[test]
    fn built_ins_load_from_their_own_toml() {
        let s = load([Layer::built_in()]);
        assert_eq!(said(&s), Vec::<String>::new());
        assert_eq!(
            ids(&s),
            [
                "text", "rect", "circle", "line", "arrow", "card", "sticky", "table", "venn2"
            ]
        );
        let glyphs: String = s.glyphs.iter().map(|g| g.symbol).collect();
        assert_eq!(glyphs, "✓✗•★◆→");
        let venn = s.get("venn2").unwrap();
        assert_eq!(venn.shape.regions().len(), 3);
        assert_eq!(venn.origin, Origin::BuiltIn);
        let sticky = s.get("sticky").unwrap();
        assert_eq!(
            sticky.definition.on[0].action.as_deref(),
            Some("make-a-note")
        );
    }

    #[test]
    fn later_origins_replace_earlier_ones_and_say_so() {
        let ext = Layer::extension(
            "lanes",
            [(
                "lanes.lane".to_string(),
                toml::from_str("parts = [{ rect = { from = [0, 0], to = [1, 1] } }]").unwrap(),
            )],
        );
        let config = config(
            "[[shapes]]\nid = \"lanes.lane\"\nparts = [{ circle = { center = [0.5, 0.5], radius = 0.5 } }]\n\
             [glyphs]\ncheck = \"√\"",
        );
        let ws = workspace(
            "version = 1\n[[shapes]]\nid = \"rect\"\ntitle = \"Mine\"\nparts = [{ rect = { from = [0, 0], to = [1, 0.5] } }]",
        );
        let s = load([Layer::built_in(), ext, config, ws]);
        assert_eq!(said(&s), Vec::<String>::new());
        let lane = s.get("lanes.lane").unwrap();
        assert_eq!(
            lane.origin,
            Origin::Config {
                path: "/c/config.toml".into()
            }
        );
        assert_eq!(s.get("rect").unwrap().definition.title, "Mine");
        assert_eq!(
            s.notes,
            [
                "/c/config.toml: glyph check replaces the one from built in",
                "/c/config.toml: shape lanes.lane replaces the one from extension lanes",
                "/w/.cold-lava/shapes.toml: shape rect replaces the one from built in",
            ]
        );
        assert_eq!(*ids(&s).last().unwrap(), "rect", "a replacement goes last");
        assert_eq!(s.glyphs.last().unwrap().symbol, '√');
    }

    #[test]
    fn each_mistake_drops_its_shape_with_a_clear_error() {
        let cases = [
            (
                "parts = [{ hexagon = { center = [0.5, 0.5] } }]",
                "unknown variant `hexagon`, expected one of `rect`, `circle`, `ellipse`, `line`, `arrow`, `path`, `text`, `shape`",
            ),
            (
                "parts = [{ circle = { id = \"a\", center = [0.5, 0.5], radius = 0.4 } }]\nregions = [{ id = \"r\", area = \"a & b\" }]",
                "region `r`: no part is called `b`",
            ),
            (
                "parts = [{ circle = { id = \"a\", center = [0.5, 0.5], radius = 0.4 } }]\nregions = [{ id = \"r\", area = \"a &\" }]",
                "region `r`: the area expression ends early (at 3)",
            ),
            (
                "parts = [{ line = { from = \"a.top\", to = [1, 1] } }]",
                "no anchor is called `a.top` (parts can only name the parts before them)",
            ),
            (
                "parts = [{ rect = { id = \"a\", from = [0, 0], to = [1, 1] } }, { rect = { id = \"a\", from = [0, 0], to = [1, 1] } }]",
                "two parts are called `a`",
            ),
            (
                "anchors = { a = [0.5, 0.5] }\nparts = [{ rect = { id = \"a\", from = [0, 0], to = [1, 1] } }]",
                "`a` names an anchor and another anchor or part",
            ),
            ("parts = []", "it has no parts"),
            (
                "parts = [{ text = { value = \"hi\", at = \"center\", from = [0, 0], to = [1, 1] } }]",
                "part 1: a text has `at` (a label beside a point) or `from` and `to` (a box), not both",
            ),
            (
                "inputs = [\"a\"]\nparts = [{ text = { value = \"${b.title}\", at = \"center\" } }]",
                "part 1: `${b.title}`: b is not one of its inputs",
            ),
            (
                "inputs = [\"a\"]\nparts = [{ circle = { id = \"c\", center = [0.5, 0.5], radius = 0.4 } }]\nregions = [{ id = \"r\", area = \"c\", shows = \"a & b where done = true\" }]",
                "region r: shows \"a & b where done = true\": b is not one of its inputs",
            ),
            (
                "inputs = [\"a\"]\nparts = [{ circle = { id = \"c\", center = [0.5, 0.5], radius = 0.4 } }]\nregions = [{ id = \"r\", area = \"c\", shows = \"a limit 3 sort by name\" }]",
                "region r: shows \"a limit 3 sort by name\": unexpected `sort`: after the sets come where, sort by, limit and label, each once and in that order",
            ),
            (
                "parts = [{ circle = { center = [0.5, 0.5], radius = 0 } }]",
                "part 1: a circle's radius is above 0",
            ),
            (
                "parts = [{ circle = { id = \"c\", center = [0.5, 0.5], radius = 0.4 } }]\n[[shapes.on]]\nclick = \"c\"\nkey = \"x\"\nedit = { item = \"clicked\", toggle = true }",
                "on 1: say what starts it with exactly one of drop, click, key and action",
            ),
            (
                "parts = [{ circle = { id = \"c\", center = [0.5, 0.5], radius = 0.4 } }]\n[[shapes.on]]\ndrop = { from = \"c\", to = \"nowhere\" }\nedit = { item = \"dropped\", toggle = true }",
                "on 1: drop: no region is called c",
            ),
            (
                "parts = [{ circle = { id = \"c\", center = [0.5, 0.5], radius = 0.4 } }]\n[[shapes.on]]\naction = \"go\"\ndo = [{ edit = { item = \"x\", toggle = true, recipe = \"r\" } }]",
                "on 1: an edit is exactly one of set-field, toggle and recipe",
            ),
            (
                "parts = [{ rect = { from = [0, 0], to = [1, 1] } }]\n[[shapes.on]]\naction = \"go\"\ndo = [{ rebind = { shape = \"nowhere\" } }]",
                "its rebind names shape nowhere, which isn't loaded",
            ),
            (
                "parts = [{ shape = { id = \"n\", use = \"nowhere\", from = [0, 0], to = [1, 1] } }]",
                "it nests shape nowhere, which isn't loaded",
            ),
            (
                "parts = [{ path = { points = [[0, 0]] } }]",
                "part 1: a path needs two points or more",
            ),
            (
                "parts = [{ path = { start = [0, 0], segments = [] } }]",
                "part 1: a path needs two points or more",
            ),
            (
                "parts = [{ path = { points = [[0, 0], [1, 1]], start = [0, 0], segments = [{ line = [1, 0] }] } }]",
                "part 1: a path is points = [ … ], or a start and segments = [ … ], not both",
            ),
            (
                "parts = [{ path = { segments = [{ line = [1, 0] }] } }]",
                "part 1: a path's segments go on from a start = [x, y]: give it one",
            ),
            (
                "parts = [{ path = { start = [0, 0], segments = [{ curve = { c1 = [0, 1], c2 = [1, 1] } }] } }]",
                "missing field `to`",
            ),
            (
                "parts = [{ path = { start = [0, 0], segments = [{ line = { id = \"tip\" } }] } }]",
                "a line is a point, line = [x, y], or line = { to = [x, y], id = \"name\" }",
            ),
            (
                "parts = [{ path = { start = [0, 0], segments = [{ arc = [1, 0] }] } }]",
                "unknown variant `arc`, expected one of `line`, `curve`, `quad`",
            ),
            (
                "parts = [{ path = { start = [0, 0], segments = [{ line = { to = [1, 0], id = \"tip\" } }] } }]",
                "part 1: its vertices have ids but the path has none",
            ),
            (
                "parts = [{ path = { id = \"p\", start = [0, 0], segments = [{ line = { to = [1, 0], id = \"end\" } }] } }]",
                "part 1: two vertices are called end (start and end are the path's own)",
            ),
        ];
        for (shape, expected) in cases {
            let text = format!(
                "[[shapes]]\nid = \"bad\"\n{shape}\n\n[[shapes]]\nid = \"fine\"\nparts = [{{ rect = {{ from = [0, 0], to = [1, 1] }} }}]\n"
            );
            let s = load([
                Layer::built_in(),
                workspace(&format!("version = 1\n{text}")),
            ]);
            let lines = said(&s);
            let errors: Vec<&str> = (lines.iter())
                .map(|e| {
                    e.strip_prefix("/w/.cold-lava/shapes.toml: shape bad: ")
                        .unwrap_or(e)
                })
                .collect();
            assert!(
                errors.iter().any(|e| e.contains(expected)),
                "{shape}\nwanted: {expected}\ngot: {errors:?}"
            );
            assert!(s.get("bad").is_none(), "{shape}");
            assert!(
                s.get("fine").is_some(),
                "a broken shape's siblings load: {shape}"
            );
        }
    }

    /// A bubble written with every kind of segment: its curves bend, its named tip is an anchor
    /// later parts use, and a region over it is its curved inside.
    #[test]
    fn curved_paths_load_with_their_anchors_and_regions() {
        let shapes = load([
            Layer::built_in(),
            workspace(
                "version = 1\n[[shapes]]\nid = \"bubble\"\nparts = [\n\
                 { path = { id = \"b\", closed = true, start = [0.5, 0], segments = [\n\
                   { curve = { c1 = [1, 0], c2 = [1, 0.8], to = [0.5, 0.8] } },\n\
                   { line = { to = [0.2, 1], id = \"tip\" } },\n\
                   { quad = { c = [0, 0], to = [0.5, 0] } },\n\
                 ] } },\n\
                 { text = { at = \"b.tip\", value = \"hi\" } },\n\
                 ]\nregions = [{ id = \"inside\", area = \"b\" }]\n",
            ),
        ]);
        assert_eq!(said(&shapes), Vec::<String>::new());
        let bubble = &shapes.get("bubble").unwrap().shape;
        let p = Point::new;
        assert_eq!(bubble.anchor("b.tip"), Some(p(0.2, 1.0)));
        assert_eq!(bubble.anchor("b.start"), Some(p(0.5, 0.0)));
        assert_eq!(bubble.anchor("b.end"), Some(p(0.5, 0.0)));
        let Kind::Path(path) = &bubble.parts()[0].kind else {
            panic!("a path")
        };
        let steps = canvas::part::CUBIC_STEPS;
        assert_eq!(path.points().len(), 1 + steps + 1 + steps);
        // The curve bulges right to x = 0.875 at its middle; a straight side would stay at 0.5.
        let mid = path.points()[steps / 2];
        assert!((mid.x - 0.875).abs() < 1e-9, "{mid:?}");
        // The quad from the tip back to the start equals its cubic: halfway it is at
        // (0.2/4 + 0 + 0.5/4, 1/4 + 0 + 0) = (0.175, 0.25).
        let half = path.points()[steps + 1 + steps / 2];
        assert!(
            (half.x - 0.175).abs() < 1e-9 && (half.y - 0.25).abs() < 1e-9,
            "{half:?}"
        );
        let Kind::Label { at, .. } = &bubble.parts()[1].kind else {
            panic!("a label")
        };
        assert_eq!(*at, p(0.2, 1.0));
        let inside = &bubble.regions()[0].area;
        assert!(
            inside.contains(bubble.parts(), p(0.8, 0.4)),
            "inside the curve"
        );
        assert!(
            !inside.contains(bubble.parts(), p(0.95, 0.1)),
            "outside the curve's corner"
        );
    }

    #[test]
    fn a_path_over_the_limit_is_refused_wherever_it_comes_from() {
        let line = "{ line = [0.5, 0.5] }, ";
        let curve = "{ curve = { c1 = [0, 1], c2 = [1, 1], to = [1, 0] } }, ";
        let steps = canvas::part::CUBIC_STEPS;
        let fits = format!("{}{curve}", line.repeat(PATH_LIMIT - 1 - steps));
        let over = format!("{fits}{line}");
        let shape = |segments: &str| -> toml::Table {
            toml::from_str(&format!(
                "parts = [{{ path = {{ start = [0, 0], segments = [{segments}] }} }}]"
            ))
            .unwrap()
        };
        let s = load([Layer::extension(
            "big",
            [
                ("big.fits".to_string(), shape(&fits)),
                ("big.over".to_string(), shape(&over)),
            ],
        )]);
        assert_eq!(
            said(&s),
            [format!(
                "extension big: shape big.over: part 1: this path is {} points once its curves are flattened (a curve is {steps}), and a path may be at most {PATH_LIMIT}, so a screen of shapes still draws within a frame: simplify it or split it into several paths",
                PATH_LIMIT + 1
            )]
        );
        assert!(s.get("big.fits").is_some());
        let points = vec!["[0, 0]"; PATH_LIMIT + 1].join(", ");
        let errors = errors_of(&format!(
            "[[shapes]]\nid = \"dots\"\nparts = [{{ path = {{ points = [{points}] }} }}]"
        ));
        assert!(
            errors[0].contains(&format!("this path is {} points", PATH_LIMIT + 1)),
            "{errors:?}"
        );
    }

    #[test]
    fn nested_shapes_cycle_is_an_error_for_each_member() {
        let errors = errors_of(
            "[[shapes]]\nid = \"a\"\nparts = [{ shape = { id = \"x\", use = \"b\", from = [0, 0], to = [1, 1] } }]\n\
             [[shapes]]\nid = \"b\"\nparts = [{ shape = { id = \"y\", use = \"a\", from = [0, 0], to = [1, 1] } }]\n\
             [[shapes]]\nid = \"c\"\nparts = [{ shape = { id = \"z\", use = \"a\", from = [0, 0], to = [1, 1] } }]",
        );
        assert_eq!(
            errors,
            [
                "/w/.cold-lava/shapes.toml: shape a: nested shapes make a cycle: a → b → a",
                "/w/.cold-lava/shapes.toml: shape b: nested shapes make a cycle: a → b → a",
                "/w/.cold-lava/shapes.toml: shape c: it nests shape a, which didn't load",
            ]
        );
    }

    #[test]
    fn file_and_entry_mistakes() {
        let skipped = load([workspace("[[shapes]]\nid = \"a\"")]);
        assert_eq!(
            said(&skipped),
            [
                "/w/.cold-lava/shapes.toml: no version: start the file with version = 1; the file is skipped"
            ]
        );
        let errors = errors_of("[[shapes]]\ntitle = \"x\"\n[glyphs]\nwide = \"ab\"\n[colours]");
        assert_eq!(
            errors,
            [
                "/w/.cold-lava/shapes.toml: \"colours\" is not a setting (only version, [[shapes]] and [glyphs])",
                "/w/.cold-lava/shapes.toml: shape 1 has no id: give it one, like id = \"lane\"",
                "/w/.cold-lava/shapes.toml: [glyphs] wide: a glyph is one symbol in quotes, like wide = \"✓\"; got \"ab\"",
            ]
        );
    }

    #[test]
    fn nested_parts_are_named_under_the_nest_and_follow_its_box() {
        let s = load([
            Layer::built_in(),
            workspace(
                "version = 1\n[[shapes]]\nid = \"pair\"\nparts = [\n\
                 { shape = { id = \"l\", use = \"circle\", from = [0, 0], to = [0.5, 1] } },\n\
                 { arrow = { from = \"l.right\", to = [1, 0.5] } },\n\
                 { text = { at = \"l.top\", value = \":star: top\" } },\n]\n\
                 regions = [{ id = \"inside\", area = \"l.circle\" }]",
            ),
        ]);
        assert_eq!(said(&s), Vec::<String>::new());
        let pair = &s.get("pair").unwrap().shape;
        let parts = pair.parts();
        assert_eq!(parts[0].id.as_deref(), Some("l.circle"));
        assert_eq!(
            parts[0].kind,
            Kind::Ellipse {
                center: Point::new(0.25, 0.5),
                radii: Point::new(0.25, 0.5)
            },
            "a circle in a box twice as tall is an ellipse"
        );
        assert_eq!(
            parts[1].kind,
            Kind::Arrow {
                from: Point::new(0.5, 0.5),
                to: Point::new(1.0, 0.5)
            }
        );
        assert_eq!(
            parts[2].kind,
            Kind::Label {
                at: Point::new(0.25, 0.0),
                side: Side::Above,
                value: "★ top".into()
            },
            "the glyph is looked up, the side follows the anchor"
        );
        assert_eq!(pair.anchor("l"), Some(Point::new(0.25, 0.5)));
    }

    #[test]
    fn the_guides_example_shape_loads() {
        let guide = include_str!("../../../docs/extensions-guide.md");
        let start = guide.find("[[shapes]]\nid = \"csv-table.sheet\"").unwrap();
        let toml = &guide[start..start + guide[start..].find("```").unwrap()];
        let s = load([Layer::built_in(), config(toml)]);
        assert_eq!(said(&s), Vec::<String>::new());
        assert_eq!(
            s.get("csv-table.sheet").unwrap().shape.regions()[0].id,
            "rows"
        );
    }

    #[test]
    fn placeholders_and_glyphs() {
        assert_eq!(
            fill("${a.title} and ${b}", |n| n.to_uppercase()),
            "A.TITLE and B"
        );
        assert_eq!(fill("no ${end", |_| "x".into()), "no ${end");
        let glyphs = HashMap::from([("check", '✓')]);
        assert_eq!(
            with_glyphs("done :check: at 10:30 :nope:", &glyphs),
            "done ✓ at 10:30 :nope:"
        );
    }
}
