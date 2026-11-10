//! Rasterizing: areas and strokes to a grid of cells, as plain data a frontend paints.
//!
//! Everything goes through one step, a [`Mask`]: the samples of the grid an area or a stroke
//! covers, at some number of samples per cell ([`Res`]). One sample per cell gives terminal
//! cells (fill and border), 2×4 gives braille dots, a cell's size in pixels gives a Kitty
//! image. An area's mask asks the region tree about each sample's centre, mapped back into the
//! shape's unit box, so rotation and stretching cost nothing extra; strokes are walked in
//! half-sample steps. The [`Grid`] then holds a character and two style tokens per cell; text
//! is always upright, one glyph per cell.

use crate::area::Area;
use crate::geom::Point;
use crate::part::Kind;
use crate::shape::Placed;

/// An arrow's head, in world units (a cell's width in [`Viewport::terminal`]).
pub const ARROW_HEAD: f64 = 2.0;

/// Samples per cell, across and down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Res {
    pub x: u32,
    pub y: u32,
}

impl Res {
    pub const CELLS: Res = Res { x: 1, y: 1 };
    pub const BRAILLE: Res = Res { x: 2, y: 4 };
}

/// The grid being drawn and the part of the world it shows: cell (0, 0)'s top-left corner is
/// at `origin`, and a cell is `cell` world units across and down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub cols: u16,
    pub rows: u16,
    pub origin: Point,
    pub cell: Point,
}

impl Viewport {
    /// A terminal's grid at the world's origin: a cell is one unit wide and two tall (cells are
    /// about twice as tall as wide), so a circle in a square box comes out round.
    pub fn terminal(cols: u16, rows: u16) -> Viewport {
        Viewport {
            cols,
            rows,
            origin: Point::new(0.0, 0.0),
            cell: Point::new(1.0, 2.0),
        }
    }

    /// The world size of the whole grid.
    pub fn world_size(&self) -> Point {
        Point::new(
            f64::from(self.cols) * self.cell.x,
            f64::from(self.rows) * self.cell.y,
        )
    }

    /// The cell a world point falls in, if it is on the grid.
    pub fn cell_at(&self, p: Point) -> Option<(u16, u16)> {
        let (u, v) = self.sample_coords(Res::CELLS, p);
        let (col, row) = (u.floor(), v.floor());
        let on = (0.0..f64::from(self.cols)).contains(&col)
            && (0.0..f64::from(self.rows)).contains(&row);
        on.then_some((col as u16, row as u16))
    }

    /// The world point at the centre of a cell.
    pub fn cell_center(&self, col: u16, row: u16) -> Point {
        self.sample_center(Res::CELLS, i64::from(col), i64::from(row))
    }

    fn samples(&self, res: Res) -> (i64, i64) {
        (
            i64::from(self.cols) * i64::from(res.x),
            i64::from(self.rows) * i64::from(res.y),
        )
    }

    fn sample_coords(&self, res: Res, p: Point) -> (f64, f64) {
        (
            (p.x - self.origin.x) / self.cell.x * f64::from(res.x),
            (p.y - self.origin.y) / self.cell.y * f64::from(res.y),
        )
    }

    fn sample_center(&self, res: Res, sx: i64, sy: i64) -> Point {
        Point::new(
            self.origin.x + (sx as f64 + 0.5) * self.cell.x / f64::from(res.x),
            self.origin.y + (sy as f64 + 0.5) * self.cell.y / f64::from(res.y),
        )
    }
}

/// The samples of the grid something covers. It only stores the box it can touch, clipped to
/// the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    res: Res,
    /// The whole grid, in samples.
    grid: (i64, i64),
    x0: i64,
    y0: i64,
    w: i64,
    h: i64,
    bits: Vec<bool>,
}

impl Mask {
    fn empty(view: &Viewport, res: Res) -> Mask {
        Mask {
            res,
            grid: view.samples(res),
            x0: 0,
            y0: 0,
            w: 0,
            h: 0,
            bits: Vec::new(),
        }
    }

    /// Samples in `x0..x1`, `y0..y1` (clipped to the grid), set where `inside` says so.
    fn sampled(
        view: &Viewport,
        res: Res,
        (x0, y0, x1, y1): (i64, i64, i64, i64),
        mut inside: impl FnMut(i64, i64) -> bool,
    ) -> Mask {
        let grid = view.samples(res);
        let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(grid.0), y1.min(grid.1));
        if x0 >= x1 || y0 >= y1 {
            return Mask::empty(view, res);
        }
        let (w, h) = (x1 - x0, y1 - y0);
        let mut bits = Vec::with_capacity((w * h) as usize);
        for sy in y0..y1 {
            for sx in x0..x1 {
                bits.push(inside(sx, sy));
            }
        }
        Mask {
            res,
            grid,
            x0,
            y0,
            w,
            h,
            bits,
        }
    }

    /// The samples whose centre is inside `area` of a placed shape.
    pub fn area(placed: &Placed, area: &Area, view: &Viewport, res: Res) -> Mask {
        let Some(to_local) = placed.to_local() else {
            return Mask::empty(view, res);
        };
        let parts = placed.shape.parts();
        let world = placed.placement.world_bounds(&area.bounds(parts));
        let (u0, v0) = view.sample_coords(res, world.min);
        let (u1, v1) = view.sample_coords(res, world.max);
        let range = (
            (u0 - 0.5).floor() as i64,
            (v0 - 0.5).floor() as i64,
            (u1 + 0.5).ceil() as i64,
            (v1 + 0.5).ceil() as i64,
        );
        // Sample centres step evenly, so their unit-box points do too.
        let local = |sx, sy| to_local.apply(view.sample_center(res, sx, sy));
        let origin = local(0, 0);
        let (across, down) = (local(1, 0) - origin, local(0, 1) - origin);
        Mask::sampled(view, res, range, |sx, sy| {
            let p = origin + across * sx as f64 + down * sy as f64;
            area.contains(parts, p)
        })
    }

    /// The samples world polylines pass through.
    pub fn strokes(lines: &[Vec<Point>], view: &Viewport, res: Res) -> Mask {
        let mut hits = Vec::new();
        for line in lines {
            for pair in line.windows(2) {
                let (u0, v0) = view.sample_coords(res, pair[0]);
                let (u1, v1) = view.sample_coords(res, pair[1]);
                let steps = ((u1 - u0).abs().max((v1 - v0).abs()) * 2.0).ceil().max(1.0) as i64;
                for i in 0..=steps {
                    let t = i as f64 / steps as f64;
                    let (u, v) = (u0 + (u1 - u0) * t, v0 + (v1 - v0) * t);
                    if u.is_finite() && v.is_finite() {
                        hits.push((u.floor() as i64, v.floor() as i64));
                    }
                }
            }
        }
        let grid = view.samples(res);
        hits.retain(|&(x, y)| (0..grid.0).contains(&x) && (0..grid.1).contains(&y));
        let Some(&(fx, fy)) = hits.first() else {
            return Mask::empty(view, res);
        };
        let (mut x0, mut y0, mut x1, mut y1) = (fx, fy, fx, fy);
        for &(x, y) in &hits {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let mut mask = Mask::sampled(view, res, (x0, y0, x1 + 1, y1 + 1), |_, _| false);
        for (x, y) in hits {
            let i = mask.index(x, y).expect("inside the box of the hits");
            mask.bits[i] = true;
        }
        mask
    }

    /// What drawing a part inks: an area's outline (the edge of its inside), a stroke's line,
    /// nothing for a text box or a label (their text is fitted, not drawn as a shape).
    pub fn outline(placed: &Placed, part: usize, view: &Viewport, res: Res) -> Mask {
        let p = &placed.shape.parts()[part];
        match p.kind {
            Kind::Text { .. } | Kind::Label { .. } => Mask::empty(view, res),
            _ if p.is_area() => Mask::area(placed, &Area::Part(part), view, res).edge(),
            _ => Mask::strokes(
                &p.world_strokes(&placed.placement.transform(), ARROW_HEAD),
                view,
                res,
            ),
        }
    }

    pub fn res(&self) -> Res {
        self.res
    }

    fn index(&self, sx: i64, sy: i64) -> Option<usize> {
        let (x, y) = (sx - self.x0, sy - self.y0);
        ((0..self.w).contains(&x) && (0..self.h).contains(&y)).then(|| (y * self.w + x) as usize)
    }

    /// Whether a sample (in grid samples, not cells) is covered.
    pub fn get(&self, sx: i64, sy: i64) -> bool {
        self.index(sx, sy).is_some_and(|i| self.bits[i])
    }

    /// The covered samples, row by row.
    pub fn iter(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.bits
            .iter()
            .enumerate()
            .filter(|(_, b)| **b)
            .map(|(i, _)| (self.x0 + i as i64 % self.w, self.y0 + i as i64 / self.w))
    }

    pub fn count(&self) -> usize {
        self.bits.iter().filter(|b| **b).count()
    }

    /// The covered samples next to an uncovered one (left, right, above or below): the area's
    /// outline, drawn just inside it. Off the grid counts as covered, so an area cut off by the
    /// edge of the grid has no outline along it.
    pub fn edge(&self) -> Mask {
        let (w, h) = (self.w, self.h);
        let at = |x: i64, y: i64| self.bits[(y * w + x) as usize];
        // A neighbour outside the stored box is uncovered, unless it is off the grid.
        let covered = |x: i64, y: i64| {
            if (0..w).contains(&x) && (0..h).contains(&y) {
                at(x, y)
            } else {
                let (gx, gy) = (self.x0 + x, self.y0 + y);
                !(0..self.grid.0).contains(&gx) || !(0..self.grid.1).contains(&gy)
            }
        };
        let mut edge = self.clone();
        for y in 0..h {
            for x in 0..w {
                if at(x, y) {
                    let inner = covered(x - 1, y)
                        && covered(x + 1, y)
                        && covered(x, y - 1)
                        && covered(x, y + 1);
                    edge.bits[(y * w + x) as usize] = !inner;
                }
            }
        }
        edge
    }

    /// The covered samples that are not on the outline.
    pub fn interior(&self) -> Mask {
        let edge = self.edge();
        let mut inner = self.clone();
        for (bit, on_edge) in inner.bits.iter_mut().zip(&edge.bits) {
            *bit &= !on_edge;
        }
        inner
    }

    /// The cells every one of whose samples is covered.
    pub fn whole_cells(&self) -> Vec<(u16, u16)> {
        let (rx, ry) = (i64::from(self.res.x), i64::from(self.res.y));
        let cells =
            |lo: i64, len: i64, r: i64| (lo + r - 1).div_euclid(r)..(lo + len).div_euclid(r);
        let mut out = Vec::new();
        for row in cells(self.y0, self.h, ry) {
            for col in cells(self.x0, self.w, rx) {
                let all =
                    (0..ry).all(|dy| (0..rx).all(|dx| self.get(col * rx + dx, row * ry + dy)));
                if all {
                    out.push((col as u16, row as u16));
                }
            }
        }
        out
    }
}

/// A style token. The engine doesn't know colours; a frontend maps tokens to its own (a region's
/// tint, an outline's ink). `Style::NONE` is the frontend's default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style(pub u16);

impl Style {
    pub const NONE: Style = Style(0);
}

/// One terminal cell: a glyph in `fg` on `bg`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Style,
    pub bg: Style,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell {
            ch: ' ',
            fg: Style::NONE,
            bg: Style::NONE,
        }
    }
}

const BRAILLE_BLANK: u32 = 0x2800;

/// A braille cell's bit for the dot `dx` (0..2) across and `dy` (0..4) down.
fn braille_bit(dx: i64, dy: i64) -> u32 {
    const BITS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    BITS[dx as usize][dy as usize]
}

fn braille_bits(ch: char) -> Option<u32> {
    let c = u32::from(ch);
    (BRAILLE_BLANK..BRAILLE_BLANK + 0x100)
        .contains(&c)
        .then(|| c - BRAILLE_BLANK)
}

/// The rasterized frame: what the TUI paints, cell for cell. Painting happens in the caller's
/// order, so what is painted last is on top.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
}

impl Grid {
    pub fn new(view: &Viewport) -> Grid {
        Grid {
            cols: view.cols,
            rows: view.rows,
            cells: vec![Cell::default(); usize::from(view.cols) * usize::from(view.rows)],
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }

    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub fn cell(&self, col: u16, row: u16) -> Cell {
        self.cells[usize::from(row) * usize::from(self.cols) + usize::from(col)]
    }

    fn cell_mut(&mut self, col: i64, row: i64) -> Option<&mut Cell> {
        let on =
            (0..i64::from(self.cols)).contains(&col) && (0..i64::from(self.rows)).contains(&row);
        on.then(|| &mut self.cells[row as usize * usize::from(self.cols) + col as usize])
    }

    /// Tints the background of each covered cell (a one-sample-per-cell mask).
    pub fn fill(&mut self, mask: &Mask, style: Style) {
        assert_eq!(mask.res, Res::CELLS, "fill takes a mask of whole cells");
        for (col, row) in mask.iter() {
            if let Some(cell) = self.cell_mut(col, row) {
                cell.bg = style;
            }
        }
    }

    /// Puts `ch` in each covered cell (a one-sample-per-cell mask): a cell-mode border.
    pub fn mark(&mut self, mask: &Mask, ch: char, style: Style) {
        assert_eq!(mask.res, Res::CELLS, "mark takes a mask of whole cells");
        for (col, row) in mask.iter() {
            if let Some(cell) = self.cell_mut(col, row) {
                cell.ch = ch;
                cell.fg = style;
            }
        }
    }

    /// Adds braille dots (a 2×4 mask), merging with dots already in a cell. A cell holding
    /// another glyph (text) keeps it.
    pub fn dots(&mut self, mask: &Mask, style: Style) {
        assert_eq!(mask.res, Res::BRAILLE, "dots takes a braille mask");
        for (sx, sy) in mask.iter() {
            let Some(cell) = self.cell_mut(sx.div_euclid(2), sy.div_euclid(4)) else {
                continue;
            };
            let old = if cell.ch == ' ' {
                Some(0)
            } else {
                braille_bits(cell.ch)
            };
            if let Some(old) = old {
                let bits = old | braille_bit(sx.rem_euclid(2), sy.rem_euclid(4));
                cell.ch = char::from_u32(BRAILLE_BLANK + bits).expect("braille block");
                cell.fg = style;
            }
        }
    }

    /// Writes text from a cell rightwards, one glyph per cell, cut at the grid's edge.
    pub fn text(&mut self, col: u16, row: u16, text: &str, style: Style) {
        for (i, ch) in text.chars().enumerate() {
            match self.cell_mut(i64::from(col) + i as i64, i64::from(row)) {
                Some(cell) => {
                    cell.ch = ch;
                    cell.fg = style;
                }
                None => break,
            }
        }
    }

    /// The glyphs, a line per row, trailing blanks dropped.
    pub fn to_text(&self) -> String {
        self.lines(|c| c.ch)
    }

    /// The backgrounds, a line per row: `.` for none, else the token in base 36.
    pub fn backgrounds(&self) -> String {
        self.lines(|c| match c.bg {
            Style::NONE => '.',
            Style(n) => char::from_digit(u32::from(n) % 36, 36).expect("base 36"),
        })
    }

    fn lines(&self, glyph: impl Fn(&Cell) -> char) -> String {
        let mut out = String::new();
        for row in self.cells.chunks(usize::from(self.cols).max(1)) {
            let line: String = row.iter().map(&glyph).collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

/// A pixel buffer of style tokens, for a frontend that draws images (Kitty graphics): it maps
/// tokens to colours and encodes the image itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub px: Vec<Style>,
}

impl Pixels {
    /// A buffer covering the grid, `res` pixels per cell.
    pub fn new(view: &Viewport, res: Res) -> Pixels {
        let (w, h) = view.samples(res);
        Pixels {
            width: w as u32,
            height: h as u32,
            px: vec![Style::NONE; (w * h) as usize],
        }
    }

    pub fn paint(&mut self, mask: &Mask, style: Style) {
        for (x, y) in mask.iter() {
            if (0..i64::from(self.width)).contains(&x) && (0..i64::from(self.height)).contains(&y) {
                self.px[y as usize * self.width as usize + x as usize] = style;
            }
        }
    }

    pub fn get(&self, x: u32, y: u32) -> Style {
        self.px[y as usize * self.width as usize + x as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Placement, Rect};
    use crate::part::Part;
    use crate::shape::Shape;
    use std::sync::Arc;

    fn placed(parts: Vec<Part>, at: Point, size: Point) -> Placed {
        Placed::new(
            Arc::new(Shape::new(parts, &[]).unwrap()),
            Placement::new(at, size),
        )
    }

    fn square(at: Point, size: f64) -> Placed {
        placed(
            vec![Part::new(Some("box"), Kind::Rect(Rect::UNIT))],
            at,
            Point::new(size, size),
        )
    }

    #[test]
    fn cell_mask_matches_cell_centres() {
        let view = Viewport::terminal(10, 5);
        let sq = square(Point::new(2.0, 2.0), 4.0);
        let mask = Mask::area(&sq, &Area::Part(0), &view, Res::CELLS);
        let cells: Vec<_> = mask.iter().collect();
        // x 2..6 holds centres 2.5..5.5 (cols 2..=5); y 2..6 holds centres 3 and 5 (rows 1, 2).
        let expected: Vec<_> = (1..=2).flat_map(|r| (2..=5).map(move |c| (c, r))).collect();
        assert_eq!(cells, expected);
    }

    #[test]
    fn edge_is_the_inner_ring_and_skips_the_grid_border() {
        let view = Viewport::terminal(6, 6);
        let inner = square(Point::new(1.0, 2.0), 4.0);
        let edge = Mask::area(&inner, &Area::Part(0), &view, Res::CELLS).edge();
        assert_eq!(edge.count(), 8, "a 4×2 block is all edge");
        let over = square(Point::new(-5.0, -5.0), 100.0);
        assert_eq!(
            Mask::area(&over, &Area::Part(0), &view, Res::CELLS)
                .edge()
                .count(),
            0
        );
    }

    #[test]
    fn braille_dots_merge_and_text_wins() {
        let view = Viewport::terminal(2, 1);
        let mut grid = Grid::new(&view);
        let line = |y| vec![vec![Point::new(0.0, y), Point::new(0.9, y)]];
        grid.dots(&Mask::strokes(&line(0.1), &view, Res::BRAILLE), Style(1));
        assert_eq!(grid.cell(0, 0).ch, '⠉');
        grid.dots(&Mask::strokes(&line(1.9), &view, Res::BRAILLE), Style(1));
        assert_eq!(grid.cell(0, 0).ch, '⣉');
        grid.text(1, 0, "ab", Style(2));
        assert_eq!(grid.to_text(), "⣉a\n", "text is cut at the grid's edge");
        grid.dots(
            &Mask::strokes(
                &[vec![Point::new(1.2, 0.2), Point::new(1.2, 1.8)]],
                &view,
                Res::BRAILLE,
            ),
            Style(1),
        );
        assert_eq!(grid.cell(1, 0).ch, 'a');
    }

    #[test]
    fn strokes_clip_to_the_grid() {
        let view = Viewport::terminal(4, 2);
        let mask = Mask::strokes(
            &[vec![Point::new(-10.0, 1.0), Point::new(10.0, 1.0)]],
            &view,
            Res::CELLS,
        );
        assert_eq!(
            mask.iter().collect::<Vec<_>>(),
            vec![(0, 0), (1, 0), (2, 0), (3, 0)]
        );
        let off = Mask::strokes(
            &[vec![Point::new(-10.0, -1.0), Point::new(-5.0, -1.0)]],
            &view,
            Res::CELLS,
        );
        assert_eq!(off.count(), 0);
    }

    #[test]
    fn pixels_cover_the_circle_area() {
        let view = Viewport::terminal(20, 10);
        let disc = placed(
            vec![Part::new(
                Some("c"),
                Kind::Circle {
                    center: Point::new(0.5, 0.5),
                    radius: 0.5,
                },
            )],
            Point::new(0.0, 0.0),
            Point::new(20.0, 20.0),
        );
        let res = Res { x: 8, y: 16 };
        let mut px = Pixels::new(&view, res);
        px.paint(&Mask::area(&disc, &Area::Part(0), &view, res), Style(3));
        let painted = px.px.iter().filter(|s| **s == Style(3)).count() as f64;
        // A world unit is 8 pixels across and 8 down, so the disc is π·(10·8)² pixels.
        let expected = std::f64::consts::PI * 80.0 * 80.0;
        assert!(
            (painted / expected - 1.0).abs() < 0.01,
            "{painted} vs {expected}"
        );
        assert_eq!(px.get(80, 80), Style(3));
        assert_eq!(px.get(0, 0), Style::NONE);
    }

    #[test]
    fn whole_cells_need_every_sample() {
        let view = Viewport::terminal(10, 5);
        // x 1..5 covers dots 2..10, i.e. cells 1..=4 fully; y 1..7 covers dot rows 2..14, i.e.
        // cells rows 1 and 2 fully (dot rows 4..12), row 0 and 3 partly.
        let sq = placed(
            vec![Part::new(Some("box"), Kind::Rect(Rect::UNIT))],
            Point::new(1.0, 1.0),
            Point::new(4.0, 6.0),
        );
        let mask = Mask::area(&sq, &Area::Part(0), &view, Res::BRAILLE);
        let cells = mask.whole_cells();
        let expected: Vec<_> = (1..=2).flat_map(|r| (1..=4).map(move |c| (c, r))).collect();
        assert_eq!(cells, expected);
    }

    #[test]
    fn cell_lookup() {
        let view = Viewport::terminal(10, 5);
        assert_eq!(view.cell_at(Point::new(3.5, 5.0)), Some((3, 2)));
        assert_eq!(view.cell_at(Point::new(10.0, 1.0)), None);
        assert_eq!(view.cell_center(3, 2), Point::new(3.5, 5.0));
    }
}
