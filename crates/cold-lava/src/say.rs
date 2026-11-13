//! What the command line prints. Every line on stdout and stderr goes out here, and no text
//! a file or an extension chose reaches the terminal as it is written: a line is escaped
//! whole (`shown::escape`) on its way out, whatever it was made of, so nothing has to be
//! remembered where a line is made. Nothing else in the app prints (`tests/one_door.rs`
//! reads the source for it).
//!
//! Which of these is called says what a line is:
//!
//! - `out!`, `Out::listing`, `listing_block`: a **listing**, a line that is the user's own
//!   content (`items`, `agenda`, `table`, `search`, `tidy`, a shape drawn as text, a diff of
//!   their notes, the screen `drive` read). Escaped as content.
//! - `told!` on stdout and `problem!` on stderr: a **message**, what a command did, what it
//!   has, or why it didn't. Escaped as a message.
//! - `fixed`: the app's own text, known when it was built (the usage), as it is.
//! - `json`, `json_line`, `Out::json`, `Out::json_pretty`: JSON, which escapes the way JSON
//!   does and is for a program to read. It is written as it is.
//!
//! One call is one line. A line break inside the text of a listing line or a message is
//! escaped like any other character a terminal acts on, so text from outside can't begin a
//! line of its own. What has several lines by the app's own structure says so: one call a
//! line, a `Problem` (the engine's `problem.rs`) for an error, which `main` prints a line
//! each, or `listing_block` for a text whose lines are a listing.

use cold_lava_views::shown::{Level, escape};
use serde::Serialize;
use std::fmt::Arguments;
use std::io::{self, StdoutLock, Write};

/// `text` with each of its lines escaped as content. The line ends stay as they are, a CR
/// before one included (a note whose lines end in CR LF prints as it did).
fn lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = match body.len() < line.len() {
            true => body.strip_suffix('\r').unwrap_or(body),
            false => body,
        };
        out.push_str(&escape(body, Level::Content));
        out.push_str(&line[body.len()..]);
    }
    out
}

/// The words as one line of a listing.
fn content(words: Arguments<'_>) -> String {
    escape(&words.to_string(), Level::Content).into_owned()
}

/// The words as one line of a message.
fn message(words: Arguments<'_>) -> String {
    escape(&words.to_string(), Level::Message).into_owned()
}

// The entries below print with `println!` and `eprintln!` themselves, and don't go through
// `Out`, so that a closed stdout ends the program exactly as it did when each command
// printed with them: the same panic, from the same place in the standard library.

/// One line of a listing on stdout.
pub fn listing(words: Arguments<'_>) {
    println!("{}", content(words));
}

/// Lines of a listing that come as one text (a shape drawn as text, a diff), written as
/// they are but for what is escaped: no line end is added.
pub fn listing_block(text: &str) {
    print!("{}", lines(text));
}

/// One line of a message on stdout.
pub fn message_out(words: Arguments<'_>) {
    println!("{}", message(words));
}

/// One line of a message on stderr.
pub fn message_err(words: Arguments<'_>) {
    eprintln!("{}", message(words));
}

/// The app's own text on stdout, as it is: only text known when the app was built.
pub fn fixed(text: &'static str) {
    println!("{text}");
}

/// A value as JSON over several lines, on stdout.
pub fn json<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// A value as JSON on one line, on stdout.
pub fn json_line<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

/// Stdout held for a long listing, which says when its reader is gone (`| head`) instead of
/// stopping the program.
pub struct Out(StdoutLock<'static>);

/// Stdout, held until the `Out` is dropped.
pub fn held() -> Out {
    Out(io::stdout().lock())
}

impl Out {
    /// One line of a listing.
    pub fn listing(&mut self, words: Arguments<'_>) -> io::Result<()> {
        writeln!(self.0, "{}", content(words))
    }

    /// A value as JSON on one line.
    pub fn json<T: Serialize + ?Sized>(&mut self, value: &T) -> io::Result<()> {
        writeln!(self.0, "{}", serde_json::to_string(value)?)
    }

    /// A value as JSON over several lines.
    pub fn json_pretty<T: Serialize + ?Sized>(&mut self, value: &T) -> io::Result<()> {
        writeln!(self.0, "{}", serde_json::to_string_pretty(value)?)
    }
}

/// One line of a listing on stdout, written as `println!` is.
macro_rules! out {
    () => { $crate::say::listing(format_args!("")) };
    ($($words:tt)*) => { $crate::say::listing(format_args!($($words)*)) };
}

/// One line of a message on stdout, written as `println!` is.
macro_rules! told {
    () => { $crate::say::message_out(format_args!("")) };
    ($($words:tt)*) => { $crate::say::message_out(format_args!($($words)*)) };
}

/// One line of a message on stderr, written as `eprintln!` is.
macro_rules! problem {
    ($($words:tt)*) => { $crate::say::message_err(format_args!($($words)*)) };
}

pub(crate) use {out, problem, told};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_line_of_a_block_is_escaped_and_its_end_kept() {
        let text = "one \u{1b}[2J\ntwo\r\nthree \u{202e}\r";
        assert_eq!(lines(text), "one \\u{1b}[2J\ntwo\r\nthree \u{202e}\\r");
        for plain in ["", "\n", "a\n\nb", "no end", "ends\n", "tab\there\r\n"] {
            assert_eq!(lines(plain), plain);
        }
    }

    #[test]
    fn a_line_break_inside_a_line_is_escaped() {
        let forged = "Plan.md\ncold-lava: all is well \u{202e}";
        assert_eq!(
            content(format_args!("{forged}: (name matches)")),
            "Plan.md\\ncold-lava: all is well \u{202e}: (name matches)"
        );
        assert_eq!(
            message(format_args!("warning: {forged}")),
            "warning: Plan.md\\ncold-lava: all is well \\u{202e}"
        );
        assert_eq!(
            message(format_args!("  {}", "plain\twords")),
            "  plain\twords"
        );
    }
}
