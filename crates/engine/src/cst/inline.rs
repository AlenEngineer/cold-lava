//! A line of a block's text, read left to right into code spans, wikilinks and embeds, Markdown
//! links and images, Dataview fields, comments, math, tags and block ids; the rest is text.
//! What starts first wins, so a wikilink inside a code span stays code. Emphasis and the like
//! stay text: nothing reads them from the tree yet.

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;
use rowan::GreenNodeBuilder;

use super::Gates;
use super::Kind::{self, *};
use super::block::Ev;
use crate::markdown::Syntax as S;

/// Build `events` into `b`, reading each line of text as it comes.
pub(super) fn build(text: &str, events: &[Ev], gates: Gates, b: &mut GreenNodeBuilder<'_>) {
    for ev in events {
        match *ev {
            Ev::Start(kind) => b.start_node(kind.into()),
            Ev::Finish => b.finish_node(),
            Ev::Tok(kind, a, z) => b.token(kind.into(), &text[a..z]),
            Ev::Inline(a, z) => {
                let line = Line::new(text, gates, a);
                match line.line_field(a, z) {
                    Some(end) => line.emit(b, LineField, a, end),
                    None => line.run(b, a, z, false),
                }
            }
            Ev::Yaml(a, z) => yaml(text, gates, b, a, z),
        }
    }
}

fn token(b: &mut GreenNodeBuilder<'_>, kind: Kind, text: &str, a: usize, z: usize) {
    if a < z {
        b.token(kind.into(), &text[a..z]);
    }
}

/// A frontmatter line: its wikilinks, which count as the note's links, and text.
fn yaml(text: &str, gates: Gates, b: &mut GreenNodeBuilder<'_>, a: usize, z: usize) {
    let line = Line {
        text,
        g: gates,
        tags: Vec::new(),
    };
    let mut plain = a;
    let mut i = a;
    while let Some(k) = text[i..z].find("[[") {
        let at = i + k;
        match line.wikilink(at, z).filter(|_| gates.has(S::WikiLinks)) {
            Some(end) => {
                token(b, Yaml, text, plain, at);
                line.emit_wikilink(b, at, at, end);
                (plain, i) = (end, end);
            }
            None => i = at + 1,
        }
    }
    token(b, Yaml, text, plain, z);
}

struct Line<'t> {
    text: &'t str,
    g: Gates,
    /// The tags on this line, from their `#` to their end, found as they always were: by the
    /// tag pattern over the whole line, the markers before it included.
    tags: Vec<Range<usize>>,
}

fn tag_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(crate::markdown::TAG).unwrap())
}

impl<'t> Line<'t> {
    fn new(text: &'t str, g: Gates, at: usize) -> Self {
        let mut tags = Vec::new();
        if g.has(S::Tags) {
            let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
            let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
            let line = &text[start..end];
            if line.contains('#') {
                for c in tag_pattern().captures_iter(line) {
                    let name = c.get(1).expect("the pattern has a name");
                    if !name.as_str().bytes().all(|b| b.is_ascii_digit()) {
                        tags.push(start + name.start() - 1..start + name.end());
                    }
                }
            }
        }
        Line { text, g, tags }
    }

    /// Read `a..z`; `in_link` inside a link's text, where no other link starts.
    fn run(&self, b: &mut GreenNodeBuilder<'_>, a: usize, z: usize, in_link: bool) {
        let t = self.text.as_bytes();
        let mut plain = a;
        let mut i = a;
        while i < z {
            let found = match t[i] {
                b'`' => {
                    let run = t[i..z].iter().take_while(|&&c| c == b'`').count();
                    match self.code_span(i, run, z) {
                        Some(end) => Some((CodeSpan, end)),
                        None => {
                            i += run;
                            continue;
                        }
                    }
                }
                b'!' if t.get(i + 1) == Some(&b'[') => {
                    if t.get(i + 2) == Some(&b'[') && self.g.has(S::Embeds) {
                        self.wikilink(i + 1, z).map(|e| (Embed, e))
                    } else if !in_link {
                        self.link(i + 1, z).map(|e| (Image, e))
                    } else {
                        None
                    }
                }
                b'[' if t.get(i + 1) == Some(&b'[') => match self.g.has(S::WikiLinks) {
                    true => self.wikilink(i, z).map(|e| (WikiLink, e)),
                    false => None,
                },
                b'[' => (self.field(i, z).map(|e| (InlineField, e))).or_else(|| match in_link {
                    true => None,
                    false => self.link(i, z).map(|e| (Link, e)),
                }),
                b'(' => self.field(i, z).map(|e| (InlineField, e)),
                b'%' if self.g.has(S::Comments) && t.get(i + 1) == Some(&b'%') => {
                    (self.text[i + 2..z].find("%%")).map(|k| (Comment, i + 2 + k + 2))
                }
                b'$' if self.g.has(S::Math) => self.math(i, z).map(|e| (InlineMath, e)),
                b'#' => (self.tags.iter())
                    .find(|r| r.start == i && r.end <= z)
                    .map(|r| (Tag, r.end)),
                b'^' if self.g.has(S::BlockIds) => self.block_id(a, i, z).map(|e| (BlockId, e)),
                _ => None,
            };
            match found {
                Some((kind, end)) => {
                    token(b, Text, self.text, plain, i);
                    self.emit(b, kind, i, end);
                    (plain, i) = (end, end);
                }
                None => i += 1,
            }
        }
        token(b, Text, self.text, plain, z);
    }

    fn emit(&self, b: &mut GreenNodeBuilder<'_>, kind: Kind, a: usize, z: usize) {
        let text = self.text;
        match kind {
            Tag | BlockId => token(b, kind, text, a, z),
            CodeSpan => {
                let run = text[a..z].bytes().take_while(|&c| c == b'`').count();
                b.start_node(CodeSpan.into());
                token(b, Backticks, text, a, a + run);
                token(b, CodeText, text, a + run, z - run);
                token(b, Backticks, text, z - run, z);
                b.finish_node();
            }
            WikiLink => self.emit_wikilink(b, a, a, z),
            Embed => self.emit_wikilink(b, a, a + 1, z),
            Link | Image => {
                let open = if kind == Image { a + 2 } else { a + 1 };
                let close = self.close_bracket(open - 1, z).expect("a link has its `]`");
                let paren = text[close..z]
                    .find(')')
                    .map(|k| close + k)
                    .expect("and its `)`");
                b.start_node(kind.into());
                let opener = if kind == Image { ImageOpen } else { LBracket };
                token(b, opener, text, a, open);
                b.start_node(LinkText.into());
                self.run(b, open, close, true);
                b.finish_node();
                token(b, RBracket, text, close, close + 1);
                token(b, LParen, text, close + 1, close + 2);
                token(b, LinkDest, text, close + 2, paren);
                token(b, RParen, text, paren, paren + 1);
                b.finish_node();
            }
            InlineField | LineField => {
                let inline = kind == InlineField;
                let key = if inline { a + 1 } else { a };
                let colons = key + text[key..z].find("::").expect("a field has its `::`");
                let value_end = if inline { z - 1 } else { z };
                b.start_node(kind.into());
                if inline {
                    let open = if text.as_bytes()[a] == b'[' {
                        LBracket
                    } else {
                        LParen
                    };
                    token(b, open, text, a, a + 1);
                }
                token(b, FieldKey, text, key, colons);
                token(b, FieldColons, text, colons, colons + 2);
                self.run(b, colons + 2, value_end, false);
                if inline {
                    let close = if text.as_bytes()[a] == b'[' {
                        RBracket
                    } else {
                        RParen
                    };
                    token(b, close, text, z - 1, z);
                }
                b.finish_node();
            }
            Comment => {
                b.start_node(Comment.into());
                token(b, CommentMarker, text, a, a + 2);
                token(b, CommentText, text, a + 2, z - 2);
                token(b, CommentMarker, text, z - 2, z);
                b.finish_node();
            }
            InlineMath => {
                let run = if text[a..z].starts_with("$$") { 2 } else { 1 };
                b.start_node(InlineMath.into());
                token(b, MathMarker, text, a, a + run);
                token(b, MathText, text, a + run, z - run);
                token(b, MathMarker, text, z - run, z);
                b.finish_node();
            }
            _ => unreachable!("not an inline: {kind:?}"),
        }
    }

    /// A code span opened by `run` backticks at `a`: where it ends, after a run of exactly as
    /// many.
    fn code_span(&self, a: usize, run: usize, z: usize) -> Option<usize> {
        let t = self.text.as_bytes();
        let mut i = a + run;
        while i < z {
            if t[i] == b'`' {
                let n = t[i..z].iter().take_while(|&&c| c == b'`').count();
                if n == run {
                    return Some(i + n);
                }
                i += n;
            } else {
                i += 1;
            }
        }
        None
    }

    /// `[[…]]` at `a`: where it ends. As the link pattern always read it, the first `]` after
    /// `[[` must be the first of `]]`.
    fn wikilink(&self, a: usize, z: usize) -> Option<usize> {
        if a + 2 > z {
            return None;
        }
        let close = self.text[a + 2..z].find(']')? + a + 2;
        (self.text.as_bytes().get(close + 1) == Some(&b']') && close + 2 <= z).then_some(close + 2)
    }

    /// A wikilink whose `[[` is at `open` (`a` is its `!` for an embed): target, then `#…` or
    /// `^…`, then `|alias`.
    fn emit_wikilink(&self, b: &mut GreenNodeBuilder<'_>, a: usize, open: usize, z: usize) {
        let text = self.text;
        let (inner_a, inner_z) = (open + 2, z - 2);
        let inner = &text[inner_a..inner_z];
        let target = inner.find(['|', '#', '^']).unwrap_or(inner.len()) + inner_a;
        let pipe = text[target..inner_z].find('|').map(|k| target + k);
        let anchor_end = pipe.unwrap_or(inner_z);
        b.start_node(if a < open { Embed } else { WikiLink }.into());
        token(
            b,
            if a < open { EmbedOpen } else { WikiOpen },
            text,
            a,
            inner_a,
        );
        token(b, Target, text, inner_a, target);
        token(b, Anchor, text, target, anchor_end);
        if let Some(p) = pipe {
            token(b, Pipe, text, p, p + 1);
            token(b, Alias, text, p + 1, inner_z);
        }
        token(b, WikiClose, text, inner_z, z);
        b.finish_node();
    }

    /// The `]` matching the `[` at `a`, before `z`.
    fn close_bracket(&self, a: usize, z: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (k, c) in self.text[a..z].bytes().enumerate() {
            match c {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(a + k);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// `[text](dest)` with its `[` at `a`: where it ends.
    fn link(&self, a: usize, z: usize) -> Option<usize> {
        let close = self.close_bracket(a, z)?;
        if self.text.as_bytes().get(close + 1) != Some(&b'(') || close + 1 >= z {
            return None;
        }
        let paren = self.text[close + 2..z].find(')')? + close + 2;
        Some(paren + 1)
    }

    /// Dataview's `[key:: value]` or `(key:: value)` at `a`: where it ends.
    fn field(&self, a: usize, z: usize) -> Option<usize> {
        let t = self.text.as_bytes();
        let (open, close) = match t[a] {
            b'[' => (b'[', b']'),
            _ => (b'(', b')'),
        };
        let colons = self.key(a + 1, z)?;
        let mut depth = 1usize;
        for (k, &c) in t[colons + 2..z].iter().enumerate() {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(colons + 2 + k + 1);
                }
            }
        }
        None
    }

    /// A field's key from `a`: where its `::` is. A key is words, not brackets or code.
    fn key(&self, a: usize, z: usize) -> Option<usize> {
        let rest = &self.text[a..z];
        let k = rest.find("::")?;
        let key = &rest[..k];
        let ok = !key.trim().is_empty()
            && !key.starts_with([' ', '\t'])
            && !key.contains(['[', ']', '(', ')', '`', '|', ':', '#']);
        ok.then_some(a + k)
    }

    /// Dataview's `key:: value` taking the line from `a`.
    fn line_field(&self, a: usize, z: usize) -> Option<usize> {
        self.key(a, z).map(|_| z)
    }

    /// `$x$` or `$$x$$` at `a`: where it ends.
    fn math(&self, a: usize, z: usize) -> Option<usize> {
        let t = self.text.as_bytes();
        if t.get(a + 1) == Some(&b'$') {
            return self.text[a + 2..z].find("$$").map(|k| a + 2 + k + 2);
        }
        if t.get(a + 1).is_none_or(|c| c.is_ascii_whitespace()) || a + 1 >= z {
            return None;
        }
        let mut i = a + 1;
        while i < z {
            if t[i] == b'$' {
                let before = t[i - 1];
                let after = t.get(i + 1).filter(|_| i + 1 < z);
                if !before.is_ascii_whitespace() && after.is_none_or(|c| !c.is_ascii_digit()) {
                    return Some(i + 1);
                }
            }
            i += 1;
        }
        None
    }

    /// A block id, ` ^id` closing the line, with its `^` at `i`: where the id ends.
    fn block_id(&self, a: usize, i: usize, z: usize) -> Option<usize> {
        let t = self.text.as_bytes();
        if i == a || !matches!(t[i - 1], b' ' | b'\t') {
            return None;
        }
        let n = t[i + 1..z]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-')
            .count();
        let end = i + 1 + n;
        (n > 0 && t[end..z].iter().all(|c| *c == b' ' || *c == b'\t')).then_some(end)
    }
}
