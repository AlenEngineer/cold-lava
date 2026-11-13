//! A problem: why something was refused, failed or was left out, in the words a person
//! reads. A problem says its own lines, so that whoever shows it never has to find them in
//! a string: a line break between two lines is where its maker put one, and one inside a
//! line is text, which a frontend shows as text (`say` on the command line, `Notice` in the
//! terminal app). The engine keeps the words as they were written and knows no terminal.
//!
//! The ways problems are put together are here, once: on one line where a context holds one
//! (`one_line`, `joined`, and what `--json` and the gateway carry), a TOML error's lines
//! read from the parser's parts (`Problem::toml`), and a problem picked out of a list by
//! what it is about, never by its words (`about`). `tests/one_door.rs` in the app lists what
//! still does one of these by hand.

use std::fmt;

use serde::{Serialize, Serializer};

/// Why something was refused, failed or was left out. It has a line at least: every way to
/// make one gives it one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// What is said before its first line, its separator included: where it is (`at`), or
    /// the section it is under (`under`).
    before: Option<String>,
    /// Its words, a line each, as they were made. Never empty.
    lines: Vec<String>,
    /// What it is about (a shape's id, a section of config.toml), for whoever picks problems
    /// out of a list. It is never said: the words already say it as they were written. It is
    /// a string while two vocabularies use it, shapes and config.toml's sections; a third
    /// makes it an enum.
    about: Option<String>,
    /// Which of its lines says what is wrong: the first, but a TOML error's message, which
    /// comes after where it is (`headline`).
    gist: usize,
    /// How it is said where one line is held (`one_line`).
    one: OneLine,
}

/// How a problem is said where one line is held: `--json`, a message on the gateway, a list
/// `joined`. Its maker chooses; what `--json` has written for a kind of problem stays.
#[derive(Clone, Debug, PartialEq, Eq)]
enum OneLine {
    /// Its lines, a space between two, each with its own characters.
    Lines,
    /// The text given: a TOML error's as the parser wrote it, every line break a space, as
    /// types, shapes and attributes have always written it (a CR before a line break kept).
    Text(String),
    /// Its `headline`: what is wrong, without where in the file it is.
    Headline,
}

impl Problem {
    /// A problem of one line.
    pub fn line(words: impl Into<String>) -> Problem {
        Problem::of(words.into(), [])
    }

    /// A problem of these lines, each made where the problem is: `first`, then `rest`.
    pub fn of(first: String, rest: impl IntoIterator<Item = String>) -> Problem {
        Problem {
            before: None,
            lines: std::iter::once(first).chain(rest).collect(),
            about: None,
            gist: 0,
            one: OneLine::Lines,
        }
    }

    /// A problem that is the app's own text (a usage), whose line breaks are known when the
    /// app is built: a line for each.
    pub fn usage(text: &'static str) -> Problem {
        let mut lines = text.split('\n').map(String::from);
        let first = lines.next().unwrap_or_default();
        Problem::of(first, lines)
    }

    /// And after its lines each line of `text`, the app's own (a usage).
    pub fn then(mut self, text: &'static str) -> Problem {
        self.lines.extend(text.split('\n').map(String::from));
        self
    }

    /// Said at `place`, before its first line with `": "`.
    pub fn at(mut self, place: impl fmt::Display) -> Problem {
        self.before = Some(format!("{place}: "));
        self
    }

    /// Why the file at `place` can't be read: said `{place}: can't read it: …`.
    pub fn unreadable(self, place: impl fmt::Display) -> Problem {
        self.at(format!("{place}: can't read it"))
    }

    /// Said on one line (`one_line`) by its `headline` alone: a TOML error that has always
    /// been said by its message alone where one line is held (`style`'s errors in `--json`
    /// and on the gateway), while a person reading it on the command line sees where it is.
    pub fn brief(mut self) -> Problem {
        self.one = OneLine::Headline;
        self
    }

    /// Said under `label` (a section of a file, like `[style]`), before its first line with a
    /// space: config.toml's sections and a type's `[types.NAME]`. Saying them with `at`'s
    /// `": "` would change their words, which the owner has not decided, so they keep it.
    pub fn under(mut self, label: impl fmt::Display) -> Problem {
        self.before = Some(format!("{label} "));
        self
    }

    /// About `what`, for whoever picks it out of a list.
    pub fn about(mut self, what: impl Into<String>) -> Problem {
        self.about = Some(what.into());
        self
    }

    /// Whether it is about `what`.
    pub fn is_about(&self, what: &str) -> bool {
        self.about.as_deref() == Some(what)
    }

    /// What it is about, when its maker said.
    pub fn subject(&self) -> Option<&str> {
        self.about.as_deref()
    }

    /// The lines a person reads: what is said before the first, then the rest.
    pub fn said(&self) -> Vec<String> {
        let mut said = self.lines.clone();
        if let Some(before) = &self.before {
            said[0] = format!("{before}{}", said[0]);
        }
        said
    }

    /// What is wrong and where it is, on one short line: what is said before its first line,
    /// then the line that says what is wrong (a TOML error's message). What a status line
    /// shows.
    pub fn headline(&self) -> String {
        format!(
            "{}{}",
            self.before.as_deref().unwrap_or(""),
            self.lines[self.gist]
        )
    }

    /// It on one line, as a context that holds one says it (a list `joined`, `--json`, a
    /// message on the gateway): its lines with a space between two, each with its own
    /// characters, a line break inside one included; a TOML error as types, shapes and
    /// attributes have always written it (`Problem::toml`); or its `headline` (`brief`).
    pub fn one_line(&self) -> String {
        format!("{}{}", self.before.as_deref().unwrap_or(""), self.words())
    }

    /// Its own words on one line (`one_line`), without what is said before them: what a
    /// list that says where each problem is by itself shows (the shape editor's mistakes).
    pub fn words(&self) -> String {
        match &self.one {
            OneLine::Lines => self.lines.join(" "),
            OneLine::Text(text) => text.clone(),
            OneLine::Headline => self.lines[self.gist].clone(),
        }
    }

    /// A TOML error as the lines the parser shows it on (toml 1.2.0), taken from its parts:
    /// its text is cut only where the parser's layout breaks it, never where what it quotes
    /// from the file does. Its message is one line. The parser writes where the error is in
    /// one of two ways:
    ///
    /// - for a text it read (the error has a span), before the message: a header, the line
    ///   of the file and a caret under it, each a line of its own (a line of the file holds
    ///   no line break), then a line break;
    /// - for a value with no text (a table read into a setting), after the message, on one
    ///   line: the keys that lead to the value.
    ///
    /// So where it is, is what is left of the parser's text without the message, before it
    /// or after it. A text that is neither is one line. The line of the file keeps a CR
    /// that ends it (`toml_file` takes it off). On one line it is the parser's text with
    /// every line break a space (`OneLine::Text`).
    pub fn toml(e: &toml::de::Error) -> Problem {
        let (whole, message) = (e.to_string(), e.message().trim_end());
        let one = OneLine::Text(whole.replace('\n', " ").trim().to_string());
        let whole = whole.trim_end();
        let before = || -> Option<Problem> {
            let place = whole.strip_suffix(message)?;
            if !place.is_empty() && !place.ends_with('\n') {
                return None;
            }
            let mut lines = place.split_terminator('\n').map(String::from);
            let mut problem = match lines.next() {
                Some(header) => Problem::of(header, lines.chain([message.to_string()])),
                None => Problem::line(message),
            };
            problem.gist = problem.lines.len() - 1;
            Some(problem)
        };
        let after = || -> Option<Problem> {
            let keys = whole.strip_prefix(message)?.strip_prefix('\n')?;
            Some(Problem::of(message.to_string(), [keys.to_string()]))
        };
        let problem = match e.span() {
            Some(_) => before().or_else(after),
            None => after(),
        };
        let mut problem = problem.unwrap_or_else(|| Problem::line(whole));
        problem.one = one;
        problem
    }

    /// Why `text` doesn't parse, as the parser shows it (`toml`). The line of the file it
    /// quotes is given without the CR of a CR LF line end: a CR that no LF follows is the
    /// line's own and stays.
    pub fn toml_file(e: &toml::de::Error, text: &str) -> Problem {
        let mut problem = Problem::toml(e);
        // The line the error is on ends in LF when one follows where the error is.
        let at = |start: usize| start.min(text.len().saturating_sub(1));
        let ends_in_lf =
            (e.span()).is_some_and(|span| text.as_bytes()[at(span.start)..].contains(&b'\n'));
        if let (true, Some((_message, place))) = (ends_in_lf, problem.lines.split_last_mut()) {
            for line in place {
                if line.ends_with('\r') {
                    line.pop();
                }
            }
        }
        problem
    }
}

/// The problems one answer from another program may have reported: the first 100, then how
/// many more (`capped`).
pub const PROBLEMS: usize = 100;

/// `problems` with only the first `PROBLEMS` kept, and a last one that says how many more
/// there were.
pub fn capped(mut problems: Vec<Problem>) -> Vec<Problem> {
    if problems.len() > PROBLEMS {
        let more = problems.len() - PROBLEMS;
        problems.truncate(PROBLEMS);
        problems.push(Problem::line(format!("and {more} more")));
    }
    problems
}

/// The first `most` characters of what another program sent, for a message, with `…` where
/// it is cut: every value an answer gave that a message quotes is cut so.
pub fn cut(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// What goes on one line where several are `joined`: a `Problem` as `one_line` says it, or
/// the words of one not yet a `Problem` (an owner's left-out pieces, the cards a merge left
/// out, the notes a tag edit left alone), which stay as they are.
pub trait OnOneLine {
    fn on_one_line(&self) -> String;
}

impl OnOneLine for Problem {
    fn on_one_line(&self) -> String {
        self.one_line()
    }
}

/// The words of a problem not yet a `Problem`. What keeps this alive: the cards a merge
/// left out and the notes a tag edit left alone (`shape_editor::merged`, the menu's
/// `refused`), which are a notice's words (#291). Once they are gone, so is this.
impl OnOneLine for String {
    fn on_one_line(&self) -> String {
        self.clone()
    }
}

/// Several problems on one line, in order.
pub fn joined<T: OnOneLine>(problems: &[T]) -> String {
    let words: Vec<String> = problems.iter().map(T::on_one_line).collect();
    words.join("; ")
}

impl From<String> for Problem {
    fn from(line: String) -> Problem {
        Problem::line(line)
    }
}

impl From<&str> for Problem {
    fn from(line: &str) -> Problem {
        Problem::line(line)
    }
}

/// Its lines as a person reads them (`said`), a line break between two.
impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.said().join("\n"))
    }
}

impl std::error::Error for Problem {}

/// On one line (`one_line`), one string. What it is about is not said.
impl Serialize for Problem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.one_line())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    /// How types, shapes and attributes have always written a TOML error on one line.
    fn folded(e: &toml::de::Error) -> String {
        e.to_string().replace('\n', " ").trim().to_string()
    }

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Strict {
        #[serde(default)]
        x: u8,
        #[serde(default)]
        t: BTreeMap<String, u8>,
    }

    /// Texts that don't parse: the first line, the last, a last line with no line end, LF,
    /// CR LF and a lone CR, tabs and wide characters, a key holding a line break.
    const TEXTS: [&str; 15] = [
        "[types.x\n",
        "a = 1\nb = [\n",
        "a = 1\nb = = 2",
        "[types.x]\r\nfoo = [\r\n",
        "[types.x\r\nfoo = 1\r\n",
        "a = 1\r\nb = = 2",
        "a = 1\rb = 2\n",
        "a = [\r",
        "\ta = = 1\n",
        "日本語 = = 1\n",
        "\"日本語のキー\" = = 1\n",
        "\"a\\nb\" = 1\n\"a\\nb\" = 2\n",
        "[a]\n[a]\n",
        "=",
        "a = \"\t\" x\n",
    ];

    /// Errors with a span (texts the parser read) and without (values read into a type), on
    /// the first line, the last, a last line with no line end, with LF, CR LF and a lone CR,
    /// tabs and wide characters, and messages that quote a key holding a line break.
    fn errors() -> Vec<toml::de::Error> {
        let mut errors: Vec<toml::de::Error> = (TEXTS.iter())
            .map(|t| toml::from_str::<toml::Table>(t).expect_err(t))
            .collect();
        for text in [
            "\"a\\nb\" = 1\n",
            "x = \"no\"\n",
            "x = 1\r\ny = 2\r\n",
            "[t]\n\"日本\" = \"x\"",
        ] {
            errors.push(toml::from_str::<Strict>(text).expect_err(text));
            let table: toml::Table = toml::from_str(text).expect(text);
            let read = toml::Value::Table(table).try_into::<Strict>();
            errors.push(read.expect_err(text));
        }
        let table: toml::Table = toml::from_str("[t]\n\"a\\nFAKE\" = \"x\"\n").unwrap();
        let read = toml::Value::Table(table).try_into::<BTreeMap<String, BTreeMap<String, u8>>>();
        errors.push(read.expect_err("a string is no number"));
        errors
    }

    #[test]
    fn a_toml_error_on_one_line_is_the_parsers_text_folded() {
        for e in errors() {
            let problem = Problem::toml(&e);
            assert_eq!(problem.one_line(), folded(&e), "{e}");
            let at = problem.clone().unreadable("w/types.toml");
            let written = format!("w/types.toml: can't read it: {}", folded(&e));
            assert_eq!(at.one_line(), written, "{e}");
            assert_eq!(problem.headline(), e.message(), "what is wrong: {e}");
            assert_eq!(problem.clone().brief().one_line(), e.message(), "{e}");
            let whole = e.to_string();
            let whole = whole.trim_end();
            match problem.lines.as_slice() {
                [one] => assert_eq!(one, whole),
                lines => assert_eq!(lines.join("\n"), whole, "cut only at its line breaks"),
            }
        }
    }

    #[test]
    fn a_toml_error_keeps_the_parsers_lines_and_its_message_whole() {
        for e in errors() {
            let problem = Problem::toml(&e);
            let lines = &problem.lines;
            match e.span() {
                Some(_) => {
                    assert_eq!(lines.len(), 5, "a header, a gutter, the line, a caret: {e}");
                    assert_eq!(lines[4], e.message().trim_end(), "{e}");
                }
                None => {
                    assert_eq!(lines[0], e.message().trim_end(), "{e}");
                    let keys = &lines[1..];
                    assert!(keys.len() <= 1, "the message, then where: {e}");
                    assert!(keys.iter().all(|k| k.starts_with("in `")), "{e}");
                }
            }
        }
    }

    /// A file's error shows its lines without the CR of a CR LF line end, and is written on
    /// one line as it always was, that CR kept.
    #[test]
    fn a_files_error_on_one_line_keeps_what_its_lines_leave_out() {
        for text in TEXTS {
            let e = toml::from_str::<toml::Table>(text).expect_err(text);
            let problem = Problem::toml_file(&e, text);
            assert_eq!(problem.one_line(), folded(&e), "{text:?}");
            assert_eq!(problem.headline(), e.message(), "{text:?}");
            if text.contains("\r\n") {
                let ends = |line: &String| line.ends_with('\r');
                assert!(!problem.lines.iter().any(ends), "{text:?}");
            }
        }
    }

    #[test]
    fn a_file_loses_the_cr_of_a_cr_lf_line_end_only() {
        for (text, line) in [
            ("[keys]\r\n\"graph.open\" = [\r\n", "2 | \"graph.open\" = ["),
            ("[keys]\n\"graph.open\" = [\r", "2 | \"graph.open\" = [\r"),
            ("a = 1\rb = 2\n", "1 | a = 1\rb = 2"),
        ] {
            let e = toml::from_str::<toml::Table>(text).expect_err(text);
            assert_eq!(Problem::toml_file(&e, text).lines[2], line, "{text:?}");
        }
    }

    #[test]
    fn a_problem_is_said_where_it_is_and_picked_out_by_what_it_is_about() {
        let p = Problem::line("it nests shape b, which didn't load")
            .at("/w/shapes.toml: shape a")
            .about("a");
        assert_eq!(
            p.to_string(),
            "/w/shapes.toml: shape a: it nests shape b, which didn't load"
        );
        assert_eq!(p.words(), "it nests shape b, which didn't load");
        assert!(p.is_about("a") && !p.is_about("b"));

        let section = Problem::line("tasks: invalid type").under("[style]");
        assert_eq!(section.said(), ["[style] tasks: invalid type"]);
        assert_eq!(section.words(), "tasks: invalid type");

        let usage = Problem::of("no such thing: a\nb".to_string(), []).then("usage: x\n  y");
        assert_eq!(usage.said(), ["no such thing: a\nb", "usage: x", "  y"]);
        assert_eq!(usage.to_string(), "no such thing: a\nb\nusage: x\n  y");
        assert_eq!(Problem::usage("usage: x\n  y").said(), ["usage: x", "  y"]);
    }

    /// `--json` and a message on the gateway hold one line. A problem of one line is that
    /// line exactly, a line break inside it included (a shape's id that holds one is written
    /// as main wrote it), and one of several lines puts a space between them.
    #[test]
    fn where_one_line_is_held_a_problem_is_said_on_one() {
        let id = Problem::line("its rebind names shape a\nb, which isn't loaded")
            .at("extension x: shape a\nb")
            .about("a\nb");
        let said = "extension x: shape a\nb: its rebind names shape a\nb, which isn't loaded";
        assert_eq!(id.one_line(), said);
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            serde_json::to_string(said).unwrap(),
            "as it is said, and not what it is about"
        );
        let several = Problem::of("one".to_string(), ["  two".to_string()]).at("p");
        assert_eq!(several.to_string(), "p: one\n  two");
        assert_eq!(several.one_line(), "p: one   two");
        assert_eq!(serde_json::to_string(&several).unwrap(), "\"p: one   two\"");
        assert_eq!(joined(&[several, id]), format!("p: one   two; {said}"));
    }

    #[test]
    fn problems_are_joined_in_order() {
        let words = ["a", "b c\nd", " e "].map(String::from);
        assert_eq!(joined(&words), "a; b c\nd;  e ");
        assert_eq!(joined::<Problem>(&[]), "");
        assert_eq!(
            joined(&[Problem::line("one"), Problem::line("two").at("x")]),
            "one; x: two"
        );
        assert_eq!(joined(&["kept".to_string()]), "kept");
    }

    /// The first 100 problems are kept, then how many more there were.
    #[test]
    fn problems_are_capped_at_a_hundred() {
        let some = |n: usize| (0..n).map(|i| Problem::line(format!("p{i}"))).collect();
        assert_eq!(capped(some(100)).len(), 100);
        let many = capped(some(50_000));
        assert_eq!(many.len(), 101);
        assert_eq!(many[99].one_line(), "p99");
        assert_eq!(many[100].one_line(), "and 49900 more");
    }
}
