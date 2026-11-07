//! A note's frontmatter, read line by line the way Obsidian's properties are written: top-level
//! `key: value`, `key: [a, b]` and `key:` followed by `- item` lines. Every value keeps its byte
//! range in the note, so it can be written back in place. Nested maps are seen, not read.
//!
//! `edit_tag` adds or removes a tag in `tags` the same way: by editing those bytes, never by
//! writing the YAML out again.

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

/// Adding a tag to a note's frontmatter, or taking it away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagEdit {
    Add,
    Remove,
}

/// `text` with `tag` (`#` optional) added to or removed from its frontmatter `tags`, written the
/// way the note writes its list (`[a, b]` or `- a` lines), or `None` when there is nothing to do.
/// A note without frontmatter gets one; a `tags` it can't edit safely is refused with the reason.
pub fn edit_tag(text: &str, tag: &str, edit: TagEdit) -> Result<Option<String>, String> {
    let tag = tag.trim().trim_start_matches('#');
    if !is_tag(tag) {
        return Err(format!("{tag:?} isn't a tag"));
    }
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let is = |w: &Written| w.text.trim_start_matches('#').to_lowercase() == tag.to_lowercase();
    let add = edit == TagEdit::Add;
    let Some(fm) = read(text) else {
        if lines(text)
            .next()
            .is_some_and(|(_, l)| l.trim_end() == "---")
        {
            return Err("its frontmatter has no closing ---".into());
        }
        return Ok(add.then(|| format!("---{nl}tags:{nl}  - {tag}{nl}---{nl}{text}")));
    };
    let fm_lines: Vec<(usize, &str)> = lines(text).take(fm.end_line).collect();
    let key = ["tags", "tag"].into_iter().find(|k| fm.get(k).is_some());
    let Some((key, value)) = key.and_then(|k| Some((k, fm.get(k)?))) else {
        let (close, _) = fm_lines[fm.end_line - 1];
        return Ok(add.then(|| splice(text, close..close, &format!("tags:{nl}  - {tag}{nl}"))));
    };
    let (at, line) = *fm_lines
        .iter()
        .skip(1)
        .find(|(_, l)| top_level_key(l) == Some(key))
        .ok_or("its tags key can't be found")?;
    let line_end = at + line.len() + text[at + line.len()..].find('\n').map_or(0, |i| i + 1);
    let edited = match value {
        Value::Nested => return Err("its tags aren't a plain list".into()),
        Value::Empty if add => splice(text, line_end..line_end, &format!("  - {tag}{nl}")),
        Value::Empty => return Ok(None),
        Value::Scalar(w) if !is_tag(w.text.trim_start_matches('#')) => {
            return Err("its tags are one line of text, not a list".into());
        }
        Value::Scalar(w) => {
            let quoted =
                w.span.start > at && matches!(text.as_bytes()[w.span.start - 1], b'"' | b'\'');
            let whole = w.span.start - quoted as usize..w.span.end + quoted as usize;
            match (add, is(w)) {
                (true, true) | (false, false) => return Ok(None),
                (true, false) => splice(text, whole, &format!("[{}, {tag}]", w.text)),
                (false, true) => splice(text, whole, "[]"),
            }
        }
        Value::List(items) if line.contains('[') => {
            if items
                .iter()
                .any(|w| w.text.contains(['[', ']', '{', '}', '"', '\'']))
            {
                return Err("its tags list has quotes or brackets inside".into());
            }
            let open = at + line.find('[').unwrap_or(0) + 1;
            let close = at + line.rfind(']').ok_or("its tags list isn't closed")?;
            let parts = flow_parts(text, open..close);
            match (
                add,
                parts
                    .iter()
                    .position(|p| is(&scalar(&text[p.clone()], p.start))),
            ) {
                (true, Some(_)) | (false, None) => return Ok(None),
                (true, None) if items.is_empty() => splice(text, open..close, tag),
                (true, None) => splice(text, close..close, &format!(", {tag}")),
                (false, Some(0)) if parts.len() > 1 => {
                    let next = &text[parts[1].clone()];
                    let to = parts[1].start + next.len() - next.trim_start().len();
                    splice(text, parts[0].start..to, "")
                }
                (false, Some(0)) => splice(text, open..close, ""),
                (false, Some(k)) => splice(text, parts[k - 1].end..parts[k].end, ""),
            }
        }
        Value::List(items) => match (add, items.iter().find(|w| is(w))) {
            (true, Some(_)) | (false, None) => return Ok(None),
            (true, None) => {
                let last = items.last().ok_or("its tags list is empty")?;
                let (start, end) = line_around(text, last.span.start);
                let prefix = text[start..last.span.start].trim_end_matches(['"', '\'']);
                splice(text, end..end, &format!("{prefix}{tag}{nl}"))
            }
            (false, Some(w)) => {
                let (start, end) = line_around(text, w.span.start);
                splice(text, start..end, "")
            }
        },
    };
    // A tag listed twice goes entirely.
    if !add {
        return Ok(Some(edit_tag(&edited, tag, edit)?.unwrap_or(edited)));
    }
    Ok(Some(edited))
}

/// A tag as Obsidian takes one: letters, digits, `_`, `-` and `/`, and not only digits.
pub fn is_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
        && !tag.chars().all(|c| c.is_ascii_digit())
}

/// The key of a frontmatter line that isn't indented or a list item.
fn top_level_key(line: &str) -> Option<&str> {
    if line.starts_with([' ', '\t', '-', '#']) {
        return None;
    }
    let key = line[..line.find(':')?].trim();
    Some(key.trim_matches(|c| c == '"' || c == '\''))
}

/// The byte ranges between the commas of a flow list's inside.
fn flow_parts(text: &str, inside: Range<usize>) -> Vec<Range<usize>> {
    let mut parts = Vec::new();
    let mut at = inside.start;
    for part in text[inside].split(',') {
        if !part.trim().is_empty() {
            parts.push(at..at + part.len());
        }
        at += part.len() + 1;
    }
    parts
}

/// The line holding byte `at`: where it starts, and where the next one does.
fn line_around(text: &str, at: usize) -> (usize, usize) {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
    (start, end)
}

fn splice(text: &str, range: Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..range.start]);
    out.push_str(with);
    out.push_str(&text[range.end..]);
    out
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

    #[test]
    fn adds_and_removes_a_tag_in_the_notes_own_list_style() {
        let add = |t: &str, tag: &str| edit_tag(t, tag, TagEdit::Add).unwrap();
        let remove = |t: &str, tag: &str| edit_tag(t, tag, TagEdit::Remove).unwrap();
        let flow = "---\ntitle: X\ntags: [volcano, \"hot\"] # mine\n---\nBody #inline\n";
        let flow_added =
            "---\ntitle: X\ntags: [volcano, \"hot\", review] # mine\n---\nBody #inline\n";
        assert_eq!(add(flow, "#review").as_deref(), Some(flow_added));
        assert_eq!(add(flow, "Volcano"), None, "already there, in any case");
        assert_eq!(remove(flow_added, "review").as_deref(), Some(flow));
        assert_eq!(
            remove(flow, "volcano").as_deref(),
            Some("---\ntitle: X\ntags: [\"hot\"] # mine\n---\nBody #inline\n")
        );
        assert_eq!(
            remove(flow, "inline"),
            None,
            "only frontmatter tags are edited"
        );
        assert_eq!(
            add("---\ntags: []\n---\n", "a").as_deref(),
            Some("---\ntags: [a]\n---\n")
        );
        assert_eq!(
            remove("---\ntags: [a]\n---\n", "a").as_deref(),
            Some("---\ntags: []\n---\n")
        );

        let block = "---\ntags:\n    - 'volcano'\n    - hot\nnext: 1\n---\n";
        let block_added = "---\ntags:\n    - 'volcano'\n    - hot\n    - review\nnext: 1\n---\n";
        assert_eq!(add(block, "review").as_deref(), Some(block_added));
        assert_eq!(add(block, "hot"), None);
        assert_eq!(remove(block_added, "review").as_deref(), Some(block));
        assert_eq!(
            remove(block, "volcano").as_deref(),
            Some("---\ntags:\n    - hot\nnext: 1\n---\n")
        );
        assert_eq!(
            add("---\ntags:\n    - 'x'\n---\n", "y").as_deref(),
            Some("---\ntags:\n    - 'x'\n    - y\n---\n"),
            "the new item copies the last one's indent, not its quotes"
        );

        assert_eq!(
            add("# Lava\n", "rock").as_deref(),
            Some("---\ntags:\n  - rock\n---\n# Lava\n")
        );
        assert_eq!(remove("# Lava\n", "rock"), None);
        assert_eq!(
            add("---\ntitle: Lava\n---\nx\n", "rock").as_deref(),
            Some("---\ntitle: Lava\ntags:\n  - rock\n---\nx\n")
        );
        assert_eq!(
            add("---\r\ntags:\r\n---\r\nx\r\n", "rock").as_deref(),
            Some("---\r\ntags:\r\n  - rock\r\n---\r\nx\r\n")
        );
        assert_eq!(
            add("---\ntags: volcano\n---\n", "rock").as_deref(),
            Some("---\ntags: [volcano, rock]\n---\n")
        );
        assert_eq!(
            remove("---\ntag: \"rock\"\n---\n", "rock").as_deref(),
            Some("---\ntag: []\n---\n")
        );
        assert_eq!(
            remove("---\ntags: [a, b, a]\n---\n", "a").as_deref(),
            Some("---\ntags: [b]\n---\n")
        );
    }

    #[test]
    fn refuses_tags_it_cant_edit_safely() {
        for (text, tag) in [
            ("---\ntags:\n  a: 1\n---\n", "x"),
            ("---\ntags: volcano, hot\n---\n", "x"),
            ("---\ntags: [a, \"b, c\"]\n---\n", "x"),
            ("---\ntags: [a,\n  b]\n---\n", "x"),
            ("---\ntags:\n  - a\n", "x"),
            ("# x\n", "two words"),
            ("# x\n", "123"),
            ("# x\n", ""),
        ] {
            assert!(
                edit_tag(text, tag, TagEdit::Add).is_err(),
                "{text:?} + {tag:?}"
            );
        }
    }
}
