//! The Venn of `docs/shapes.md` with names fitted into its three regions, at several sizes and
//! turned.

mod common;

use cold_lava_canvas::{Grid, Viewport};
use common::{paint_labelled_venn, render, snapshot, venn_on};

fn labelled(cols: u16, rows: u16, rotation: f64) -> String {
    let view = Viewport::terminal(cols, rows);
    let mut grid = Grid::new(&view);
    paint_labelled_venn(&mut grid, &venn_on(&view, rotation), &view);
    render(&grid)
}

#[test]
fn labelled_venn_40x12() {
    snapshot("labelled_venn_40x12", &labelled(40, 12, 0.0));
}

#[test]
fn labelled_venn_60x18() {
    snapshot("labelled_venn_60x18", &labelled(60, 18, 0.0));
}

#[test]
fn labelled_venn_80x24() {
    snapshot("labelled_venn_80x24", &labelled(80, 24, 0.0));
}

#[test]
fn labelled_venn_60x18_turned_60_degrees() {
    snapshot("labelled_venn_60x18_turned_60", &labelled(60, 18, 60.0));
}
