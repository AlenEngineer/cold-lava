//! The primitives a shape is drawn from, in the shape's own unit box (0..1 on each axis).
//!
//! A part is either an **area** (it has an inside: `rect`, `circle`, `ellipse`, a closed `path`,
//! a `text` box) or a **stroke** (only a line: `line`, `arrow`, an open `path`). Areas answer
//! [`Part::contains`] exactly, from their implicit form or, for paths, their flattened polygon;
//! that is what regions, rasterizing and hit testing are built on.

use crate::geom::{Point, Rect, Transform};

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    /// What anchors and region expressions call it (`left` in `left.top` and `left - right`).
    pub id: Option<String>,
    pub kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Rect(Rect),
    Circle {
        center: Point,
        radius: f64,
    },
    Ellipse {
        center: Point,
        radii: Point,
    },
    Line {
        from: Point,
        to: Point,
    },
    Arrow {
        from: Point,
        to: Point,
    },
    Path(Path),
    /// A box that holds laid-out text; its inside is the box.
    Text {
        bounds: Rect,
        value: String,
    },
}

/// A polyline or Bézier path (freehand strokes come in as many short segments). It is flattened
/// once, when built, so membership and drawing read the same points.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    closed: bool,
    points: Vec<Point>,
    bounds: Rect,
    /// The bounds cut into horizontal bands, each listing the edges (by their end point's index)
    /// that reach into it, so a membership test only looks at edges near its row.
    bands: Vec<Vec<u32>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    Line(Point),
    Cubic { c1: Point, c2: Point, to: Point },
}

/// Line pieces per cubic. Parts live in a unit box, so a fixed count is smooth at any size a
/// terminal or a window reaches.
const CUBIC_STEPS: usize = 24;

impl Path {
    pub fn new(start: Point, segments: &[Segment], closed: bool) -> Path {
        let mut points = vec![start];
        for seg in segments {
            let from = *points.last().expect("starts with a point");
            match *seg {
                Segment::Line(to) => points.push(to),
                Segment::Cubic { c1, c2, to } => points.extend((1..=CUBIC_STEPS).map(|i| {
                    let t = i as f64 / CUBIC_STEPS as f64;
                    let u = 1.0 - t;
                    from * (u * u * u)
                        + c1 * (3.0 * u * u * t)
                        + c2 * (3.0 * u * t * t)
                        + to * (t * t * t)
                })),
            }
        }
        Path::polyline(points, closed)
    }

    pub fn polyline(points: Vec<Point>, closed: bool) -> Path {
        let bounds = Rect::around(points.iter().copied())
            .unwrap_or(Rect::new(Point::default(), Point::default()));
        let n = points.len();
        let mut bands = vec![Vec::new(); (n / 2).clamp(1, 64)];
        let height = bounds.max.y - bounds.min.y;
        let band = |y: f64| {
            let k = if height > 0.0 {
                (y - bounds.min.y) / height
            } else {
                0.0
            };
            ((k * bands.len() as f64) as usize).min(bands.len() - 1)
        };
        let ranges: Vec<_> = (0..n)
            .map(|i| {
                let (a, b) = (points[i], points[(i + n - 1) % n]);
                band(a.y.min(b.y))..=band(a.y.max(b.y))
            })
            .collect();
        for (i, range) in ranges.into_iter().enumerate() {
            for k in range {
                bands[k].push(i as u32);
            }
        }
        Path {
            closed,
            points,
            bounds,
            bands,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The flattened points; a closed path's last point joins back to the first.
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// Even-odd rule, so a self-crossing freehand loop has holes where it crosses itself.
    fn contains(&self, p: Point) -> bool {
        if !self.bounds.contains(p) {
            return false;
        }
        let pts = &self.points;
        let n = pts.len();
        let height = self.bounds.max.y - self.bounds.min.y;
        let k = if height > 0.0 {
            (p.y - self.bounds.min.y) / height
        } else {
            0.0
        };
        let band = &self.bands[((k * self.bands.len() as f64) as usize).min(self.bands.len() - 1)];
        let mut inside = false;
        for &i in band {
            let i = i as usize;
            let (a, b) = (pts[i], pts[(i + n - 1) % n]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
        }
        inside
    }
}

impl Part {
    pub fn new(id: Option<&str>, kind: Kind) -> Part {
        Part {
            id: id.map(str::to_string),
            kind,
        }
    }

    /// Has an inside, so it can be filled and named in a region.
    pub fn is_area(&self) -> bool {
        match &self.kind {
            Kind::Line { .. } | Kind::Arrow { .. } => false,
            Kind::Path(path) => path.closed,
            Kind::Rect(_) | Kind::Circle { .. } | Kind::Ellipse { .. } | Kind::Text { .. } => true,
        }
    }

    /// Whether a point of the unit box is inside; always false for strokes.
    pub fn contains(&self, p: Point) -> bool {
        match &self.kind {
            Kind::Rect(r) | Kind::Text { bounds: r, .. } => r.contains(p),
            Kind::Circle { center, radius } => {
                let d = p - *center;
                d.dot(d) <= radius * radius
            }
            Kind::Ellipse { center, radii } => {
                let (dx, dy) = ((p.x - center.x) / radii.x, (p.y - center.y) / radii.y);
                dx * dx + dy * dy <= 1.0
            }
            Kind::Path(path) => path.closed && path.contains(p),
            Kind::Line { .. } | Kind::Arrow { .. } => false,
        }
    }

    /// The box around the part in its shape's unit coordinates.
    pub fn bounds(&self) -> Rect {
        match &self.kind {
            Kind::Rect(r) | Kind::Text { bounds: r, .. } => *r,
            Kind::Circle { center, radius } => {
                let r = Point::new(*radius, *radius);
                Rect::new(*center - r, *center + r)
            }
            Kind::Ellipse { center, radii } => Rect::new(*center - *radii, *center + *radii),
            Kind::Line { from, to } | Kind::Arrow { from, to } => {
                Rect::around([*from, *to]).expect("two points")
            }
            Kind::Path(path) => path.bounds,
        }
    }

    /// A named point: the part's box anchors (`top`, `center`, `bottom-left`, …), and `start`
    /// and `end` of a line, arrow or path.
    pub fn anchor(&self, name: &str) -> Option<Point> {
        let ends = match &self.kind {
            Kind::Line { from, to } | Kind::Arrow { from, to } => Some((*from, *to)),
            Kind::Path(path) => Some((*path.points.first()?, *path.points.last()?)),
            _ => None,
        };
        match (name, ends) {
            ("start", Some((from, _))) => Some(from),
            ("end", Some((_, to))) => Some(to),
            _ => self.bounds().anchor(name),
        }
    }

    /// The lines a stroke draws, in world coordinates. An arrow's head is sized in world units
    /// (`head`, capped at a third of the shaft), so stretching the shape never squashes it.
    /// Areas draw their outline from their inside instead, and return nothing here.
    pub fn world_strokes(&self, to_world: &Transform, head: f64) -> Vec<Vec<Point>> {
        match &self.kind {
            Kind::Line { from, to } => vec![vec![to_world.apply(*from), to_world.apply(*to)]],
            Kind::Arrow { from, to } => {
                let (a, b) = (to_world.apply(*from), to_world.apply(*to));
                let shaft = a - b;
                let len = shaft.length();
                if len == 0.0 {
                    return vec![vec![a, b]];
                }
                let back = shaft * (head.min(len / 3.0) / len);
                let wing = |deg: f64| b + Transform::rotate(deg).apply(back);
                vec![vec![a, b], vec![wing(25.0), b, wing(-25.0)]]
            }
            Kind::Path(path) if !path.closed => {
                vec![path.points.iter().map(|p| to_world.apply(*p)).collect()]
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn implicit_forms() {
        let circle = Part::new(
            None,
            Kind::Circle {
                center: p(0.5, 0.5),
                radius: 0.25,
            },
        );
        assert!(circle.contains(p(0.5, 0.74)));
        assert!(!circle.contains(p(0.7, 0.7)));
        let ellipse = Part::new(
            None,
            Kind::Ellipse {
                center: p(0.5, 0.5),
                radii: p(0.4, 0.1),
            },
        );
        assert!(ellipse.contains(p(0.85, 0.5)));
        assert!(!ellipse.contains(p(0.5, 0.65)));
    }

    #[test]
    fn closed_path_is_an_area_open_path_a_stroke() {
        let tri = [p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0)];
        let closed = Part::new(None, Kind::Path(Path::polyline(tri.to_vec(), true)));
        let open = Part::new(None, Kind::Path(Path::polyline(tri.to_vec(), false)));
        assert!(closed.is_area() && closed.contains(p(0.2, 0.2)));
        assert!(!closed.contains(p(0.8, 0.8)));
        assert!(!open.is_area() && !open.contains(p(0.2, 0.2)));
        assert_eq!(open.world_strokes(&Transform::IDENTITY, 1.0).len(), 1);
    }

    #[test]
    fn cubic_flattens_through_its_ends() {
        let path = Path::new(
            p(0.0, 0.0),
            &[Segment::Cubic {
                c1: p(0.0, 1.0),
                c2: p(1.0, 1.0),
                to: p(1.0, 0.0),
            }],
            false,
        );
        assert_eq!(path.points().len(), CUBIC_STEPS + 1);
        assert_eq!(*path.points().last().unwrap(), p(1.0, 0.0));
        // The curve's midpoint is at 3/4 of the control height.
        let mid = path.points()[CUBIC_STEPS / 2];
        assert!((mid.y - 0.75).abs() < 1e-9 && (mid.x - 0.5).abs() < 1e-9);
    }

    #[test]
    fn anchors() {
        let circle = Part::new(
            Some("left"),
            Kind::Circle {
                center: p(0.38, 0.5),
                radius: 0.32,
            },
        );
        let top = circle.anchor("top").unwrap();
        assert!((top.x - 0.38).abs() < 1e-9 && (top.y - 0.18).abs() < 1e-9);
        let line = Part::new(
            None,
            Kind::Line {
                from: p(0.1, 0.2),
                to: p(0.9, 0.8),
            },
        );
        assert_eq!(line.anchor("end"), Some(p(0.9, 0.8)));
        assert_eq!(circle.anchor("end"), None);
        assert_eq!(circle.anchor("nowhere"), None);
    }

    #[test]
    fn arrow_head_keeps_its_world_size_when_stretched() {
        let arrow = Part::new(
            None,
            Kind::Arrow {
                from: p(0.0, 0.5),
                to: p(1.0, 0.5),
            },
        );
        let wide = Transform::scale(100.0, 1.0);
        let strokes = arrow.world_strokes(&wide, 2.0);
        let head = &strokes[1];
        let wing = (head[0] - head[1]).length();
        assert!((wing - 2.0).abs() < 1e-9, "{wing}");
    }
}
