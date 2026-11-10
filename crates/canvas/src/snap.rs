//! Guides for drawing a shape by hand: where a point the pointer is near should go, so a few
//! coarse clicks make a clean square, a regular hexagon or two circles of one size. Guides are
//! how points are placed, never data: what is kept is only where each point ended up.
//!
//! Everything here is in world units (a terminal cell is 1×2), so an angle is the angle the eye
//! sees, whatever the shape's box.

use crate::geom::Point;

/// Angles snap to multiples of this, in degrees: 90° and 45°, and the 60° and 120° of hexagons
/// and triangles.
pub const ANGLE_STEP: f64 = 15.0;
/// How far, in degrees, a side may point from a guide angle and still snap to it.
pub const ANGLE_REACH: f64 = 5.0;

/// The guide that placed a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Guide {
    /// Onto a point already there (closing a path on its first vertex, too).
    Point,
    /// The next corner of a regular polygon: the last side again, turned as the last corner was.
    Regular,
    /// The side from the previous point at this angle (degrees, clockwise from the right).
    Angle(f64),
    /// That angle, and as long as the previous side.
    AngleLength(f64),
    /// Level with another point (same row in the world).
    Level,
    /// Plumb with another point (same column).
    Plumb,
    /// A size equal to another one's (a circle's radius).
    Equal,
}

impl Guide {
    /// What the editor says while it guides: `60°`, `= side`, `regular`.
    pub fn describe(&self) -> String {
        match self {
            Guide::Point => "on a point".into(),
            Guide::Regular => "regular: same side, same angle".into(),
            Guide::Angle(a) => format!("{}°", a.round()),
            Guide::AngleLength(a) => format!("{}° · = side", a.round()),
            Guide::Level => "level".into(),
            Guide::Plumb => "plumb".into(),
            Guide::Equal => "= size".into(),
        }
    }
}

/// Where a point goes, and which guide put it there (`None`: where the pointer is).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapped {
    pub at: Point,
    pub guide: Option<Guide>,
}

/// The direction of `d` in degrees clockwise from the right, 0..360.
fn angle(d: Point) -> f64 {
    d.y.atan2(d.x).to_degrees().rem_euclid(360.0)
}

fn towards(degrees: f64, length: f64) -> Point {
    let r = degrees.to_radians();
    Point::new(r.cos(), r.sin()) * length
}

/// A vertex at `p`, drawn after the vertices of `chain` (the path so far, in order), among
/// `points` (every other point of the shape), each guide reaching `reach` world units. In order:
/// onto a point (`chain`'s first closes the path), the next corner of a regular polygon, the
/// side's angle and length, then level or plumb with a point.
pub fn vertex(p: Point, chain: &[Point], points: &[Point], reach: f64) -> Snapped {
    let near = |q: &Point| (*q - p).length() <= reach;
    if let Some(q) = chain.iter().chain(points).find(|q| near(q)) {
        return Snapped {
            at: *q,
            guide: Some(Guide::Point),
        };
    }
    if let [.., a, b, c] = chain {
        let (side, last) = (*c - *b, *b - *a);
        let turn = angle(side) - angle(last);
        let next = *c + towards(angle(side) + turn, side.length());
        if (next - p).length() <= reach * 1.5 && side.length() > 0.0 {
            return Snapped {
                at: next,
                guide: Some(Guide::Regular),
            };
        }
    }
    if let Some(last) = chain.last() {
        let d = p - *last;
        let a = angle(d);
        let guide = (a / ANGLE_STEP).round() * ANGLE_STEP;
        let off = (a - guide).abs();
        if d.length() > reach && off.min(360.0 - off) <= ANGLE_REACH {
            if let [.., before, _] = chain {
                let previous = (*last - *before).length();
                if (d.length() - previous).abs() <= reach {
                    return Snapped {
                        at: *last + towards(guide, previous),
                        guide: Some(Guide::AngleLength(guide.rem_euclid(360.0))),
                    };
                }
            }
            return Snapped {
                at: *last + towards(guide, d.length()),
                guide: Some(Guide::Angle(guide.rem_euclid(360.0))),
            };
        }
    }
    let level = chain
        .iter()
        .chain(points)
        .find(|q| (q.y - p.y).abs() <= reach / 2.0);
    if let Some(q) = level {
        return Snapped {
            at: Point::new(p.x, q.y),
            guide: Some(Guide::Level),
        };
    }
    let plumb = chain
        .iter()
        .chain(points)
        .find(|q| (q.x - p.x).abs() <= reach / 2.0);
    match plumb {
        Some(q) => Snapped {
            at: Point::new(q.x, p.y),
            guide: Some(Guide::Plumb),
        },
        None => Snapped { at: p, guide: None },
    }
}

/// A size (a radius) of `r`, snapped to one of `others` within `reach`.
pub fn size(r: f64, others: &[f64], reach: f64) -> (f64, Option<Guide>) {
    match others.iter().find(|o| (**o - r).abs() <= reach) {
        Some(o) => (*o, Some(Guide::Equal)),
        None => (r, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a - b).length() < 1e-9
    }

    #[test]
    fn coarse_clicks_make_a_regular_hexagon() {
        let reach = 2.0;
        let mut chain = vec![Point::new(30.0, 20.0)];
        // A side to the right, then one turned 60°, each a cell or so off.
        let clicks = [
            Point::new(50.5, 20.6),
            Point::new(60.4, 37.9),
            Point::new(50.2, 54.0),
            Point::new(30.0, 55.0),
            Point::new(20.6, 37.0),
        ];
        let mut guides = Vec::new();
        for click in clicks {
            let s = vertex(click, &chain, &[], reach);
            guides.push(s.guide);
            chain.push(s.at);
        }
        let closing = vertex(Point::new(30.4, 20.7), &chain, &[], reach);
        assert_eq!(closing.guide, Some(Guide::Point));
        assert!(close(closing.at, chain[0]));
        assert_eq!(guides[0], Some(Guide::Angle(0.0)));
        assert_eq!(guides[1], Some(Guide::AngleLength(60.0)));
        assert_eq!(guides[2..], [Some(Guide::Regular); 3]);
        let n = chain.len();
        let side = (chain[1] - chain[0]).length();
        for i in 0..n {
            let (a, b, c) = (chain[i], chain[(i + 1) % n], chain[(i + 2) % n]);
            assert!(((b - a).length() - side).abs() < 1e-9, "side {i}");
            let turn = (angle(c - b) - angle(b - a)).rem_euclid(360.0);
            assert!((turn - 60.0).abs() < 1e-9, "corner {i}: {turn}");
        }
    }

    #[test]
    fn a_square_snaps_to_right_angles_and_closes() {
        let reach = 2.0;
        let mut chain = vec![Point::new(0.0, 0.0)];
        for click in [Point::new(20.0, 0.8), Point::new(20.6, 19.4)] {
            chain.push(vertex(click, &chain, &[], reach).at);
        }
        let fourth = vertex(Point::new(0.5, 20.4), &chain, &[], reach);
        assert_eq!(fourth.guide, Some(Guide::Regular));
        let side = chain[1].x;
        assert!(close(chain[1], Point::new(side, 0.0)), "{:?}", chain[1]);
        assert!(close(chain[2], Point::new(side, side)), "{:?}", chain[2]);
        assert!(close(fourth.at, Point::new(0.0, side)), "{:?}", fourth.at);
    }

    #[test]
    fn far_from_every_guide_a_point_stays_where_it_is() {
        let p = Point::new(13.0, 10.5);
        let s = vertex(p, &[Point::new(0.0, 0.0)], &[Point::new(40.0, 40.0)], 2.0);
        assert_eq!(s, Snapped { at: p, guide: None });
        assert_eq!(size(10.0, &[10.6], 1.0), (10.6, Some(Guide::Equal)));
        assert_eq!(size(10.0, &[14.0], 1.0), (10.0, None));
    }
}
