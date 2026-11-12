//! Symbols put straight into the screen's cells. What the app draws through ratatui's text
//! (`Span`, `Line`, `Paragraph`, `Buffer::set_string`) loses its control characters there, but
//! a symbol set on a cell is written to the terminal as it is, so a label drawn a character
//! at a time (a graph's, an arrow's, a shape's) carried an escape sequence to the terminal
//! whole. Every symbol set on a cell goes through `put`, and nothing else in the app sets
//! one (`tests/one_door.rs` reads the source for it).
//!
//! A label is made ready before it is laid out (`label`), so what a terminal would act on
//! shows as `\u{1b}` and takes the room it needs. `put` is what holds when one wasn't: a
//! cell has room for one symbol, so a character that is never shown as written becomes
//! `STAND_IN` there.
//!
//! What this does not reach: text drawn through ratatui keeps the separators U+2028 and
//! U+2029 and whatever else ratatui lets through (the tests below pin what that is), since
//! ratatui drops only what `char::is_control` names and what has no width.

use cold_lava_views::shown::{Level, escape};
use ratatui::buffer::Cell;
use std::borrow::Cow;

/// What a cell shows for a symbol that is never shown as written.
pub const STAND_IN: &str = "\u{fffd}";

/// Set the cell's symbol: `symbol` as it is, or the stand-in when it holds what a terminal
/// would act on. A tab is one of those here, though a printed line keeps it: in a cell it
/// would move the cursor to a tab stop and draw the rest of the row off its cells.
pub fn put<'a>(cell: &'a mut Cell, symbol: &str) -> &'a mut Cell {
    match escape(symbol, Level::Content) {
        Cow::Borrowed(symbol) if !symbol.contains('\t') => cell.set_symbol(symbol),
        _ => cell.set_symbol(STAND_IN),
    }
}

/// Set the cell's symbol to one character, as `put` does.
pub fn put_char(cell: &mut Cell, ch: char) -> &mut Cell {
    put(cell, ch.encode_utf8(&mut [0; 4]))
}

/// A label for drawing a character a cell: a node's in the graph, an arrow's, what a region
/// of a placed shape shows. It is someone else's words (a file's name, a card's text, an
/// extension's), so what a terminal would act on is escaped as in a listing, and a tab is a
/// space, since a cell holds no tab. Call it before the label is measured, cut or fitted,
/// so the escape takes the room it needs. The layout then measures the label as shown, not
/// as written: where room runs out ("+N more") can differ from what another frontend
/// computes from the label as written. With one renderer for every frontend (#227) this
/// becomes a step of its input.
pub fn label(text: &str) -> Cow<'_, str> {
    match escape(text, Level::Content) {
        Cow::Borrowed(text) if !text.contains('\t') => Cow::Borrowed(text),
        shown => Cow::Owned(shown.replace('\t', " ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Paragraph, Widget, Wrap};

    /// Text with an escape sequence, DEL and one of C1 in it.
    const ACTS: &str = "a\u{1b}]0;title\u{7}b\u{7f}c\u{9b}2Jd";

    fn controls(buf: &Buffer) -> Vec<String> {
        let symbols = buf.content().iter().map(|cell| cell.symbol().to_string());
        symbols
            .filter(|s| s.chars().any(char::is_control))
            .collect()
    }

    #[test]
    fn a_symbol_a_terminal_would_act_on_is_the_stand_in() {
        let mut cell = Cell::default();
        for acts in [
            '\u{1b}', '\u{7}', '\0', '\u{1f}', '\u{7f}', '\u{9b}', '\n', '\u{2028}', '\t',
        ] {
            assert_eq!(put_char(&mut cell, acts).symbol(), STAND_IN, "{acts:?}");
        }
        assert_eq!(put(&mut cell, "e\u{1b}").symbol(), STAND_IN);
        assert_eq!(put(&mut cell, "a\tb").symbol(), STAND_IN);
    }

    #[test]
    fn a_label_is_escaped_as_a_listing_is_and_its_tabs_are_spaces() {
        assert_eq!(label("Esc\u{1b}]0;t\u{7}Name"), "Esc\\u{1b}]0;t\\u{7}Name");
        assert_eq!(label("tab\there\u{1b}"), "tab here\\u{1b}");
        assert_eq!(label("tab\there"), "tab here");
        assert_eq!(label("a\nb \u{202e}c"), "a\\nb \u{202e}c");
        let plain = "Parícutin 日本語 👍";
        assert!(matches!(label(plain), Cow::Borrowed(same) if same == plain));
    }

    #[test]
    fn any_other_symbol_is_set_as_it_is() {
        let mut cell = Cell::default();
        for ch in ['a', 'é', '█', '⠿', '日', '\u{202e}'] {
            let mut bytes = [0; 4];
            let symbol: &str = ch.encode_utf8(&mut bytes);
            assert_eq!(put_char(&mut cell, ch).symbol(), symbol);
        }
        for symbol in [" ", "e\u{301}", "👨\u{200d}👩\u{200d}👧"] {
            assert_eq!(put(&mut cell, symbol).symbol(), symbol);
        }
    }

    /// What the rest of the app relies on: text drawn through ratatui reaches no cell with
    /// a control character in it. It is ratatui's doing, so an upgrade that changed it would
    /// fail here.
    #[test]
    fn ratatui_drops_control_characters_from_the_text_it_draws() {
        let area = Rect::new(0, 0, 40, 2);
        let mut buf = Buffer::empty(area);
        buf.set_string(0, 0, ACTS, Style::new());
        assert_eq!(controls(&buf), Vec::<String>::new(), "set_string");

        let mut buf = Buffer::empty(area);
        Line::from(ACTS).render(area, &mut buf);
        assert_eq!(controls(&buf), Vec::<String>::new(), "a line");

        let mut buf = Buffer::empty(area);
        Paragraph::new(ACTS).render(area, &mut buf);
        assert_eq!(controls(&buf), Vec::<String>::new(), "a paragraph");
        let shown: String = (buf.content().iter().map(|cell| cell.symbol())).collect();
        assert!(shown.starts_with("a]0;titlebc2Jd"), "{shown:?}");

        // Wrapped, the text is on two rows; a block's title is a line of its own kind.
        let narrow = Rect::new(0, 0, 8, 4);
        let mut buf = Buffer::empty(narrow);
        let wrapped = Paragraph::new(ACTS).wrap(Wrap { trim: false });
        wrapped.render(narrow, &mut buf);
        assert_eq!(controls(&buf), Vec::<String>::new(), "a wrapped paragraph");

        let mut buf = Buffer::empty(area);
        Block::bordered().title(ACTS).render(area, &mut buf);
        assert_eq!(controls(&buf), Vec::<String>::new(), "a block's title");
    }

    /// What ratatui does with the characters it doesn't call control characters, drawn
    /// each of the ways above: which of them reach a cell. A character that turns the
    /// direction of text has no width and reaches none. The two separators that end a line
    /// are kept, so text drawn through ratatui can hold them: that is the limit of what the
    /// terminal app escapes, stated in this module's doc.
    #[test]
    fn what_ratatui_keeps_of_the_characters_that_are_not_control_characters() {
        let area = Rect::new(0, 0, 20, 3);
        let kept = |draw: &dyn Fn(&str, &mut Buffer)| -> Vec<&str> {
            let named = [
                ("U+202E", '\u{202e}'),
                ("U+2066", '\u{2066}'),
                ("U+2028", '\u{2028}'),
                ("U+2029", '\u{2029}'),
            ];
            let reaches = |c: char| {
                let mut buf = Buffer::empty(area);
                draw(&format!("a{c}b"), &mut buf);
                buf.content().iter().any(|cell| cell.symbol().contains(c))
            };
            let kept = named.into_iter().filter(|&(_, c)| reaches(c));
            kept.map(|(name, _)| name).collect()
        };
        let found = [
            kept(&|text, buf| {
                buf.set_string(0, 0, text, Style::new());
            }),
            kept(&|text, buf| Line::from(text.to_string()).render(area, buf)),
            kept(&|text, buf| Paragraph::new(text.to_string()).render(area, buf)),
            kept(&|text, buf| {
                let wrapped = Paragraph::new(text.to_string()).wrap(Wrap { trim: false });
                wrapped.render(area, buf)
            }),
            kept(&|text, buf| Block::bordered().title(text.to_string()).render(area, buf)),
        ];
        let separators = vec!["U+2028", "U+2029"];
        assert_eq!(
            found,
            [
                separators.clone(),
                separators.clone(),
                separators.clone(),
                separators.clone(),
                separators
            ],
            "set_string, a line, a paragraph, a wrapped paragraph, a block's title"
        );
    }
}
