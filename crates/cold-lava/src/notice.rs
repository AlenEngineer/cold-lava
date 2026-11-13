//! A notice: words the terminal app says on a status line, about what it did, what it has
//! or why it didn't. They may quote a file or an extension (a note's name, an extension's
//! error text, the words an extension asks to show), so a notice is kept as it is written
//! and escaped as a message where it is shown: its only way out is `Display`, which every
//! status line already draws with. Whoever keeps a notice keeps a `Notice`, so no store has
//! to remember the escape and none can show the words another way.
//!
//! The view an extension sends (a title, a row) is not a notice: it is content, drawn
//! through ratatui's text (`cells.rs` says what that keeps).

use crate::problem::Problem;
use cold_lava_views::shown::{Level, escape};
use std::fmt;

/// The words of a notice, as they were written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice(String);

impl Notice {
    /// This notice with `more` of the app's own words after it.
    pub fn and(&self, more: &str) -> Notice {
        Notice(format!("{}{more}", self.0))
    }

    /// This notice, then `next`, with `between` them.
    pub fn then(&self, between: &'static str, next: &Notice) -> Notice {
        Notice(format!("{}{between}{}", self.0, next.0))
    }
}

impl From<String> for Notice {
    fn from(words: String) -> Notice {
        Notice(words)
    }
}

impl From<&str> for Notice {
    fn from(words: &str) -> Notice {
        Notice(words.to_string())
    }
}

/// A problem on a status line, which holds one short line: what is wrong and where, without
/// the lines that show where in a file (`Problem::headline`).
impl From<&Problem> for Notice {
    fn from(problem: &Problem) -> Notice {
        Notice(problem.headline())
    }
}

impl From<&String> for Notice {
    fn from(words: &String) -> Notice {
        Notice(words.clone())
    }
}

impl fmt::Display for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&escape(&self.0, Level::Message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_shown_escaped_as_a_message_and_kept_as_written() {
        let words = "tally: no\u{1b}]0;t\u{7}\u{202e}pe\nnext";
        let notice = Notice::from(words);
        assert_eq!(
            format!(" {notice}"),
            " tally: no\\u{1b}]0;t\\u{7}\\u{202e}pe\\nnext"
        );
        assert_eq!(notice, Notice::from(words.to_string()), "as written");
        assert_eq!(Notice::from("saved").to_string(), "saved");
    }

    #[test]
    fn notices_are_put_together_as_written_and_escaped_once() {
        let first = Notice::from("a wanted \\u{1b} as typed");
        let said = first.and(" (2 requests refused)");
        let line = said.then(" · ", &Notice::from("saved\u{7}"));
        assert_eq!(
            line.to_string(),
            "a wanted \\u{1b} as typed (2 requests refused) · saved\\u{7}"
        );
    }
}
