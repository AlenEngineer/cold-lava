//! Text from outside, for showing. A note's words, a card's, a file's name, an owner's place,
//! an extension's error text: someone else chose every byte, and a terminal acts on some of
//! them (an escape sequence can retitle it, clear it, or hide what follows). `escape` is the
//! one function that makes such text safe to show, and a frontend calls it where the text
//! leaves for the screen: the command line on every line it prints, the terminal app on
//! the labels and notices it draws.
//!
//! It is for showing only, and only a frontend shows. The engine and the host never call
//! it: what they keep, compose into a message, answer over the gateway, hash or compare is
//! the text as it is written.

use std::borrow::Cow;

/// What a line is, which decides how much of it is shown as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The user's own content, listed: a task's text, a note's line, a label. Only what a
    /// terminal acts on is escaped, so text in a script written right to left stays as its
    /// author wrote it.
    Content,
    /// Words about something: an error, a problem, a notice, what a command did. The
    /// characters that turn the direction of text are escaped too, since there they can
    /// make a message read as another.
    Message,
}

/// Whether `c` is never shown as written at `level` (`escape` asks this of each character,
/// and so does whoever shows one character at a time, a cell of a screen): a control character (C0 but the tab,
/// DEL, C1), a separator that ends a line (U+2028, U+2029), and in a message the characters
/// that embed, override or isolate a direction (U+202A to U+202E, U+2066 to U+2069).
pub fn escaped(c: char, level: Level) -> bool {
    let acts = (c.is_control() && c != '\t') || matches!(c, '\u{2028}' | '\u{2029}');
    let turns = matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    acts || (turns && level == Level::Message)
}

/// `text` for showing at `level`: each character that is never shown as written becomes
/// `\n`, `\r` or `\u{…}` with its number in hexadecimal, as Rust writes it. Everything else
/// stays, a tab and a backslash included, so text with none of those characters comes back
/// as it is (borrowed), and escaping what is already escaped changes nothing.
pub fn escape(text: &str, level: Level) -> Cow<'_, str> {
    let Some(first) = text.find(|c| escaped(c, level)) else {
        return Cow::Borrowed(text);
    };
    let mut out = String::with_capacity(text.len() + 8);
    out.push_str(&text[..first]);
    for c in text[first..].chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if escaped(c, level) => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TURNS: [char; 9] = [
        '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}',
        '\u{2068}', '\u{2069}',
    ];

    #[test]
    fn what_a_terminal_acts_on_is_shown_as_its_number() {
        for level in [Level::Content, Level::Message] {
            assert_eq!(
                escape("a\u{1b}]0;title\u{7}b", level),
                "a\\u{1b}]0;title\\u{7}b"
            );
            assert_eq!(escape("\0\u{1f}", level), "\\u{0}\\u{1f}", "a twin's place");
            assert_eq!(
                escape("del\u{7f} c1\u{9b}2J", level),
                "del\\u{7f} c1\\u{9b}2J"
            );
            assert_eq!(escape("one\ntwo\r\n", level), "one\\ntwo\\r\\n");
            assert_eq!(
                escape("a\u{2028}b\u{2029}c", level),
                "a\\u{2028}b\\u{2029}c"
            );
        }
    }

    #[test]
    fn every_control_character_is_escaped_but_the_tab() {
        let controls = (0..=0x9f_u32).filter_map(char::from_u32);
        for c in controls.filter(|c| c.is_control()) {
            let text = c.to_string();
            for level in [Level::Content, Level::Message] {
                let shown = escape(&text, level);
                match c {
                    '\t' => assert_eq!(shown, "\t"),
                    _ => assert!(
                        shown.starts_with('\\') && !shown.contains(c),
                        "{:x}: {shown}",
                        c as u32
                    ),
                }
            }
        }
    }

    #[test]
    fn a_turn_of_direction_is_escaped_in_a_message_and_kept_in_content() {
        for c in TURNS {
            let text = format!("invoice{c}fdp.exe");
            assert_eq!(escape(&text, Level::Content), text);
            let shown = escape(&text, Level::Message);
            assert_eq!(shown, format!("invoice\\u{{{:x}}}fdp.exe", c as u32));
        }
        // The marks a script written right to left needs stay, in a message too.
        let marks = "\u{200e}\u{200f}\u{61c}";
        assert_eq!(escape(marks, Level::Message), marks);
    }

    #[test]
    fn ordinary_text_comes_back_as_it_is() {
        let texts = [
            "",
            "plain words",
            "tab\there",
            "a backslash \\n and \\u{1b} as typed",
            "Parícutin, Eyjafjallajökull, ʻokina",
            "👨\u{200d}👩\u{200d}👧 joined, می\u{200c}خواهم, zero\u{200b}width\u{feff}",
            "日本語 עברית العربية",
        ];
        for text in texts {
            for level in [Level::Content, Level::Message] {
                assert!(
                    matches!(escape(text, level), Cow::Borrowed(same) if same == text),
                    "{text:?}"
                );
            }
        }
    }

    #[test]
    fn escaping_what_is_escaped_changes_nothing() {
        let mut text = String::from("a\\b\tc");
        text.extend((0..=0x9f_u32).filter_map(char::from_u32));
        text.extend(TURNS);
        text.push_str("\u{2028}\u{2029}é👍");
        for level in [Level::Content, Level::Message] {
            let once = escape(&text, level).into_owned();
            assert_eq!(escape(&once, level), once);
            assert_eq!(escape(&once, Level::Message), escape(&text, Level::Message));
        }
    }
}
