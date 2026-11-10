//! A shape: its parts and named regions, in its own unit box; and a placed shape, which is a
//! shape where the user put it.

use crate::area::{Area, AreaError};
use crate::geom::{Placement, Point, Transform};
use crate::part::{Kind, Part};
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    parts: Vec<Part>,
    regions: Vec<Region>,
    /// Named points that aren't a part's: `tip`, or a nested shape's box (`card`, `card.top`).
    anchors: Vec<(String, Point)>,
}

/// A named area of a shape (`both` = `left & right`).
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub id: String,
    pub area: Area,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShapeError {
    DuplicatePart(String),
    /// A named anchor called like a part or another anchor.
    DuplicateAnchor(String),
    DuplicateRegion(String),
    Region {
        id: String,
        error: AreaError,
    },
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ShapeError::DuplicatePart(id) => write!(f, "two parts are called `{id}`"),
            ShapeError::DuplicateAnchor(id) => {
                write!(f, "`{id}` names an anchor and another anchor or part")
            }
            ShapeError::DuplicateRegion(id) => write!(f, "two regions are called `{id}`"),
            ShapeError::Region { id, error } => write!(f, "region `{id}`: {error}"),
        }
    }
}

impl std::error::Error for ShapeError {}

impl Shape {
    /// A shape from its parts and its regions' `(id, area expression)`.
    pub fn new(parts: Vec<Part>, regions: &[(&str, &str)]) -> Result<Shape, ShapeError> {
        for (i, part) in parts.iter().enumerate() {
            if let Some(id) = &part.id
                && parts[..i].iter().any(|p| p.id.as_ref() == Some(id))
            {
                return Err(ShapeError::DuplicatePart(id.clone()));
            }
        }
        let mut built: Vec<Region> = Vec::with_capacity(regions.len());
        for (id, expr) in regions {
            if built.iter().any(|r| r.id == *id) {
                return Err(ShapeError::DuplicateRegion(id.to_string()));
            }
            let area = Area::parse(expr, &parts).map_err(|error| ShapeError::Region {
                id: id.to_string(),
                error,
            })?;
            built.push(Region {
                id: id.to_string(),
                area,
            });
        }
        Ok(Shape {
            parts,
            regions: built,
            anchors: Vec::new(),
        })
    }

    /// The shape with named points of its own, besides its parts' anchors.
    pub fn with_anchors(mut self, anchors: Vec<(String, Point)>) -> Result<Shape, ShapeError> {
        for (i, (name, _)) in anchors.iter().enumerate() {
            if self.part(name).is_some() || anchors[..i].iter().any(|(n, _)| n == name) {
                return Err(ShapeError::DuplicateAnchor(name.clone()));
            }
        }
        self.anchors = anchors;
        Ok(self)
    }

    /// The same shape with each text and label's value passed through `f` (inputs filled in,
    /// glyphs looked up).
    pub fn map_text(&self, f: impl Fn(&str) -> String) -> Shape {
        let mut shape = self.clone();
        for part in &mut shape.parts {
            if let Kind::Text { value, .. } | Kind::Label { value, .. } = &mut part.kind {
                *value = f(value);
            }
        }
        shape
    }

    pub fn anchors(&self) -> &[(String, Point)] {
        &self.anchors
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn part(&self, id: &str) -> Option<usize> {
        self.parts.iter().position(|p| p.id.as_deref() == Some(id))
    }

    pub fn region(&self, id: &str) -> Option<usize> {
        self.regions.iter().position(|r| r.id == id)
    }

    /// A named point in the unit box: see [`anchor_in`].
    pub fn anchor(&self, name: &str) -> Option<Point> {
        anchor_in(&self.parts, &self.anchors, name)
    }
}

/// A named point among `parts` and named `anchors`, in this order: a named anchor; a part's id
/// alone (its centre); `part.anchor` (`left.top`, where the part's id may itself hold dots, as
/// `card.frame.top`); one of the box's own anchors (`center`, `top-left`, …). So a part's id
/// wins over a box anchor of the same name.
pub fn anchor_in(parts: &[Part], anchors: &[(String, Point)], name: &str) -> Option<Point> {
    let part = |id: &str| parts.iter().find(|p| p.id.as_deref() == Some(id));
    if let Some((_, p)) = anchors.iter().find(|(n, _)| n == name) {
        return Some(*p);
    }
    if let Some(p) = part(name) {
        return p.anchor("center");
    }
    if let Some((id, anchor)) = name.rsplit_once('.') {
        return part(id)?.anchor(anchor);
    }
    crate::geom::Rect::UNIT.anchor(name)
}

/// A shape where the user put it. Shapes are shared (`Arc`): a hundred placed Venns hold one.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub shape: Arc<Shape>,
    pub placement: Placement,
}

impl Placed {
    pub fn new(shape: Arc<Shape>, placement: Placement) -> Placed {
        Placed { shape, placement }
    }

    /// A shape anchor in world coordinates.
    pub fn anchor(&self, name: &str) -> Option<Point> {
        Some(self.placement.transform().apply(self.shape.anchor(name)?))
    }

    /// World to the shape's unit box; `None` for a zero-size placement.
    pub fn to_local(&self) -> Option<Transform> {
        self.placement.transform().inverse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::part::Kind;

    fn circle(id: &str, x: f64) -> Part {
        Part::new(
            Some(id),
            Kind::Circle {
                center: Point::new(x, 0.5),
                radius: 0.32,
            },
        )
    }

    #[test]
    fn anchors_resolve_through_parts_and_box() {
        let shape = Shape::new(vec![circle("left", 0.38), circle("top", 0.62)], &[]).unwrap();
        let top = shape.anchor("left.top").unwrap();
        assert!((top.x - 0.38).abs() < 1e-9 && (top.y - 0.18).abs() < 1e-9);
        assert_eq!(shape.anchor("center"), Some(Point::new(0.5, 0.5)));
        // A part called `top` hides the box's `top`.
        assert_eq!(shape.anchor("top"), Some(Point::new(0.62, 0.5)));
        assert_eq!(shape.anchor("left.nowhere"), None);
        assert_eq!(shape.anchor("nowhere.top"), None);
    }

    #[test]
    fn named_anchors_and_dotted_part_ids() {
        let shape = Shape::new(vec![circle("card.left", 0.38)], &[])
            .unwrap()
            .with_anchors(vec![("tip".into(), Point::new(0.9, 0.1))])
            .unwrap();
        assert_eq!(shape.anchor("tip"), Some(Point::new(0.9, 0.1)));
        assert_eq!(shape.anchor("tip.top"), None);
        assert_eq!(shape.anchor("card.left"), Some(Point::new(0.38, 0.5)));
        let top = shape.anchor("card.left.top").unwrap();
        assert!((top.y - 0.18).abs() < 1e-9);
        let clash = Shape::new(vec![circle("tip", 0.38)], &[])
            .unwrap()
            .with_anchors(vec![("tip".into(), Point::new(0.0, 0.0))]);
        assert_eq!(
            clash.unwrap_err(),
            ShapeError::DuplicateAnchor("tip".into())
        );
    }

    #[test]
    fn placed_anchor_follows_the_placement() {
        let shape = Arc::new(Shape::new(vec![circle("left", 0.38)], &[]).unwrap());
        let placed = Placed::new(
            shape,
            Placement::new(Point::new(10.0, 0.0), Point::new(100.0, 50.0)),
        );
        let top = placed.anchor("left.top").unwrap();
        assert!((top.x - 48.0).abs() < 1e-9 && (top.y - 9.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_duplicates_and_bad_regions() {
        let dup = Shape::new(vec![circle("a", 0.3), circle("a", 0.6)], &[]);
        assert_eq!(dup.unwrap_err(), ShapeError::DuplicatePart("a".into()));
        let regions = [("r", "a"), ("r", "a")];
        let dup = Shape::new(vec![circle("a", 0.3)], &regions);
        assert_eq!(dup.unwrap_err(), ShapeError::DuplicateRegion("r".into()));
        let bad = Shape::new(vec![circle("a", 0.3)], &[("r", "a & b")]).unwrap_err();
        assert_eq!(bad.to_string(), "region `r`: no part is called `b`");
    }
}
