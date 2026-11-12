//! Symbols put straight into the screen's cells. What the app draws through ratatui's text
//! (`Span`, `Line`, `Paragraph`, `Buffer::set_string`) loses its control characters there, but
//! a symbol set on a cell is written to the terminal as it is, so a label drawn a character
//! at a time (a graph's, an arrow's, a shape's) carried an escape sequence to the terminal
//! whole. Every symbol set on a cell goes through `put`, and nothing else in the app sets
//! one (`tests/one_door.rs` reads the source for it).
//!
//! A label is escaped before it is laid out (`outside::escape`), so what a terminal would
//! act on shows as `\u{1b}` and takes the room it needs. `put` is what holds when one wasn't:
//! a cell has room for one symbol, so a character that is never shown as written becomes
//! `STAND_IN` there.

use crate::outside::{Level, escape};
use ratatui::buffer::Cell;
use std::borrow::Cow;

/// What a cell shows for a symbol that is never shown as written.
pub const STAND_IN: &str = "\u{fffd}";

/// Set the cell's symbol: `symbol` as it is, or the stand-in when it holds what a terminal
/// would act on.
pub fn put<'a>(cell: &'a mut Cell, symbol: &str) -> &'a mut Cell {
    match escape(symbol, Level::Content) {
        Cow::Borrowed(symbol) => cell.set_symbol(symbol),
        Cow::Owned(_) => cell.set_symbol(STAND_IN),
    }
}

/// Set the cell's symbol to one character, as `put` does.
pub fn put_char(cell: &mut Cell, ch: char) -> &mut Cell {
    put(cell, ch.encode_utf8(&mut [0; 4]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Widget};

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
            '\u{1b}', '\u{7}', '\0', '\u{1f}', '\u{7f}', '\u{9b}', '\n', '\u{2028}',
        ] {
            assert_eq!(put_char(&mut cell, acts).symbol(), STAND_IN, "{acts:?}");
        }
        assert_eq!(put(&mut cell, "e\u{1b}").symbol(), STAND_IN);
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
    }
}
