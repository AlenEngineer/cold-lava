//! A list picked from by typing: what a key does to its cursor and its query, and how its rows
//! are drawn.

use crate::app::{ACCENT, HIT, MUTED, NOTE};
use crate::line_input::{self, CtrlU};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use std::borrow::Cow;

/// What a key did to the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picked {
    /// The cursor moved, or stayed at the end it was at.
    Moved,
    /// The query changed; the caller says which row the cursor starts on.
    Typed,
}

/// Apply `k` to a list of `rows` rows with its cursor at `sel` and `query` typed: Down, Tab,
/// Ctrl-N and Ctrl-J go down a row and Up, Shift-Tab, Ctrl-P and Ctrl-K up one, stopping at
/// the ends; other keys edit the query (`line_input`, with Ctrl-U as `ctrl_u` says). `None`
/// when neither changed: any other key is the caller's.
pub fn key(
    query: &mut String,
    sel: &mut usize,
    rows: usize,
    k: KeyEvent,
    ctrl_u: CtrlU,
) -> Option<Picked> {
    match moved(*sel, rows, k) {
        Some(to) => {
            *sel = to;
            Some(Picked::Moved)
        }
        None => line_input::key(query, k, ctrl_u).map(|_| Picked::Typed),
    }
}

/// Where `k` moves a cursor at `sel` in a list of `rows` rows, or `None` when it is no
/// movement key.
fn moved(sel: usize, rows: usize, k: KeyEvent) -> Option<usize> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let down = (sel + 1).min(rows.saturating_sub(1));
    let up = sel.saturating_sub(1);
    match k.code {
        KeyCode::Down | KeyCode::Tab => Some(down),
        KeyCode::Char('n') | KeyCode::Char('j') if ctrl => Some(down),
        KeyCode::Up | KeyCode::BackTab => Some(up),
        KeyCode::Char('p') | KeyCode::Char('k') if ctrl => Some(up),
        _ => None,
    }
}

/// The chosen row: black on the accent.
pub const CHOSEN: Style = Style::new().fg(Color::Black).bg(ACCENT);

/// A character a match hit, on the chosen row.
pub const CHOSEN_HIT: Style = CHOSEN.add_modifier(Modifier::BOLD.union(Modifier::UNDERLINED));

/// A character a match hit, on the other rows.
const HIT_BOLD: Style = Style::new().fg(HIT).add_modifier(Modifier::BOLD);

/// A picked list as it is drawn: how many rows it has, which one is chosen, what it says when
/// it has none, and how it counts a row's text.
pub struct List {
    pub len: usize,
    pub sel: usize,
    pub none: &'static str,
    pub measure: Measure,
}

/// How a row's text is counted to fill the gap before its end. The two counts differ for wide
/// and zero-width characters (#365).
#[derive(Clone, Copy)]
pub enum Measure {
    /// In characters.
    Chars,
    /// In the columns they take on screen.
    Columns,
}

/// A row of a picked list as its caller writes it: its text, left to right, and what follows.
pub struct Row<'a> {
    pub text: Vec<Piece<'a>>,
    pub end: End<'a>,
}

/// What follows a row's text.
pub enum End<'a> {
    /// On the chosen row, its colour on to the right edge.
    Bar,
    /// This piece against the right edge, after a gap in the row's colour.
    Right(Piece<'a>),
    /// Nothing: the chosen row's colour stops with its text (#365).
    Nothing,
}

/// A piece of a row's text, and how it looks on a row that isn't chosen.
pub struct Piece<'a> {
    text: Cow<'a, str>,
    style: Style,
    /// The characters a match hit, by their place in `text`, when a match reads it.
    hits: Option<&'a [usize]>,
}

impl<'a> Piece<'a> {
    /// Text no match reads, drawn whole.
    pub fn plain(text: impl Into<Cow<'a, str>>, style: Style) -> Piece<'a> {
        let (text, hits) = (text.into(), None);
        Piece { text, style, hits }
    }

    /// Text a match reads, drawn a character at a time, the characters at `hits` bold. A
    /// character that takes no column alone, such as a combining mark, is not drawn (#365).
    pub fn matched(text: impl Into<Cow<'a, str>>, hits: &'a [usize], style: Style) -> Piece<'a> {
        let (text, hits) = (text.into(), Some(hits));
        Piece { text, style, hits }
    }

    fn spans(self, chosen: bool) -> Vec<Span<'a>> {
        let style = if chosen { CHOSEN } else { self.style };
        let Some(hits) = self.hits else {
            return vec![Span::styled(self.text, style)];
        };
        let hit = if chosen { CHOSEN_HIT } else { HIT_BOLD };
        let style_of = |i: usize| if hits.contains(&i) { hit } else { style };
        (self.text.chars().enumerate())
            .map(|(i, c)| Span::styled(c.to_string(), style_of(i)))
            .collect()
    }
}

/// Draw `list` in `area`, `row(i)` giving its row `i`: from the first row that keeps the chosen
/// one on screen, the chosen row in `CHOSEN` and the others as their pieces say, the characters
/// a match hit in bold. With no rows, it says `none`, muted.
pub fn draw<'a>(buf: &mut Buffer, area: Rect, list: List, row: impl Fn(usize) -> Row<'a>) {
    let (width, height) = (area.width as usize, area.height as usize);
    let top = first(list.sel, height);
    let mut lines: Vec<Line> = (top..list.len.min(top + height))
        .map(|i| line(row(i), i == list.sel, width, list.measure))
        .collect();
    if list.len == 0 {
        lines.push(Line::styled(list.none, Style::new().fg(MUTED)));
    }
    Paragraph::new(lines).render(area, buf);
}

/// The first row to show in `height` rows, so that the chosen row `sel` is among them.
pub fn first(sel: usize, height: usize) -> usize {
    sel.saturating_sub(height.saturating_sub(1))
}

/// `row` as a line `width` columns wide, chosen or not.
fn line(row: Row<'_>, chosen: bool, width: usize, measure: Measure) -> Line<'_> {
    let mut spans: Vec<Span> = (row.text.into_iter())
        .flat_map(|p| p.spans(chosen))
        .collect();
    let gap =
        |used: usize, style: Style| Span::styled(" ".repeat(width.saturating_sub(used)), style);
    match row.end {
        End::Bar if chosen => spans.push(gap(measure.of(&spans), CHOSEN)),
        End::Right(right) => {
            let right = right.spans(chosen);
            let used = measure.of(&spans) + measure.of(&right);
            let style = if chosen {
                CHOSEN
            } else {
                Style::new().fg(NOTE)
            };
            spans.push(gap(used, style));
            spans.extend(right);
        }
        End::Bar | End::Nothing => {}
    }
    Line::from(spans)
}

impl Measure {
    fn of(self, spans: &[Span]) -> usize {
        match self {
            Measure::Chars => spans.iter().map(|s| s.content.chars().count()).sum(),
            Measure::Columns => spans.iter().map(Span::width).sum(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The query, the cursor and what the key did, after `k` on a list of `rows` rows.
    fn press(
        sel: usize,
        rows: usize,
        k: KeyEvent,
        ctrl_u: CtrlU,
    ) -> (String, usize, Option<Picked>) {
        let (mut query, mut sel) = ("no".to_string(), sel);
        let picked = key(&mut query, &mut sel, rows, k, ctrl_u);
        (query, sel, picked)
    }

    fn plain(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn the_cursor_moves_a_row_and_stops_at_the_ends() {
        let moved = |sel: usize| (String::from("no"), sel, Some(Picked::Moved));
        for k in [
            plain(KeyCode::Down),
            plain(KeyCode::Tab),
            ctrl('n'),
            ctrl('j'),
        ] {
            assert_eq!(press(1, 3, k, CtrlU::Clears), moved(2), "{k:?}");
            assert_eq!(
                press(2, 3, k, CtrlU::Clears),
                moved(2),
                "{k:?} at the last row"
            );
            assert_eq!(press(0, 0, k, CtrlU::Clears), moved(0), "{k:?} on no rows");
        }
        for k in [
            plain(KeyCode::Up),
            plain(KeyCode::BackTab),
            ctrl('p'),
            ctrl('k'),
        ] {
            assert_eq!(press(1, 3, k, CtrlU::Clears), moved(0), "{k:?}");
            assert_eq!(
                press(0, 3, k, CtrlU::Clears),
                moved(0),
                "{k:?} at the first row"
            );
        }
    }

    #[test]
    fn typing_edits_the_query_and_leaves_the_cursor_to_the_caller() {
        let typed = press(2, 3, plain(KeyCode::Char('t')), CtrlU::Ignored);
        assert_eq!(typed, ("not".to_string(), 2, Some(Picked::Typed)));
        let erased = press(2, 3, plain(KeyCode::Backspace), CtrlU::Ignored);
        assert_eq!(erased, ("n".to_string(), 2, Some(Picked::Typed)));
        let cleared = press(2, 3, ctrl('u'), CtrlU::Clears);
        assert_eq!(cleared, (String::new(), 2, Some(Picked::Typed)));
        let ignored = press(2, 3, ctrl('u'), CtrlU::Ignored);
        assert_eq!(ignored, ("no".to_string(), 2, None));
    }

    #[test]
    fn other_keys_are_the_callers() {
        for k in [
            plain(KeyCode::Enter),
            plain(KeyCode::Esc),
            plain(KeyCode::Left),
            ctrl('w'),
        ] {
            assert_eq!(
                press(1, 3, k, CtrlU::Clears),
                ("no".to_string(), 1, None),
                "{k:?}"
            );
        }
    }

    /// `list` drawn in an area `w` columns wide and `h` rows high.
    fn drawn<'a>(w: u16, h: u16, list: List, row: impl Fn(usize) -> Row<'a>) -> Buffer {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, list, row);
        buf
    }

    fn text(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn list(len: usize, sel: usize, measure: Measure) -> List {
        let none = " nothing matches";
        List {
            len,
            sel,
            none,
            measure,
        }
    }

    fn row(text: &str, end: End<'static>) -> Row<'static> {
        let text = vec![Piece::plain(text.to_string(), Style::new().fg(NOTE))];
        Row { text, end }
    }

    #[test]
    fn the_rows_shown_keep_the_chosen_one_on_screen() {
        assert_eq!(first(0, 3), 0);
        assert_eq!(first(2, 3), 0, "the last row that fits");
        assert_eq!(first(3, 3), 1);
        assert_eq!(first(9, 3), 7, "the chosen row is the last one shown");
        assert_eq!(first(4, 0), 4, "no room");
        let numbered = |i: usize| row(&format!("row {i}"), End::Nothing);
        let buf = drawn(6, 3, list(6, 4, Measure::Chars), numbered);
        assert_eq!(
            [0, 1, 2].map(|y| text(&buf, y)),
            ["row 2 ", "row 3 ", "row 4 "]
        );
        let buf = drawn(6, 3, list(2, 1, Measure::Chars), numbered);
        assert_eq!(
            [0, 1, 2].map(|y| text(&buf, y)),
            ["row 0 ", "row 1 ", "      "]
        );
    }

    #[test]
    fn the_chosen_row_is_black_on_the_accent_and_the_characters_hit_are_bold() {
        let muted = Style::new().fg(MUTED);
        let matched = |_: usize| Row {
            text: vec![Piece::matched("lava", &[0, 2], muted)],
            end: End::Nothing,
        };
        let buf = drawn(4, 2, list(2, 1, Measure::Chars), matched);
        let look = |x: u16, y: u16| (buf[(x, y)].fg, buf[(x, y)].bg, buf[(x, y)].modifier);
        assert_eq!(look(0, 0), (HIT, Color::Reset, Modifier::BOLD), "a hit");
        assert_eq!(
            look(1, 0),
            (MUTED, Color::Reset, Modifier::empty()),
            "its own"
        );
        let underlined = Modifier::BOLD | Modifier::UNDERLINED;
        assert_eq!(
            look(2, 1),
            (Color::Black, ACCENT, underlined),
            "a hit, chosen"
        );
        assert_eq!(
            look(3, 1),
            (Color::Black, ACCENT, Modifier::empty()),
            "chosen"
        );
        assert_eq!([0, 1].map(|y| text(&buf, y)), ["lava", "lava"]);
    }

    #[test]
    fn what_follows_the_text_runs_to_the_edge_sits_against_it_or_is_nothing() {
        let bar = drawn(5, 2, list(2, 0, Measure::Columns), |_| row("ab", End::Bar));
        assert_eq!(bar[(4, 0)].bg, ACCENT, "the chosen row to the edge");
        assert_eq!(
            bar[(2, 1)].fg,
            Color::Reset,
            "the other row stops with its text"
        );

        let keys = || End::Right(Piece::plain("k", Style::new().fg(ACCENT)));
        let right = drawn(5, 2, list(2, 0, Measure::Chars), |_| row("ab", keys()));
        assert_eq!([0, 1].map(|y| text(&right, y)), ["ab  k", "ab  k"]);
        assert_eq!(
            (right[(2, 0)].bg, right[(4, 0)].bg),
            (ACCENT, ACCENT),
            "chosen"
        );
        assert_eq!(
            (right[(2, 1)].fg, right[(4, 1)].fg),
            (NOTE, ACCENT),
            "the gap, the piece"
        );

        let nothing = drawn(5, 1, list(1, 0, Measure::Chars), |_| {
            row("ab", End::Nothing)
        });
        assert_eq!(
            (nothing[(1, 0)].bg, nothing[(2, 0)].bg),
            (ACCENT, Color::Reset)
        );
    }

    #[test]
    fn characters_and_columns_count_a_wide_character_apart() {
        // One character, two columns.
        let wide = |_: usize| row("火", End::Right(Piece::plain("k", Style::new().fg(ACCENT))));
        let columns = drawn(5, 1, list(1, 0, Measure::Columns), wide);
        assert_eq!(columns[(4, 0)].symbol(), "k");
        let chars = drawn(5, 1, list(1, 0, Measure::Chars), wide);
        assert_eq!(chars[(4, 0)].symbol(), " ", "pushed past the edge");
    }

    #[test]
    fn matched_text_is_drawn_a_character_at_a_time() {
        let accented = "e\u{301}";
        let whole = drawn(3, 1, list(1, 0, Measure::Chars), |_| {
            row(accented, End::Nothing)
        });
        assert_eq!(whole[(0, 0)].symbol(), accented);
        let matched = |_: usize| Row {
            text: vec![Piece::matched(accented, &[], Style::new().fg(NOTE))],
            end: End::Nothing,
        };
        let alone = drawn(3, 1, list(1, 0, Measure::Chars), matched);
        assert_eq!(
            alone[(0, 0)].symbol(),
            "e",
            "the mark takes no column of its own"
        );
    }

    #[test]
    fn with_no_rows_it_says_so_muted() {
        let buf = drawn(18, 2, list(0, 0, Measure::Chars), |_| {
            row("never", End::Bar)
        });
        assert_eq!(text(&buf, 0), " nothing matches  ");
        assert_eq!(buf[(1, 0)].fg, MUTED);
        assert_eq!(text(&buf, 1), " ".repeat(18));
    }
}
