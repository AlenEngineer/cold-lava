//! Hit testing: what is under a point, for clicks, drags and handles.

use crate::geom::{Handle, Point, Rect};
use crate::part::Part;
use crate::raster::ARROW_HEAD;
use crate::shape::Placed;

/// How far above a shape's top edge its rotate handle stands, in world units.
pub const ROTATE_REACH: f64 = 2.0;

/// What a point is on: the topmost placed shape there, and within it a handle (only on the
/// selected shape), the topmost part, and the region. A point inside a shape's box but on no
/// part or region still hits the shape, so its blank space can be grabbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub shape: usize,
    pub handle: Option<Handle>,
    pub part: Option<usize>,
    pub region: Option<usize>,
}

/// The hit at world point `p`. Later shapes in `scene` are on top. `tolerance` (world units) is
/// how near a stroke or a handle must be.
pub fn hit(scene: &[Placed], p: Point, tolerance: f64, selected: Option<usize>) -> Option<Hit> {
    if let Some(shape) = selected
        && let Some(placed) = scene.get(shape)
        && let Some((handle, _)) = placed
            .placement
            .handles(ROTATE_REACH)
            .into_iter()
            .find(|(_, at)| (*at - p).length() <= tolerance)
    {
        return Some(Hit {
            shape,
            handle: Some(handle),
            part: None,
            region: None,
        });
    }
    scene.iter().enumerate().rev().find_map(|(shape, placed)| {
        let local = placed.to_local()?.apply(p);
        let parts = placed.shape.parts();
        let to_world = placed.placement.transform();
        let near = |part: &Part| {
            part.world_strokes(&to_world, ARROW_HEAD)
                .iter()
                .flat_map(|line| line.windows(2))
                .any(|w| p.distance_to_segment(w[0], w[1]) <= tolerance)
        };
        let part = parts
            .iter()
            .rposition(|part| part.contains(local) || near(part));
        let region = placed
            .shape
            .regions()
            .iter()
            .position(|r| r.area.contains(parts, local));
        (part.is_some() || region.is_some() || Rect::UNIT.contains(local)).then_some(Hit {
            shape,
            handle: None,
            part,
            region,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Placement;
    use crate::part::Kind;
    use crate::shape::Shape;
    use std::sync::Arc;

    fn venn() -> Arc<Shape> {
        let circle = |id, x| {
            Part::new(
                Some(id),
                Kind::Circle {
                    center: Point::new(x, 0.5),
                    radius: 0.32,
                },
            )
        };
        Arc::new(
            Shape::new(
                vec![circle("left", 0.38), circle("right", 0.62)],
                &[
                    ("only-a", "left - right"),
                    ("both", "left & right"),
                    ("only-b", "right - left"),
                ],
            )
            .unwrap(),
        )
    }

    fn at(shape: Arc<Shape>, x: f64, y: f64, size: f64) -> Placed {
        Placed::new(
            shape,
            Placement::new(Point::new(x, y), Point::new(size, size)),
        )
    }

    #[test]
    fn regions_and_topmost_part() {
        let scene = [at(venn(), 0.0, 0.0, 100.0)];
        let h = |x, y| hit(&scene, Point::new(x, y), 0.5, None);
        let lens = h(50.0, 50.0).unwrap();
        assert_eq!(
            (lens.part, lens.region),
            (Some(1), Some(1)),
            "right is on top of left"
        );
        let only_a = h(15.0, 50.0).unwrap();
        assert_eq!((only_a.part, only_a.region), (Some(0), Some(0)));
        let corner = h(2.0, 2.0).unwrap();
        assert_eq!((corner.shape, corner.part, corner.region), (0, None, None));
        assert_eq!(h(120.0, 50.0), None);
    }

    #[test]
    fn rotation_is_undone_before_testing() {
        // Turned 90°, the left circle is on top and the right one below.
        let mut placed = at(venn(), 0.0, 0.0, 100.0);
        placed.placement = placed.placement.rotated(90.0);
        let scene = [placed];
        let top = hit(&scene, Point::new(50.0, 15.0), 0.5, None).unwrap();
        assert_eq!(top.region, Some(0));
        let bottom = hit(&scene, Point::new(50.0, 85.0), 0.5, None).unwrap();
        assert_eq!(bottom.region, Some(2));
    }

    #[test]
    fn later_shapes_are_on_top() {
        let scene = [at(venn(), 0.0, 0.0, 100.0), at(venn(), 40.0, 40.0, 20.0)];
        assert_eq!(
            hit(&scene, Point::new(50.0, 50.0), 0.5, None)
                .unwrap()
                .shape,
            1
        );
        assert_eq!(
            hit(&scene, Point::new(20.0, 50.0), 0.5, None)
                .unwrap()
                .shape,
            0
        );
    }

    #[test]
    fn handles_only_on_the_selected_shape() {
        let scene = [at(venn(), 10.0, 10.0, 50.0)];
        let corner = Point::new(60.4, 60.0);
        assert_eq!(hit(&scene, corner, 0.5, None), None);
        let grip = hit(&scene, corner, 0.5, Some(0)).unwrap();
        assert_eq!(grip.handle, Some(Handle::BottomRight));
        let turn = hit(&scene, Point::new(35.0, 10.0 - ROTATE_REACH), 0.5, Some(0)).unwrap();
        assert_eq!(turn.handle, Some(Handle::Rotate));
    }

    #[test]
    fn strokes_hit_within_tolerance() {
        let arrow = Arc::new(
            Shape::new(
                vec![Part::new(
                    Some("arrow"),
                    Kind::Arrow {
                        from: Point::new(0.0, 0.5),
                        to: Point::new(1.0, 0.5),
                    },
                )],
                &[],
            )
            .unwrap(),
        );
        let scene = [Placed::new(
            arrow,
            Placement::new(Point::new(0.0, 0.0), Point::new(40.0, 10.0)),
        )];
        assert_eq!(
            hit(&scene, Point::new(20.0, 5.4), 0.5, None).unwrap().part,
            Some(0)
        );
        assert_eq!(
            hit(&scene, Point::new(20.0, 7.0), 0.5, None).unwrap().part,
            None
        );
        assert_eq!(hit(&scene, Point::new(20.0, 12.0), 0.5, None), None);
    }
}
