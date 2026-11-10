#![allow(dead_code)]

use cold_lava_canvas::fit::{self, Align, Flow, Room};
use cold_lava_canvas::{
    Grid, Kind, Mask, Part, Placed, Placement, Point, Res, Shape, Style, Viewport,
};
use std::sync::Arc;

pub const LABELS: [&[&str]; 3] = [
    &[
        "apple", "banana", "cherry", "grape", "lemon", "mango", "pear",
    ],
    &["kiwi", "lime", "plum"],
    &["olive", "peach", "quince", "fig"],
];

/// The Venn with each region's labels fitted in a column, and the circles' titles centred
/// one row above their `top` anchors.
pub fn paint_labelled_venn(grid: &mut Grid, venn: &Placed, view: &Viewport) {
    paint_venn(grid, venn, view);
    for (region, labels) in venn.shape.regions().iter().zip(LABELS) {
        let room = Room::inside(&Mask::area(venn, &region.area, view, Res::BRAILLE));
        fit::labels(&room, labels, Flow::Column, Align::Center).paint(grid, Style(7));
    }
    for (anchor, title) in [("left.top", "Fruit"), ("right.top", "Green")] {
        let Some((col, row)) = venn.anchor(anchor).and_then(|p| view.cell_at(p)) else {
            continue;
        };
        let start = col.saturating_sub(title.len() as u16 / 2);
        grid.text(start, row.saturating_sub(1), title, Style(8));
    }
}

/// Compares `actual` with `tests/snapshots/NAME.txt`; `UPDATE_SNAPSHOTS=1` rewrites it instead.
pub fn snapshot(name: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, actual).expect("write the snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no snapshot {}; run with UPDATE_SNAPSHOTS=1",
            path.display()
        )
    });
    assert!(
        expected == actual,
        "snapshot {name} differs\n--- expected\n{expected}--- actual\n{actual}"
    );
}

/// The Venn in the largest square (in world units) that fits the grid, centred.
pub fn venn_on(view: &Viewport, rotation: f64) -> Placed {
    let world = view.world_size();
    let side = world.x.min(world.y);
    let at = Point::new((world.x - side) / 2.0, (world.y - side) / 2.0);
    Placed::new(
        Arc::new(venn2()),
        Placement::new(at, Point::new(side, side)).rotated(rotation),
    )
}

pub const OUTLINE: Style = Style(9);

/// Regions tinted 1, 2, 3 by cell; circles outlined in braille.
pub fn paint_venn(grid: &mut Grid, venn: &Placed, view: &Viewport) {
    for (i, region) in venn.shape.regions().iter().enumerate() {
        let mask = Mask::area(venn, &region.area, view, Res::CELLS);
        grid.fill(&mask, Style(i as u16 + 1));
    }
    for part in 0..venn.shape.parts().len() {
        grid.dots(&Mask::outline(venn, part, view, Res::BRAILLE), OUTLINE);
    }
}

pub fn render(grid: &Grid) -> String {
    format!("{}--- backgrounds\n{}", grid.to_text(), grid.backgrounds())
}

/// The two-circle Venn of `docs/shapes.md`.
pub fn venn2() -> Shape {
    let circle = |id, x| {
        Part::new(
            Some(id),
            Kind::Circle {
                center: Point::new(x, 0.5),
                radius: 0.32,
            },
        )
    };
    Shape::new(
        vec![circle("left", 0.38), circle("right", 0.62)],
        &[
            ("only-a", "left - right"),
            ("both", "left & right"),
            ("only-b", "right - left"),
        ],
    )
    .expect("the Venn is valid")
}

/// xorshift64*: deterministic, so a failing case reruns the same way.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `lo..hi`.
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() >> 11) as f64 / (1u64 << 53) as f64 * (hi - lo)
    }

    pub fn point(&mut self, lo: f64, hi: f64) -> Point {
        Point::new(self.range(lo, hi), self.range(lo, hi))
    }
}
