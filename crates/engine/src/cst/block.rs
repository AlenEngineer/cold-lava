//! Blocks, line by line, the way CommonMark finds them: each line first continues the open
//! containers (block quotes, list items) it can, then closes the rest, then opens new blocks.
//! The result is a flat list of events in byte order; `inline` builds them into the tree.
//!
//! A container's markers on a later line (`>`, a list item's indentation) are tokens of the
//! innermost block still open when they are read, so a paragraph in a quote holds the `>` of its
//! second line. A line where nothing is open is a fresh start: the blocks from there on depend
//! only on the text from there on, which is what `Tree::edit` relies on.

use std::sync::OnceLock;

use regex::Regex;
use rowan::{GreenNode, GreenNodeBuilder, NodeOrToken};

use super::Kind::{self, *};
use super::{Definitions, Gates, inline};
use crate::markdown::Syntax as S;

/// One step of the tree, in byte order.
#[derive(Clone, Copy, Debug)]
pub(super) enum Ev {
    Start(Kind),
    Finish,
    Tok(Kind, usize, usize),
    /// A line of a block's text, read by `inline`.
    Inline(usize, usize),
    /// A frontmatter line, where only wikilinks are read.
    Yaml(usize, usize),
}

/// The whole note's tree, and its link definitions.
pub(super) fn document(text: &str, gates: Gates) -> (GreenNode, Definitions) {
    let (events, defs) = events(text, gates);
    (build(text, &events, gates, &defs), defs)
}

/// The whole note's blocks, as events, and its link definitions.
pub(super) fn events(text: &str, gates: Gates) -> (Vec<Ev>, Definitions) {
    let mut p = Parser::new(text, gates);
    let at = p.prefix();
    p.run(at, |_| false);
    (p.events, p.defs)
}

/// The top-level blocks from byte `from`, a line where nothing is open, up to the first fresh
/// line `stop` accepts (or the end): their events, where they end, and the link definitions
/// among them.
pub(super) fn region(
    text: &str,
    from: usize,
    gates: Gates,
    stop: impl Fn(usize) -> bool,
) -> (Vec<Ev>, usize, Definitions) {
    let mut p = Parser::new(text, gates);
    let end = p.run(from, stop);
    (p.events, end, p.defs)
}

/// The top-level blocks `events` make in a note whose link definitions are `defs`.
pub(super) fn nodes(
    text: &str,
    events: &[Ev],
    gates: Gates,
    defs: &Definitions,
) -> Vec<NodeOrToken<GreenNode, rowan::GreenToken>> {
    let doc = build(text, events, gates, defs);
    doc.children().map(|c| c.to_owned()).collect()
}

fn build(text: &str, events: &[Ev], gates: Gates, defs: &Definitions) -> GreenNode {
    let mut b = GreenNodeBuilder::new();
    b.start_node(Document.into());
    inline::build(text, events, gates, defs, &mut b);
    b.finish_node();
    b.finish()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Marker {
    /// `-`, `+` or `*`.
    Bullet(u8),
    /// `.` or `)` after the number.
    Ordered(u8),
}

#[derive(Clone, Copy, Debug)]
enum Open {
    Quote,
    List(Marker),
    Item {
        /// The column its content starts at; a later line goes on in it from there.
        col: usize,
        /// Its first line had nothing after the marker.
        blank_start: bool,
        /// A line of it had something.
        content: bool,
    },
}

#[derive(Clone, Copy, Debug)]
enum Leaf {
    Para {
        /// The index of its `Start`, for a setext heading or a table to rewrite.
        start: usize,
        lines: usize,
        /// Its last line, which may be a table's header.
        last_line: ParaLine,
    },
    Fence {
        ch: u8,
        len: usize,
    },
    /// Indented code; `end` is the event after its last line that isn't blank, where it ends.
    Indented {
        end: usize,
    },
    Html(HtmlEnd),
    Math,
    Comment,
    Table,
}

/// A paragraph's line.
#[derive(Clone, Copy, Debug)]
struct ParaLine {
    /// The index of its first event, and how many of its events from there are its
    /// containers' markers.
    ev: usize,
    markers: usize,
    /// Where its indentation starts, its text starts, its line ending, the next line.
    pos: usize,
    fp: usize,
    le: usize,
    next: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HtmlEnd {
    /// A line holding `</script>`, `</pre>`, `</style>` or `</textarea>`.
    Raw,
    Holds(&'static str),
    /// The next blank line, which isn't part of it.
    Blank,
}

/// A list item's first line.
struct ItemStart {
    /// Where its marker starts and ends.
    marker: (usize, usize),
    kind: Marker,
    /// Where its content starts, and at what column.
    content: usize,
    col: usize,
    empty: bool,
    can_interrupt: bool,
}

struct Parser<'t> {
    text: &'t str,
    g: Gates,
    events: Vec<Ev>,
    stack: Vec<Open>,
    leaf: Option<Leaf>,
    prefix: Vec<(usize, usize, Kind)>,
    /// How many events at the end are this line's container markers, with nothing after them.
    pending: usize,
    /// The link definitions found.
    defs: Definitions,
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

impl<'t> Parser<'t> {
    fn new(text: &'t str, g: Gates) -> Self {
        Parser {
            text,
            g,
            events: Vec::new(),
            stack: Vec::new(),
            leaf: None,
            prefix: Vec::new(),
            pending: 0,
            defs: Definitions::default(),
        }
    }

    fn bytes(&self) -> &'t [u8] {
        self.text.as_bytes()
    }

    fn tok(&mut self, kind: Kind, a: usize, z: usize) {
        if a < z {
            self.push(Ev::Tok(kind, a, z));
        }
    }

    fn start(&mut self, kind: Kind) {
        self.push(Ev::Start(kind));
    }

    fn finish(&mut self) {
        self.push(Ev::Finish);
    }

    fn push(&mut self, ev: Ev) {
        self.events.push(ev);
        self.pending = 0;
    }

    /// End the innermost open block, before this line's container markers if nothing came
    /// after them: they belong to what the line holds.
    fn close(&mut self) {
        self.events
            .insert(self.events.len() - self.pending, Ev::Finish);
    }

    /// A line from `ls`: where its text ends (before `\n` or `\r\n`) and where the next starts.
    fn line_at(&self, ls: usize) -> (usize, usize) {
        let t = self.bytes();
        match t[ls..].iter().position(|&b| b == b'\n') {
            Some(i) => {
                let n = ls + i;
                let le = if n > ls && t[n - 1] == b'\r' {
                    n - 1
                } else {
                    n
                };
                (le, n + 1)
            }
            None => (t.len(), t.len()),
        }
    }

    /// The column after the bytes `a..z` from column `col`: a tab goes to the next multiple of
    /// four, any other character is one.
    fn cols(&self, a: usize, z: usize, col: usize) -> usize {
        self.text[a..z].chars().fold(col, |c, ch| match ch {
            '\t' => c + 4 - c % 4,
            _ => c + 1,
        })
    }

    /// Past the spaces and tabs at `pos` before `le`.
    fn skip_ws(&self, pos: usize, col: usize, le: usize) -> (usize, usize) {
        let t = self.bytes();
        let (mut p, mut c) = (pos, col);
        while p < le && is_ws(t[p]) {
            c = self.cols(p, p + 1, c);
            p += 1;
        }
        (p, c)
    }

    /// Past the spaces and tabs at `pos` until column `to`.
    fn skip_to(&self, pos: usize, col: usize, le: usize, to: usize) -> (usize, usize) {
        let t = self.bytes();
        let (mut p, mut c) = (pos, col);
        while p < le && c < to && is_ws(t[p]) {
            c = self.cols(p, p + 1, c);
            p += 1;
        }
        (p, c)
    }

    /// The BOM and the frontmatter; where the blocks start.
    fn prefix(&mut self) -> usize {
        let text = self.text;
        if text.starts_with('\u{feff}') {
            self.tok(Bom, 0, 3);
            return 3;
        }
        if !self.g.has(S::Frontmatter) {
            return 0;
        }
        let (le, next) = self.line_at(0);
        if text[..le].trim_end() != "---" {
            return 0;
        }
        let mut ls = next;
        let mut lines = vec![(0, le, next)];
        let close = loop {
            if ls >= text.len() {
                return 0;
            }
            let (le, next) = self.line_at(ls);
            lines.push((ls, le, next));
            let t = text[ls..le].trim_end();
            if t == "---" || t == "..." {
                break next;
            }
            ls = next;
        };
        self.start(Frontmatter);
        let last = lines.len() - 1;
        for (n, (ls, le, next)) in lines.into_iter().enumerate() {
            if n == 0 || n == last {
                self.tok(FrontmatterFence, ls, ls + 3);
                self.tok(Whitespace, ls + 3, le);
            } else if ls < le {
                self.push(Ev::Yaml(ls, le));
            }
            self.tok(Newline, le, next);
        }
        self.finish();
        close
    }

    /// Blocks from `from` (a line where nothing is open) until a later fresh line `stop`
    /// accepts, or the end; where they stop.
    fn run(&mut self, from: usize, stop: impl Fn(usize) -> bool) -> usize {
        let mut ls = from;
        while ls < self.text.len() {
            if ls > from && self.stack.is_empty() && self.leaf.is_none() && stop(ls) {
                return ls;
            }
            ls = self.line(ls);
        }
        self.close_leaf();
        while self.stack.pop().is_some() {
            self.finish();
        }
        ls
    }

    fn close_leaf(&mut self) {
        match self.leaf.take() {
            None => {}
            Some(Leaf::Indented { end }) => self.events.insert(end, Ev::Finish),
            Some(_) => self.close(),
        }
    }

    fn newline(&mut self, le: usize, next: usize) {
        self.tok(Newline, le, next);
    }

    /// One line; where the next starts.
    fn line(&mut self, ls: usize) -> usize {
        let (le, next) = self.line_at(ls);
        let t = self.bytes();
        let mut prefix = std::mem::take(&mut self.prefix);
        prefix.clear();
        self.pending = 0;
        let (mut pos, mut col) = (ls, 0);
        let mut matched = 0;
        // The line is the next item of an open list.
        let mut sibling = false;
        while matched < self.stack.len() {
            match self.stack[matched] {
                Open::Quote => {
                    let (p, c) = self.skip_to(pos, col, le, col + 3);
                    if p < le && t[p] == b'>' {
                        prefix.push((pos, p, Whitespace));
                        prefix.push((p, p + 1, QuoteMarker));
                        (pos, col) = (p + 1, c + 1);
                        if pos < le && is_ws(t[pos]) {
                            prefix.push((pos, pos + 1, Whitespace));
                            col = self.cols(pos, pos + 1, col);
                            pos += 1;
                        }
                        matched += 1;
                    } else {
                        break;
                    }
                }
                Open::List(marker) => {
                    if let Some((p, c)) = self.item_goes_on(matched + 1, pos, col, le) {
                        prefix.push((pos, p, Whitespace));
                        (pos, col) = (p, c);
                        matched += 2;
                    } else {
                        let same = self
                            .item_start(pos, col, le)
                            .is_some_and(|i| i.kind == marker);
                        matched += same as usize;
                        sibling = same;
                        break;
                    }
                }
                Open::Item { .. } => break,
            }
        }
        let all = matched == self.stack.len();
        let (fp, fc) = self.skip_ws(pos, col, le);
        let blank = fp == le;

        // The open leaf goes on.
        if all {
            match self.leaf {
                Some(Leaf::Fence { ch, len }) => {
                    self.emit_prefix(&prefix);
                    let run = t[fp..le].iter().take_while(|&&b| b == ch).count();
                    if fc - col <= 3 && run >= len && t[fp + run..le].iter().all(|&b| is_ws(b)) {
                        self.tok(Whitespace, pos, fp);
                        self.tok(Fence, fp, fp + run);
                        self.tok(Whitespace, fp + run, le);
                        self.newline(le, next);
                        self.close_leaf();
                    } else {
                        self.tok(CodeText, pos, le);
                        self.newline(le, next);
                    }
                    return self.done(prefix, next);
                }
                Some(Leaf::Indented { .. }) if blank || fc - col >= 4 => {
                    self.emit_prefix(&prefix);
                    if blank {
                        self.blank_line(pos, le, next);
                    } else {
                        self.tok(CodeText, pos, le);
                        self.newline(le, next);
                        self.leaf = Some(Leaf::Indented {
                            end: self.events.len(),
                        });
                    }
                    return self.done(prefix, next);
                }
                Some(Leaf::Html(end)) if !(blank && end == HtmlEnd::Blank) => {
                    self.emit_prefix(&prefix);
                    self.tok(HtmlText, pos, le);
                    self.newline(le, next);
                    if html_ends(end, &self.text[pos..le]) {
                        self.close_leaf();
                    }
                    return self.done(prefix, next);
                }
                Some(Leaf::Math) | Some(Leaf::Comment) => {
                    self.emit_prefix(&prefix);
                    let (kind, mark) = match self.leaf {
                        Some(Leaf::Math) => (MathText, "$$"),
                        _ => (CommentText, "%%"),
                    };
                    self.tok(kind, pos, le);
                    self.newline(le, next);
                    if self.text[pos..le].contains(mark) {
                        self.close_leaf();
                    }
                    return self.done(prefix, next);
                }
                Some(Leaf::Table) if !blank && !self.interrupts(pos, col, le, false) => {
                    self.emit_prefix(&prefix);
                    self.row(TableRow, pos, le, next);
                    return self.done(prefix, next);
                }
                Some(Leaf::Para { .. }) | None => {}
                Some(_) => self.close_leaf(),
            }
        } else if let Some(Leaf::Para { start, lines, .. }) = self.leaf
            && !blank
            && !sibling
            && !self.interrupts(pos, col, le, true)
        {
            // A lazy line: the paragraph goes on without its containers' markers.
            self.emit_prefix(&prefix);
            let last_line = self.para_line(pos, fp, le, next);
            self.tok(Whitespace, pos, fp);
            self.push(Ev::Inline(fp, le));
            self.newline(le, next);
            self.leaf = Some(Leaf::Para {
                start,
                lines: lines + 1,
                last_line,
            });
            return self.done(prefix, next);
        }

        if !all {
            self.close_leaf();
            while self.stack.len() > matched {
                self.stack.pop();
                self.finish();
            }
        }
        self.emit_prefix(&prefix);
        self.open(pos, col, le, next);
        self.done(prefix, next)
    }

    fn done(&mut self, prefix: Vec<(usize, usize, Kind)>, next: usize) -> usize {
        self.prefix = prefix;
        next
    }

    /// A paragraph's line from `pos`, its containers' markers just read.
    fn para_line(&self, pos: usize, fp: usize, le: usize, next: usize) -> ParaLine {
        ParaLine {
            ev: self.events.len() - self.pending,
            markers: self.pending,
            pos,
            fp,
            le,
            next,
        }
    }

    /// This line's container markers, which a block closed next goes before.
    fn emit_prefix(&mut self, prefix: &[(usize, usize, Kind)]) {
        let before = self.events.len();
        for &(a, z, kind) in prefix {
            if a < z {
                self.events.push(Ev::Tok(kind, a, z));
            }
        }
        self.pending = self.events.len() - before;
    }

    /// How many cells a table's header row in `a..z` has.
    fn header_cells(&self, a: usize, z: usize) -> usize {
        let z = a + self.text[a..z].trim_end().len();
        let pipes = self.cells(a, z);
        let edges = (pipes.first() == Some(&a)) as usize
            + (pipes.len() > 1 && pipes.last() == Some(&(z - 1))) as usize;
        (pipes.len() + 1).saturating_sub(edges)
    }

    /// Whether the list item at `stack[i]` goes on at `pos`: where its content starts if so
    /// (nothing taken on a blank line).
    fn item_goes_on(&self, i: usize, pos: usize, col: usize, le: usize) -> Option<(usize, usize)> {
        let Some(&Open::Item {
            col: icol,
            blank_start,
            content,
        }) = self.stack.get(i)
        else {
            return None;
        };
        let (fp, fc) = self.skip_ws(pos, col, le);
        if fp == le {
            return (!(blank_start && !content)).then_some((pos, col));
        }
        (fc >= icol).then(|| self.skip_to(pos, col, le, icol))
    }

    /// The list item starting at `pos`, if one does.
    fn item_start(&self, pos: usize, col: usize, le: usize) -> Option<ItemStart> {
        let t = self.bytes();
        let (fp, fc) = self.skip_ws(pos, col, le);
        if fp == le || fc - col > 3 || self.thematic(fp, le) {
            return None;
        }
        let (kind, me, one) = match t[fp] {
            b @ (b'-' | b'+' | b'*') => (Marker::Bullet(b), fp + 1, true),
            b'0'..=b'9' => {
                let digits = t[fp..le].iter().take_while(|b| b.is_ascii_digit()).count();
                let d = fp + digits;
                if digits > 9 || d >= le || !matches!(t[d], b'.' | b')') {
                    return None;
                }
                let one = self.text[fp..d].parse::<u32>() == Ok(1);
                (Marker::Ordered(t[d]), d + 1, one)
            }
            _ => return None,
        };
        if me < le && !is_ws(t[me]) {
            return None;
        }
        let mcol = fc + (me - fp);
        let (sp, sc) = self.skip_ws(me, mcol, le);
        let empty = sp == le;
        let (content, col) = if empty {
            (le, mcol + 1)
        } else if sc - mcol >= 5 {
            (me + 1, self.cols(me, me + 1, mcol))
        } else {
            (sp, sc)
        };
        Some(ItemStart {
            marker: (fp, me),
            kind,
            content,
            col,
            empty,
            can_interrupt: !empty && one,
        })
    }

    fn thematic(&self, fp: usize, le: usize) -> bool {
        let t = &self.bytes()[fp..le];
        let Some(&c) = t.first() else { return false };
        matches!(c, b'-' | b'*' | b'_')
            && t.iter().all(|&b| b == c || is_ws(b))
            && t.iter().filter(|&&b| b == c).count() >= 3
    }

    /// The `#`s of an ATX heading at `fp`.
    fn atx(&self, fp: usize, le: usize) -> Option<usize> {
        let t = self.bytes();
        let n = t[fp..le].iter().take_while(|&&b| b == b'#').count();
        ((1..=6).contains(&n) && (fp + n == le || is_ws(t[fp + n]))).then_some(fp + n)
    }

    /// An opening code fence at `fp`: its character and length.
    fn fence(&self, fp: usize, le: usize) -> Option<(u8, usize)> {
        let t = self.bytes();
        let ch = *t.get(fp).filter(|&&b| b == b'`' || b == b'~')?;
        let n = t[fp..le].iter().take_while(|&&b| b == ch).count();
        (n >= 3 && (ch == b'~' || !t[fp + n..le].contains(&b'`'))).then_some((ch, n))
    }

    /// Whether the line at `pos` starts a block that ends a paragraph or a table. On a `lazy`
    /// line, which the paragraph's containers don't go on to, any list item does too: the line
    /// isn't read inside the paragraph's container.
    fn interrupts(&self, pos: usize, col: usize, le: usize, lazy: bool) -> bool {
        let t = self.bytes();
        let (fp, fc) = self.skip_ws(pos, col, le);
        if fp == le || fc - col >= 4 {
            return false;
        }
        t[fp] == b'>'
            || self.thematic(fp, le)
            || self
                .item_start(pos, col, le)
                .is_some_and(|i| lazy || i.can_interrupt)
            || self.atx(fp, le).is_some()
            || self.fence(fp, le).is_some()
            || html_start(&self.text[fp..le], true).is_some()
            || self.math_start(fp, le)
            || self.comment_start(fp, le)
    }

    fn math_start(&self, fp: usize, le: usize) -> bool {
        self.g.has(S::Math) && self.text[fp..le].starts_with("$$")
    }

    fn comment_start(&self, fp: usize, le: usize) -> bool {
        self.g.has(S::Comments)
            && self.text[fp..le].starts_with("%%")
            && !self.text[fp + 2..le].contains("%%")
    }

    fn blank_line(&mut self, pos: usize, le: usize, next: usize) {
        self.start(BlankLine);
        self.tok(Whitespace, pos, le);
        self.newline(le, next);
        self.finish();
    }

    fn mark_content(&mut self) {
        for o in &mut self.stack {
            if let Open::Item { content, .. } = o {
                *content = true;
            }
        }
    }

    /// New blocks from `pos` on a line whose containers are settled.
    fn open(&mut self, mut pos: usize, mut col: usize, le: usize, next: usize) {
        let t = self.bytes();
        let mut opened = false;
        loop {
            let (fp, fc) = self.skip_ws(pos, col, le);
            let indent = fc - col;
            let para = match self.leaf {
                Some(Leaf::Para {
                    start,
                    lines,
                    last_line,
                }) => Some((start, lines, last_line)),
                _ => None,
            };
            if fp == le {
                if opened {
                    self.tok(Whitespace, pos, le);
                    self.newline(le, next);
                } else {
                    self.close_leaf();
                    self.blank_line(pos, le, next);
                }
                return;
            }
            self.mark_content();
            if indent >= 4 {
                if para.is_none() {
                    self.close_leaf();
                    self.start(IndentedCode);
                    self.tok(CodeText, pos, le);
                    self.newline(le, next);
                    self.leaf = Some(Leaf::Indented {
                        end: self.events.len(),
                    });
                    return;
                }
            } else {
                if let Some((start, lines, last)) = para {
                    let line = self.text[fp..le].trim_end();
                    if !line.is_empty()
                        && (line.bytes().all(|b| b == b'=') || line.bytes().all(|b| b == b'-'))
                    {
                        self.events[start] = Ev::Start(SetextHeading);
                        self.tok(Whitespace, pos, fp);
                        self.tok(SetextUnderline, fp, fp + line.len());
                        self.tok(Whitespace, fp + line.len(), le);
                        self.newline(le, next);
                        self.close_leaf();
                        return;
                    }
                    // A table's header is a paragraph's only line, or its last when it starts
                    // with `|`: such a table ends the paragraph before it, as pulldown-cmark
                    // and GitHub read it.
                    let heavy = t[last.fp] == b'|';
                    if self.g.has(S::Tables) && (lines == 1 || heavy) {
                        let header = self.header_cells(last.fp, last.le);
                        if delimiter_cells(&self.text[fp..le]).is_some_and(|n| n == header) {
                            // This line's markers came after the paragraph's line; keep them.
                            let markers = self.events.split_off(self.events.len() - self.pending);
                            if lines == 1 {
                                self.events.truncate(start);
                            } else {
                                // The header line's markers go after the paragraph's end.
                                let mut head = self.events.split_off(last.ev);
                                head.truncate(last.markers);
                                self.finish();
                                self.events.extend(head);
                            }
                            self.start(Table);
                            self.row(TableRow, last.pos, last.le, last.next);
                            self.events.extend(markers);
                            self.row(TableDelimiterRow, pos, le, next);
                            self.leaf = Some(Leaf::Table);
                            return;
                        }
                    }
                }
                let b = t[fp];
                if b == b'>' {
                    self.close_leaf();
                    self.start(BlockQuote);
                    let at = self.events.len() - 1;
                    self.tok(Whitespace, pos, fp);
                    self.tok(QuoteMarker, fp, fp + 1);
                    (pos, col) = (fp + 1, fc + 1);
                    if pos < le && is_ws(t[pos]) {
                        self.tok(Whitespace, pos, pos + 1);
                        col = self.cols(pos, pos + 1, col);
                        pos += 1;
                    }
                    self.stack.push(Open::Quote);
                    opened = true;
                    if let Some(m) = self.callout(pos, le) {
                        self.events[at] = Ev::Start(Callout);
                        self.tok(CalloutMarker, pos, m);
                        self.start(CalloutTitle);
                        let (tp, _) = self.skip_ws(m, 0, le);
                        self.tok(Whitespace, m, tp);
                        if tp < le {
                            self.push(Ev::Inline(tp, le));
                        }
                        self.newline(le, next);
                        self.finish();
                        return;
                    }
                    continue;
                }
                if self.thematic(fp, le) {
                    self.close_leaf();
                    self.start(ThematicBreak);
                    self.tok(Whitespace, pos, fp);
                    let end = fp + self.text[fp..le].trim_end().len();
                    self.tok(Rule, fp, end);
                    self.tok(Whitespace, end, le);
                    self.newline(le, next);
                    self.finish();
                    return;
                }
                if let Some(item) = self.item_start(pos, col, le)
                    && (para.is_none() || item.can_interrupt)
                {
                    self.close_leaf();
                    match self.stack.last() {
                        Some(Open::List(m)) if *m == item.kind => {}
                        Some(Open::List(_)) => {
                            self.stack.pop();
                            self.close();
                            self.start(List);
                            self.stack.push(Open::List(item.kind));
                        }
                        _ => {
                            self.start(List);
                            self.stack.push(Open::List(item.kind));
                        }
                    }
                    self.start(ListItem);
                    let (ma, mz) = item.marker;
                    self.tok(Whitespace, pos, ma);
                    self.tok(ListMarker, ma, mz);
                    self.tok(Whitespace, mz, item.content);
                    pos = item.content;
                    col = item.col;
                    if !item.empty
                        && self.g.has(S::TaskLists)
                        && let Some(end) = task_marker(&self.text[pos..le])
                    {
                        self.tok(TaskMarker, pos, pos + end);
                        col = self.cols(pos, pos + end, col);
                        pos += end;
                    }
                    self.stack.push(Open::Item {
                        col: item.col,
                        blank_start: item.empty,
                        content: !item.empty,
                    });
                    opened = true;
                    continue;
                }
                if let Some(he) = self.atx(fp, le) {
                    self.close_leaf();
                    self.start(AtxHeading);
                    self.tok(Whitespace, pos, fp);
                    self.tok(HeadingMarker, fp, he);
                    let (tp, _) = self.skip_ws(he, 0, le);
                    self.tok(Whitespace, he, tp);
                    if tp < le {
                        self.push(Ev::Inline(tp, le));
                    }
                    self.newline(le, next);
                    self.finish();
                    return;
                }
                if let Some((ch, len)) = self.fence(fp, le) {
                    self.close_leaf();
                    self.start(FencedCode);
                    self.tok(Whitespace, pos, fp);
                    self.tok(Fence, fp, fp + len);
                    let (ip, _) = self.skip_ws(fp + len, 0, le);
                    self.tok(Whitespace, fp + len, ip);
                    self.tok(Info, ip, le);
                    self.newline(le, next);
                    self.leaf = Some(Leaf::Fence { ch, len });
                    return;
                }
                if let Some(end) = html_start(&self.text[fp..le], para.is_some()) {
                    self.close_leaf();
                    self.start(HtmlBlock);
                    self.tok(HtmlText, pos, le);
                    self.newline(le, next);
                    self.leaf = Some(Leaf::Html(end));
                    if end != HtmlEnd::Blank && html_ends(end, &self.text[fp..le]) {
                        self.close_leaf();
                    }
                    return;
                }
                for (kind, inner, mark, starts) in [
                    (MathBlock, MathText, MathMarker, self.math_start(fp, le)),
                    (
                        CommentBlock,
                        CommentText,
                        CommentMarker,
                        self.comment_start(fp, le),
                    ),
                ] {
                    if !starts {
                        continue;
                    }
                    self.close_leaf();
                    self.start(kind);
                    self.tok(Whitespace, pos, fp);
                    self.tok(mark, fp, fp + 2);
                    self.tok(inner, fp + 2, le);
                    self.newline(le, next);
                    let closed = kind == MathBlock && self.text[fp + 2..le].contains("$$");
                    self.leaf = Some(match kind {
                        MathBlock => Leaf::Math,
                        _ => Leaf::Comment,
                    });
                    if closed {
                        self.close_leaf();
                    }
                    return;
                }
            }
            // A link definition, where no paragraph is open: a block of its own, which no
            // paragraph goes on from, as pulldown-cmark reads it.
            if para.is_none()
                && t[fp] == b'['
                && let Some((label, dest)) = self.definition(fp, le)
            {
                self.start(LinkDefinition);
                self.tok(Whitespace, pos, fp);
                self.tok(LBracket, fp, fp + 1);
                self.tok(DefinitionLabel, fp + 1, label);
                self.tok(RBracket, label, label + 1);
                self.tok(Colon, label + 1, label + 2);
                self.tok(Whitespace, label + 2, dest.0);
                self.tok(DefinitionDest, dest.0, dest.1);
                self.tok(Whitespace, dest.1, le);
                self.newline(le, next);
                self.finish();
                let text = self.text;
                self.defs.add(&text[fp + 1..label], &text[dest.0..dest.1]);
                return;
            }
            // A paragraph's line.
            let last_line = self.para_line(pos, fp, le, next);
            match para {
                Some((start, lines, _)) => {
                    self.tok(Whitespace, pos, fp);
                    self.push(Ev::Inline(fp, le));
                    self.newline(le, next);
                    self.leaf = Some(Leaf::Para {
                        start,
                        lines: lines + 1,
                        last_line,
                    });
                }
                None => {
                    let start = self.events.len();
                    self.start(Paragraph);
                    self.tok(Whitespace, pos, fp);
                    self.push(Ev::Inline(fp, le));
                    self.newline(le, next);
                    self.leaf = Some(Leaf::Para {
                        start,
                        lines: 1,
                        last_line,
                    });
                }
            }
            return;
        }
    }

    /// A link definition, `[label]: destination "title"`, taking the line `fp..le`, which
    /// starts with `[`: where its label ends, and its destination with its title. With
    /// footnotes, `[^label]:` is a footnote; with wikilinks, a destination that starts one
    /// (`[[`) leaves the line text.
    fn definition(&self, fp: usize, le: usize) -> Option<(usize, (usize, usize))> {
        let t = self.bytes();
        if t.get(fp + 1) == Some(&b'[') || !self.text[fp..le].contains("]:") {
            return None;
        }
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| {
            Regex::new(
                r#"^\[((?:[^\[\]\\]|\\.)+)\]:[ \t]*((?:<[^<>\n]*>|[^\s<]\S*)(?:[ \t]+(?:"[^"]*"|'[^']*'|\([^()]*\)))?)[ \t]*$"#,
            )
            .unwrap()
        });
        let c = re.captures(&self.text[fp..le])?;
        let (label, dest) = (c.get(1)?, c.get(2)?);
        let footnote = self.g.has(S::Footnotes) && label.as_str().starts_with('^');
        // `[Source]: [[Book]]` is a line with a wikilink, which a rename must still find.
        let wikilink = self.g.has(S::WikiLinks) && dest.as_str().starts_with("[[");
        if label.as_str().trim().is_empty() || footnote || wikilink {
            return None;
        }
        Some((fp + label.end(), (fp + dest.start(), fp + dest.end())))
    }

    /// `[!type]`, `[!type]+` or `[!type]-` at `pos`: where it ends.
    fn callout(&self, pos: usize, le: usize) -> Option<usize> {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"^\[!([^\]\s]+)\]([+-]?)").unwrap());
        let line = &self.text[pos..le];
        let m = re.captures(line)?;
        let kind = m.get(1)?.as_str();
        let end = m.get(0)?.end();
        let alert = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"]
            .iter()
            .any(|a| a.eq_ignore_ascii_case(kind));
        let ok = self.g.has(S::Callouts)
            || (self.g.has(S::Alerts)
                && alert
                && m.get(2)?.is_empty()
                && line[end..].trim().is_empty());
        ok.then_some(pos + end)
    }

    /// The separators between a table row's cells in `a..z`: the `|`s not escaped and not
    /// inside a wikilink.
    fn cells(&self, a: usize, z: usize) -> Vec<usize> {
        let t = self.bytes();
        let wikilinks = self.g.has(S::WikiLinks);
        let mut out = Vec::new();
        let mut i = a;
        while i < z {
            match t[i] {
                b'\\' => i += 1,
                b'[' if wikilinks && t.get(i + 1) == Some(&b'[') => {
                    if let Some(end) = self.text[i + 2..z].find("]]") {
                        i += 2 + end + 1;
                    }
                }
                b'|' => out.push(i),
                _ => {}
            }
            i += 1;
        }
        out
    }

    /// A table row from `pos`: its cells, or its delimiters.
    fn row(&mut self, kind: Kind, pos: usize, le: usize, next: usize) {
        let (fp, _) = self.skip_ws(pos, 0, le);
        let te = fp + self.text[fp..le].trim_end().len();
        self.start(kind);
        self.tok(Whitespace, pos, fp);
        let mut at = fp;
        let pipes = self.cells(fp, te);
        for q in pipes.into_iter().chain([te]) {
            if q > at {
                match kind {
                    TableDelimiterRow => self.tok(Delimiter, at, q),
                    _ => {
                        self.start(TableCell);
                        self.push(Ev::Inline(at, q));
                        self.finish();
                    }
                }
            }
            if q < te {
                self.tok(Pipe, q, q + 1);
            }
            at = q + 1;
        }
        self.tok(Whitespace, te, le);
        self.newline(le, next);
        self.finish();
    }
}

/// A task's checkbox at the start of `rest`: one character between brackets, then a space or
/// the end of the line. Where it ends.
fn task_marker(rest: &str) -> Option<usize> {
    let mut chars = rest.char_indices();
    if chars.next()?.1 != '[' {
        return None;
    }
    let (_, c) = chars.next()?;
    if c == '\n' || c == '\r' {
        return None;
    }
    let (i, close) = chars.next()?;
    let end = i + 1;
    (close == ']'
        && rest[end..]
            .chars()
            .next()
            .is_none_or(|c| c == ' ' || c == '\t'))
    .then_some(end)
}

/// The cells of a table's delimiter row (`| --- | :-: |`), if `line` is one.
fn delimiter_cells(line: &str) -> Option<usize> {
    let l = line.trim();
    let piped = l.contains('|');
    let l = l.strip_prefix('|').unwrap_or(l);
    let l = l.strip_suffix('|').unwrap_or(l);
    let mut n = 0;
    for cell in l.split('|') {
        let c = cell.trim();
        let c = c.strip_prefix(':').unwrap_or(c);
        let c = c.strip_suffix(':').unwrap_or(c);
        if c.is_empty() || !c.bytes().all(|b| b == b'-') {
            return None;
        }
        n += 1;
    }
    (piped || n > 1).then_some(n)
}

const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

/// The HTML block `line` starts, by CommonMark's seven kinds, and what ends it. The seventh
/// (any lone tag) can't interrupt a paragraph.
fn html_start(line: &str, in_paragraph: bool) -> Option<HtmlEnd> {
    if !line.starts_with('<') {
        return None;
    }
    let lower = line
        .chars()
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    for tag in ["script", "pre", "style", "textarea"] {
        if let Some(rest) = lower[1..].strip_prefix(tag)
            && rest
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == '>')
        {
            return Some(HtmlEnd::Raw);
        }
    }
    if line.starts_with("<!--") {
        return Some(HtmlEnd::Holds("-->"));
    }
    if line.starts_with("<?") {
        return Some(HtmlEnd::Holds("?>"));
    }
    if line.starts_with("<![CDATA[") {
        return Some(HtmlEnd::Holds("]]>"));
    }
    if line.starts_with("<!") && line[2..].starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Some(HtmlEnd::Holds(">"));
    }
    let name_at = if line.starts_with("</") { 2 } else { 1 };
    let name: String = line[name_at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let after = &line[name_at + name.len()..];
    if BLOCK_TAGS.contains(&name.as_str())
        && (after.is_empty()
            || after.starts_with(|c: char| c.is_whitespace())
            || after.starts_with('>')
            || after.starts_with("/>"))
    {
        return Some(HtmlEnd::Blank);
    }
    static LONE: OnceLock<Regex> = OnceLock::new();
    let lone = LONE.get_or_init(|| {
        Regex::new(
            r#"^(?:<[A-Za-z][A-Za-z0-9-]*(?:\s+[A-Za-z_:][A-Za-z0-9_.:-]*(?:\s*=\s*(?:[^\s"'=<>`]+|'[^']*'|"[^"]*"))?)*\s*/?>|</[A-Za-z][A-Za-z0-9-]*\s*>)\s*$"#,
        )
        .unwrap()
    });
    (!in_paragraph && lone.is_match(line)).then_some(HtmlEnd::Blank)
}

/// Whether `line` ends an HTML block that `end` ends.
fn html_ends(end: HtmlEnd, line: &str) -> bool {
    match end {
        HtmlEnd::Raw => {
            let l = line.to_ascii_lowercase();
            ["</script>", "</pre>", "</style>", "</textarea>"]
                .iter()
                .any(|t| l.contains(t))
        }
        HtmlEnd::Holds(s) => line.contains(s),
        HtmlEnd::Blank => false,
    }
}
