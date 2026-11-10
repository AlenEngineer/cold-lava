//! Snapshots of rasterized shapes: the Venn of `docs/shapes.md` at several sizes and turned.

mod common;

use cold_lava_canvas::{Area, Grid, Mask, Point, Res, Viewport};
use common::{Rng, paint_venn, render, snapshot, venn_on};

fn venn(cols: u16, rows: u16, rotation: f64) -> String {
    let view = Viewport::terminal(cols, rows);
    let mut grid = Grid::new(&view);
    paint_venn(&mut grid, &venn_on(&view, rotation), &view);
    render(&grid)
}

#[test]
fn venn_40x12() {
    snapshot("venn_40x12", &venn(40, 12, 0.0));
}

#[test]
fn venn_60x18() {
    snapshot("venn_60x18", &venn(60, 18, 0.0));
}

#[test]
fn venn_80x24() {
    snapshot("venn_80x24", &venn(80, 24, 0.0));
}

#[test]
fn venn_60x18_turned_60_degrees() {
    snapshot("venn_60x18_turned_60", &venn(60, 18, 60.0));
}

#[test]
fn region_masks_agree_with_the_region_at_every_sample() {
    let mut rng = Rng::new(5);
    let view = Viewport::terminal(50, 16);
    for _ in 0..20 {
        let placed = venn_on(&view, rng.range(0.0, 360.0));
        let to_local = placed.to_local().unwrap();
        let parts = placed.shape.parts();
        for res in [Res::CELLS, Res::BRAILLE] {
            for region in placed.shape.regions() {
                let mask = Mask::area(&placed, &region.area, &view, res);
                for sy in 0..i64::from(view.rows) * i64::from(res.y) {
                    for sx in 0..i64::from(view.cols) * i64::from(res.x) {
                        let world = Point::new(
                            view.cell.x * (sx as f64 + 0.5) / f64::from(res.x),
                            view.cell.y * (sy as f64 + 0.5) / f64::from(res.y),
                        );
                        let inside = region.area.contains(parts, to_local.apply(world));
                        assert_eq!(mask.get(sx, sy), inside, "{} at {sx},{sy}", region.id);
                    }
                }
            }
        }
    }
}

#[test]
fn every_cell_of_the_circles_is_in_exactly_one_region() {
    let view = Viewport::terminal(60, 18);
    let placed = venn_on(&view, 30.0);
    let union = Area::parse("left | right", placed.shape.parts()).unwrap();
    let all = Mask::area(&placed, &union, &view, Res::CELLS);
    let regions: Vec<Mask> = placed
        .shape
        .regions()
        .iter()
        .map(|r| Mask::area(&placed, &r.area, &view, Res::CELLS))
        .collect();
    assert!(all.count() > 0);
    for (x, y) in all.iter() {
        assert_eq!(regions.iter().filter(|m| m.get(x, y)).count(), 1, "{x},{y}");
    }
    assert_eq!(regions.iter().map(Mask::count).sum::<usize>(), all.count());
}
