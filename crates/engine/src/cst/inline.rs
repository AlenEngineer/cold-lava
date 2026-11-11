//! A line of a block's text, read left to right into code spans, wikilinks and embeds, Markdown
//! links and images (inline, or by reference to one of the note's link definitions), Dataview
//! fields, comments, math, tags and block ids; the rest is text. What starts first wins, so a
//! wikilink inside a code span stays code. Emphasis and the like stay text: nothing reads them
//! from the tree yet, but a tag may follow their opening markers (`**#tag**`). A link's text
//! may go on to a later line of its block: its `](destination)` there is read (`LinkEnd`). A
//! link's `)` comes after its destination and its title, either of which may hold parentheses.

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

use super::Kind::{self, *};
use super::block::Ev;
use super::{Definitions, Gates, Sink};
use crate::markdown::Syntax as S;

/// Build `events` into `b`, reading each line of text as it comes; `defs` are the link
/// definitions of the whole note, which make a `[label]` a link.
pub(super) fn build<'t>(
    text: &'t str,
    events: &[Ev],
    gates: Gates,
    defs: &Definitions,
    b: &mut impl Sink<'t>,
) {
    // A code span an earlier line of the paragraph opened: its closing backticks.
    let mut code: Option<Range<usize>> = None;
    // How many `[` the block's earlier lines left open.
    let mut open = 0;
    // Whether the text read is a table cell's.
    let mut cell = false;
    for (k, ev) in events.iter().enumerate() {
        match *ev {
            Ev::Start(kind) => {
                open = 0;
                cell = kind == TableCell;
                b.start(kind)
            }
            Ev::Finish => {
                open = 0;
                cell = false;
                b.finish()
            }
            Ev::Tok(kind, a, z) => b.token(kind, &text[a..z]),
            Ev::Inline(a, z) => {
                let line = Line::new(text, gates, defs, cell, a, z);
                let from = match code.take() {
                    Some(close) if close.start >= z => {
                        token(b, CodeText, text, a, z);
                        code = Some(close);
                        continue;
                    }
                    Some(close) => {
                        token(b, CodeText, text, a, close.start);
                        token(b, Backticks, text, close.start, close.end);
                        b.finish();
                        close.end
                    }
                    None => match line.line_field(a, z) {
                        Some(end) => {
                            line.emit(b, LineField, a, end);
                            continue;
                        }
                        None => a,
                    },
                };
                code = line.run(b, from, z, false, &events[k + 1..], &mut open);
            }
            Ev::Yaml(a, z) => yaml(text, gates, defs, b, a, z),
        }
    }
}

fn token<'t>(b: &mut impl Sink<'t>, kind: Kind, text: &'t str, a: usize, z: usize) {
    if a < z {
        b.token(kind, &text[a..z]);
    }
}

/// A frontmatter line: its wikilinks, which count as the note's links, and text.
fn yaml<'t>(
    text: &'t str,
    gates: Gates,
    defs: &Definitions,
    b: &mut impl Sink<'t>,
    a: usize,
    z: usize,
) {
    let line = Line {
        text,
        g: gates,
        defs,
        cell: false,
        tags: Vec::new(),
        colons: false,
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

struct Line<'t, 'd> {
    text: &'t str,
    g: Gates,
    /// The note's link definitions.
    defs: &'d Definitions,
    /// Whether the line is a table cell, where the pipe before a wikilink's alias is written
    /// `\|`, as Obsidian writes it, so that it doesn't end the cell.
    cell: bool,
    /// The tags on this line, from their `#` to their end, found as they always were: by the
    /// tag pattern over the whole line, the markers before it included.
    tags: Vec<Range<usize>>,
    /// Whether the line has a `::`, without which it has no Dataview field.
    colons: bool,
}

fn tag_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(crate::markdown::MARKED_TAG).unwrap())
}

/// Whether `markers`, written between what a tag may follow and its `#`, open emphasis (`*`,
/// `_`), a strikethrough (`~~`) or a highlight (`==`) in the syntax `g`. The tag after them is
/// then a tag, as in Obsidian.
fn opens(markers: &str, g: Gates) -> bool {
    let m = markers.as_bytes();
    let mut i = 0;
    while i < m.len() {
        let run = m[i..].iter().take_while(|&&c| c == m[i]).count();
        let ok = match m[i] {
            b'*' | b'_' => true,
            b'~' => run == 2 && g.has(S::Strikethrough),
            _ => run == 2 && g.has(S::Highlights),
        };
        if !ok {
            return false;
        }
        i += run;
    }
    true
}

impl<'t, 'd> Line<'t, 'd> {
    /// The line holding `at..z`, the text read.
    fn new(
        text: &'t str,
        g: Gates,
        defs: &'d Definitions,
        cell: bool,
        at: usize,
        z: usize,
    ) -> Self {
        let mut tags = Vec::new();
        if g.has(S::Tags) && text[at..z].contains('#') {
            let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
            let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
            let line = &text[start..end];
            for c in tag_pattern().captures_iter(line) {
                let name = c.get(1).expect("the pattern has a name");
                let hash = name.start() - 1;
                let before = line[..hash].trim_end_matches(['*', '_', '~', '=']);
                let markers = &line[before.len()..hash];
                if !opens(markers, g) {
                    continue;
                }
                // After `_`, the `_` that closes the emphasis isn't the name's.
                let named = match markers.contains('_') {
                    true => name.as_str().trim_end_matches('_'),
                    false => name.as_str(),
                };
                if !named.bytes().all(|b| b.is_ascii_digit()) {
                    let hash = start + hash;
                    tags.push(hash..hash + 1 + named.len());
                }
            }
        }
        let colons = text[at..z].contains("::");
        Line {
            text,
            g,
            defs,
            cell,
            tags,
            colons,
        }
    }

    /// Read `a..z`; `in_link` inside a link's text, where no other link starts. A code span
    /// that the line doesn't close may close on one of the block's `later` lines: then it is
    /// left open at the line's end, and its closing backticks are what this returns. `open`
    /// counts the `[` that nothing closed yet, on this line and the block's earlier ones: a
    /// `](destination)` closes one as a link.
    fn run(
        &self,
        b: &mut impl Sink<'t>,
        a: usize,
        z: usize,
        in_link: bool,
        later: &[Ev],
        open: &mut usize,
    ) -> Option<Range<usize>> {
        let t = self.text.as_bytes();
        let mut plain = a;
        let mut i = a;
        while i < z {
            let found = match t[i] {
                b'`' => {
                    let run = t[i..z].iter().take_while(|&&c| c == b'`').count();
                    if let Some(end) = self.code_span(i, run, z) {
                        Some((CodeSpan, end))
                    } else if let Some(close) = self.closing(run, later) {
                        token(b, Text, self.text, plain, i);
                        b.start(CodeSpan);
                        token(b, Backticks, self.text, i, i + run);
                        token(b, CodeText, self.text, i + run, z);
                        return Some(close);
                    } else {
                        i += run;
                        continue;
                    }
                }
                b'!' if t.get(i + 1) == Some(&b'[') => {
                    if t.get(i + 2) == Some(&b'[') && self.g.has(S::Embeds) {
                        self.wikilink(i + 1, z).map(|e| (Embed, e))
                    } else {
                        // A link's text may hold an image, though no other link.
                        let end = self.link(i + 1, z).or_else(|| self.reference(i + 1, z));
                        end.map(|e| (Image, e))
                    }
                }
                b'[' if t.get(i + 1) == Some(&b'[') => match self.g.has(S::WikiLinks) {
                    true => self.wikilink(i, z).map(|e| (WikiLink, e)),
                    false => None,
                },
                b'[' => {
                    let field = self.field(i, z).map(|e| (InlineField, e));
                    let found = field.or_else(|| match in_link {
                        true => None,
                        false => {
                            let end = self.link(i, z).or_else(|| self.reference(i, z));
                            end.map(|e| (Link, e))
                        }
                    });
                    *open += (found.is_none() && !in_link) as usize;
                    found
                }
                b']' if !in_link && *open > 0 => {
                    *open -= 1;
                    self.link_end(i, z).map(|e| (LinkEnd, e))
                }
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
        None
    }

    /// The backticks closing a code span of `run` backticks on one of the block's `later`
    /// lines, which end where it does.
    fn closing(&self, run: usize, later: &[Ev]) -> Option<Range<usize>> {
        let t = self.text.as_bytes();
        for ev in later {
            match *ev {
                Ev::Inline(a, z) => {
                    let mut i = a;
                    while i < z {
                        let n = t[i..z].iter().take_while(|&&c| c == b'`').count();
                        if n == run {
                            return Some(i..i + n);
                        }
                        i += n.max(1);
                    }
                }
                Ev::Tok(..) => {}
                _ => return None,
            }
        }
        None
    }

    fn emit(&self, b: &mut impl Sink<'t>, kind: Kind, a: usize, z: usize) {
        let text = self.text;
        match kind {
            Tag | BlockId => token(b, kind, text, a, z),
            CodeSpan => {
                let run = text[a..z].bytes().take_while(|&c| c == b'`').count();
                b.start(CodeSpan);
                token(b, Backticks, text, a, a + run);
                token(b, CodeText, text, a + run, z - run);
                token(b, Backticks, text, z - run, z);
                b.finish();
            }
            WikiLink => self.emit_wikilink(b, a, a, z),
            Embed => self.emit_wikilink(b, a, a + 1, z),
            Link | Image => {
                let open = if kind == Image { a + 2 } else { a + 1 };
                let close = self.close_bracket(open - 1, z).expect("a link has its `]`");
                let after = close + 1;
                b.start(kind);
                let opener = if kind == Image { ImageOpen } else { LBracket };
                token(b, opener, text, a, open);
                if after < z && text.as_bytes()[after] == b'(' {
                    // `link` ended it at its `)`.
                    let paren = z - 1;
                    b.start(LinkText);
                    self.run(b, open, close, true, &[], &mut 0);
                    b.finish();
                    token(b, RBracket, text, close, after);
                    token(b, LParen, text, after, after + 1);
                    token(b, LinkDest, text, after + 1, paren);
                    token(b, RParen, text, paren, paren + 1);
                } else if after + 2 < z {
                    // `[text][label]`.
                    b.start(LinkText);
                    self.run(b, open, close, true, &[], &mut 0);
                    b.finish();
                    token(b, RBracket, text, close, after);
                    token(b, LBracket, text, after, after + 1);
                    token(b, LinkLabel, text, after + 1, z - 1);
                    token(b, RBracket, text, z - 1, z);
                } else {
                    // `[label]` or `[label][]`: the label is what the link shows.
                    token(b, LinkLabel, text, open, close);
                    token(b, RBracket, text, close, after);
                    if after < z {
                        token(b, LBracket, text, after, after + 1);
                        token(b, RBracket, text, after + 1, z);
                    }
                }
                b.finish();
            }
            LinkEnd => {
                b.start(LinkEnd);
                token(b, RBracket, text, a, a + 1);
                token(b, LParen, text, a + 1, a + 2);
                token(b, LinkDest, text, a + 2, z - 1);
                token(b, RParen, text, z - 1, z);
                b.finish();
            }
            InlineField | LineField => {
                let inline = kind == InlineField;
                let key = if inline { a + 1 } else { a };
                let colons = key + text[key..z].find("::").expect("a field has its `::`");
                let value_end = if inline { z - 1 } else { z };
                b.start(kind);
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
                self.run(b, colons + 2, value_end, false, &[], &mut 0);
                if inline {
                    let close = if text.as_bytes()[a] == b'[' {
                        RBracket
                    } else {
                        RParen
                    };
                    token(b, close, text, z - 1, z);
                }
                b.finish();
            }
            Comment => {
                b.start(Comment);
                token(b, CommentMarker, text, a, a + 2);
                token(b, CommentText, text, a + 2, z - 2);
                token(b, CommentMarker, text, z - 2, z);
                b.finish();
            }
            InlineMath => {
                let run = if text[a..z].starts_with("$$") { 2 } else { 1 };
                b.start(InlineMath);
                token(b, MathMarker, text, a, a + run);
                token(b, MathText, text, a + run, z - run);
                token(b, MathMarker, text, z - run, z);
                b.finish();
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
    /// `^…`, then `|alias`. In a table cell the pipe may be written `\|`.
    fn emit_wikilink(&self, b: &mut impl Sink<'t>, a: usize, open: usize, z: usize) {
        let text = self.text;
        let (inner_a, inner_z) = (open + 2, z - 2);
        let bar = text[inner_a..inner_z].find('|').map(|k| inner_a + k);
        let pipe = bar.map(|p| p - (self.cell && text[inner_a..p].ends_with('\\')) as usize);
        let anchor_end = pipe.unwrap_or(inner_z);
        let named = &text[inner_a..anchor_end];
        let target = named.find(['#', '^']).unwrap_or(named.len()) + inner_a;
        b.start(if a < open { Embed } else { WikiLink });
        token(
            b,
            if a < open { EmbedOpen } else { WikiOpen },
            text,
            a,
            inner_a,
        );
        token(b, Target, text, inner_a, target);
        token(b, Anchor, text, target, anchor_end);
        if let (Some(p), Some(bar)) = (pipe, bar) {
            token(b, Pipe, text, p, bar + 1);
            token(b, Alias, text, bar + 1, inner_z);
        }
        token(b, WikiClose, text, inner_z, z);
        b.finish();
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
        Some(self.close_paren(close + 2, z)? + 1)
    }

    /// The `)` that ends a link whose `(` is just before `p`, before `z`: the one after its
    /// destination (in `<>`, or with its parentheses balanced) and its title (`"…"`, `'…'` or
    /// `(…)`), as in CommonMark. Where that isn't what is written, the first `)`, as it was
    /// always read.
    fn close_paren(&self, p: usize, z: usize) -> Option<usize> {
        let t = self.text.as_bytes();
        let first = || self.text[p..z].find(')').map(|k| p + k);
        let blanks = |mut i: usize| {
            while i < z && matches!(t[i], b' ' | b'\t') {
                i += 1;
            }
            i
        };
        let mut i = blanks(p);
        if i < z && t[i] == b'<' {
            match self.text[i..z].find('>') {
                Some(k) => i += k + 1,
                None => return first(),
            }
        } else {
            let mut depth = 0usize;
            while i < z && !matches!(t[i], b' ' | b'\t') {
                match t[i] {
                    b'\\' => i += 1,
                    b'(' => depth += 1,
                    b')' if depth == 0 => break,
                    b')' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth > 0 {
                return first();
            }
        }
        let after = blanks(i);
        if after >= z {
            return first();
        }
        let closes = match t[after] {
            b')' => return Some(after),
            b'"' | b'\'' if after > i => t[after],
            b'(' if after > i => b')',
            _ => return first(),
        };
        let mut end = after + 1;
        while end < z && t[end] != closes {
            end += 1 + (t[end] == b'\\') as usize;
        }
        let paren = blanks(end + 1);
        match paren < z && t[paren] == b')' {
            true => Some(paren),
            false => first(),
        }
    }

    /// `](dest)` with its `]` at `a`, closing a link whose `[` an earlier line left open:
    /// where it ends.
    fn link_end(&self, a: usize, z: usize) -> Option<usize> {
        if self.text.as_bytes().get(a + 1) != Some(&b'(') || a + 1 >= z {
            return None;
        }
        Some(self.close_paren(a + 2, z)? + 1)
    }

    /// A link by reference with its `[` at `a`, `[text][label]`, `[label][]` or `[label]`,
    /// where the note defines the label: where it ends. Without a definition the brackets are
    /// text, as in CommonMark.
    fn reference(&self, a: usize, z: usize) -> Option<usize> {
        if self.defs.is_empty() {
            return None;
        }
        let close = self.close_bracket(a, z)?;
        let after = close + 1;
        let own = &self.text[a + 1..close];
        let rest = &self.text[after..z];
        if rest.starts_with('[')
            && let Some(k) = rest[1..].find(']')
        {
            if k == 0 {
                return self.defs.get(own).map(|_| after + 2);
            }
            if !rest[1..1 + k].trim().is_empty() {
                return self.defs.get(&rest[1..1 + k]).map(|_| after + k + 2);
            }
        }
        self.defs.get(own).map(|_| after)
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
        if !self.colons {
            return None;
        }
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
