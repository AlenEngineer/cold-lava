//! Points, boxes and affine transforms, and the placement that puts a shape's unit box in the
//! world. World coordinates grow right and down; angles are degrees, clockwise on screen.

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    pub fn dot(self, other: Point) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// Distance from `self` to the segment `a`–`b`.
    pub fn distance_to_segment(self, a: Point, b: Point) -> f64 {
        let ab = b - a;
        let len2 = ab.dot(ab);
        let t = if len2 == 0.0 {
            0.0
        } else {
            ((self - a).dot(ab) / len2).clamp(0.0, 1.0)
        };
        (self - (a + ab * t)).length()
    }
}

impl Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Point {
    type Output = Point;
    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f64> for Point {
    type Output = Point;
    fn mul(self, k: f64) -> Point {
        Point::new(self.x * k, self.y * k)
    }
}

/// An axis-aligned box, `min` top-left and `max` bottom-right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min: Point,
    pub max: Point,
}

impl Rect {
    pub const UNIT: Rect = Rect::new(Point::new(0.0, 0.0), Point::new(1.0, 1.0));

    pub const fn new(min: Point, max: Point) -> Rect {
        Rect { min, max }
    }

    /// The smallest box holding every point; `None` for no points.
    pub fn around(points: impl IntoIterator<Item = Point>) -> Option<Rect> {
        let mut points = points.into_iter();
        let first = points.next()?;
        Some(points.fold(Rect::new(first, first), |r, p| {
            Rect::new(
                Point::new(r.min.x.min(p.x), r.min.y.min(p.y)),
                Point::new(r.max.x.max(p.x), r.max.y.max(p.y)),
            )
        }))
    }

    pub fn contains(&self, p: Point) -> bool {
        (self.min.x..=self.max.x).contains(&p.x) && (self.min.y..=self.max.y).contains(&p.y)
    }

    pub fn union(&self, o: &Rect) -> Rect {
        Rect::around([self.min, self.max, o.min, o.max]).expect("four points")
    }

    /// The overlap; empty boxes come out with `min` past `max`, which contains nothing.
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect::new(
            Point::new(self.min.x.max(o.min.x), self.min.y.max(o.min.y)),
            Point::new(self.max.x.min(o.max.x), self.max.y.min(o.max.y)),
        )
    }

    pub fn corners(&self) -> [Point; 4] {
        [
            self.min,
            Point::new(self.max.x, self.min.y),
            self.max,
            Point::new(self.min.x, self.max.y),
        ]
    }

    /// A named point of the box: `center`, `top`, `bottom`, `left`, `right`, `top-left`,
    /// `top-right`, `bottom-left`, `bottom-right`.
    pub fn anchor(&self, name: &str) -> Option<Point> {
        let (x0, x1, y0, y1) = (self.min.x, self.max.x, self.min.y, self.max.y);
        let (xm, ym) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        let (x, y) = match name {
            "center" => (xm, ym),
            "top" => (xm, y0),
            "bottom" => (xm, y1),
            "left" => (x0, ym),
            "right" => (x1, ym),
            "top-left" => (x0, y0),
            "top-right" => (x1, y0),
            "bottom-left" => (x0, y1),
            "bottom-right" => (x1, y1),
            _ => return None,
        };
        Some(Point::new(x, y))
    }
}

/// An affine map: `x' = a·x + c·y + e`, `y' = b·x + d·y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(dx: f64, dy: f64) -> Transform {
        Transform {
            e: dx,
            f: dy,
            ..Transform::IDENTITY
        }
    }

    pub fn scale(sx: f64, sy: f64) -> Transform {
        Transform {
            a: sx,
            d: sy,
            ..Transform::IDENTITY
        }
    }

    /// Clockwise on screen (y grows down), in degrees, about the origin.
    pub fn rotate(degrees: f64) -> Transform {
        let (s, c) = degrees.to_radians().sin_cos();
        Transform {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    /// `self` first, then `next`.
    pub fn then(&self, next: &Transform) -> Transform {
        let n = next;
        Transform {
            a: n.a * self.a + n.c * self.b,
            b: n.b * self.a + n.d * self.b,
            c: n.a * self.c + n.c * self.d,
            d: n.b * self.c + n.d * self.d,
            e: n.a * self.e + n.c * self.f + n.e,
            f: n.b * self.e + n.d * self.f + n.f,
        }
    }

    pub fn apply(&self, p: Point) -> Point {
        Point::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    /// `None` when the map squashes the plane to a line or a point (a zero-size placement).
    pub fn inverse(&self) -> Option<Transform> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Transform {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        })
    }
}

/// Where the user put a shape: its unit box stretched to `size`, turned `rotation` degrees
/// clockwise about its centre, with the unturned box's top-left corner at `at`. This is what is
/// saved; everything else is derived from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub at: Point,
    pub size: Point,
    pub rotation: f64,
}

/// The grips a frontend draws on a selected shape: four corners to stretch, one to turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
    Rotate,
}

impl Placement {
    pub fn new(at: Point, size: Point) -> Placement {
        Placement {
            at,
            size,
            rotation: 0.0,
        }
    }

    pub fn rotated(self, degrees: f64) -> Placement {
        Placement {
            rotation: degrees,
            ..self
        }
    }

    /// The shape's unit box to world coordinates.
    pub fn transform(&self) -> Transform {
        let (w, h) = (self.size.x, self.size.y);
        Transform::scale(w, h)
            .then(&Transform::translate(-w / 2.0, -h / 2.0))
            .then(&Transform::rotate(self.rotation))
            .then(&Transform::translate(
                self.at.x + w / 2.0,
                self.at.y + h / 2.0,
            ))
    }

    /// The world box around a box given in the shape's own unit coordinates.
    pub fn world_bounds(&self, local: &Rect) -> Rect {
        let t = self.transform();
        Rect::around(local.corners().map(|p| t.apply(p))).expect("four corners")
    }

    /// Where each handle sits in the world. The rotate handle stands `reach` world units above
    /// the top edge's middle, in the shape's own up direction.
    pub fn handles(&self, reach: f64) -> [(Handle, Point); 5] {
        let t = self.transform();
        let up = reach / self.size.y.abs().max(f64::EPSILON);
        [
            (Handle::TopLeft, t.apply(Point::new(0.0, 0.0))),
            (Handle::TopRight, t.apply(Point::new(1.0, 0.0))),
            (Handle::BottomRight, t.apply(Point::new(1.0, 1.0))),
            (Handle::BottomLeft, t.apply(Point::new(0.0, 1.0))),
            (Handle::Rotate, t.apply(Point::new(0.5, -up))),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a - b).length() < 1e-9
    }

    #[test]
    fn rotate_is_clockwise_on_screen() {
        let p = Transform::rotate(90.0).apply(Point::new(1.0, 0.0));
        assert!(close(p, Point::new(0.0, 1.0)), "{p:?}");
    }

    #[test]
    fn then_applies_left_first() {
        let t = Transform::scale(2.0, 3.0).then(&Transform::translate(1.0, 1.0));
        assert!(close(t.apply(Point::new(1.0, 1.0)), Point::new(3.0, 4.0)));
    }

    #[test]
    fn placement_maps_unit_box_corners() {
        let p = Placement::new(Point::new(10.0, 20.0), Point::new(4.0, 2.0));
        let t = p.transform();
        assert!(close(t.apply(Point::new(0.0, 0.0)), Point::new(10.0, 20.0)));
        assert!(close(t.apply(Point::new(1.0, 1.0)), Point::new(14.0, 22.0)));
    }

    #[test]
    fn rotation_turns_about_the_centre() {
        let p = Placement::new(Point::new(0.0, 0.0), Point::new(4.0, 2.0)).rotated(90.0);
        let t = p.transform();
        assert!(close(t.apply(Point::new(0.5, 0.5)), Point::new(2.0, 1.0)));
        // The top-left corner swings to the top-right of the turned box.
        assert!(close(t.apply(Point::new(0.0, 0.0)), Point::new(3.0, -1.0)));
    }

    #[test]
    fn inverse_undoes_and_degenerate_has_none() {
        let t = Placement::new(Point::new(3.0, -2.0), Point::new(5.0, 7.0))
            .rotated(33.0)
            .transform();
        let inv = t.inverse().unwrap();
        let p = Point::new(0.3, 0.9);
        assert!(close(inv.apply(t.apply(p)), p));
        assert!(Transform::scale(0.0, 1.0).inverse().is_none());
    }

    #[test]
    fn rotate_handle_stands_above_the_top_edge() {
        let p = Placement::new(Point::new(0.0, 0.0), Point::new(10.0, 4.0));
        let [.., (handle, at)] = p.handles(2.0);
        assert_eq!(handle, Handle::Rotate);
        assert!(close(at, Point::new(5.0, -2.0)));
    }

    #[test]
    fn segment_distance() {
        let (a, b) = (Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        assert_eq!(Point::new(5.0, 3.0).distance_to_segment(a, b), 3.0);
        assert_eq!(Point::new(13.0, 4.0).distance_to_segment(a, b), 5.0);
    }
}
