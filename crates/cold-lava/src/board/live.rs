//! The placed shapes of the open canvas, live: each shaped node's binding (its inputs followed
//! through the core), what it paints at the current view, and what is under the mouse. Nothing
//! here is saved; it all follows from the nodes (`placed`) and the workspace.

use super::CNode;
use super::placed::{Stored, UNIT};
use crate::act::{Answered, Ask, ShapeAnswer};
use crate::bind::{Binding, Entry};
use crate::core::{Core, Event};
use crate::gateway::News;
use crate::shapes::Shapes;
use cold_lava_canvas::fit::{self, Align, Flow, Room};
use cold_lava_canvas::{
    Grid, Handle, Hit, Mask, Placed, Placement, Point, Rect, Res, Style, Viewport, draw, hit,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// The number for the next binding of a placed shape. Never reused while the program runs, so
/// an extension's late answer to a binding that is gone (the shape editor's save binds every
/// shape again, a closed board lets go of its own) names no binding and is dropped.
static NEXT_PLACED: AtomicU64 = AtomicU64::new(1);

/// The shapes of one canvas, by node id.
pub struct Live {
    pub shapes: Shapes,
    nodes: HashMap<String, Shaped>,
    /// `shape/region` asks waiting to go to the extension that ships the shape.
    asks: Vec<(String, Ask)>,
}

struct Shaped {
    /// The shape and its sources as saved: a change binds it again.
    key: String,
    binding: Result<Binding, String>,
    /// Names this binding of the placed shape in an extension's answers.
    placed: u64,
    /// Bumped whenever what it shows may have changed.
    version: u64,
    painted: Option<Painted>,
}

/// One shape painted at one view: its cells from `(col, row)` of the board's area, and where
/// each region's members were put.
pub struct Painted {
    key: PaintKey,
    pub col: i32,
    pub row: i32,
    pub grid: Grid,
    spots: Vec<Spot>,
}

#[derive(PartialEq)]
struct PaintKey {
    version: u64,
    /// The placement relative to the grid's top-left corner, so panning by whole cells reuses it.
    at: (i64, i64, i64, i64, i64),
    cell: (i64, i64),
    size: (u16, u16),
}

/// A member's label where it was painted: region `region`'s entry `entry`, on `row` of the
/// shape's grid from `col`, `width` cells.
struct Spot {
    region: usize,
    entry: usize,
    row: u16,
    col: u16,
    width: u16,
}

/// A member of a region under the mouse: the region's id and the member's id.
pub struct Member {
    pub region: String,
    pub item: String,
}

fn q(v: f64) -> i64 {
    (v * 1000.0).round() as i64
}

impl Live {
    pub fn new(shapes: Shapes) -> Live {
        Live {
            shapes,
            nodes: HashMap::new(),
            asks: Vec::new(),
        }
    }

    /// Bind each shaped node whose shape or sources changed, and let go of nodes that are gone
    /// or no longer shapes. Returns what couldn't be bound, once per change.
    pub fn sync(&mut self, core: &mut Core, nodes: &[CNode]) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen = Vec::new();
        for node in nodes {
            let Some(stored) = Stored::of(node) else {
                continue;
            };
            seen.push(node.id.clone());
            let (key, made) = match stored {
                Ok(s) => {
                    let sources = s.sources(node);
                    let key = format!("{}{}", s.shape, serde_json::json!(sources));
                    (key, Some((s.shape, sources)))
                }
                Err(e) => (format!("error {e}"), None),
            };
            if self.nodes.get(&node.id).is_some_and(|s| s.key == key) {
                continue;
            }
            if let Some(mut old) = self.nodes.remove(&node.id)
                && let Ok(b) = &mut old.binding
            {
                b.close(core);
            }
            let binding = match made {
                Some((shape, sources)) => Binding::new(core, &self.shapes, &shape, sources),
                None => Err(format!("a placed shape doesn't read: {key}")),
            };
            if let Err(e) = &binding {
                problems.push(e.clone());
            }
            let placed = NEXT_PLACED.fetch_add(1, Ordering::Relaxed);
            let shaped = Shaped {
                key,
                binding,
                placed,
                version: 0,
                painted: None,
            };
            self.ask_regions(&shaped);
            self.nodes.insert(node.id.clone(), shaped);
        }
        let gone: Vec<String> = (self.nodes.keys())
            .filter(|id| !seen.contains(id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(Shaped {
                binding: Ok(mut b), ..
            }) = self.nodes.remove(&id)
            {
                b.close(core);
            }
        }
        problems
    }

    fn ask_regions(&mut self, shaped: &Shaped) {
        if let Ok(b) = &shaped.binding
            && let Some(ext) = b.extension()
        {
            let asks = b.region_asks(shaped.placed);
            self.asks
                .extend(asks.into_iter().map(|a| (ext.to_string(), a)));
        }
    }

    /// `shape/region` asks for the extensions that ship shapes on this canvas.
    pub fn take_asks(&mut self) -> Vec<(String, Ask)> {
        std::mem::take(&mut self.asks)
    }

    /// An extension's answer about one of these shapes.
    pub fn answered(&mut self, answer: ShapeAnswer) {
        let placed = answer.ask.placed();
        let Some(shaped) = self.nodes.values_mut().find(|s| s.placed == placed) else {
            return;
        };
        if let (Ask::Region { region, .. }, Ok(Answered::Region { items }), Ok(b)) =
            (&answer.ask, answer.answer, &mut shaped.binding)
        {
            b.answered(region, items);
            shaped.version += 1;
        }
    }

    /// News for the canvas's subscriptions: the binding it is for reads its input again.
    pub fn heard(&mut self, core: &Core, news: &News) {
        let mut again = Vec::new();
        for (id, s) in &mut self.nodes {
            if let Ok(b) = &mut s.binding
                && b.heard(core, news)
            {
                s.version += 1;
                again.push(id.clone());
            }
        }
        self.ask_again(&again);
    }

    /// A core event: bindings of notes that changed read them again.
    pub fn noticed(&mut self, core: &Core, event: &Event) {
        let mut again = Vec::new();
        for (id, s) in &mut self.nodes {
            if let Ok(b) = &mut s.binding
                && b.noticed(core, event)
            {
                s.version += 1;
                again.push(id.clone());
            }
        }
        self.ask_again(&again);
    }

    fn ask_again(&mut self, ids: &[String]) {
        for id in ids {
            if let Some(s) = self.nodes.remove(id) {
                self.ask_regions(&s);
                self.nodes.insert(id.clone(), s);
            }
        }
    }

    /// Ends every binding's subscriptions.
    pub fn close(&mut self, core: &mut Core) {
        for s in self.nodes.values_mut() {
            if let Ok(b) = &mut s.binding {
                b.close(core);
            }
        }
        self.nodes.clear();
    }

    /// The number that names the node's binding in an extension's answers.
    pub fn placed_id(&self, id: &str) -> Option<u64> {
        Some(self.nodes.get(id)?.placed)
    }

    pub fn is_shape(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    pub fn binding(&self, id: &str) -> Option<&Binding> {
        self.nodes.get(id)?.binding.as_ref().ok()
    }

    /// What couldn't be bound, for a node that is a shape.
    pub fn problem(&self, id: &str) -> Option<&str> {
        self.nodes
            .get(id)?
            .binding
            .as_ref()
            .err()
            .map(String::as_str)
    }

    /// The node as a placed shape, in world units: the bound shape, or the unbound one when its
    /// binding failed.
    pub fn placed(&self, node: &CNode) -> Option<Placed> {
        let stored = Stored::of(node)?.ok()?;
        let shape = match self.binding(&node.id) {
            Some(b) => b.canvas(),
            None => std::sync::Arc::new(self.shapes.get(&stored.shape)?.unbound()),
        };
        Some(Placed::new(shape, stored.placement(node)))
    }

    /// What is under world point `p` (canvas pixels) on the shaped node, within `tolerance`
    /// world units; handles only when it is `selected`.
    pub fn hit(&self, node: &CNode, p: (f64, f64), tolerance: f64, selected: bool) -> Option<Hit> {
        let placed = self.placed(node)?;
        let p = Point::new(p.0 / UNIT, p.1 / UNIT);
        hit(&[placed], p, tolerance, selected.then_some(0))
    }

    /// The node painted at `view` (the board's area, in world units), clipped to it: from the
    /// cache when neither the shape, what it shows, nor the view changed.
    pub fn paint(&mut self, node: &CNode, view: &Viewport) -> Option<&Painted> {
        let placed = self.placed(node)?;
        let shaped = self.nodes.get_mut(&node.id)?;
        let b = placed.placement.world_bounds(&Rect::UNIT);
        // Room for labels beside the box and the rotate grip above it.
        let cell = view.cell;
        let margin = Point::new(cell.x * 3.0, cell.y * 2.0);
        let (min, max) = (b.min - margin, b.max + margin);
        let col = |x: f64| ((x - view.origin.x) / cell.x).floor() as i64;
        let row = |y: f64| ((y - view.origin.y) / cell.y).floor() as i64;
        let (c0, r0) = (col(min.x).max(0), row(min.y).max(0));
        let (c1, r1) = (
            (col(max.x) + 1).min(i64::from(view.cols)),
            (row(max.y) + 1).min(i64::from(view.rows)),
        );
        if c1 <= c0 || r1 <= r0 {
            return None;
        }
        let local = Viewport {
            cols: (c1 - c0) as u16,
            rows: (r1 - r0) as u16,
            origin: view.origin + Point::new(c0 as f64 * cell.x, r0 as f64 * cell.y),
            cell,
        };
        let p = &placed.placement;
        let key = PaintKey {
            version: shaped.version,
            at: (
                q(p.at.x - local.origin.x),
                q(p.at.y - local.origin.y),
                q(p.size.x),
                q(p.size.y),
                q(p.rotation),
            ),
            cell: (q(cell.x), q(cell.y)),
            size: (local.cols, local.rows),
        };
        let fresh = shaped.painted.as_ref().is_some_and(|p| p.key == key);
        if !fresh {
            let mut grid = Grid::new(&local);
            draw::paint(&mut grid, &placed, &local);
            let contents: Vec<Vec<Entry>> = match &shaped.binding {
                Ok(b) => b.shown().regions.into_iter().map(|r| r.items).collect(),
                Err(_) => Vec::new(),
            };
            let spots = paint_members(&mut grid, &placed, &local, &contents);
            shaped.painted = Some(Painted {
                key,
                col: c0 as i32,
                row: r0 as i32,
                grid,
                spots,
            });
        }
        shaped.painted.as_ref()
    }

    /// The region member painted at `(col, row)` of the board's area, as last painted.
    pub fn member_at(&self, id: &str, col: i32, row: i32) -> Option<Member> {
        let s = self.nodes.get(id)?;
        let painted = s.painted.as_ref()?;
        let (c, r) = (col - painted.col, row - painted.row);
        let spot = painted.spots.iter().find(|sp| {
            i32::from(sp.row) == r && (i32::from(sp.col)..i32::from(sp.col + sp.width)).contains(&c)
        })?;
        let b = s.binding.as_ref().ok()?;
        let region = b.shape().definition.regions.get(spot.region)?;
        let shown = b.shown();
        let entry = shown.regions.get(spot.region)?.items.get(spot.entry)?;
        Some(Member {
            region: region.id.clone(),
            item: entry.id.clone(),
        })
    }
}

/// What each region shows, fitted inside it as `draw::paint_contents` does, remembering where
/// each member went.
fn paint_members(
    grid: &mut Grid,
    placed: &Placed,
    view: &Viewport,
    contents: &[Vec<Entry>],
) -> Vec<Spot> {
    let mut spots = Vec::new();
    for (region, (r, entries)) in placed.shape.regions().iter().zip(contents).enumerate() {
        if entries.is_empty() {
            continue;
        }
        let room = Room::inside(&Mask::area(placed, &r.area, view, Res::BRAILLE));
        // A label is an item's words or an extension's: escaped before it is fitted.
        let escaped = entries.iter().map(|e| {
            cold_lava_views::shown::escape(&e.label, cold_lava_views::shown::Level::Content)
        });
        let escaped: Vec<_> = escaped.collect();
        let labels: Vec<&str> = escaped.iter().map(|label| label.as_ref()).collect();
        let fitted = fit::labels(&room, &labels, Flow::Column, Align::Center);
        fitted.paint(grid, draw::TEXT);
        for (entry, line) in fitted.lines.iter().take(fitted.shown).enumerate() {
            spots.push(Spot {
                region,
                entry,
                row: line.row,
                col: line.col,
                width: line.text.chars().count() as u16,
            });
        }
    }
    spots
}

/// The placement `p` with its corner `handle` dragged to world point `to`, the opposite corner
/// staying where it is, in the shape's own turned frame. Never smaller than `min` on a side.
pub fn stretch(p: Placement, handle: Handle, to: Point, min: Point) -> Placement {
    let (hx, hy) = match handle {
        Handle::TopLeft => (0.0, 0.0),
        Handle::TopRight => (1.0, 0.0),
        Handle::BottomRight => (1.0, 1.0),
        Handle::BottomLeft | Handle::Rotate => (0.0, 1.0),
    };
    let t = p.transform();
    let fixed = t.apply(Point::new(1.0 - hx, 1.0 - hy));
    let turn = |v: Point, deg: f64| cold_lava_canvas::Transform::rotate(deg).apply(v);
    let d = turn(to - fixed, -p.rotation);
    let (sx, sy) = (2.0 * hx - 1.0, 2.0 * hy - 1.0);
    let (w, h) = ((d.x * sx).max(min.x), (d.y * sy).max(min.y));
    let centre = fixed + turn(Point::new(sx * w / 2.0, sy * h / 2.0), p.rotation);
    Placement::new(centre - Point::new(w / 2.0, h / 2.0), Point::new(w, h)).rotated(p.rotation)
}

/// The turn that points the rotate grip of `p` at world point `to`, in steps of `step` degrees.
pub fn turn_towards(p: Placement, to: Point, step: f64) -> f64 {
    let c = p.at + p.size * 0.5;
    let d = to - c;
    let deg = d.x.atan2(-d.y).to_degrees();
    ((deg / step).round() * step).rem_euclid(360.0)
}

/// Whether a style token is a region's tint (regions are `Style(1)` up to the outline's).
pub fn is_region_tint(s: Style) -> bool {
    s != Style::NONE && s.0 < draw::OUTLINE.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a - b).length() < 1e-6
    }

    #[test]
    fn stretching_keeps_the_opposite_corner() {
        let p = Placement::new(Point::new(10.0, 10.0), Point::new(20.0, 10.0));
        let s = stretch(
            p,
            Handle::BottomRight,
            Point::new(40.0, 30.0),
            Point::new(1.0, 1.0),
        );
        assert!(close(s.at, Point::new(10.0, 10.0)));
        assert!(close(s.size, Point::new(30.0, 20.0)));
        let s = stretch(
            p,
            Handle::TopLeft,
            Point::new(0.0, 5.0),
            Point::new(1.0, 1.0),
        );
        assert!(close(s.at, Point::new(0.0, 5.0)));
        assert!(close(s.size, Point::new(30.0, 15.0)));
    }

    #[test]
    fn stretching_a_turned_shape_works_in_its_frame() {
        let p = Placement::new(Point::new(0.0, 0.0), Point::new(20.0, 10.0)).rotated(90.0);
        let fixed = p.transform().apply(Point::new(0.0, 0.0));
        let corner = p.transform().apply(Point::new(1.0, 1.0));
        let s = stretch(p, Handle::BottomRight, corner, Point::new(1.0, 1.0));
        assert!(close(s.size, p.size) && close(s.at, p.at), "{s:?}");
        let s = stretch(
            p,
            Handle::BottomRight,
            corner + Point::new(0.0, 10.0),
            Point::new(1.0, 1.0),
        );
        assert!(close(s.size, Point::new(30.0, 10.0)), "{s:?}");
        assert!(close(s.transform().apply(Point::new(0.0, 0.0)), fixed));
    }

    #[test]
    fn an_answer_to_a_shape_bound_before_the_shapes_were_read_again_is_dropped() {
        use crate::bind::Source;
        use crate::shapes::{Layer, load};
        let dir = std::env::temp_dir().join(format!("cold-lava-live-asks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Krakatoa.md"), "# Krakatoa\n").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let tally: toml::Table = toml::from_str(
            r#"
            inputs = ["a"]
            parts = [{ rect = { id = "box", from = [0, 0], to = [1, 1] } }]
            regions = [{ id = "count", area = "box", extension = true }]
            "#,
        )
        .unwrap();
        let shapes = || {
            load([
                Layer::built_in(),
                Layer::extension("tally", [("tally.list".to_string(), tally.clone())]),
            ])
        };
        let mut node: CNode = serde_json::from_value(serde_json::json!(
            {"id": "n", "type": "text", "x": 0, "y": 0, "width": 1, "height": 1}
        ))
        .unwrap();
        let sources: indexmap::IndexMap<String, Source> =
            [("a".to_string(), Source::Items(vec!["Krakatoa".into()]))].into();
        let at = Placement::new(Point::new(0.0, 0.0), Point::new(20.0, 10.0));
        let inputs = ["a".to_string()];
        super::super::placed::place(&mut node, "tally.list", &inputs, "Tally", &sources, at);
        let nodes = [node];
        let answer = |ask: &Ask, label: &str| ShapeAnswer {
            by: "tally".into(),
            ask: ask.clone(),
            answer: Ok(Answered::Region {
                items: vec![Entry {
                    id: String::new(),
                    label: label.into(),
                }],
            }),
        };
        let shown = |live: &Live| -> Vec<String> {
            let b = live.binding("n").expect("bound");
            (b.shown().regions[0].items.iter())
                .map(|e| e.label.clone())
                .collect()
        };

        let mut before = Live::new(shapes());
        assert_eq!(before.sync(&mut core, &nodes), Vec::<String>::new());
        let asks = before.take_asks();
        let [(_, old)] = &asks[..] else {
            panic!("one shape/region ask: {asks:?}")
        };
        before.close(&mut core);
        let mut after = Live::new(shapes());
        assert_eq!(after.sync(&mut core, &nodes), Vec::<String>::new());
        let asks = after.take_asks();
        let [(_, new)] = &asks[..] else {
            panic!("one shape/region ask: {asks:?}")
        };

        after.answered(answer(old, "asked before"));
        assert_eq!(
            shown(&after),
            Vec::<String>::new(),
            "the answer was to a binding that is gone"
        );
        after.answered(answer(new, "asked now"));
        assert_eq!(shown(&after), ["asked now"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn turning_follows_the_mouse_in_steps() {
        let p = Placement::new(Point::new(0.0, 0.0), Point::new(10.0, 10.0));
        assert_eq!(turn_towards(p, Point::new(5.0, -10.0), 15.0), 0.0);
        assert_eq!(turn_towards(p, Point::new(20.0, 5.0), 15.0), 90.0);
        assert_eq!(turn_towards(p, Point::new(-10.0, 4.0), 15.0), 270.0);
    }
}
