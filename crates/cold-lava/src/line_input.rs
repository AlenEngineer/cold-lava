//! A line of text typed into one of the menu's prompts or pickers: what a key does to it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key did to the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// A character went on its end.
    Typed,
    /// Backspace took its last character, if it had one.
    Erased,
    /// Ctrl-U emptied it.
    Cleared,
}

/// Whether Ctrl-U empties the line, or is taken and does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CtrlU {
    Clears,
    Ignored,
}

/// Apply `k` to `line`: a character typed, Backspace, or Ctrl-U as `ctrl_u` says. `None` when
/// the line is left as it was: a character typed with Ctrl held is no character, and any
/// other key is the caller's.
pub fn key(line: &mut String, k: KeyEvent, ctrl_u: CtrlU) -> Option<Edit> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Backspace => {
            line.pop();
            Some(Edit::Erased)
        }
        KeyCode::Char('u') if ctrl && ctrl_u == CtrlU::Clears => {
            line.clear();
            Some(Edit::Cleared)
        }
        KeyCode::Char(c) if !ctrl => {
            line.push(c);
            Some(Edit::Typed)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(
        line: &str,
        code: KeyCode,
        modifiers: KeyModifiers,
        ctrl_u: CtrlU,
    ) -> (String, Option<Edit>) {
        let mut line = line.to_string();
        let edit = key(&mut line, KeyEvent::new(code, modifiers), ctrl_u);
        (line, edit)
    }

    #[test]
    fn a_character_goes_on_the_end_and_backspace_takes_it_off() {
        let none = KeyModifiers::NONE;
        let typed = press("Inbo", KeyCode::Char('x'), none, CtrlU::Clears);
        assert_eq!(typed, ("Inbox".to_string(), Some(Edit::Typed)));
        let shifted = press("a", KeyCode::Char('B'), KeyModifiers::SHIFT, CtrlU::Clears);
        assert_eq!(shifted, ("aB".to_string(), Some(Edit::Typed)));
        let erased = press("Inbox", KeyCode::Backspace, none, CtrlU::Clears);
        assert_eq!(erased, ("Inbo".to_string(), Some(Edit::Erased)));
        let empty = press("", KeyCode::Backspace, none, CtrlU::Ignored);
        assert_eq!(
            empty,
            (String::new(), Some(Edit::Erased)),
            "said even with nothing to take"
        );
    }

    #[test]
    fn ctrl_u_clears_only_where_it_is_asked_to() {
        let ctrl = KeyModifiers::CONTROL;
        let cleared = press("Inbox", KeyCode::Char('u'), ctrl, CtrlU::Clears);
        assert_eq!(cleared, (String::new(), Some(Edit::Cleared)));
        let ignored = press("Inbox", KeyCode::Char('u'), ctrl, CtrlU::Ignored);
        assert_eq!(ignored, ("Inbox".to_string(), None));
    }

    #[test]
    fn a_chord_and_any_other_key_leave_the_line_alone() {
        let ctrl = KeyModifiers::CONTROL;
        assert_eq!(
            press("a", KeyCode::Char('w'), ctrl, CtrlU::Clears),
            ("a".to_string(), None)
        );
        let none = KeyModifiers::NONE;
        for code in [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Tab,
            KeyCode::Down,
            KeyCode::Delete,
        ] {
            assert_eq!(
                press("a", code, none, CtrlU::Clears),
                ("a".to_string(), None),
                "{code:?}"
            );
        }
    }
}
