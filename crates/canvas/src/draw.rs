//! A whole placed shape drawn into a grid, the way `cold-lava shapes --render` prints it and the
//! snapshots pin it: regions tinted, parts outlined in braille, text boxes filled, labels beside
//! their anchors.

use crate::area::Area;
use crate::fit::{self, Align, Flow, Room};
use crate::geom::{Placement, Point};
use crate::part::Kind;
use crate::raster::{Grid, Mask, Res, Style, Viewport};
use crate::shape::{Placed, Shape};
use std::sync::Arc;

/// Region `i` is tinted `Style(i + 1)`; these come after the first eight regions.
pub const OUTLINE: Style = Style(9);
pub const TEXT: Style = Style(10);
pub const LABEL: Style = Style(11);

/// Paints `placed`: each region's cells tinted `Style(i + 1)` in order, every part's outline in
/// braille, each text box's text wrapped inside it, then the labels (`fit::anchored`). Returns how
/// many labels found no place.
pub fn paint(grid: &mut Grid, placed: &Placed, view: &Viewport) -> usize {
    for (i, region) in placed.shape.regions().iter().enumerate() {
        let mask = Mask::area(placed, &region.area, view, Res::CELLS);
        grid.fill(&mask, Style(i as u16 + 1));
    }
    for part in 0..placed.shape.parts().len() {
        grid.dots(&Mask::outline(placed, part, view, Res::BRAILLE), OUTLINE);
    }
    for (i, part) in placed.shape.parts().iter().enumerate() {
        if let Kind::Text { value, .. } = &part.kind {
            let room = Room::covered(&Mask::area(placed, &Area::Part(i), view, Res::CELLS));
            fit::text(&room, value, Align::Start).paint(grid, TEXT);
        }
    }
    let labels = fit::anchored(placed, view);
    labels.paint(grid, LABEL);
    labels.hidden
}

/// Paints what each region shows: `contents[i]` are region `i`'s labels, in a column centred
/// inside it, with "+N more" for those that don't fit. Returns how many were left out.
pub fn paint_contents(
    grid: &mut Grid,
    placed: &Placed,
    view: &Viewport,
    contents: &[Vec<String>],
) -> usize {
    let mut hidden = 0;
    for (region, labels) in placed.shape.regions().iter().zip(contents) {
        if labels.is_empty() {
            continue;
        }
        let room = Room::inside(&Mask::area(placed, &region.area, view, Res::BRAILLE));
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let fitted = fit::labels(&room, &labels, Flow::Column, Align::Center);
        fitted.paint(grid, TEXT);
        hidden += fitted.hidden;
    }
    hidden
}

/// `shape` alone on a `cols`×`rows` terminal, as text: in the largest box of `aspect` (width over
/// height in world units, where a cell is 1×2) centred on the grid a cell in from its edges, or
/// that whole space without one, each region showing its `contents` (`paint_contents`; empty for
/// a shape with nothing bound), turned `rotation` degrees clockwise about its centre. The glyphs,
/// then each region's tint by row (`.` for none, else the region's number).
pub fn preview(
    shape: Arc<Shape>,
    cols: u16,
    rows: u16,
    aspect: Option<f64>,
    rotation: f64,
    contents: &[Vec<String>],
) -> String {
    let view = Viewport::terminal(cols, rows);
    // A cell of margin, so outlines along the box's edges aren't cut off with the grid.
    let margin = view.cell;
    let world = view.world_size() - margin * 2.0;
    let size = match aspect {
        Some(a) if a > 0.0 => {
            let w = world.x.min(world.y * a);
            Point::new(w, w / a)
        }
        _ => world,
    };
    let at = margin + Point::new((world.x - size.x) / 2.0, (world.y - size.y) / 2.0);
    let placed = Placed::new(shape, Placement::new(at, size).rotated(rotation));
    let mut grid = Grid::new(&view);
    let left_out = paint(&mut grid, &placed, &view);
    paint_contents(&mut grid, &placed, &view, contents);
    let mut out = grid.to_text();
    if left_out > 0 {
        let s = if left_out == 1 { "" } else { "s" };
        out += &format!("--- {left_out} label{s} left out: no room beside the anchor\n");
    }
    let regions = placed.shape.regions();
    if !regions.is_empty() {
        let names: Vec<String> = (regions.iter().enumerate())
            .map(|(i, r)| format!("{} {}", i + 1, r.id))
            .collect();
        out += &format!("--- regions: {}\n{}", names.join(" · "), grid.backgrounds());
    }
    out
}
