//! The shape being edited, held as the `Definition` it is saved as: there is no other model, so
//! what the editor shows is what the TOML says. This module is the editing on it, with no
//! drawing: where each part's grips are and what dragging one does, making parts, naming
//! regions, anchors and parts. Nothing in it knows one shape from another.
//!
//! Points are in the shape's unit box (0..1 on each axis); the screen turns them into cells.

use crate::shapes::{
    CircleDef, CurveDef, Definition, EllipseDef, LineDef, PartDef, PathDef, Pos, RectDef,
    RegionDef, SegmentDef, StrokeDef, TextDef, Vertex, Xy,
};
use cold_lava_canvas::{self as canvas, Point};

pub fn pt([x, y]: Xy) -> Point {
    Point::new(x, y)
}

pub fn xy(p: Point) -> Xy {
    [p.x, p.y]
}

/// A handle on a part: what dragging it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// A path's vertex, from 0 (its start).
    Vertex(usize),
    /// The middle of a path's side `i` (vertex `i` to the next); dragging it bends the side.
    Side(usize),
    /// A circle's or an ellipse's centre.
    Center,
    /// The point on a circle's right, or an ellipse's bottom-right corner of its box.
    Radius,
    /// A rect's, a line's or a text box's first corner or end, and its second.
    From,
    To,
    /// Where a label is.
    At,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GripAt {
    pub part: usize,
    pub grip: Grip,
    pub at: Xy,
}

/// Every grip of every part the editor can change, in part order. A line ending at an anchor,
/// a label at an anchor and a nested shape have none: they are kept as written.
pub fn grips(d: &Definition) -> Vec<GripAt> {
    let mut out = Vec::new();
    for (part, def) in d.parts.iter().enumerate() {
        let mut add = |grip, at| out.push(GripAt { part, grip, at });
        match def {
            PartDef::Rect(r) => {
                add(Grip::From, r.from);
                add(Grip::To, r.to);
            }
            PartDef::Circle(c) => {
                add(Grip::Center, c.center);
                add(Grip::Radius, [c.center[0] + c.radius, c.center[1]]);
            }
            PartDef::Ellipse(e) => {
                add(Grip::Center, e.center);
                add(
                    Grip::Radius,
                    [e.center[0] + e.radii[0], e.center[1] + e.radii[1]],
                );
            }
            PartDef::Line(s) | PartDef::Arrow(s) => {
                if let Pos::Point(p) = s.from {
                    add(Grip::From, p);
                }
                if let Pos::Point(p) = s.to {
                    add(Grip::To, p);
                }
            }
            PartDef::Path(p) => {
                let v = vertices(p);
                for (i, at) in v.iter().enumerate() {
                    add(Grip::Vertex(i), *at);
                }
                for (i, mid) in sides(p).into_iter().enumerate() {
                    add(Grip::Side(i), mid);
                }
            }
            PartDef::Text(t) => match (&t.at, t.from, t.to) {
                (Some(Pos::Point(p)), _, _) => add(Grip::At, *p),
                (None, Some(f), Some(to)) => {
                    add(Grip::From, f);
                    add(Grip::To, to);
                }
                _ => {}
            },
            PartDef::Shape(_) => {}
        }
    }
    out
}

/// A path with `segments` whose last one ends back on its start: that segment is the side that
/// closes it (a closing side that was bent), not a vertex of its own.
fn closes_on_start(p: &PathDef) -> bool {
    p.closed
        && p.start.is_some()
        && p.segments.last().map(|s| end(s).0) == p.start
        && p.segments.len() > 1
}

fn end(s: &SegmentDef) -> (Xy, Option<&str>) {
    match s {
        SegmentDef::Line(LineDef::To(to)) => (*to, None),
        SegmentDef::Line(LineDef::Named(v)) => (v.to, v.id.as_deref()),
        SegmentDef::Curve(c) => (c.to, c.id.as_deref()),
        SegmentDef::Quad(q) => (q.to, q.id.as_deref()),
    }
}

/// A path's vertices in order: its points, or its start and each segment's end.
pub fn vertices(p: &PathDef) -> Vec<Xy> {
    match p.start {
        None => p.points.clone(),
        Some(start) => {
            let mut v = vec![start];
            v.extend(p.segments.iter().map(|s| end(s).0));
            if closes_on_start(p) {
                v.pop();
            }
            v
        }
    }
}

/// The middle of each side, as drawn: side `i` from vertex `i` to the next, and the closing side
/// last on a closed path.
fn sides(p: &PathDef) -> Vec<Xy> {
    let v = vertices(p);
    let n = v.len();
    let count = if p.closed { n } else { n.saturating_sub(1) };
    (0..count)
        .map(|i| {
            let (a, b) = (pt(v[i]), pt(v[(i + 1) % n]));
            let mid = match p.start.and(p.segments.get(i)) {
                Some(SegmentDef::Curve(c)) => (a + b) * 0.125 + (pt(c.c1) + pt(c.c2)) * 0.375,
                Some(SegmentDef::Quad(q)) => (a + b) * 0.25 + pt(q.c) * 0.5,
                _ => (a + b) * 0.5,
            };
            xy(mid)
        })
        .collect()
}

/// A path written as `points` becomes a `start` and `segments`, the form that can hold curves.
fn as_segments(p: &mut PathDef) {
    if p.start.is_some() || p.points.is_empty() {
        return;
    }
    let mut points = std::mem::take(&mut p.points).into_iter();
    p.start = points.next();
    p.segments = points.map(|to| SegmentDef::Line(LineDef::To(to))).collect();
}

fn line_to(to: Xy, id: Option<String>) -> SegmentDef {
    match id {
        Some(id) => SegmentDef::Line(LineDef::Named(Vertex { to, id: Some(id) })),
        None => SegmentDef::Line(LineDef::To(to)),
    }
}

fn shift(xy: &mut Xy, d: Point) {
    *xy = [xy[0] + d.x, xy[1] + d.y];
}

/// Moves vertex `k` of a path to `to`; a curve's control points next to it go with it, so a
/// bent side keeps its bend.
fn move_vertex(p: &mut PathDef, k: usize, to: Xy) {
    as_segments(p);
    let Some(start) = p.start else { return };
    let from = if k == 0 {
        start
    } else {
        match p.segments.get(k - 1) {
            Some(s) => end(s).0,
            None => return,
        }
    };
    let d = pt(to) - pt(from);
    let explicit = closes_on_start(p);
    let n = p.segments.len();
    let set_end = |i: usize, p: &mut PathDef| match &mut p.segments[i] {
        SegmentDef::Line(LineDef::To(t)) => *t = to,
        SegmentDef::Line(LineDef::Named(v)) => v.to = to,
        SegmentDef::Curve(c) => {
            c.to = to;
            shift(&mut c.c2, d);
        }
        SegmentDef::Quad(q) => q.to = to,
    };
    if k == 0 {
        p.start = Some(to);
        if explicit {
            set_end(n - 1, p);
        }
    } else {
        set_end(k - 1, p);
    }
    if let Some(SegmentDef::Curve(c)) = p.segments.get_mut(k) {
        shift(&mut c.c1, d);
    }
}

/// Bends side `k` of a path so its middle passes through `mid` (a cubic `curve`), or makes it
/// straight again with `None`.
fn bend(p: &mut PathDef, k: usize, mid: Option<Xy>) {
    as_segments(p);
    let v = vertices(p);
    let n = v.len();
    if k >= n || (!p.closed && k + 1 >= n) {
        return;
    }
    let (a, b) = (v[k], v[(k + 1) % n]);
    if k == p.segments.len() {
        // The implicit closing side: written out, so it can bend.
        p.segments.push(line_to(b, None));
    }
    let id = end(&p.segments[k]).1.map(String::from);
    p.segments[k] = match mid {
        None => line_to(b, id),
        Some(m) => {
            // The quadratic through `m` at its middle, as the cubic it equals.
            let (a, b, m) = (pt(a), pt(b), pt(m));
            let q = m * 2.0 - (a + b) * 0.5;
            SegmentDef::Curve(CurveDef {
                c1: xy(a + (q - a) * (2.0 / 3.0)),
                c2: xy(b + (q - b) * (2.0 / 3.0)),
                to: xy(b),
                id,
            })
        }
    };
    // A closing side bent and then straightened is implicit again.
    if p.closed
        && k + 1 == p.segments.len()
        && matches!(p.segments[k], SegmentDef::Line(LineDef::To(_)))
        && Some(end(&p.segments[k]).0) == p.start
    {
        p.segments.pop();
    }
}

/// Drags `grip` of part `part` to `to`. A side's grip bends it; `straight` makes it straight.
pub fn drag(d: &mut Definition, part: usize, grip: Grip, to: Xy, straight: bool) {
    let Some(def) = d.parts.get_mut(part) else {
        return;
    };
    let dist = |a: Xy, b: Xy| (pt(a) - pt(b)).length();
    match (def, grip) {
        (PartDef::Rect(r), Grip::From) => r.from = to,
        (PartDef::Rect(r), Grip::To) => r.to = to,
        (PartDef::Circle(c), Grip::Center) => c.center = to,
        (PartDef::Circle(c), Grip::Radius) => c.radius = dist(to, c.center).max(0.005),
        (PartDef::Ellipse(e), Grip::Center) => e.center = to,
        (PartDef::Ellipse(e), Grip::Radius) => {
            e.radii = [
                (to[0] - e.center[0]).abs().max(0.005),
                (to[1] - e.center[1]).abs().max(0.005),
            ]
        }
        (PartDef::Line(s) | PartDef::Arrow(s), Grip::From) => s.from = Pos::Point(to),
        (PartDef::Line(s) | PartDef::Arrow(s), Grip::To) => s.to = Pos::Point(to),
        (PartDef::Path(p), Grip::Vertex(k)) => move_vertex(p, k, to),
        (PartDef::Path(p), Grip::Side(k)) => bend(p, k, (!straight).then_some(to)),
        (PartDef::Text(t), Grip::At) => t.at = Some(Pos::Point(to)),
        (PartDef::Text(t), Grip::From) => t.from = Some(to),
        (PartDef::Text(t), Grip::To) => t.to = Some(to),
        _ => {}
    }
}

/// Moves a whole part by `by`. Ends at anchors stay at their anchors.
pub fn translate(d: &mut Definition, part: usize, by: Point) {
    let Some(def) = d.parts.get_mut(part) else {
        return;
    };
    let pos = |p: &mut Pos| {
        if let Pos::Point(xy) = p {
            shift(xy, by);
        }
    };
    match def {
        PartDef::Rect(r) => {
            shift(&mut r.from, by);
            shift(&mut r.to, by);
        }
        PartDef::Circle(c) => shift(&mut c.center, by),
        PartDef::Ellipse(e) => shift(&mut e.center, by),
        PartDef::Line(s) | PartDef::Arrow(s) => {
            pos(&mut s.from);
            pos(&mut s.to);
        }
        PartDef::Path(p) => {
            p.points.iter_mut().for_each(|xy| shift(xy, by));
            if let Some(s) = &mut p.start {
                shift(s, by);
            }
            for s in &mut p.segments {
                match s {
                    SegmentDef::Line(LineDef::To(t)) => shift(t, by),
                    SegmentDef::Line(LineDef::Named(v)) => shift(&mut v.to, by),
                    SegmentDef::Curve(c) => {
                        shift(&mut c.c1, by);
                        shift(&mut c.c2, by);
                        shift(&mut c.to, by);
                    }
                    SegmentDef::Quad(q) => {
                        shift(&mut q.c, by);
                        shift(&mut q.to, by);
                    }
                }
            }
        }
        PartDef::Text(t) => {
            if let Some(at) = &mut t.at {
                pos(at);
            }
            if let Some(f) = &mut t.from {
                shift(f, by);
            }
            if let Some(to) = &mut t.to {
                shift(to, by);
            }
        }
        PartDef::Shape(n) => {
            shift(&mut n.from, by);
            shift(&mut n.to, by);
        }
    }
}

/// `base` and the first number that makes it a name no part, anchor or region has: `circle1`.
pub fn fresh_id(d: &Definition, base: &str) -> String {
    let taken = |id: &str| {
        d.parts
            .iter()
            .any(|p| p.id() == Some(id) || matches!(p, PartDef::Shape(n) if n.id == id))
            || d.anchors.contains_key(id)
            || d.regions.iter().any(|r| r.id == id)
    };
    (1..)
        .map(|n| format!("{base}{n}"))
        .find(|id| !taken(id))
        .expect("a free number")
}

/// What the editor makes with its tools.
pub enum New {
    /// A path through these vertices, closed or open, its sides straight.
    Path {
        vertices: Vec<Xy>,
        closed: bool,
    },
    Circle {
        center: Xy,
        radius: f64,
    },
    Ellipse {
        center: Xy,
        radii: Xy,
    },
    Rect {
        from: Xy,
        to: Xy,
    },
    /// A label: its text at a point or an anchor.
    Text {
        at: Pos,
        value: String,
    },
}

/// Adds a part, named after its kind (`path1`); returns its index.
pub fn add(d: &mut Definition, new: New) -> usize {
    let part = match new {
        New::Path { vertices, closed } => {
            let mut v = vertices.into_iter();
            let start = v.next();
            PartDef::Path(PathDef {
                id: Some(fresh_id(d, "path")),
                points: Vec::new(),
                start,
                segments: v.map(|to| line_to(to, None)).collect(),
                closed,
            })
        }
        New::Circle { center, radius } => PartDef::Circle(CircleDef {
            id: Some(fresh_id(d, "circle")),
            center,
            radius,
        }),
        New::Ellipse { center, radii } => PartDef::Ellipse(EllipseDef {
            id: Some(fresh_id(d, "ellipse")),
            center,
            radii,
        }),
        New::Rect { from, to } => PartDef::Rect(RectDef {
            id: Some(fresh_id(d, "rect")),
            from,
            to,
        }),
        New::Text { at, value } => PartDef::Text(TextDef {
            id: None,
            value,
            at: Some(at),
            side: None,
            from: None,
            to: None,
        }),
    };
    d.parts.push(part);
    d.parts.len() - 1
}

/// The area under a point, from what contains it: `left & right`, `left - right`. Parts that
/// don't contain it are taken away only where they could overlap the ones that do. With the
/// parts it is in, for the editor to say. `None` outside every area.
pub fn area_at(shape: &canvas::Shape, p: Point) -> Option<(String, Vec<String>)> {
    let named = |part: &canvas::Part| {
        let area = part.is_area() && !matches!(part.kind, canvas::Kind::Text { .. });
        part.id.clone().filter(|_| area)
    };
    let mut inside = Vec::new();
    let mut bounds: Option<canvas::Rect> = None;
    for part in shape.parts() {
        if let Some(id) = named(part)
            && part.contains(p)
        {
            bounds = Some(match bounds {
                None => part.bounds(),
                Some(b) => b.intersect(&part.bounds()),
            });
            inside.push(id);
        }
    }
    let bounds = bounds?;
    let overlaps = |r: &canvas::Rect| {
        r.min.x < bounds.max.x
            && bounds.min.x < r.max.x
            && r.min.y < bounds.max.y
            && bounds.min.y < r.max.y
    };
    let outside: Vec<String> = (shape.parts().iter())
        .filter(|part| !part.contains(p) && overlaps(&part.bounds()))
        .filter_map(named)
        .collect();
    let mut expr = inside.join(" & ");
    for o in &outside {
        expr += &format!(" - {o}");
    }
    Some((expr, inside))
}

/// Names the area `area` as region `id`: a region already called `id` takes the new area, one
/// with the same area takes the new name, else it is added.
pub fn name_region(d: &mut Definition, id: &str, area: &str) {
    if let Some(r) = d.regions.iter_mut().find(|r| r.id == id) {
        r.area = area.to_string();
    } else if let Some(r) = d.regions.iter_mut().find(|r| r.area == area) {
        r.id = id.to_string();
    } else {
        d.regions.push(RegionDef {
            id: id.to_string(),
            area: area.to_string(),
            shows: None,
            extension: false,
        });
    }
}

/// Names a vertex of a path `name`: a vertex a segment ends at gets the segment's `id` (the
/// anchor `<path>.<name>`); the start is named in `anchors`. Returns the anchor's full name.
pub fn name_vertex(d: &mut Definition, part: usize, k: usize, name: &str) -> Option<String> {
    let PartDef::Path(p) = d.parts.get_mut(part)? else {
        return None;
    };
    as_segments(p);
    let at = *vertices(p).get(k)?;
    if k == 0 {
        d.anchors.insert(name.to_string(), at);
        return Some(name.to_string());
    }
    let path = p.id.clone()?;
    let seg = p.segments.get_mut(k - 1)?;
    match seg {
        SegmentDef::Line(l) => {
            *l = LineDef::Named(Vertex {
                to: at,
                id: Some(name.to_string()),
            })
        }
        SegmentDef::Curve(c) => c.id = Some(name.to_string()),
        SegmentDef::Quad(q) => q.id = Some(name.to_string()),
    }
    Some(format!("{path}.{name}"))
}

/// Renames part `part` to `new`, and every region, anchor and click that names it.
pub fn rename_part(d: &mut Definition, part: usize, new: &str) {
    let Some(old) = d.parts.get(part).and_then(PartDef::id).map(String::from) else {
        return;
    };
    match &mut d.parts[part] {
        PartDef::Rect(RectDef { id, .. })
        | PartDef::Circle(CircleDef { id, .. })
        | PartDef::Ellipse(EllipseDef { id, .. })
        | PartDef::Line(StrokeDef { id, .. })
        | PartDef::Arrow(StrokeDef { id, .. })
        | PartDef::Path(PathDef { id, .. })
        | PartDef::Text(TextDef { id, .. }) => *id = Some(new.to_string()),
        PartDef::Shape(_) => return,
    }
    let anchor = |name: &mut String| {
        if *name == old {
            *name = new.to_string();
        } else if let Some(rest) = name.strip_prefix(&format!("{old}.")) {
            *name = format!("{new}.{rest}");
        }
    };
    for p in &mut d.parts {
        let ends: Vec<&mut Pos> = match p {
            PartDef::Line(s) | PartDef::Arrow(s) => vec![&mut s.from, &mut s.to],
            PartDef::Text(TextDef { at: Some(at), .. }) => vec![at],
            _ => Vec::new(),
        };
        for end in ends {
            if let Pos::Anchor(name) = end {
                anchor(name);
            }
        }
    }
    for r in &mut d.regions {
        r.area = rename_word(&r.area, &old, new);
    }
    for b in &mut d.on {
        if b.click.as_deref() == Some(old.as_str()) {
            b.click = Some(new.to_string());
        }
    }
}

/// `expr` with the name `old` (a whole name, not part of `only-old`) replaced by `new`.
fn rename_word(expr: &str, old: &str, new: &str) -> String {
    rename_words(expr, &[(old.to_string(), new.to_string())])
}

/// `expr` with every name renamed at once by `names` (old, new), so renames never chain; a
/// nested shape's parts (`old.part`) follow their shape.
fn rename_words(expr: &str, names: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let renamed = names.iter().find_map(|(old, new)| {
            let rest = word.strip_prefix(old.as_str())?;
            (rest.is_empty() || rest.starts_with('.')).then(|| format!("{new}{rest}"))
        });
        out.push_str(renamed.as_deref().unwrap_or(word));
        word.clear();
    };
    for c in expr.chars() {
        if matches!(c, '&' | '|' | '(' | ')') || c.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

/// A region's `shows`, cut into what the form asks for: the sets, then `where`, `sort by`,
/// `limit` and `label`, each empty when it isn't there.
pub fn split_shows(shows: &str) -> [String; 5] {
    let mut parts: [Vec<&str>; 5] = Default::default();
    let mut at = 0;
    let words: Vec<&str> = shows.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = match w {
            "where" => Some(1),
            "sort" if words.get(i + 1) == Some(&"by") => {
                i += 1;
                Some(2)
            }
            "limit" => Some(3),
            "label" => Some(4),
            _ => None,
        };
        match next {
            Some(n) if n > at => at = n,
            _ => parts[at].push(w),
        }
        i += 1;
    }
    parts.map(|p| p.join(" "))
}

/// The `shows` the form's five fields make; `None` when the sets are empty.
pub fn join_shows(fields: &[String]) -> Option<String> {
    let get = |i: usize| fields.get(i).map(|s| s.trim()).unwrap_or("");
    if get(0).is_empty() {
        return None;
    }
    let mut s = get(0).to_string();
    for (i, word) in [(1, "where"), (2, "sort by"), (3, "limit"), (4, "label")] {
        if !get(i).is_empty() {
            s += &format!(" {word} {}", get(i));
        }
    }
    Some(s)
}

/// One placed shape `merged` takes: its definition, the shape built from it (for its anchors) and
/// where it is placed.
pub struct Member<'a> {
    pub definition: &'a Definition,
    pub shape: &'a canvas::Shape,
    pub at: canvas::Placement,
}

/// A new shape of the parts of several placed shapes, in one box around them, each part where it
/// was: what "make a shape" opens the editor on. Curves stay curves (an affine map keeps a
/// Bézier one); a turned rectangle becomes a path and a turned ellipse four curves; ends and
/// labels at anchors are put at the points the anchors were. Regions keep their areas, renamed
/// with their parts when two members use one name; what they show, the inputs and behaviour are
/// the members' own and are left out. Returns it with what was left out.
pub fn merged(members: &[Member]) -> (Definition, Vec<String>) {
    let mut d: Definition = toml::from_str("parts = []").expect("an empty shape");
    let mut notes = Vec::new();
    let Some(bounds) = members
        .iter()
        .map(|m| m.at.world_bounds(&canvas::Rect::UNIT))
        .reduce(|a, b| a.union(&b))
    else {
        return (d, notes);
    };
    let size = bounds.max - bounds.min;
    if size.x <= 0.0 || size.y <= 0.0 {
        return (d, notes);
    }
    d.aspect = Some(round(size.x / size.y));
    let to_box = canvas::Transform::translate(-bounds.min.x, -bounds.min.y)
        .then(&canvas::Transform::scale(1.0 / size.x, 1.0 / size.y));
    for (k, m) in members.iter().enumerate() {
        let map = m.at.transform().then(&to_box);
        let at = |p: Xy| xy(map.apply(pt(p)));
        let square = (m.at.rotation / 90.0).fract().abs() < 1e-9;
        let quarter = ((m.at.rotation / 90.0).round() as i64).rem_euclid(2) == 1;
        let resolve = |pos: &Pos| match pos {
            Pos::Point(p) => Some(at(*p)),
            Pos::Anchor(name) => m.shape.anchor(name).map(|p| at(xy(p))),
        };
        let mut renamed: Vec<(String, String)> = Vec::new();
        let mut nests: Vec<(String, String)> = Vec::new();
        let mut fresh = |d: &Definition, id: &Option<String>| {
            let id = id.as_ref()?;
            let new = match d.parts.iter().any(|p| p.id() == Some(id.as_str())) {
                true => fresh_id(d, &format!("{id}-{}", k + 1)),
                false => id.clone(),
            };
            renamed.push((id.clone(), new.clone()));
            Some(new)
        };
        for part in &m.definition.parts {
            let new = match part {
                PartDef::Rect(r) if square => PartDef::Rect(RectDef {
                    id: fresh(&d, &r.id),
                    from: at(r.from),
                    to: at(r.to),
                }),
                PartDef::Rect(r) => {
                    let (a, b) = (r.from, r.to);
                    let corners = [a, [b[0], a[1]], b, [a[0], b[1]]];
                    PartDef::Path(PathDef {
                        id: fresh(&d, &r.id),
                        points: corners.iter().map(|c| at(*c)).collect(),
                        start: None,
                        segments: Vec::new(),
                        closed: true,
                    })
                }
                PartDef::Circle(CircleDef { id, center, radius }) => oval(
                    fresh(&d, id),
                    *center,
                    [*radius, *radius],
                    m,
                    square,
                    quarter,
                    &at,
                    size,
                ),
                PartDef::Ellipse(EllipseDef { id, center, radii }) => oval(
                    fresh(&d, id),
                    *center,
                    *radii,
                    m,
                    square,
                    quarter,
                    &at,
                    size,
                ),
                PartDef::Line(s) | PartDef::Arrow(s) => {
                    let (Some(from), Some(to)) = (resolve(&s.from), resolve(&s.to)) else {
                        notes.push("a line whose anchor is gone".into());
                        continue;
                    };
                    let stroke = StrokeDef {
                        id: fresh(&d, &s.id),
                        from: Pos::Point(from),
                        to: Pos::Point(to),
                    };
                    match part {
                        PartDef::Line(_) => PartDef::Line(stroke),
                        _ => PartDef::Arrow(stroke),
                    }
                }
                PartDef::Path(p) => {
                    let mut p = p.clone();
                    p.id = fresh(&d, &p.id);
                    p.points.iter_mut().for_each(|v| *v = at(*v));
                    if let Some(s) = &mut p.start {
                        *s = at(*s);
                    }
                    for s in &mut p.segments {
                        match s {
                            SegmentDef::Line(LineDef::To(t)) => *t = at(*t),
                            SegmentDef::Line(LineDef::Named(v)) => v.to = at(v.to),
                            SegmentDef::Curve(c) => {
                                (c.c1, c.c2, c.to) = (at(c.c1), at(c.c2), at(c.to));
                            }
                            SegmentDef::Quad(q) => (q.c, q.to) = (at(q.c), at(q.to)),
                        }
                    }
                    PartDef::Path(p)
                }
                PartDef::Text(t) => {
                    let mut t = t.clone();
                    t.id = fresh(&d, &t.id);
                    if let Some(pos) = &t.at {
                        if let Pos::Anchor(name) = pos
                            && t.side.is_none()
                        {
                            t.side = Some(side(canvas::Side::outside(name)));
                        }
                        let Some(p) = resolve(pos) else { continue };
                        t.at = Some(Pos::Point(p));
                    }
                    if let (Some(a), Some(b)) = (t.from, t.to) {
                        let corners = [a, [b[0], a[1]], b, [a[0], b[1]]].map(|c| pt(at(c)));
                        let r = canvas::Rect::around(corners).expect("four corners");
                        (t.from, t.to) = (Some(xy(r.min)), Some(xy(r.max)));
                    }
                    PartDef::Text(t)
                }
                PartDef::Shape(n) if square && !quarter => {
                    let mut n = n.clone();
                    if d.parts
                        .iter()
                        .any(|p| matches!(p, PartDef::Shape(o) if o.id == n.id))
                    {
                        let new = fresh_id(&d, &format!("{}-{}", n.id, k + 1));
                        nests.push((n.id.clone(), new.clone()));
                        n.id = new;
                    }
                    (n.from, n.to) = (at(n.from), at(n.to));
                    PartDef::Shape(n)
                }
                PartDef::Shape(n) => {
                    notes.push(format!("the nested shape {} (it is turned)", n.id));
                    continue;
                }
            };
            d.parts.push(new);
        }
        for r in &m.definition.regions {
            let area = rename_words(&r.area, &[&renamed[..], &nests[..]].concat());
            let id = match d.regions.iter().any(|o| o.id == r.id) {
                true => fresh_id(&d, &format!("{}-{}", r.id, k + 1)),
                false => r.id.clone(),
            };
            d.regions.push(RegionDef {
                id,
                area,
                shows: None,
                extension: false,
            });
        }
        if !m.definition.inputs.is_empty() || !m.definition.on.is_empty() {
            notes.push("inputs, what regions show and behaviour (each shape's own)".into());
        }
    }
    notes.dedup();
    (d, notes)
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn side(s: canvas::Side) -> crate::shapes::SideDef {
    use crate::shapes::SideDef;
    match s {
        canvas::Side::Above => SideDef::Above,
        canvas::Side::Below => SideDef::Below,
        canvas::Side::Left => SideDef::Left,
        canvas::Side::Right => SideDef::Right,
        canvas::Side::Center => SideDef::Center,
    }
}

/// A circle or an ellipse of a member, in the new box: still one when it is turned a whole
/// quarter (or round in the world), else four curves.
#[allow(clippy::too_many_arguments)]
fn oval(
    id: Option<String>,
    center: Xy,
    radii: Xy,
    m: &Member,
    square: bool,
    quarter: bool,
    at: &impl Fn(Xy) -> Xy,
    size: Point,
) -> PartDef {
    let world = Point::new(radii[0] * m.at.size.x, radii[1] * m.at.size.y);
    let round_in_world = (world.x - world.y).abs() < 1e-9;
    if square || round_in_world {
        let world = match quarter && !round_in_world {
            true => Point::new(world.y, world.x),
            false => world,
        };
        let r = [world.x / size.x, world.y / size.y];
        return match (r[0] - r[1]).abs() < 1e-9 {
            true => PartDef::Circle(CircleDef {
                id,
                center: at(center),
                radius: r[0],
            }),
            false => PartDef::Ellipse(EllipseDef {
                id,
                center: at(center),
                radii: r,
            }),
        };
    }
    // The usual four-cubic circle, which an affine map keeps an ellipse.
    const K: f64 = 0.552_284_749_8;
    let (c, rx, ry) = (pt(center), radii[0], radii[1]);
    let p = |x: f64, y: f64| at(xy(c + Point::new(x, y)));
    let quarter_arc = |(x0, y0): (f64, f64), (x1, y1): (f64, f64)| {
        SegmentDef::Curve(CurveDef {
            c1: p(x0 + K * x1, y0 + K * y1),
            c2: p(x1 + K * x0, y1 + K * y0),
            to: p(x1, y1),
            id: None,
        })
    };
    let ends = [(rx, 0.0), (0.0, ry), (-rx, 0.0), (0.0, -ry)];
    PartDef::Path(PathDef {
        id,
        points: Vec::new(),
        start: Some(p(rx, 0.0)),
        segments: (0..4)
            .map(|i| quarter_arc(ends[i], ends[(i + 1) % 4]))
            .collect(),
        closed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Definition {
        toml::from_str("parts = []").unwrap()
    }

    fn path(d: &Definition, i: usize) -> &PathDef {
        match &d.parts[i] {
            PartDef::Path(p) => p,
            _ => panic!("a path"),
        }
    }

    #[test]
    fn a_side_bends_into_a_curve_through_the_point_and_back() {
        let mut d = empty();
        let square = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        add(
            &mut d,
            New::Path {
                vertices: square,
                closed: true,
            },
        );
        let side = grips(&d)
            .into_iter()
            .find(|g| g.grip == Grip::Side(0))
            .unwrap();
        assert_eq!(side.at, [0.5, 0.0]);
        drag(&mut d, 0, Grip::Side(0), [0.5, -0.2], false);
        let mid = grips(&d)
            .into_iter()
            .find(|g| g.grip == Grip::Side(0))
            .unwrap();
        assert!(
            (pt(mid.at) - Point::new(0.5, -0.2)).length() < 1e-9,
            "{mid:?}"
        );
        assert!(matches!(path(&d, 0).segments[0], SegmentDef::Curve(_)));
        // The closing side, bent, is written out, and is not a vertex of its own.
        drag(&mut d, 0, Grip::Side(3), [-0.2, 0.5], false);
        assert_eq!(vertices(path(&d, 0)).len(), 4);
        assert_eq!(path(&d, 0).segments.len(), 4);
        drag(&mut d, 0, Grip::Side(3), [0.0, 0.5], true);
        assert_eq!(path(&d, 0).segments.len(), 3);
        // Moving a vertex keeps the bend next to it.
        drag(&mut d, 0, Grip::Vertex(1), [1.1, 0.0], false);
        let SegmentDef::Curve(c) = &path(&d, 0).segments[0] else {
            panic!()
        };
        assert_eq!(c.to, [1.1, 0.0]);
    }

    #[test]
    fn a_path_of_points_becomes_segments_when_it_is_changed() {
        let mut d: Definition = toml::from_str(
            "parts = [{ path = { id = \"z\", points = [[0, 0], [0.5, 1], [1, 0]] } }]",
        )
        .unwrap();
        assert_eq!(grips(&d).len(), 3 + 2);
        drag(&mut d, 0, Grip::Vertex(2), [0.9, 0.1], false);
        let p = path(&d, 0);
        assert!(p.points.is_empty());
        assert_eq!(vertices(p), [[0.0, 0.0], [0.5, 1.0], [0.9, 0.1]]);
    }

    #[test]
    fn regions_are_read_from_what_contains_the_point() {
        let circle = |id, x| {
            let kind = canvas::Kind::Circle {
                center: Point::new(x, 0.5),
                radius: 0.32,
            };
            canvas::Part::new(Some(id), kind)
        };
        let corner = canvas::Rect::new(Point::new(0.9, 0.9), Point::new(1.0, 1.0));
        let parts = vec![
            circle("left", 0.38),
            circle("right", 0.62),
            canvas::Part::new(Some("far"), canvas::Kind::Rect(corner)),
        ];
        let shape = canvas::Shape::new(parts, &[]).unwrap();
        let at = |x, y| area_at(&shape, Point::new(x, y)).map(|(e, _)| e);
        assert_eq!(at(0.5, 0.5).as_deref(), Some("left & right"));
        assert_eq!(at(0.15, 0.5).as_deref(), Some("left - right"));
        assert_eq!(at(0.85, 0.5).as_deref(), Some("right - left"));
        assert_eq!(at(0.02, 0.02), None);
    }

    #[test]
    fn renaming_a_part_renames_what_names_it() {
        let mut d: Definition = toml::from_str(
            r#"parts = [
              { circle = { id = "circle1", center = [0.4, 0.5], radius = 0.3 } },
              { circle = { id = "circle12", center = [0.6, 0.5], radius = 0.3 } },
              { text = { at = "circle1.top", value = "A" } },
            ]
            regions = [{ id = "only", area = "circle1 - circle12" }]
            [[on]]
            click = "circle1"
            edit = { item = "clicked", toggle = true }"#,
        )
        .unwrap();
        rename_part(&mut d, 0, "left");
        assert_eq!(d.regions[0].area, "left - circle12");
        assert!(
            matches!(&d.parts[2], PartDef::Text(TextDef { at: Some(Pos::Anchor(a)), .. }) if a == "left.top")
        );
        assert_eq!(d.on[0].click.as_deref(), Some("left"));
    }

    #[test]
    fn shows_splits_into_the_form_and_back() {
        let s = "volcanoes - a where vei >= 6 and name != \"x\" sort by elevation desc limit 5";
        let f = split_shows(s);
        assert_eq!(f[0], "volcanoes - a");
        assert_eq!(f[1], "vei >= 6 and name != \"x\"");
        assert_eq!(f[2], "elevation desc");
        assert_eq!(f[3], "5");
        assert_eq!(join_shows(&f).as_deref(), Some(s));
        assert_eq!(join_shows(&[String::new()]), None);
    }
}
