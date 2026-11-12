//! What the command line prints. Every line on stdout and stderr goes out here, and each
//! goes through `outside::escape` on the way, so text a file or an extension chose never
//! reaches the terminal as it is written. Nothing else in the app prints
//! (`tests/one_door.rs` reads the source for it).
//!
//! How a line is escaped is decided by which of these is called, not where it is called:
//!
//! - `out!`, `Out::listing`, `listing_block`: a **listing**, a line that is the user's own
//!   content (`items`, `agenda`, `table`, `search`, `tidy`, a shape drawn as text, a diff of
//!   their notes, the screen `drive` read). Escaped as content, a line break inside a value
//!   included, so one item is one line.
//! - `told!` on stdout and `problem!` on stderr: a **message**, what a command did, what it
//!   has, or why it didn't. Escaped as a message, line by line: a message may have several
//!   lines, and outside text put into one was escaped where the message was made
//!   (`outside::Outside`).
//! - `json`, `json_line`, `Out::json`, `Out::json_pretty`: JSON, which escapes the way JSON
//!   does. It is written as it is, since it is for a program, not for the terminal.

use crate::outside::{Level, escape};
use serde::Serialize;
use std::fmt::Arguments;
use std::io::{self, StdoutLock, Write};

/// `text` with each of its lines escaped at `level`. The line ends stay as they are, a CR
/// before one included (a note whose lines end in CR LF prints as it did).
fn lines(text: &str, level: Level) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        let body = match body.len() < line.len() {
            true => body.strip_suffix('\r').unwrap_or(body),
            false => body,
        };
        out.push_str(&escape(body, level));
        out.push_str(&line[body.len()..]);
    }
    out
}

/// One line of a listing on stdout.
pub fn listing(words: Arguments<'_>) {
    println!("{}", escape(&words.to_string(), Level::Content));
}

/// Lines of a listing that come as one text (a shape drawn as text, a diff), written as
/// they are but for what is escaped: no line end is added.
pub fn listing_block(text: &str) {
    print!("{}", lines(text, Level::Content));
}

/// A message on stdout.
pub fn told(words: Arguments<'_>) {
    println!("{}", lines(&words.to_string(), Level::Message));
}

/// A message on stderr.
pub fn problem(words: Arguments<'_>) {
    eprintln!("{}", lines(&words.to_string(), Level::Message));
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
pub fn stdout() -> Out {
    Out(io::stdout().lock())
}

impl Out {
    /// One line of a listing.
    pub fn listing(&mut self, words: Arguments<'_>) -> io::Result<()> {
        writeln!(self.0, "{}", escape(&words.to_string(), Level::Content))
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

/// A message on stdout, written as `println!` is.
macro_rules! told {
    () => { $crate::say::told(format_args!("")) };
    ($($words:tt)*) => { $crate::say::told(format_args!($($words)*)) };
}

/// A message on stderr, written as `eprintln!` is.
macro_rules! problem {
    ($($words:tt)*) => { $crate::say::problem(format_args!($($words)*)) };
}

pub(crate) use {out, problem, told};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_line_is_escaped_and_its_end_kept() {
        let text = "one \u{1b}[2J\ntwo\r\nthree \u{202e}\r";
        assert_eq!(
            lines(text, Level::Content),
            "one \\u{1b}[2J\ntwo\r\nthree \u{202e}\\r"
        );
        assert_eq!(
            lines(text, Level::Message),
            "one \\u{1b}[2J\ntwo\r\nthree \\u{202e}\\r"
        );
        for plain in ["", "\n", "a\n\nb", "no end", "ends\n", "tab\there\r\n"] {
            assert_eq!(lines(plain, Level::Message), plain);
        }
    }
}
