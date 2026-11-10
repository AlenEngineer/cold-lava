//! Properties of regions and placements, over many random points.

mod common;

use cold_lava_canvas::{Area, Kind, Part, Path, Placed, Placement, Point, Rect};
use common::{Rng, venn2};
use std::sync::Arc;

fn random_area(rng: &mut Rng, id: &str) -> Part {
    let kind = match rng.next() % 4 {
        0 => Kind::Circle {
            center: rng.point(0.2, 0.8),
            radius: rng.range(0.05, 0.4),
        },
        1 => Kind::Ellipse {
            center: rng.point(0.2, 0.8),
            radii: Point::new(rng.range(0.05, 0.4), rng.range(0.05, 0.4)),
        },
        2 => {
            let (a, b) = (rng.point(0.0, 1.0), rng.point(0.0, 1.0));
            Kind::Rect(Rect::around([a, b]).unwrap())
        }
        _ => Kind::Path(Path::polyline(
            (0..5).map(|_| rng.point(0.0, 1.0)).collect(),
            true,
        )),
    };
    Part::new(Some(id), kind)
}

#[test]
fn operators_mean_both_either_and_but_not() {
    let mut rng = Rng::new(7);
    for _ in 0..200 {
        let parts = vec![
            random_area(&mut rng, "a"),
            random_area(&mut rng, "b"),
            random_area(&mut rng, "c"),
        ];
        let parse = |s| Area::parse(s, &parts).unwrap();
        let (and, or, minus) = (parse("a & b"), parse("a | b"), parse("a - b"));
        let nested = parse("(a | b) - c & a");
        for _ in 0..200 {
            let p = rng.point(-0.1, 1.1);
            let [a, b, c] = [0, 1, 2].map(|i| parts[i].contains(p));
            assert_eq!(and.contains(&parts, p), a && b, "{p:?}");
            assert_eq!(or.contains(&parts, p), a || b, "{p:?}");
            assert_eq!(minus.contains(&parts, p), a && !b, "{p:?}");
            assert_eq!(nested.contains(&parts, p), (a || b) && !(c && a), "{p:?}");
            for area in [&and, &or, &minus, &nested] {
                if area.contains(&parts, p) {
                    assert!(area.bounds(&parts).contains(p), "bounds hold {p:?}");
                }
            }
        }
    }
}

#[test]
fn venn_regions_partition_the_two_circles() {
    let venn = venn2();
    let union = Area::parse("left | right", venn.parts()).unwrap();
    let mut rng = Rng::new(11);
    for _ in 0..20_000 {
        let p = rng.point(0.0, 1.0);
        let inside = venn
            .regions()
            .iter()
            .filter(|r| r.area.contains(venn.parts(), p))
            .count();
        let expected = usize::from(union.contains(venn.parts(), p));
        assert_eq!(inside, expected, "{p:?}");
    }
}

#[test]
fn placement_round_trips_any_point() {
    let mut rng = Rng::new(3);
    let shape = Arc::new(venn2());
    for _ in 0..1000 {
        let placement = Placement::new(rng.point(-100.0, 100.0), rng.point(0.5, 80.0))
            .rotated(rng.range(-360.0, 360.0));
        let placed = Placed::new(shape.clone(), placement);
        let to_world = placement.transform();
        let p = rng.point(-2.0, 3.0);
        let back = placed.to_local().unwrap().apply(to_world.apply(p));
        assert!((back - p).length() < 1e-9, "{p:?} came back as {back:?}");
        let b = placement.world_bounds(&Rect::UNIT);
        let w = to_world.apply(rng.point(0.0, 1.0));
        let eps = 1e-9;
        assert!(
            w.x >= b.min.x - eps
                && w.x <= b.max.x + eps
                && w.y >= b.min.y - eps
                && w.y <= b.max.y + eps,
            "the world box holds every point of the unit box"
        );
    }
}
