//! A list picked from by typing (the quick switcher, the outline, the bookmarks, the palette
//! and the search): what a key does to its cursor and its query.

use crate::line_input::{self, CtrlU};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Down | KeyCode::Tab => *sel = (*sel + 1).min(rows.saturating_sub(1)),
        KeyCode::Char('n') | KeyCode::Char('j') if ctrl => {
            *sel = (*sel + 1).min(rows.saturating_sub(1))
        }
        KeyCode::Up | KeyCode::BackTab => *sel = sel.saturating_sub(1),
        KeyCode::Char('p') | KeyCode::Char('k') if ctrl => *sel = sel.saturating_sub(1),
        _ => return line_input::key(query, k, ctrl_u).map(|_| Picked::Typed),
    }
    Some(Picked::Moved)
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
}
