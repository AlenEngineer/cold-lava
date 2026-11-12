//! Text from outside, on its way into a message. An owner's place, a label, a value, an
//! extension's words: someone else chose every byte and how many of them. A message that
//! quotes such text puts it in as an `Outside`, which marks it as not the engine's own and
//! cuts it to a length, so no message repeats without end what an owner sent.
//!
//! The text itself is not changed. Making it safe to show is the frontend's, where it is
//! shown (`cold_lava_views::shown::escape`); a message, like everything else the engine
//! keeps or answers, holds the text as it is written.

use std::fmt;

/// The characters of a place that a message repeats, since an owner may give one of any
/// length.
const PLACE: usize = 80;
/// The characters of a value or a reason that a message repeats.
const VALUE: usize = 200;

/// Outside text in a message. The only thing it can do is be written, and written it is
/// its first characters with `…` when it was cut, as it is written otherwise.
#[derive(Clone, Copy)]
pub struct Outside<'a> {
    text: &'a str,
    most: Option<usize>,
}

impl<'a> Outside<'a> {
    /// The whole text.
    pub fn whole(text: &'a str) -> Outside<'a> {
        Outside { text, most: None }
    }

    /// A place: its first characters.
    pub fn place(text: &'a str) -> Outside<'a> {
        Outside::cut(text, PLACE)
    }

    /// A value or a reason: its first characters, more of them than of a place.
    pub fn value(text: &'a str) -> Outside<'a> {
        Outside::cut(text, VALUE)
    }

    fn cut(text: &'a str, most: usize) -> Outside<'a> {
        let most = Some(most);
        Outside { text, most }
    }
}

impl fmt::Display for Outside<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let end = self
            .most
            .and_then(|most| self.text.char_indices().nth(most));
        match end {
            Some((end, _)) => write!(f, "{}…", &self.text[..end]),
            None => f.write_str(self.text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_text_is_written_as_it_is_and_cut() {
        let place = "\u{1b}]0;x\u{7}\u{202e}p\nq";
        assert_eq!(Outside::whole(place).to_string(), place);
        assert_eq!(Outside::place(place).to_string(), place);
        assert_eq!(Outside::place("short").to_string(), "short");
        let long = "é".repeat(81);
        assert_eq!(
            Outside::place(&long).to_string(),
            format!("{}…", "é".repeat(80))
        );
        assert_eq!(Outside::place(&long[..160]).to_string(), "é".repeat(80));
        let long = "v".repeat(300);
        assert_eq!(
            Outside::value(&long).to_string(),
            format!("{}…", "v".repeat(200))
        );
        assert_eq!(Outside::whole(&long).to_string(), long);
    }
}
