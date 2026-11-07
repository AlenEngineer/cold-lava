//! A note's frontmatter, read line by line the way Obsidian's properties are written: top-level
//! `key: value`, `key: [a, b]` and `key:` followed by `- item` lines. Every value keeps its byte
//! range in the note, so it can be written back in place. Nested maps are seen, not read.

use std::ops::Range;

/// A value as written, and where it is in the note.
#[derive(Clone, Debug, PartialEq)]
pub struct Written<'t> {
    pub text: &'t str,
    pub span: Range<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value<'t> {
    Scalar(Written<'t>),
    List(Vec<Written<'t>>),
    /// A key with nothing after it.
    Empty,
    /// An indented map, or anything else these rules don't read.
    Nested,
}

#[derive(Debug, PartialEq)]
pub struct Frontmatter<'t> {
    /// The line of the closing `---` (1-based): the frontmatter is lines 1 to this.
    pub end_line: usize,
    pub properties: Vec<(&'t str, Value<'t>)>,
}

impl<'t> Frontmatter<'t> {
    pub fn get(&self, key: &str) -> Option<&Value<'t>> {
        self.properties
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
    }

    /// Every value of `key`: a list's items, or a scalar alone.
    pub fn values(&self, key: &str) -> Vec<&Written<'t>> {
        match self.get(key) {
            Some(Value::Scalar(w)) => vec![w],
            Some(Value::List(l)) => l.iter().collect(),
            _ => Vec::new(),
        }
    }
}

/// The frontmatter at the top of `text`, if it has one that is closed.
pub fn read(text: &str) -> Option<Frontmatter<'_>> {
    let mut lines = lines(text);
    let (_, first) = lines.next()?;
    if first.trim_end() != "---" {
        return None;
    }
    let mut properties: Vec<(&str, Value)> = Vec::new();
    for (n, (at, line)) in lines.enumerate() {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            return Some(Frontmatter {
                end_line: n + 2,
                properties,
            });
        }
        if trimmed.trim_start().is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t', '-']) {
            let Some((_, value)) = properties.last_mut() else {
                continue;
            };
            let item = trimmed.trim_start();
            match (item.strip_prefix('-'), &mut *value) {
                (Some(rest), Value::Empty | Value::List(_))
                    if rest.is_empty() || rest.starts_with([' ', '\t']) =>
                {
                    let start = at + (trimmed.len() - rest.len());
                    let item = scalar(rest, start);
                    match value {
                        Value::List(l) => l.push(item),
                        _ => *value = Value::List(vec![item]),
                    }
                }
                _ => *value = Value::Nested,
            }
            continue;
        }
        let Some(colon) = trimmed.find(':') else {
            continue;
        };
        let key = trimmed[..colon].trim();
        let key = key.trim_matches(|c| c == '"' || c == '\'');
        let rest = &trimmed[colon + 1..];
        let start = at + colon + 1;
        let lead = rest.len() - rest.trim_start().len();
        let body = rest.trim_start();
        let value = if body.is_empty() {
            Value::Empty
        } else if body.starts_with('[') && !body.starts_with("[[") {
            flow_list(body, start + lead)
        } else {
            Value::Scalar(scalar(rest, start))
        };
        properties.push((key, value));
    }
    None
}

/// `text`'s lines with the byte offset each starts at.
fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n').map(move |l| {
        let start = at;
        at += l.len();
        (start, l.trim_end_matches(['\n', '\r']))
    })
}

/// One plain or quoted scalar, starting at byte `start`: quotes, a ` # comment` and the space
/// around it are not part of the value.
fn scalar(raw: &str, start: usize) -> Written<'_> {
    let lead = raw.len() - raw.trim_start().len();
    let mut text = raw.trim_start();
    let mut at = start + lead;
    match text.chars().next() {
        Some(q @ ('"' | '\'')) => {
            if let Some(end) = text[1..].find(q) {
                text = &text[1..1 + end];
                at += 1;
            }
        }
        _ => {
            if let Some(i) = text.find(" #") {
                text = &text[..i];
            }
            text = text.trim_end();
        }
    }
    Written {
        text,
        span: at..at + text.len(),
    }
}

/// `[a, "b", c]` starting at byte `start`.
fn flow_list(body: &str, start: usize) -> Value<'_> {
    let Some(end) = body.rfind(']') else {
        return Value::Nested;
    };
    let inner = &body[1..end];
    let mut items = Vec::new();
    let mut at = start + 1;
    for part in inner.split(',') {
        if !part.trim().is_empty() {
            items.push(scalar(part, at));
        }
        at += part.len() + 1;
    }
    Value::List(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice<'a>(text: &'a str, w: &Written) -> &'a str {
        &text[w.span.clone()]
    }

    #[test]
    fn reads_scalars_lists_and_quotes_with_their_spans() {
        let text = "---\n\
                    country: Indonesia\n\
                    elevation_m: 813 # about\n\
                    tags: [volcano/caldera, \"island\"]\n\
                    aliases:\n  - Krakatau\n  - 'Anak'\n\
                    author: \"[[Ana]]\"\n\
                    nested:\n  a: 1\n\
                    blank:\n\
                    ---\n\
                    # Krakatoa\n";
        let fm = read(text).unwrap();
        assert_eq!(fm.end_line, 12);
        let scalar = |k: &str| match fm.get(k) {
            Some(Value::Scalar(w)) => (w.text, slice(text, w)),
            v => panic!("{k}: {v:?}"),
        };
        assert_eq!(scalar("country"), ("Indonesia", "Indonesia"));
        assert_eq!(scalar("elevation_m"), ("813", "813"));
        assert_eq!(scalar("author"), ("[[Ana]]", "[[Ana]]"));
        let list = |k: &str| {
            fm.values(k)
                .iter()
                .map(|w| (w.text, slice(text, w)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            list("tags"),
            [("volcano/caldera", "volcano/caldera"), ("island", "island")]
        );
        assert_eq!(
            list("aliases"),
            [("Krakatau", "Krakatau"), ("Anak", "Anak")]
        );
        assert_eq!(fm.get("nested"), Some(&Value::Nested));
        assert_eq!(fm.get("blank"), Some(&Value::Empty));
    }

    #[test]
    fn needs_both_fences() {
        assert_eq!(read("# Title\n"), None);
        assert_eq!(read("---\na: 1\n"), None);
        assert_eq!(read("---\r\na: 1\r\n---\r\n").unwrap().properties.len(), 1);
    }
}
