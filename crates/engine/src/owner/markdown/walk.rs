//! A note's items, read by one walk through the note's syntax tree (`cst`), in the workspace's
//! flavour, over the same text the reader parses (without `%%comments%%`). The walk reads the
//! blocks as pulldown-cmark, which read them before, did, so ids, revs and spans stay what they
//! were (`pulldown`, a test, compares the two). Types match after it (`typed`): the walk
//! collects the blocks some type reads, and `matcher` says which type each one is and which
//! inline types sit in its text. A task is a list item the built-in `task` type
//! (`types/task.toml`) matches. Each item's key is made here (`keys`), and its id from that key
//! by `items::identify`.

use crate::cst::{self, Kind, Step};
use crate::items::{Data, Item, Raw, Settings};
use crate::markdown::{self, Flavour, Syntax};
use crate::matcher::{self, Matcher};
use crate::sources::On;
use crate::workspace::clean_target;
use block_text::{heading_range, heading_text, list_text, list_text_start, row_cells, trim_cell};
use std::ops::Range;
use typed::finish;

mod block_text;
mod keys;
#[cfg(test)]
pub(crate) mod pulldown;
mod typed;

/// The items of the note at workspace-relative `note` (with `.md`) whose text is `text`.
pub fn items(note: &str, text: &str, s: &Settings) -> Vec<Item> {
    if s.templates.is_template(note) {
        return Vec::new();
    }
    // The comments are taken out first, as for the reader, so their lines read as blank.
    // No item is made of what text itself holds (emphasis and the like), so it isn't read.
    let gates = cst::Gates::of(&s.flavour).plain();
    let gates = gates.set(Syntax::Comments, false);
    let source = s.flavour.source_with(text, |text| cst::code(text, gates));
    let lines = Lines::new(&source.text);
    let wants = Wants::of(&s.matcher);
    let (blocks, defs) = cst::blocks(lines.src, wants.gates(gates));
    let mut walk = Walk::new(&lines, &s.flavour, wants, &defs);
    blocks.read(&defs, &mut |step| walk.step(step));
    let Walk {
        mut pass, written, ..
    } = walk;
    pass.written.links = own_links(written.finish(&defs));
    finish(note, text, s, &source, &lines, pass)
}

/// A block some loaded type reads, found in the pass and matched once the note is read.
struct Candidate {
    on: On,
    /// Its text in the source.
    text: Range<usize>,
    /// Whether a type on its primitive may take it; else it is only searched for inline types.
    typeable: bool,
    /// Where inline types are looked for: its text, or all of a list item's own lines.
    own: Range<usize>,
    /// The item it would type (a list item, a heading, a paragraph with a block id); without
    /// one, a match is a new item.
    raw: Option<usize>,
    line: usize,
    end_line: usize,
    section: Vec<String>,
    parent: Option<usize>,
    block_id: Option<String>,
    in_quote: bool,
    /// A table row's table, and its cells in the source.
    table: usize,
    cells: Vec<Range<usize>>,
    /// The attributes its text doesn't hold: a heading's level, a callout's kind.
    given: matcher::Given,
}

/// What a pass through a note finds before types match.
#[derive(Default)]
struct Pass {
    /// Its items, in the order an id is given (a parent before its children).
    raws: Vec<Raw>,
    candidates: Vec<Candidate>,
    /// Code spans, code blocks and math, in order, where inline types never match.
    code: Vec<Range<usize>>,
    /// Each table's header.
    tables: Vec<Vec<String>>,
    /// The tags and links in the note's body, when some type reads them.
    tags: Vec<String>,
    links: Vec<String>,
    /// Where the tags and the links are written, when some type reads a block's own.
    written: matcher::Written,
}

/// What the workspace's types read, which is all a pass collects.
#[derive(Clone, Copy)]
struct Wants {
    typing: bool,
    rows: bool,
    paragraphs: bool,
    lists: bool,
    headings: bool,
    asides: bool,
    inline: bool,
    tags: bool,
    links: bool,
    /// A block's own links.
    own_links: bool,
}

impl Wants {
    fn of(m: &Matcher) -> Wants {
        let typing = !m.is_empty();
        let want = |on: On| typing && m.reads(on);
        Wants {
            typing,
            rows: want(On::Row),
            paragraphs: want(On::Text),
            lists: want(On::Item),
            headings: want(On::Section),
            asides: want(On::Aside),
            inline: want(On::Inline),
            tags: m.needs_tags || m.reads_tags,
            links: m.needs_links,
            own_links: m.reads_links,
        }
    }

    /// The syntax a note is walked in: `flavour`'s, but when a type reads the note's tags, or a
    /// block's own tags or links, tags and wikilinks count whatever the flavour, as they
    /// always have.
    fn gates(self, flavour: cst::Gates) -> cst::Gates {
        let gates = flavour.set(Syntax::Tags, self.tags);
        match self.own_links {
            true => gates.set(Syntax::WikiLinks, true),
            false => gates,
        }
    }

    /// Table rows are read for row types, and for the inline types in them.
    fn collect_rows(self) -> bool {
        self.rows || self.inline
    }
}

/// A note's text as a pass reads it, and where its lines are.
struct Lines<'a> {
    src: &'a str,
    newlines: Vec<usize>,
    lines: Vec<&'a str>,
}

impl<'a> Lines<'a> {
    fn new(src: &'a str) -> Lines<'a> {
        Lines {
            src,
            newlines: src.match_indices('\n').map(|(i, _)| i).collect(),
            lines: src.lines().collect(),
        }
    }

    /// The line (from 1) byte `offset` is on.
    fn of(&self, offset: usize) -> usize {
        self.newlines.partition_point(|&n| n < offset) + 1
    }

    fn line(&self, n: usize) -> &'a str {
        self.lines.get(n - 1).copied().unwrap_or_default()
    }

    /// Whether line `n` holds nothing but quote markers.
    fn blank(&self, n: usize) -> bool {
        self.line(n)
            .trim()
            .trim_start_matches('>')
            .trim()
            .is_empty()
    }

    /// Where line `n` ends, before its line break.
    fn end(&self, n: usize) -> usize {
        self.newlines.get(n - 1).copied().unwrap_or(self.src.len())
    }
}

/// The links a block's own `links` are read from, of those the Markdown owner found: where
/// each is written, whether as a Markdown link, and the note it names.
fn own_links(links: Vec<crate::owner::markdown::Link<'_>>) -> Vec<(usize, bool, String)> {
    let links = (links.iter()).filter(|l| l.by != crate::owner::markdown::By::Definition);
    links
        .filter_map(|l| Some((l.at, l.md(), l.note()?)))
        .collect()
}

/// A block the walk is inside.
struct Open {
    /// Where it starts: a paragraph, a table row or a fence at its first character, as
    /// pulldown-cmark had them.
    start: usize,
    line: usize,
    /// The `Raw` this container is, for a list item.
    item: Option<usize>,
    /// For a list item: the last line before its first nested list.
    own_end: Option<usize>,
    /// The first line of the last child block that ended.
    prev_child: Option<usize>,
    /// Whether `start` still moves past the indentation before its first character.
    leading: bool,
}

/// A quote or a callout the walk is inside, kept when some type is on `aside`.
struct Aside {
    section: Vec<String>,
    parent: Option<usize>,
    /// Its `[!kind]`: where the kind is written, and whether it folds.
    callout: Option<(Range<usize>, Option<bool>)>,
    /// Where its text starts: after its `[!kind]`, else at the first thing in it.
    text: Option<usize>,
}

/// A walk through a note's tree, step by step, reading the blocks as pulldown-cmark had them:
/// a block id, a list item's own lines and the line of the block before a lone `^id` all come
/// out the same.
struct Walk<'a> {
    lines: &'a Lines<'a>,
    flavour: &'a Flavour,
    wants: Wants,
    /// The note's link definitions, which say where a reference link points.
    defs: &'a cst::Definitions,
    pass: Pass,
    open: Vec<Open>,
    headings: Vec<(u8, String)>,
    quotes: usize,
    /// The quotes and callouts it is inside, innermost last, when some type is on `aside`.
    asides: Vec<Aside>,
    /// Some aside it is inside has no text yet. The one thing a step checks for asides, so a
    /// note costs a walk nothing more when no type is on `aside`.
    seeking: bool,
    /// Inside a code block or the frontmatter.
    verbatim: usize,
    /// The level of the heading being read.
    level: u8,
    /// Where the code span or the math being read starts.
    code_at: usize,
    /// The wikilink being read: what it says before its `|`.
    wiki: Option<String>,
    /// Inside a Markdown link, and inside an image, which a link's text may hold.
    link: bool,
    image: bool,
    /// The links as the Markdown owner reads them, for a block's own.
    written: crate::owner::markdown::Links<'a>,
    /// The last callout's `[!type]`.
    marker: Range<usize>,
    /// A callout's title just read, which pulldown-cmark read on into the paragraph on the
    /// next line, or a paragraph: where it ends, until the next block says whether it goes on.
    pending: Option<usize>,
    /// The table being read: how many rows it has had, where its header starts, its columns.
    rows: usize,
    head: usize,
    cols: usize,
}

impl<'a> Walk<'a> {
    fn new(
        lines: &'a Lines<'a>,
        flavour: &'a Flavour,
        wants: Wants,
        defs: &'a cst::Definitions,
    ) -> Walk<'a> {
        Walk {
            lines,
            flavour,
            wants,
            defs,
            pass: Pass::default(),
            open: vec![Open {
                start: 0,
                line: 1,
                item: None,
                own_end: None,
                prev_child: None,
                leading: false,
            }],
            headings: Vec::new(),
            quotes: 0,
            asides: Vec::new(),
            seeking: false,
            verbatim: 0,
            level: 0,
            code_at: 0,
            wiki: None,
            link: false,
            image: false,
            written: Default::default(),
            marker: 0..0,
            pending: None,
            rows: 0,
            head: 0,
            cols: 0,
        }
    }

    fn step(&mut self, step: Step<'a>) {
        if self.wants.own_links {
            self.written.step(step);
        }
        match step {
            Step::Enter(kind, at) => self.enter(kind, at),
            Step::Leave(kind, at) => self.leave(kind, at),
            Step::Token(kind, text, at) => self.token(kind, text, at),
        }
    }

    fn names(&self) -> Vec<String> {
        self.headings.iter().map(|h| h.1.clone()).collect()
    }

    /// The list item the walk is inside.
    fn enclosing(&self) -> Option<usize> {
        self.open.iter().rev().find_map(|o| o.item)
    }

    fn begin(&mut self, at: usize, item: Option<usize>, leading: bool) {
        let line = self.lines.of(at);
        self.open.push(Open {
            start: at,
            line,
            item,
            own_end: None,
            prev_child: None,
            leading,
        });
    }

    /// Ends the innermost block: it, and the line of the block that ended before it in its
    /// parent.
    fn end(&mut self) -> (Open, Option<usize>) {
        let block = self.open.pop().expect("a block to end");
        let parent = self.open.last_mut().expect("the note holds every block");
        let before = parent.prev_child.replace(block.line);
        (block, before)
    }

    fn enter(&mut self, kind: Kind, at: usize) {
        // The first thing in an aside starts its text; a quote inside it is no text, so what
        // that one holds starts both.
        if self.seeking && !matches!(kind, Kind::BlockQuote | Kind::Callout) {
            self.aside_text(at);
        }
        if let Some(end) = self.pending.take() {
            if kind == Kind::Paragraph && self.lines.of(at) == self.lines.of(end) {
                return;
            }
            self.paragraph(end);
        }
        match kind {
            Kind::Paragraph | Kind::TableRow => self.begin(at, None, true),
            Kind::AtxHeading | Kind::SetextHeading => self.begin(at, None, false),
            Kind::HtmlBlock => self.begin(at, None, false),
            Kind::ListItem => {
                let line = self.lines.of(at);
                let (section, parent) = (self.names(), self.enclosing());
                self.begin(at, Some(self.pass.raws.len()), false);
                self.pass.raws.push(Raw {
                    line,
                    end_line: line,
                    section,
                    parent,
                    block_id: None,
                    data: Data::Block,
                    key: String::new(),
                });
            }
            Kind::List => {
                let line = self.lines.of(at);
                if let Some(top) = self.open.last_mut()
                    && top.item.is_some()
                    && top.own_end.is_none()
                {
                    top.own_end = Some(line.saturating_sub(1).max(top.line));
                }
                self.begin(at, None, false);
            }
            Kind::BlockQuote | Kind::Callout => {
                self.quotes += 1;
                self.begin(at, None, false);
                if self.wants.asides {
                    let (section, parent) = (self.names(), self.enclosing());
                    self.asides.push(Aside {
                        section,
                        parent,
                        callout: None,
                        text: None,
                    });
                    self.seeking = true;
                }
            }
            Kind::FootnoteDefinition => self.begin(at, None, false),
            Kind::FencedCode | Kind::IndentedCode | Kind::Frontmatter => {
                self.verbatim += 1;
                self.begin(at, None, true);
            }
            Kind::Table => {
                if self.wants.collect_rows() {
                    self.pass.tables.push(Vec::new());
                }
                (self.rows, self.cols) = (0, 0);
                self.begin(at, None, false);
            }
            Kind::CalloutTitle => self.begin(self.marker.start, None, false),
            Kind::CodeSpan | Kind::InlineMath => self.code_at = at,
            // The note's own links are the flavour's.
            Kind::WikiLink if self.wants.links && self.flavour.has(Syntax::WikiLinks) => {
                self.wiki = Some(String::new())
            }
            Kind::Link => self.link = true,
            Kind::Image => self.image = true,
            _ => {}
        }
    }

    fn leave(&mut self, kind: Kind, at: usize) {
        if let Some(end) = self.pending.take() {
            self.paragraph(end);
        }
        match kind {
            Kind::Paragraph => self.pending = Some(at),
            // A GitHub alert's `[!NOTE]` was no paragraph to pulldown-cmark; any other callout's
            // title was.
            Kind::CalloutTitle => {
                if self.alert(at) {
                    self.open.pop();
                } else {
                    self.pending = Some(at);
                }
            }
            Kind::AtxHeading | Kind::SetextHeading => self.heading(at),
            Kind::ListItem => self.item(at),
            Kind::TableRow => self.row(at),
            Kind::TableDelimiterRow => self.header(),
            Kind::BlockQuote | Kind::Callout => {
                self.quotes = self.quotes.saturating_sub(1);
                let (block, _) = self.end();
                if let Some(aside) = self.asides.pop() {
                    self.seeking = self.asides.iter().any(|a| a.text.is_none());
                    self.aside(block, aside, at);
                }
            }
            Kind::FencedCode | Kind::IndentedCode => {
                self.verbatim = self.verbatim.saturating_sub(1);
                let (code, _) = self.end();
                if self.wants.typing {
                    self.pass.code.push(code.start..at);
                }
            }
            Kind::Frontmatter => {
                self.verbatim = self.verbatim.saturating_sub(1);
                self.end();
            }
            Kind::List | Kind::HtmlBlock | Kind::Table | Kind::FootnoteDefinition => {
                self.end();
            }
            Kind::CodeSpan | Kind::InlineMath if self.wants.typing => {
                self.pass.code.push(self.code_at..at)
            }
            Kind::Link => self.link = false,
            Kind::Image => self.image = false,
            Kind::WikiLink => {
                if let Some(wiki) = self.wiki.take()
                    && self.wants.links
                    && self.verbatim == 0
                {
                    let target = wiki.split('#').next().unwrap_or_default();
                    self.pass.links.extend(clean_target(target));
                }
            }
            _ => {}
        }
    }

    fn token(&mut self, kind: Kind, text: &str, at: usize) {
        if self.seeking {
            match kind {
                Kind::QuoteMarker | Kind::Whitespace | Kind::Newline => {}
                // `[!kind]`, `[!kind]+` or `[!kind]-`, as the reader takes it apart. The
                // callout's text starts after it; a quote around it starts at it.
                Kind::CalloutMarker => {
                    let name = text.trim_start_matches("[!");
                    let close = name.find(']').unwrap_or(name.len());
                    let start = at + (text.len() - name.len());
                    let fold = match &name[close..] {
                        "]-" => Some(true),
                        "]+" => Some(false),
                        _ => None,
                    };
                    if let Some(aside) = self.asides.last_mut() {
                        aside.callout = Some((start..start + close, fold));
                        aside.text = Some(at + text.len());
                    }
                    self.aside_text(at);
                }
                _ => self.aside_text(at),
            }
        }
        match kind {
            Kind::Whitespace => {
                if let Some(top) = self.open.last_mut()
                    && top.leading
                    && top.start == at
                {
                    top.start = at + text.len();
                }
            }
            Kind::HeadingMarker => self.level = text.len() as u8,
            Kind::SetextUnderline => self.level = if text.starts_with('=') { 1 } else { 2 },
            Kind::CalloutMarker => self.marker = at..at + text.len(),
            Kind::Delimiter => self.cols += 1,
            Kind::Tag if self.wants.tags => {
                self.pass.tags.push(text[1..].to_string());
                self.pass.written.tags.push(at + 1..at + text.len());
            }
            Kind::Target | Kind::Anchor => {
                if let Some(wiki) = &mut self.wiki {
                    wiki.push_str(text);
                }
            }
            // The tree says where the destination is in what the link points to. Where it
            // is none (`[a](B.md and more)`), the link names no note, here as in the index.
            Kind::LinkDest if self.link && !self.image && self.wants.links => {
                if let Some(dest) = cst::link_destination(text) {
                    self.md_link(&text[dest]);
                }
            }
            Kind::LinkLabel if self.link && !self.image && self.wants.links => {
                if let Some(dest) = self.defs.get(text) {
                    self.md_link(dest);
                }
            }
            _ => {}
        }
    }

    /// A Markdown link to `dest`: one of the note's links when it names a note.
    fn md_link(&mut self, dest: &str) {
        let target = dest.split('#').next().unwrap_or_default();
        if target.ends_with(".md") {
            self.pass.links.extend(clean_target(target));
        }
    }

    /// Whether the callout title ending at `end` is a GitHub alert's, `[!NOTE]` alone.
    fn alert(&self, end: usize) -> bool {
        let src = self.lines.src;
        let kind = src[self.marker.clone()]
            .strip_prefix("[!")
            .and_then(|k| k.strip_suffix(']'));
        let alerts = ["note", "tip", "important", "warning", "caution"];
        let is_alert = |k: &str| alerts.iter().any(|a| a.eq_ignore_ascii_case(k));
        self.flavour.has(Syntax::Alerts)
            && kind.is_some_and(is_alert)
            && src[self.marker.end..end].trim().is_empty()
    }

    /// Text starts at `at` for every aside the walk is inside that has none yet.
    fn aside_text(&mut self, at: usize) {
        for aside in &mut self.asides {
            aside.text.get_or_insert(at);
        }
        self.seeking = false;
    }

    /// A quote or a callout ends at `end`: a block for the types on `aside`. Its text is the
    /// rest of its first line, a callout's title or a quote's first words, and its lines are
    /// all of it.
    fn aside(&mut self, block: Open, aside: Aside, end: usize) {
        let lines = self.lines;
        let mut last = lines.of(end.saturating_sub(1)).max(block.line);
        while last > block.line && lines.blank(last) {
            last -= 1;
        }
        let text = match aside.text {
            Some(start) => {
                let stop = lines.end(lines.of(start)).max(start);
                let words = &lines.src[start..stop];
                let lead = words.len() - words.trim_start().len();
                start + lead..start + words.trim_end().len().max(lead)
            }
            None => block.start..block.start,
        };
        self.pass.candidates.push(Candidate {
            on: On::Aside,
            typeable: true,
            // The types in its text are found in the paragraphs it holds.
            own: text.start..text.start,
            text,
            raw: None,
            line: block.line,
            end_line: last,
            section: aside.section,
            parent: aside.parent,
            block_id: None,
            in_quote: self.quotes > 0,
            table: 0,
            cells: Vec::new(),
            given: matcher::Given {
                level: 0,
                callout: aside.callout,
            },
        });
    }

    fn heading(&mut self, end: usize) {
        let (block, _) = self.end();
        let level = self.level;
        while self.headings.last().is_some_and(|h| h.0 >= level) {
            self.headings.pop();
        }
        let src = self.lines.src;
        let range = block.start..end;
        let text = heading_text(&src[range.clone()]);
        let end_line = self.lines.of(end.saturating_sub(1)).max(block.line);
        let (section, parent) = (self.names(), self.enclosing());
        if self.wants.headings || self.wants.inline {
            let words = heading_range(src, range);
            self.pass.candidates.push(Candidate {
                on: On::Section,
                typeable: self.wants.headings,
                own: words.clone(),
                text: words,
                raw: Some(self.pass.raws.len()),
                line: block.line,
                end_line,
                section: section.clone(),
                parent,
                block_id: None,
                in_quote: self.quotes > 0,
                table: 0,
                cells: Vec::new(),
                given: matcher::Given {
                    level,
                    callout: None,
                },
            });
        }
        self.pass.raws.push(Raw {
            line: block.line,
            end_line,
            section,
            parent,
            block_id: None,
            data: Data::Heading {
                level,
                text: text.clone(),
            },
            key: String::new(),
        });
        self.headings.push((level, text));
    }

    fn item(&mut self, end: usize) {
        let (block, _) = self.end();
        let Some(at) = block.item else { return };
        let lines = self.lines;
        let src = lines.src;
        let mut last = lines.of(end.saturating_sub(1)).max(block.line);
        last = block.own_end.unwrap_or(last);
        while last > block.line && lines.blank(last) {
            last -= 1;
        }
        let first = src[block.start..].lines().next().unwrap_or_default();
        let (first, block_id) = if self.flavour.has(Syntax::BlockIds) {
            let own_last = if last == block.line {
                first
            } else {
                lines.line(last)
            };
            match markdown::split_block_id(own_last) {
                Some(cut) if last == block.line => (cut.before, Some(cut.id.to_string())),
                Some(cut) => (first, Some(cut.id.to_string())),
                None => (first, None),
            }
        } else {
            (first, None)
        };
        if self.wants.lists || self.wants.inline {
            let start = block.start + list_text_start(first);
            let end = block.start + first.trim_end().len();
            let own_end = start + src[start..lines.end(last)].trim_end().len();
            self.pass.candidates.push(Candidate {
                on: On::Item,
                typeable: self.wants.lists,
                own: start..own_end.max(start),
                text: start..end.max(start),
                raw: Some(at),
                line: block.line,
                end_line: last,
                section: self.pass.raws[at].section.clone(),
                parent: None,
                block_id: None,
                in_quote: self.quotes > 0,
                table: 0,
                cells: Vec::new(),
                given: matcher::Given::default(),
            });
        }
        let raw = &mut self.pass.raws[at];
        raw.end_line = last;
        raw.block_id = block_id;
        raw.data = Data::List {
            text: list_text(first),
        };
    }

    /// A paragraph ends at `end`; in a list item, it is part of the item.
    fn paragraph(&mut self, end: usize) {
        let (block, before) = self.end();
        if self.open.last().is_some_and(|p| p.item.is_some()) {
            return;
        }
        let lines = self.lines;
        let src = lines.src;
        let mut last = lines.of(end.saturating_sub(1)).max(block.line);
        while last > block.line && lines.blank(last) {
            last -= 1;
        }
        let ids = self.flavour.has(Syntax::BlockIds);
        let id = ids
            .then(|| markdown::split_block_id(lines.line(last)))
            .flatten();
        // A `^id` alone names the block before it.
        let alone = id
            .as_ref()
            .is_some_and(|cut| last == block.line && cut.before.is_empty());
        let (section, parent) = (self.names(), self.enclosing());
        let mut raw = None;
        if let Some(cut) = &id {
            let line = match before {
                Some(before) if alone => before,
                _ => block.line,
            };
            raw = Some(self.pass.raws.len());
            self.pass.raws.push(Raw {
                line,
                end_line: last,
                section: section.clone(),
                parent,
                block_id: Some(cut.id.to_string()),
                data: Data::Block,
                key: String::new(),
            });
        }
        if (self.wants.paragraphs || self.wants.inline) && !alone {
            let mut text = src[block.start..end].trim_end();
            if let Some(cut) = markdown::split_block_id(text)
                && id.is_some()
            {
                text = cut.before.trim_end();
            }
            let text = block.start..block.start + text.len();
            self.pass.candidates.push(Candidate {
                on: On::Text,
                typeable: self.wants.paragraphs,
                own: text.clone(),
                text,
                raw,
                line: block.line,
                end_line: last,
                section,
                parent,
                block_id: None,
                in_quote: self.quotes > 0,
                table: 0,
                cells: Vec::new(),
                given: matcher::Given::default(),
            });
        }
    }

    /// A table row ends at `end`. The first is the header, read once the delimiter row says how
    /// many columns the table has.
    fn row(&mut self, end: usize) {
        let (block, _) = self.end();
        self.rows += 1;
        if !self.wants.collect_rows() {
            return;
        }
        if self.rows == 1 {
            self.head = block.start;
            return;
        }
        let src = self.lines.src;
        let row = src[block.start..end].trim_end();
        let text = block.start..block.start + row.len();
        let cells = row_cells(src, block.start, self.cols)
            .into_iter()
            .map(|c| trim_cell(src, c))
            .collect();
        let (section, parent) = (self.names(), self.enclosing());
        self.pass.candidates.push(Candidate {
            on: On::Row,
            typeable: self.wants.rows,
            own: text.clone(),
            text,
            raw: None,
            line: block.line,
            end_line: block.line,
            section,
            parent,
            block_id: None,
            in_quote: self.quotes > 0,
            table: self.pass.tables.len().saturating_sub(1),
            cells,
            given: matcher::Given::default(),
        });
    }

    fn header(&mut self) {
        if !self.wants.collect_rows() {
            return;
        }
        let src = self.lines.src;
        let header = row_cells(src, self.head, self.cols)
            .into_iter()
            .map(|c| src[trim_cell(src, c)].to_string())
            .collect();
        if let Some(table) = self.pass.tables.last_mut() {
            *table = header;
        }
    }
}
