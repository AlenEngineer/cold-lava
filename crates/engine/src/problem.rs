//! A problem: why something was refused, failed or was left out, in the words a person
//! reads. A problem says its own lines, so that whoever shows it never has to find them in
//! a string: a line break between two lines is where its maker put one, and one inside a
//! line is text, which a frontend shows as text (`say` on the command line, `Notice` in the
//! terminal app). The engine keeps the words as they were written and knows no terminal.
//!
//! The ways problems are put together are here, once: several joined on one line
//! (`joined`), a TOML error's lines read from the parser's parts (`Problem::toml`), and a
//! problem picked out of a list by what it is about, never by its words (`about`).
//! `tests/one_door.rs` in the app lists what still does one of these by hand.

use std::fmt;

use serde::{Serialize, Serializer};

/// Why something was refused, failed or was left out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Where it is (a file, and what in it), said before its first line with `": "`.
    at: Option<String>,
    /// Its words, a line each, as they were made.
    lines: Vec<String>,
    /// What it is about (a shape's id, a section of config.toml), for whoever picks problems
    /// out of a list. It is never said: the words already say it as they were written. It is
    /// set where something picks problems out by it.
    about: Option<String>,
}

impl Problem {
    /// A problem of one line.
    pub fn line(words: impl Into<String>) -> Problem {
        Problem::of([words.into()])
    }

    /// A problem of these lines, each made where the problem is.
    pub fn of(lines: impl IntoIterator<Item = String>) -> Problem {
        Problem {
            at: None,
            lines: lines.into_iter().collect(),
            about: None,
        }
    }

    /// And after its lines each line of `text`, the app's own (a usage), whose line breaks
    /// are known when the app is built.
    pub fn then(mut self, text: &'static str) -> Problem {
        self.lines.extend(text.split('\n').map(String::from));
        self
    }

    /// Said at `place`, before its first line.
    pub fn at(mut self, place: impl Into<String>) -> Problem {
        self.at = Some(place.into());
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

    /// Its own lines, in order, without where it is.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Its own first line, without where it is.
    pub fn first(&self) -> &str {
        self.lines.first().map_or("", String::as_str)
    }

    /// The lines a person reads: where it is before the first, then the rest.
    pub fn said(&self) -> Vec<String> {
        let mut said = self.lines.clone();
        if let Some(at) = &self.at {
            match said.is_empty() {
                true => said.push(at.clone()),
                false => said[0] = format!("{at}: {}", said[0]),
            }
        }
        said
    }

    /// Its words on one line: its lines joined by a space, a line break inside a line a space
    /// too, without spaces at either end. How types, shapes and attributes say a TOML error,
    /// and what their `--json` holds.
    ///
    /// For a TOML error this is the parser's text with each line break a space, as those
    /// three made it before (`e.to_string().replace('\n', " ").trim()`): `toml` cuts the
    /// text only at a line break (its lines joined by line breaks are the text), or keeps it
    /// whole as one line.
    pub fn one_line(&self) -> String {
        let said: Vec<String> = (self.said().iter())
            .map(|line| line.replace('\n', " "))
            .collect();
        said.join(" ").trim().to_string()
    }

    /// A TOML error as the lines the parser shows it on (toml 1.2.0), taken from its parts
    /// and never from the line breaks in its message, which what it quotes from the file
    /// can hold. Its message is one line. The parser writes where the error is in one of two
    /// ways:
    ///
    /// - for a text it read (the error has a span), before the message: a header, the line
    ///   of the file and a caret under it, each a line of its own (a line of the file holds
    ///   no line break), then a line break;
    /// - for a value with no text (a table read into a setting), after the message, on one
    ///   line: the keys that lead to the value.
    ///
    /// So where it is, is what is left of the parser's text without the message, before it
    /// or after it. A text that is neither is one line. The line of the file keeps a CR
    /// that ends it (`toml_file` takes it off).
    pub fn toml(e: &toml::de::Error) -> Problem {
        let (whole, message) = (e.to_string(), e.message().trim_end());
        let whole = whole.trim_end();
        let before = || -> Option<Vec<String>> {
            let place = whole.strip_suffix(message)?;
            if !place.is_empty() && !place.ends_with('\n') {
                return None;
            }
            let place = place.split_terminator('\n').map(String::from);
            Some(place.chain([message.to_string()]).collect())
        };
        let after = || -> Option<Vec<String>> {
            let keys = whole.strip_prefix(message)?.strip_prefix('\n')?;
            Some(vec![message.to_string(), keys.to_string()])
        };
        let lines = match e.span() {
            Some(_) => before().or_else(after),
            None => after(),
        };
        Problem::of(lines.unwrap_or_else(|| vec![whole.to_string()]))
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

/// Several problems on one line, in order.
pub fn joined<T: fmt::Display>(problems: &[T]) -> String {
    let words: Vec<String> = problems.iter().map(T::to_string).collect();
    words.join("; ")
}

impl From<String> for Problem {
    fn from(line: String) -> Problem {
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

/// As it is said (`Display`), one string. What it is about is not said.
impl Serialize for Problem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    /// How types, shapes and attributes folded a TOML error before `one_line`.
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

    /// Errors with a span (texts the parser read) and without (values read into a type), on
    /// the first line, the last, a last line with no line end, with LF, CR LF and a lone CR,
    /// tabs and wide characters, and messages that quote a key holding a line break.
    fn errors() -> Vec<toml::de::Error> {
        let texts = [
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
        let mut errors: Vec<toml::de::Error> = (texts.iter())
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
            let whole = e.to_string();
            let whole = whole.trim_end();
            match problem.lines() {
                [one] => assert_eq!(one, whole),
                lines => assert_eq!(lines.join("\n"), whole, "cut only at its line breaks"),
            }
        }
    }

    #[test]
    fn a_toml_error_keeps_the_parsers_lines_and_its_message_whole() {
        for e in errors() {
            let problem = Problem::toml(&e);
            let lines = problem.lines();
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

    #[test]
    fn a_file_loses_the_cr_of_a_cr_lf_line_end_only() {
        for (text, line) in [
            ("[keys]\r\n\"graph.open\" = [\r\n", "2 | \"graph.open\" = ["),
            ("[keys]\n\"graph.open\" = [\r", "2 | \"graph.open\" = [\r"),
            ("a = 1\rb = 2\n", "1 | a = 1\rb = 2"),
        ] {
            let e = toml::from_str::<toml::Table>(text).expect_err(text);
            assert_eq!(Problem::toml_file(&e, text).lines()[2], line, "{text:?}");
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
        assert_eq!(p.first(), "it nests shape b, which didn't load");
        assert!(p.is_about("a") && !p.is_about("b"));
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            "\"/w/shapes.toml: shape a: it nests shape b, which didn't load\"",
            "what it is about is not said"
        );

        let usage = Problem::of(["no such thing: a\nb".to_string()]).then("usage: x\n  y");
        assert_eq!(usage.lines(), ["no such thing: a\nb", "usage: x", "  y"]);
        assert_eq!(usage.to_string(), "no such thing: a\nb\nusage: x\n  y");
        assert_eq!(Problem::of([]).then("usage: x").said(), ["usage: x"]);
    }

    #[test]
    fn problems_are_joined_in_order() {
        assert_eq!(joined(&["a", "b c", "d"]), "a; b c; d");
        assert_eq!(joined::<Problem>(&[]), "");
        assert_eq!(
            joined(&[Problem::line("one"), Problem::line("two").at("x")]),
            "one; x: two"
        );
    }
}
