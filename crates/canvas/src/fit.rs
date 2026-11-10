//! Fitting: text and lists of short labels laid out inside a region at the current size.
//!
//! A region's [`Room`] is, per row, the widest run of cells wholly inside it and clear of its
//! outline, so fitting works the same for a rectangle, a crescent or a turned lens. Text stays
//! upright, one glyph per cell. What doesn't fit is counted: a list ends with "+N more", text
//! ends with "…".

use crate::raster::{Grid, Mask, Style};

/// Where text can go in a region, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    runs: Vec<Run>,
}

/// Cells `col..col + width` of `row`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub row: u16,
    pub col: u16,
    pub width: u16,
}

impl Room {
    /// The room in an area's mask, at any resolution: a cell counts when every sample in it is
    /// covered and none is on the area's outline. Braille masks keep text off outlines drawn
    /// in dots.
    pub fn inside(mask: &Mask) -> Room {
        let mut runs: Vec<Run> = Vec::new();
        let mut current: Option<Run> = None;
        for (col, row) in mask.interior().whole_cells() {
            match &mut current {
                Some(run) if run.row == row && run.col + run.width == col => run.width += 1,
                _ => {
                    if let Some(run) = current.take() {
                        keep_widest(&mut runs, run);
                    }
                    current = Some(Run { row, col, width: 1 });
                }
            }
        }
        if let Some(run) = current {
            keep_widest(&mut runs, run);
        }
        Room { runs }
    }

    /// Rows given directly, top to bottom.
    pub fn from_runs(runs: Vec<Run>) -> Room {
        Room { runs }
    }

    pub fn runs(&self) -> &[Run] {
        &self.runs
    }
}

fn keep_widest(runs: &mut Vec<Run>, run: Run) {
    match runs.last_mut() {
        Some(last) if last.row == run.row => {
            if run.width > last.width {
                *last = run;
            }
        }
        _ => runs.push(run),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
}

/// How a list of labels fills the room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// One label per row.
    Column,
    /// As many labels per row as fit, two spaces apart.
    Wrap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub col: u16,
    pub row: u16,
    pub text: String,
}

/// The lines that fit, how many of the items (labels, or words of text) they show, and how
/// many are left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fitted {
    pub lines: Vec<Line>,
    pub shown: usize,
    pub hidden: usize,
}

impl Fitted {
    pub fn paint(&self, grid: &mut Grid, style: Style) {
        for line in &self.lines {
            grid.text(line.col, line.row, &line.text, style);
        }
    }
}

/// An item wider than its row is cut to fit with "…", but only in a row at least this wide;
/// narrower rows (the tip of a lens) are skipped.
const MIN_CUT: usize = 4;

fn width(s: &str) -> usize {
    s.chars().count()
}

fn cut(s: &str, room: usize) -> String {
    let mut out: String = s.chars().take(room.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Lines in order, each a run's index, its text and how many items it holds.
type Packed = Vec<(usize, String, usize)>;

/// Puts items in order into `runs[from..]`, as many per line as `many` allows.
fn pack(runs: &[Run], from: usize, items: &[&str], many: bool, sep: &str) -> Packed {
    let mut lines: Packed = Vec::new();
    let mut items = items.iter().peekable();
    let mut r = from;
    while r < runs.len() {
        let Some(&&item) = items.peek() else { break };
        let room = usize::from(runs[r].width);
        match lines.last_mut() {
            Some((at, text, count)) if *at == r => {
                if many && width(text) + width(sep) + width(item) <= room {
                    text.push_str(sep);
                    text.push_str(item);
                    *count += 1;
                    items.next();
                } else {
                    r += 1;
                }
            }
            _ if width(item) <= room => {
                lines.push((r, item.to_string(), 1));
                items.next();
                if !many {
                    r += 1;
                }
            }
            _ if room >= MIN_CUT => {
                lines.push((r, cut(item, room), 1));
                items.next();
                r += 1;
            }
            _ => r += 1,
        }
    }
    lines
}

fn shown(lines: &Packed) -> usize {
    lines.iter().map(|(_, _, n)| n).sum()
}

/// The best of laying out from each starting row: the most items shown, then the block most
/// nearly centred between the room's first and last rows.
fn best(runs: &[Run], layout: impl Fn(usize) -> (Packed, usize)) -> (Packed, usize) {
    let (Some(first), Some(last)) = (runs.first(), runs.last()) else {
        return (Vec::new(), 0);
    };
    let imbalance = |lines: &Packed| match (lines.first(), lines.last()) {
        (Some((a, ..)), Some((b, ..))) => {
            let above = i32::from(runs[*a].row) - i32::from(first.row);
            let below = i32::from(last.row) - i32::from(runs[*b].row);
            (above - below).abs()
        }
        _ => 0,
    };
    (0..runs.len())
        .map(&layout)
        .max_by_key(|(lines, count)| (*count, -imbalance(lines)))
        .expect("at least one run")
}

fn finish(runs: &[Run], lines: Packed, shown: usize, total: usize, align: Align) -> Fitted {
    let lines = lines
        .into_iter()
        .map(|(r, text, _)| {
            let run = runs[r];
            let spare = usize::from(run.width).saturating_sub(width(&text)) as u16;
            let col = match align {
                Align::Start => run.col,
                Align::Center => run.col + spare / 2,
            };
            Line {
                col,
                row: run.row,
                text,
            }
        })
        .collect();
    Fitted {
        lines,
        shown,
        hidden: total - shown,
    }
}

/// A list of labels; when not all fit, the last line says "+N more" (or "+N" where only that
/// fits), and labels make way for it.
pub fn labels(room: &Room, labels: &[&str], flow: Flow, align: Align) -> Fitted {
    let runs = room.runs();
    let total = labels.len();
    let many = flow == Flow::Wrap;
    let layout = |from: usize| -> (Packed, usize) {
        let all = pack(runs, from, labels, many, "  ");
        let fit = shown(&all);
        if fit == total {
            return (all, fit);
        }
        for keep in (0..=fit).rev() {
            let mut lines = pack(runs, from, &labels[..keep], many, "  ");
            let next = lines.last().map_or(from, |(r, ..)| r + 1);
            let hidden = total - keep;
            let notes = [format!("+{hidden} more"), format!("+{hidden}")];
            let spot = notes.iter().find_map(|note| {
                (next..runs.len())
                    .find(|&r| width(note) <= usize::from(runs[r].width))
                    .map(|r| (r, note.clone()))
            });
            if let Some((r, note)) = spot {
                lines.push((r, note, 0));
                return (lines, keep);
            }
        }
        (Vec::new(), 0)
    };
    let (lines, count) = best(runs, layout);
    finish(runs, lines, count, total, align)
}

/// Text wrapped at spaces; when not all of it fits, the last line ends with "…". `shown` and
/// `hidden` count words.
pub fn text(room: &Room, text: &str, align: Align) -> Fitted {
    let runs = room.runs();
    let words: Vec<&str> = text.split_whitespace().collect();
    let (mut lines, count) = best(runs, |from| {
        let lines = pack(runs, from, &words, true, " ");
        let count = shown(&lines);
        (lines, count)
    });
    if count < words.len()
        && let Some((r, text, _)) = lines.last_mut()
    {
        let room = usize::from(runs[*r].width);
        if width(text) < room {
            text.push('…');
        } else {
            *text = cut(text, room);
        }
    }
    finish(runs, lines, count, words.len(), align)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(widths: &[u16]) -> Room {
        Room::from_runs(
            widths
                .iter()
                .enumerate()
                .map(|(row, &width)| Run {
                    row: row as u16,
                    col: 0,
                    width,
                })
                .collect(),
        )
    }

    fn texts(f: &Fitted) -> Vec<&str> {
        f.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn a_column_that_fits_is_centred_both_ways() {
        let f = labels(
            &room(&[10; 5]),
            &["kiwi", "lime"],
            Flow::Column,
            Align::Center,
        );
        assert_eq!(texts(&f), ["kiwi", "lime"]);
        assert_eq!((f.shown, f.hidden), (2, 0));
        // Rows 0..5, two lines: rows 1 and 2 (one above, two below) or 2 and 3; either is one off.
        assert!(matches!(f.lines[0].row, 1 | 2), "{f:?}");
        assert_eq!(f.lines[0].col, 3);
    }

    #[test]
    fn overflow_makes_room_for_more() {
        let f = labels(
            &room(&[10; 3]),
            &["a", "b", "c", "d", "e"],
            Flow::Column,
            Align::Start,
        );
        assert_eq!(texts(&f), ["a", "b", "+3 more"]);
        assert_eq!((f.shown, f.hidden), (2, 3));
    }

    #[test]
    fn short_rows_take_the_count_alone() {
        let f = labels(
            &room(&[5, 3]),
            &["apple", "banana", "cherry"],
            Flow::Column,
            Align::Start,
        );
        assert_eq!(texts(&f), ["apple", "+2"]);
    }

    #[test]
    fn wrap_packs_rows() {
        let f = labels(
            &room(&[12, 12]),
            &["ab", "cd", "ef", "gh", "ij"],
            Flow::Wrap,
            Align::Start,
        );
        assert_eq!(texts(&f), ["ab  cd  ef", "gh  ij"]);
    }

    #[test]
    fn long_labels_are_cut_and_narrow_rows_skipped() {
        let f = labels(&room(&[2, 6]), &["elderberry"], Flow::Column, Align::Start);
        assert_eq!(texts(&f), ["elder…"]);
        assert_eq!(f.lines[0].row, 1);
    }

    #[test]
    fn nothing_fits_in_no_room() {
        let f = labels(&room(&[]), &["a"], Flow::Column, Align::Center);
        assert_eq!((f.lines.len(), f.shown, f.hidden), (0, 0, 1));
    }

    #[test]
    fn text_wraps_and_marks_what_is_left_out() {
        let f = text(
            &room(&[11, 11]),
            "the quick brown fox jumps over",
            Align::Start,
        );
        assert_eq!(texts(&f), ["the quick", "brown fox…"]);
        assert_eq!((f.shown, f.hidden), (4, 2));
        let whole = text(&room(&[20, 20, 20]), "short text", Align::Center);
        assert_eq!(texts(&whole), ["short text"]);
        assert_eq!(whole.lines[0].row, 1, "centred vertically");
    }

    #[test]
    fn room_keeps_the_widest_run_per_row() {
        use crate::area::Area;
        use crate::geom::{Placement, Point, Rect};
        use crate::part::{Kind, Part};
        use crate::raster::{Res, Viewport};
        use crate::shape::{Placed, Shape};
        use std::sync::Arc;
        let left = Rect::new(Point::new(0.0, 0.0), Point::new(0.3, 1.0));
        let right = Rect::new(Point::new(0.4, 0.0), Point::new(1.0, 1.0));
        let shape = Shape::new(
            vec![
                Part::new(Some("l"), Kind::Rect(left)),
                Part::new(Some("r"), Kind::Rect(right)),
            ],
            &[],
        )
        .unwrap();
        let view = Viewport::terminal(22, 6);
        let placed = Placed::new(
            Arc::new(shape),
            Placement::new(Point::new(1.0, 2.0), Point::new(20.0, 8.0)),
        );
        let area = Area::parse("l | r", placed.shape.parts()).unwrap();
        let room = Room::inside(&Mask::area(&placed, &area, &view, Res::BRAILLE));
        // The right rect covers cells 9..=20 and rows 1..=4; its outline dots take the first and
        // last of each, and the left rect's run (cols 2..=5) is narrower.
        assert_eq!(
            room.runs(),
            [
                Run {
                    row: 2,
                    col: 10,
                    width: 10
                },
                Run {
                    row: 3,
                    col: 10,
                    width: 10
                }
            ]
        );
    }
}
