//! A line of a block's text, read left to right into code spans, wikilinks and embeds, Markdown
//! links and images (inline, or by reference to one of the note's link definitions), Dataview
//! fields, comments, math, tags and block ids; the rest is text. What starts first wins, so a
//! wikilink inside a code span stays code. A link's text may go on to a later line of its
//! block: its `](destination)` there is read (`LinkEnd`). A link's `)` comes after its
//! destination and its title, either of which may hold parentheses.
//!
//! Math is what pulldown-cmark reads as math (`Dollars`): `$…$` and `$$…$$` in one block's
//! text, on one line or over several, closed by the block's next `$` in the same braces or
//! not at all. It is never a block of its own, and never goes on past its block.
//!
//! What is left as text is then read for what text itself holds (`Held`, below): escapes,
//! character references, autolinks, inline HTML, footnotes, hard line breaks, and emphasis, a
//! block at a time, since emphasis may open on one line and close on a later one. That reading
//! only cuts `Text` tokens (and a wikilink's `Alias` and the `LinkLabel` a link by reference
//! shows, which are text on screen) into more tokens and wraps them in nodes: the links, the
//! tags and everything else found first stay as they were found, so a reader that doesn't ask
//! for it (`Gates::plain`) reads the same links and tags. A tag may follow the markers that
//! open emphasis (`**#tag**`) either way.

use std::cell::OnceCell;
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
    // A reader that doesn't ask for what text holds is read into straight.
    match gates.text() {
        true => lines(text, events, gates, defs, &mut Held::new(text, gates, b)),
        false => lines(text, events, gates, defs, b),
    }
}

/// `build`, into the sink the lines are read into.
fn lines<'t>(
    text: &'t str,
    events: &[Ev],
    gates: Gates,
    defs: &Definitions,
    b: &mut impl Sink<'t>,
) {
    // The code span or the math an earlier line of the paragraph opened.
    let mut code: Option<Unclosed> = None;
    // The `$`s of the block being read, once one is asked about.
    let mut dollars = Dollars::default();
    // How many `[` the block's earlier lines left open.
    let mut open = 0;
    // Whether the text read is a table cell's.
    let mut cell = false;
    for (k, ev) in events.iter().enumerate() {
        match *ev {
            Ev::Start(kind) => {
                open = 0;
                cell = kind == TableCell;
                dollars = Dollars::default();
                b.block();
                b.start(kind)
            }
            Ev::Finish => {
                open = 0;
                cell = false;
                dollars = Dollars::default();
                b.block();
                b.finish()
            }
            Ev::Tok(kind, a, z) => b.token(kind, &text[a..z]),
            // Math keeps the indentation of the lines it goes on over, as written.
            Ev::Indent(a, z) => match &code {
                Some(math) if math.text == MathText => b.token(MathText, &text[a..z]),
                _ => b.token(Whitespace, &text[a..z]),
            },
            Ev::Inline(a, z) => {
                b.line(a, z);
                let block = Block {
                    events,
                    at: k,
                    dollars: &dollars,
                };
                let line = Line::new(text, gates, defs, cell, block, a, z);
                let from = match code.take() {
                    Some(left) if left.close.start >= z => {
                        token(b, left.text, text, a, z);
                        code = Some(left);
                        continue;
                    }
                    Some(left) => {
                        token(b, left.text, text, a, left.close.start);
                        token(b, left.marker, text, left.close.start, left.close.end);
                        b.finish();
                        left.close.end
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
    b.block();
}

fn token<'t>(b: &mut (impl Sink<'t> + ?Sized), kind: Kind, text: &'t str, a: usize, z: usize) {
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
        block: Block {
            events: &[],
            at: 0,
            dollars: &Dollars::default(),
        },
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

/// A code span or math that its line left open: the kinds of its text and of its closing
/// marker, and where that marker is, on a later line of the block.
struct Unclosed {
    text: Kind,
    marker: Kind,
    close: Range<usize>,
}

/// The block a line is in: the note's events, where the line is among them, and the block's
/// `$`s.
#[derive(Clone, Copy)]
struct Block<'d> {
    events: &'d [Ev],
    at: usize,
    dollars: &'d Dollars,
}

/// A `$` of a block's text that no backslash escapes.
#[derive(Clone, Copy)]
struct Dollar {
    at: usize,
    /// Something that isn't blank follows it, so it may open math.
    opens: bool,
    /// Something that isn't blank comes before it on its line, so it may close math.
    closes: bool,
    /// The braces it is in: math opens and closes in the same ones.
    braces: u8,
}

/// The `$`s of one block's text, found the first time one is asked about, and the braces
/// around each, numbered as pulldown-cmark numbers them (its `brace_context`). A block with
/// no `$` outside what starts before it (code, a link's destination) never pays for this.
#[derive(Default)]
struct Dollars(OnceCell<Vec<Dollar>>);

impl Block<'_> {
    fn dollars(&self, text: &str) -> &[Dollar] {
        self.dollars.0.get_or_init(|| {
            // The block's text is the lines around this one that no node's start or end
            // parts from it.
            let part = |ev: &&Ev| matches!(ev, Ev::Inline(..) | Ev::Tok(..) | Ev::Indent(..));
            let (before, after) = self.events.split_at(self.at.min(self.events.len()));
            let from = self.at - before.iter().rev().take_while(part).count();
            let to = self.at + after.iter().take_while(part).count();
            Dollars::find(text, &self.events[from..to])
        })
    }
}

impl Dollars {
    /// How deep pulldown-cmark tells braces apart; deeper, it only counts them.
    const DEEP: usize = 25;

    fn find(text: &str, block: &[Ev]) -> Vec<Dollar> {
        let t = text.as_bytes();
        let blank = |c: Option<&u8>| c.is_none_or(|&c| matches!(c, b'\t'..=b'\r' | b' '));
        let mut found = Vec::new();
        // The braces open, each by its number, under the block's own, and the next number.
        let (mut stack, mut next): (Vec<u8>, usize) = (Vec::new(), 0);
        for ev in block {
            let &Ev::Inline(a, z) = ev else { continue };
            let mut i = a;
            while i < z {
                match t[i] {
                    b'\\' if i + 1 < z && t[i + 1].is_ascii_punctuation() => i += 1,
                    b'$' => {
                        // Braces count from the block's first `$` on.
                        if stack.is_empty() {
                            stack.push(!0);
                        }
                        let braces = match stack.len() > Self::DEEP {
                            true => next as u8,
                            false => stack[stack.len() - 1],
                        };
                        found.push(Dollar {
                            at: i,
                            opens: !blank(t.get(i + 1)),
                            closes: i > a && !blank(t.get(i - 1)),
                            braces,
                        });
                    }
                    b'{' => {
                        if stack.len() == Self::DEEP {
                            stack.push(next as u8);
                            next = Self::DEEP;
                        } else if stack.len() > Self::DEEP {
                            next += 1;
                        } else if !stack.is_empty() {
                            stack.push(next as u8);
                            next += 1;
                        }
                    }
                    // One that closes nothing parts what is before it from what is after.
                    b'}' if stack.len() == 1 => stack[0] = stack[0].wrapping_sub(1),
                    b'}' if stack.len() > Self::DEEP && next > Self::DEEP => next -= 1,
                    b'}' => {
                        stack.pop();
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        found
    }
}

struct Line<'t, 'd> {
    text: &'t str,
    g: Gates,
    /// The note's link definitions.
    defs: &'d Definitions,
    /// The block the line is in.
    block: Block<'d>,
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
        block: Block<'d>,
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
            block,
            tags,
            colons,
        }
    }

    /// Read `a..z`; `in_link` inside a link's text, where no other link starts. A code span
    /// or math that the line doesn't close may close on one of the block's `later` lines: then
    /// it is left open at the line's end, and its closing marker is what this returns. `open`
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
    ) -> Option<Unclosed> {
        let t = self.text.as_bytes();
        let mut plain = a;
        let mut i = a;
        // Where the HTML tag or the autolink being read ends: a `$` in it opens no math, as
        // pulldown-cmark, which reads the tag first, has it.
        let mut tag = a;
        while i < z {
            let found = match t[i] {
                // An escaped character starts nothing and ends nothing.
                b'\\' if i + 1 < z && b"\\[]`$%!*_#=~|".contains(&t[i + 1]) => {
                    i += 2;
                    continue;
                }
                b'`' => {
                    let run = t[i..z].iter().take_while(|&&c| c == b'`').count();
                    if let Some(end) = self.code_span(i, run, z) {
                        Some((CodeSpan, end))
                    } else if let Some(close) = self.closing(run, later) {
                        token(b, Text, self.text, plain, i);
                        b.start(CodeSpan);
                        token(b, Backticks, self.text, i, i + run);
                        token(b, CodeText, self.text, i + run, z);
                        return Some(Unclosed {
                            text: CodeText,
                            marker: Backticks,
                            close,
                        });
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
                            // A link's text holds no link: the one inside is the link.
                            let end = end.filter(|_| !self.holds_link(i, z));
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
                b'<' if self.g.has(S::Math) && i >= tag && self.text[i..z].contains('$') => {
                    if let Some((_, len)) = angle(&self.text[i..z]) {
                        tag = i + len;
                    }
                    None
                }
                b'$' if self.g.has(S::Math) && i >= tag => match self.math(i) {
                    Some(close) if close.end <= z => Some((InlineMath, close.end)),
                    // It closes on a later line of the block.
                    Some(close) if !later.is_empty() => {
                        let run = close.len();
                        token(b, Text, self.text, plain, i);
                        b.start(InlineMath);
                        token(b, MathMarker, self.text, i, i + run);
                        token(b, MathText, self.text, i + run, z);
                        return Some(Unclosed {
                            text: MathText,
                            marker: MathMarker,
                            close,
                        });
                    }
                    _ => None,
                },
                b'#' => (self.tags.iter())
                    .find(|r| r.start == i && r.end <= z)
                    .map(|r| (Tag, r.end)),
                b'^' if self.g.has(S::BlockIds) => self.block_id(a, i, z).map(|e| (BlockId, e)),
                _ => None,
            };
            // Math that opens in a link's text and closes after the link is the math, and the
            // link is none.
            let found = found.filter(|&(kind, end)| !self.math_leaves(kind, i, end));
            match found {
                Some((kind, end)) => {
                    // A link ends the brackets open before it: none of them is a link's.
                    if !in_link && matches!(kind, Link | WikiLink) {
                        *open = 0;
                    }
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
                Ev::Tok(..) | Ev::Indent(..) => {}
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

    /// Whether the text between the `[` at `a` and its `]` holds a link: a wikilink, a
    /// Markdown link or one by reference, outside code. An image isn't one.
    fn holds_link(&self, a: usize, z: usize) -> bool {
        let Some(close) = self.close_bracket(a, z) else {
            return false;
        };
        let t = self.text.as_bytes();
        let mut i = a + 1;
        while i < close {
            let next = t.get(i + 1).copied();
            match t[i] {
                b'\\' => i += 2,
                b'`' => {
                    let run = t[i..close].iter().take_while(|&&c| c == b'`').count();
                    i = self.code_span(i, run, close).unwrap_or(i + run);
                }
                b'!' if next == Some(b'[') => {
                    let image = match t.get(i + 2) == Some(&b'[') && self.g.has(S::Embeds) {
                        true => self.wikilink(i + 1, close),
                        false => (self.link(i + 1, close)).or_else(|| self.reference(i + 1, close)),
                    };
                    i = image.unwrap_or(i + 2);
                }
                b'[' if next == Some(b'[') && self.g.has(S::WikiLinks) => {
                    match self.wikilink(i, close) {
                        Some(_) => return true,
                        None => i += 2,
                    }
                }
                b'[' if self.link(i, close).is_some() || self.reference(i, close).is_some() => {
                    return true;
                }
                _ => i += 1,
            }
        }
        false
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

    /// Math opened by the `$` at `a`, `$x$` or `$$x$$`: its closing marker, which may be on
    /// a later line of the block. As pulldown-cmark pairs them: the `$` may open, and the
    /// block's next `$` in the same braces closes it or nothing does. `$$` is closed by `$$`;
    /// a single `$` by one with something before it on its line.
    fn math(&self, a: usize) -> Option<Range<usize>> {
        let d = self.block.dollars(self.text);
        let n = d.binary_search_by_key(&a, |d| d.at).ok()?;
        if !d[n].opens {
            return None;
        }
        let double = |n: usize| d.get(n + 1).is_some_and(|next| next.at == d[n].at + 1);
        let display = double(n);
        let from = n + 1 + display as usize;
        let k = from
            + d.get(from..)?
                .iter()
                .position(|c| c.braces == d[n].braces)?;
        let closes = match display {
            true => double(k),
            false => d[k].closes,
        };
        closes.then_some(d[k].at..d[k].at + 1 + display as usize)
    }

    /// Whether math opens in the text of the link or the embed `kind` at `a..z` and closes
    /// after it: then the math is read, as pulldown-cmark reads it, and the brackets are text.
    fn math_leaves(&self, kind: Kind, a: usize, z: usize) -> bool {
        if !self.g.has(S::Math) || !matches!(kind, Link | Image | WikiLink | Embed) {
            return false;
        }
        // A link's destination is the link's: only its text may open math.
        let text = match kind {
            Link => self.close_bracket(a, z),
            Image => self.close_bracket(a + 1, z),
            _ => None,
        };
        let end = text.unwrap_or(z);
        if !self.text[a..end].contains('$') {
            return false;
        }
        let d = self.block.dollars(self.text);
        let mut n = d.partition_point(|d| d.at < a);
        while n < d.len() && d[n].at < end {
            match self.math(d[n].at) {
                Some(close) if close.start >= end => return true,
                Some(close) => n = d.partition_point(|d| d.at < close.end),
                None => n += 1,
            }
        }
        false
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

/// One step of a block's text, held until the block ends.
#[derive(Clone, Copy)]
enum Rec {
    Start(Kind),
    Finish,
    Tok(Kind, usize, usize),
    /// A line of the block's text starts: where it is.
    Line(usize, usize),
}

/// What a piece of a `Text` token becomes.
#[derive(Clone, Copy)]
enum Piece {
    /// A token of this kind.
    Token(Kind),
    /// A whole node of this kind: an autolink or a footnote's reference.
    Node(Kind),
    /// The marker that opens a node of this kind.
    Open(Kind),
    /// The marker that closes the node its opener opened.
    Close,
}

#[derive(Clone, Copy)]
struct Cut {
    a: usize,
    z: usize,
    piece: Piece,
}

/// A run of `*`, `_`, `~` or `=` that may open or close a span.
#[derive(Clone, Copy)]
struct Run {
    a: usize,
    len: usize,
    ch: u8,
    open: bool,
    close: bool,
    /// The inline node whose text it is in: a span opens and closes in the same one.
    scope: usize,
}

/// The sink a block's lines are read into. A block whose text may hold more than the line
/// reader finds (an escape, a character reference, `<`, a footnote, a hard break, emphasis) is
/// held back until it ends: emphasis may close on a later line than it opened on. Its `Text`
/// tokens, a wikilink's `Alias` and the `LinkLabel` a link by reference shows are then cut
/// where those are, and it goes on to the sink. Nothing else of it changes.
///
/// It holds its sink as a trait object, so that the line reader is compiled once for it and
/// not once more for each reader's sink: a reader that doesn't ask for what text holds keeps
/// the code it had.
struct Held<'s, 't> {
    text: &'t str,
    g: Gates,
    b: &'s mut dyn Sink<'t>,
    recs: Vec<Rec>,
    hold: bool,
}

impl<'t> Sink<'t> for Held<'_, 't> {
    fn start(&mut self, kind: Kind) {
        match self.hold {
            true => self.recs.push(Rec::Start(kind)),
            false => self.b.start(kind),
        }
    }

    fn finish(&mut self) {
        match self.hold {
            true => self.recs.push(Rec::Finish),
            false => self.b.finish(),
        }
    }

    fn token(&mut self, kind: Kind, text: &'t str) {
        if self.hold {
            // Every token is a piece of the note's text.
            let a = text.as_ptr() as usize - self.text.as_ptr() as usize;
            self.recs.push(Rec::Tok(kind, a, a + text.len()));
        } else {
            self.b.token(kind, text);
        }
    }

    /// A block starts, or one ends: what was held goes on.
    fn block(&mut self) {
        self.release();
    }

    /// A line of the block's text, `a..z`, is about to be read.
    fn line(&mut self, a: usize, z: usize) {
        if !self.hold && !may_hold_more(&self.text[a..z]) {
            return;
        }
        self.hold = true;
        self.recs.push(Rec::Line(a, z));
    }
}

impl<'s, 't> Held<'s, 't> {
    fn new(text: &'t str, g: Gates, b: &'s mut dyn Sink<'t>) -> Self {
        Held {
            text,
            g,
            b,
            recs: Vec::new(),
            hold: false,
        }
    }

    /// What was held goes on to the sink, its text cut.
    fn release(&mut self) {
        if !self.hold {
            return;
        }
        self.hold = false;
        let recs = std::mem::take(&mut self.recs);
        let cuts = cuts(self.text, self.g, &recs);
        let text = self.text;
        let mut cuts = cuts.iter().peekable();
        for rec in &recs {
            match *rec {
                Rec::Start(kind) => self.b.start(kind),
                Rec::Finish => self.b.finish(),
                Rec::Line(..) => {}
                Rec::Tok(kind @ (Text | Alias | LinkLabel), a, z) => {
                    let mut at = a;
                    while let Some(cut) = cuts.next_if(|c| c.a < z) {
                        token(&mut *self.b, kind, text, at, cut.a);
                        self.piece(cut);
                        at = cut.z;
                    }
                    token(&mut *self.b, kind, text, at, z);
                }
                Rec::Tok(kind, a, z) => self.b.token(kind, &text[a..z]),
            }
        }
        self.recs = recs;
        self.recs.clear();
    }

    fn piece(&mut self, cut: &Cut) {
        let (text, b) = (self.text, &mut *self.b);
        let (a, z) = (cut.a, cut.z);
        match cut.piece {
            Piece::Token(kind) => token(b, kind, text, a, z),
            Piece::Open(kind) => {
                b.start(kind);
                token(b, EmphasisMarker, text, a, z);
            }
            Piece::Close => {
                token(b, EmphasisMarker, text, a, z);
                b.finish();
            }
            Piece::Node(Autolink) => {
                b.start(Autolink);
                token(b, AngleOpen, text, a, a + 1);
                token(b, Url, text, a + 1, z - 1);
                token(b, AngleClose, text, z - 1, z);
                b.finish();
            }
            // `[^label]`.
            Piece::Node(kind) => {
                b.start(kind);
                token(b, LBracket, text, a, a + 1);
                token(b, FootnoteLabel, text, a + 1, z - 1);
                token(b, RBracket, text, z - 1, z);
                b.finish();
            }
        }
    }
}

fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// Whether `line` may hold more than the line reader finds: one of the characters that starts
/// it, or the blanks of a hard break at its end.
fn may_hold_more(line: &str) -> bool {
    let t = line.as_bytes();
    let starts = |c: &u8| matches!(c, b'*' | b'_' | b'~' | b'=' | b'\\' | b'&' | b'<' | b'^');
    t.iter().any(starts) || matches!(t, [.., a, b] if is_blank(*a) && is_blank(*b))
}

/// A footnote's `[^label]` starting `s`: where its `]` is. A label holds no bracket, and
/// something that isn't blank.
fn footnote_label(s: &str) -> Option<usize> {
    let label = s.strip_prefix("[^")?;
    let k = label.find(['[', ']'])?;
    (label.as_bytes()[k] == b']' && !label[..k].trim().is_empty()).then_some(2 + k)
}

/// What is written as a character reference at the `&` that starts `t`: how long it is.
fn entity(t: &[u8]) -> Option<usize> {
    let (from, ok): (usize, fn(&u8) -> bool) = match t.get(1..3) {
        Some([b'#', b'x' | b'X']) => (3, u8::is_ascii_hexdigit),
        Some([b'#', _]) => (2, u8::is_ascii_digit),
        _ => (1, u8::is_ascii_alphanumeric),
    };
    let n = t[from.min(t.len())..].iter().take_while(|c| ok(c)).count();
    ((1..=32).contains(&n) && t.get(from + n) == Some(&b';')).then_some(from + n + 1)
}

/// An autolink or inline HTML at the `<` that starts `s`, as CommonMark defines them: which,
/// and how long it is.
fn angle(s: &str) -> Option<(Piece, usize)> {
    static RE: OnceLock<[Regex; 2]> = OnceLock::new();
    let [autolink, html] = RE.get_or_init(|| {
        let name = r"[A-Za-z0-9](?:[A-Za-z0-9\-]{0,61}[A-Za-z0-9])?";
        let autolink = [
            r"^<(?:[A-Za-z][A-Za-z0-9+.\-]{1,31}:[^\x00-\x20<>]*",
            r"|[A-Za-z0-9.!#$%&'*+/=?^_`{|}~\-]+@",
            name,
            r"(?:\.",
            name,
            r")*)>",
        ];
        let html = [
            r#"^<(?:[A-Za-z][A-Za-z0-9\-]*(?:\s+[A-Za-z_:][A-Za-z0-9_.:\-]*"#,
            r#"(?:\s*=\s*(?:[^\s"'=<>`]+|'[^']*'|"[^"]*"))?)*\s*/?>"#,
            r#"|/[A-Za-z][A-Za-z0-9\-]*\s*>"#,
            r#"|!---?>|!--(?s:.*?)-->"#,
            r#"|\?(?s:.*?)\?>"#,
            r#"|![A-Za-z][^>]*>"#,
            r#"|!\[CDATA\[(?s:.*?)\]\]>)"#,
        ];
        [
            Regex::new(&autolink.concat()).unwrap(),
            Regex::new(&html.concat()).unwrap(),
        ]
    });
    if let Some(m) = autolink.find(s) {
        return Some((Piece::Node(Autolink), m.end()));
    }
    html.find(s).map(|m| (Piece::Token(InlineHtml), m.end()))
}

/// Where the `Text` tokens of the held `recs` are cut, in order.
fn cuts(text: &str, g: Gates, recs: &[Rec]) -> Vec<Cut> {
    let t = text.as_bytes();
    let mut cuts: Vec<Cut> = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    // The inline nodes open, and whether each holds a link's text; the block's own text is
    // scope 0.
    let mut scopes: Vec<(usize, bool)> = Vec::new();
    let mut line = (0, 0);
    // The hard break the line just read ends with, if another line follows it.
    let mut hard: Option<(usize, usize)> = None;
    for (k, rec) in recs.iter().enumerate() {
        let (a, z) = match *rec {
            Rec::Start(kind) => {
                if let (LinkText, Some(link)) = (kind, scopes.last_mut()) {
                    link.1 = true;
                }
                scopes.push((k + 1, false));
                continue;
            }
            Rec::Finish => {
                scopes.pop();
                continue;
            }
            Rec::Line(a, z) => {
                if let Some((a, z)) = hard.take() {
                    let piece = Piece::Token(HardBreak);
                    cuts.push(Cut { a, z, piece });
                }
                line = (a, z);
                continue;
            }
            // What a wikilink shows in its target's place is read as text is, and so is the
            // label a link by reference shows, having no text of its own.
            Rec::Tok(Text | Alias, a, z) => (a, z),
            Rec::Tok(LinkLabel, a, z) if scopes.last().is_some_and(|s| !s.1) => (a, z),
            Rec::Tok(..) => continue,
        };
        let scope = scopes.last().map_or(0, |s| s.0);
        let mut i = a;
        while i < z {
            let c = t[i];
            let (len, piece) = match c {
                b'\\' if i + 1 < z && t[i + 1].is_ascii_punctuation() => {
                    (2, Some(Piece::Token(Escape)))
                }
                b'\\' if i + 1 == line.1 => {
                    hard = Some((i, i + 1));
                    (1, None)
                }
                b'&' => match entity(&t[i..z]) {
                    Some(n) => (n, Some(Piece::Token(Entity))),
                    None => (1, None),
                },
                b'<' => match angle(&text[i..z]) {
                    Some((piece, n)) => (n, Some(piece)),
                    None => (1, None),
                },
                b'[' if g.has(S::Footnotes) => match footnote_label(&text[i..z]) {
                    Some(close) => (close + 1, Some(Piece::Node(FootnoteRef))),
                    None => (1, None),
                },
                // Every two `=` of a run are a marker, left to right, as the reader always
                // split them; only the last may open, having something other than a marker
                // after it.
                b'=' => {
                    let len = t[i..z].iter().take_while(|&&x| x == c).count();
                    let markers = if g.has(S::Highlights) { len / 2 } else { 0 };
                    for k in 0..markers {
                        let a = i + 2 * k;
                        let (open, close) = flanks(text, line, a, 2, c);
                        let open = open && k + 1 == markers;
                        if open || close {
                            runs.push(Run {
                                a,
                                len: 2,
                                ch: c,
                                open,
                                close,
                                scope,
                            });
                        }
                    }
                    (len, None)
                }
                b'*' | b'_' | b'~' => {
                    let len = t[i..z].iter().take_while(|&&x| x == c).count();
                    let read = match c {
                        b'~' => g.has(S::Strikethrough) && len <= 2,
                        _ => true,
                    };
                    let (open, close) = match read {
                        true => flanks(text, line, i, len, c),
                        false => (false, false),
                    };
                    if open || close {
                        runs.push(Run {
                            a: i,
                            len,
                            ch: c,
                            open,
                            close,
                            scope,
                        });
                    }
                    (len, None)
                }
                _ => (1, None),
            };
            if let Some(piece) = piece {
                cuts.push(Cut {
                    a: i,
                    z: i + len,
                    piece,
                });
            }
            i += len;
        }
        if z == line.1 {
            let blanks = t[a..z].iter().rev().take_while(|c| is_blank(**c)).count();
            if blanks >= 2 {
                hard = Some((z - blanks, z));
            }
        }
    }
    spans(&runs, &mut cuts);
    cuts.sort_by_key(|c| c.a);
    cuts
}

/// Whether the run of `len` `ch`s at `a` on `line` may open a span and may close one, by
/// CommonMark's rules for delimiter runs as pulldown-cmark applied them: an opener is
/// followed by something that isn't blank, a closer follows something that isn't blank, and
/// `_` and a single `~` do neither inside a word. `==` only asks about the blanks.
fn flanks(text: &str, line: (usize, usize), a: usize, len: usize, ch: u8) -> (bool, bool) {
    let before = text[line.0..a].chars().next_back();
    let after = text[a + len..line.1].chars().next();
    if ch == b'=' {
        let blank = |c: Option<char>| c.is_none_or(char::is_whitespace);
        return (!blank(after), !blank(before));
    }
    let punct = |c: char| {
        c.is_ascii_punctuation() || !(c.is_ascii() || c.is_alphanumeric() || c.is_whitespace())
    };
    let in_words = ch == b'*' || (ch == b'~' && len > 1);
    let open = match (before, after) {
        (_, None) => false,
        (_, Some(n)) if n.is_whitespace() => false,
        (None, _) => true,
        (Some(p), Some(n)) => {
            (ch == b'*' && !punct(n)) || (ch == b'~' && len > 1) || p.is_whitespace() || punct(p)
        }
    };
    let close = match (before, after) {
        (None, _) => false,
        (Some(p), _) if p.is_whitespace() => false,
        (_, None) => true,
        (Some(p), Some(n)) => (in_words && !punct(p)) || n.is_whitespace() || punct(n),
    };
    (open, close)
}

/// Pairs `runs` into spans, each within the inline node that holds it: emphasis, strong and
/// strikethrough first, then the highlights among what those left.
fn spans(runs: &[Run], cuts: &mut Vec<Cut>) {
    let mut order: Vec<usize> = (0..runs.len()).collect();
    // Stable, so the runs of a scope stay in the order written.
    order.sort_by_key(|&k| runs[k].scope);
    let mut from = 0;
    while from < order.len() {
        let scope = runs[order[from]].scope;
        let same = order[from..]
            .iter()
            .take_while(|&&k| runs[k].scope == scope);
        let to = from + same.count();
        let inside = emphasis(runs, &order[from..to], cuts);
        highlights(runs, &order[from..to], &inside, cuts);
        from = to;
    }
}

/// A run that opened and nothing closed yet.
#[derive(Clone, Copy)]
struct Opener {
    /// Where its markers still free start, and how many they are.
    at: usize,
    count: usize,
    /// How long the run was written.
    run: usize,
    ch: u8,
    /// Whether it could close too.
    both: bool,
}

/// Pairs the `*`, `_` and `~` runs of one scope, `group`, as CommonMark does: a closer takes
/// the nearest opener of its character that the rule of three allows (a `~` run one of its own
/// length), and what opened between them stays text. Two markers make strong and one makes
/// emphasis, from the inside out. It returns what each span holds, between its markers.
fn emphasis(runs: &[Run], group: &[usize], cuts: &mut Vec<Cut>) -> Vec<(usize, usize)> {
    let mut inside = Vec::new();
    let mut stack: Vec<Opener> = Vec::new();
    // A `~` run that found no opener rules out, for the `~` runs after it, the openers there
    // were then: pulldown-cmark kept one such bound for every length of `~` run.
    let mut tildes = 0;
    for r in group.iter().map(|&k| runs[k]).filter(|r| r.ch != b'=') {
        let both = r.open && r.close;
        let (mut at, mut count) = (r.a, r.len);
        while r.close && count > 0 {
            let fits = |o: &Opener| {
                o.ch == r.ch
                    && (r.ch != b'~' || o.run == r.len)
                    && (!(both || o.both)
                        || !(r.len + o.run).is_multiple_of(3)
                        || r.len.is_multiple_of(3))
            };
            let from = match r.ch {
                b'~' => tildes.min(stack.len()),
                _ => 0,
            };
            let Some(found) = stack[from..].iter().rposition(fits) else {
                if r.ch == b'~' {
                    tildes = stack.len();
                }
                break;
            };
            let found = from + found;
            let o = stack[found];
            stack.truncate(found);
            tildes = tildes.min(found);
            let matched = count.min(o.count);
            let (mut open, mut close, mut left) = (o.at + o.count, at, matched);
            while left > 0 {
                let n = left.min(2);
                let kind = match (r.ch, n) {
                    (b'~', _) => Strikethrough,
                    (_, 2) => Strong,
                    _ => Emphasis,
                };
                cuts.push(Cut {
                    a: open - n,
                    z: open,
                    piece: Piece::Open(kind),
                });
                cuts.push(Cut {
                    a: close,
                    z: close + n,
                    piece: Piece::Close,
                });
                inside.push((open, close));
                (open, close, left) = (open - n, close + n, left - n);
            }
            if o.count > matched {
                stack.push(Opener {
                    count: o.count - matched,
                    ..o
                });
            }
            (at, count) = (at + matched, count - matched);
        }
        if count > 0 && r.open {
            stack.push(Opener {
                at,
                count,
                run: r.len,
                ch: r.ch,
                both,
            });
        }
    }
    inside
}

/// Pairs the `==` runs of one scope, `group`, left to right as the reader always did: a `==`
/// that may close takes the last one that may open, among those directly inside the same span
/// (`inside`: what the scope's spans hold).
fn highlights(runs: &[Run], group: &[usize], inside: &[(usize, usize)], cuts: &mut Vec<Cut>) {
    // The `==` open at each level: the span it is directly inside, if it is in one.
    let mut open: Vec<(Option<usize>, usize)> = Vec::new();
    for r in group.iter().map(|&k| runs[k]).filter(|r| r.ch == b'=') {
        let holds = |k: &usize| inside[*k].0 <= r.a && r.a < inside[*k].1;
        let level = (0..inside.len()).filter(holds).max_by_key(|&k| inside[k].0);
        match open.iter().position(|o| o.0 == level) {
            Some(k) if r.close => {
                let (_, a) = open.swap_remove(k);
                cuts.push(Cut {
                    a,
                    z: a + r.len,
                    piece: Piece::Open(Highlight),
                });
                cuts.push(Cut {
                    a: r.a,
                    z: r.a + r.len,
                    piece: Piece::Close,
                });
            }
            Some(k) if r.open => open[k].1 = r.a,
            None if r.open => open.push((level, r.a)),
            _ => {}
        }
    }
}
