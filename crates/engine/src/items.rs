//! Notes as data: the core reads a note into typed items (tasks, list items, headings and
//! blocks with an id), so features ask for them instead of parsing notes themselves. See
//! "Notes as data" in docs/extensions.md.
//!
//! One walk through the note's syntax tree (`cst`), in the workspace's flavour, over the same
//! text the reader parses (without `%%comments%%`). The walk reads the blocks as pulldown-cmark,
//! which read them before, did, so ids, revs and spans stay what they were. Types match after
//! it: the walk collects the blocks some type reads, and `matcher` says which type each one is
//! and which inline types sit in its text. A task is a list item the built-in `task` type
//! (`types/task.toml`) matches.

use crate::cst::{self, Kind, Step};
use crate::daily;
use crate::markdown::{self, Flavour, Source, Syntax};
use crate::matcher::{self, Matcher, Typed};
use crate::tasks::{self, FNV_START, TasksSettings, fnv, fnv_from};
use crate::templates;
use crate::types::{self, On, Types};
use crate::workspace::clean_target;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

/// Everything about a workspace that changes how its notes read as items.
pub struct Settings {
    pub flavour: Flavour,
    pub tasks: TasksSettings,
    pub daily: daily::Settings,
    /// Notes in the templates folders aren't read: a template's tasks and types aren't real ones.
    pub templates: templates::Settings,
    /// The types its notes are matched against, and what was wrong with the rest.
    pub types: Types,
    pub matcher: Matcher,
}

impl Settings {
    pub fn load(root: &Path) -> Settings {
        Settings::load_with(root, types::for_workspace(root))
    }

    /// The workspace's settings, with these types.
    pub fn load_with(root: &Path, types: Types) -> Settings {
        Settings::new(
            markdown::flavour_for(root),
            TasksSettings::load(root),
            daily::Settings::load(root),
            templates::Settings::load(root),
            types,
        )
    }

    pub fn new(
        flavour: Flavour,
        tasks: TasksSettings,
        daily: daily::Settings,
        templates: templates::Settings,
        mut types: Types,
    ) -> Settings {
        // The plugin's core symbols first, so a status set by its type is written as they are.
        let mut statuses: Vec<(&char, &tasks::StatusType)> = tasks.statuses.iter().collect();
        statuses.sort_by_key(|(c, _)| {
            [' ', 'x', '/', '-']
                .iter()
                .position(|k| k == *c)
                .unwrap_or(usize::MAX)
        });
        types.fill(&types::Provided {
            statuses: statuses
                .into_iter()
                .map(|(c, t)| (c.to_string(), t.name().to_string()))
                .collect(),
            global_filter: tasks.global_filter.clone(),
            tasks_format: if tasks.dataview { "dataview" } else { "emoji" }.into(),
            daily_format: daily.format.clone(),
        });
        let matcher = Matcher::new(&types, &flavour, &daily);
        Settings {
            flavour,
            tasks,
            daily,
            templates,
            types,
            matcher,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    /// Stable across edits that keep the item's anchor, restarts and processes.
    pub id: String,
    /// Workspace-relative path, with `.md`.
    pub note: String,
    /// The item's own lines (1-based, inclusive); a list item's children are items of their own.
    pub line: usize,
    pub end_line: usize,
    /// The headings it sits under, outermost first.
    pub section: Vec<String>,
    /// The id of the list item it is nested in.
    pub parent: Option<String>,
    /// The author's `^id`.
    pub block_id: Option<String>,
    /// Fingerprint of the item's own lines: which version of it this is.
    pub rev: String,
    #[serde(flatten)]
    pub data: Data,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Data {
    List {
        text: String,
    },
    Heading {
        level: u8,
        text: String,
    },
    /// A paragraph, or the block before a `^id` line, named by its block id.
    Block,
    /// A block or note a type matched: `kind` is the type's name.
    #[serde(untagged)]
    Typed(Box<Typed>),
}

pub const KINDS: &[&str] = &["list", "heading", "block"];

impl Data {
    pub fn kind(&self) -> &str {
        match self {
            Data::List { .. } => "list",
            Data::Heading { .. } => "heading",
            Data::Block => "block",
            Data::Typed(t) => &t.kind,
        }
    }

    /// Whether `items KIND` lists it: its own kind, or a base it extends.
    pub fn answers_to(&self, kind: &str) -> bool {
        self.kind() == kind || matches!(self, Data::Typed(t) if t.is.iter().any(|b| b == kind))
    }
}

/// Flat, like every item: `kind`, then `is` when it extends a type, then each schema field.
impl Serialize for Typed {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        map.serialize_entry("kind", &self.kind)?;
        if !self.is.is_empty() {
            map.serialize_entry("is", &self.is)?;
        }
        for (name, value) in &self.fields {
            map.serialize_entry(&**name, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Typed {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Typed, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Typed;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a typed item's fields")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Typed, A::Error> {
                let mut t = Typed {
                    kind: String::new(),
                    is: Vec::new(),
                    fields: Default::default(),
                    spans: Default::default(),
                    unparsed: 0,
                    place: Default::default(),
                };
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "kind" => t.kind = map.next_value()?,
                        "is" => t.is = map.next_value()?,
                        _ => {
                            let value = map.next_value()?;
                            t.fields.insert(&key, value);
                        }
                    }
                }
                if t.kind.is_empty() {
                    return Err(serde::de::Error::missing_field("kind"));
                }
                Ok(t)
            }
        }
        d.deserialize_map(V)
    }
}

/// An item before its id: an id needs its parent's, so ids are given once the note is read.
struct Raw {
    line: usize,
    end_line: usize,
    section: Vec<String>,
    parent: Option<usize>,
    block_id: Option<String>,
    data: Data,
    /// A typed item's key: its shape's `key` fields, or what tells its primitive apart.
    key: String,
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
            rows: want(On::TableRow),
            paragraphs: want(On::Paragraph),
            lists: want(On::ListItem),
            headings: want(On::Heading),
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

/// The items of the note at workspace-relative `note` (with `.md`) whose text is `text`.
pub fn extract(note: &str, text: &str, s: &Settings) -> Vec<Item> {
    if s.templates.is_template(note) {
        return Vec::new();
    }
    // The comments are taken out first, as for the reader, so their lines read as blank.
    // No item is made of what text itself holds (emphasis and the like), so it isn't read.
    let gates = cst::Gates::of(&s.flavour).plain();
    let gates = gates.set(Syntax::Comments, false);
    let source = s.flavour.source_with(text, |text| code_in(text, gates));
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

/// The links a block's own `links` are read from, of those the Markdown owner found: where
/// each is written, whether as a Markdown link, and the note it names.
fn own_links(links: Vec<crate::owner::markdown::Link<'_>>) -> Vec<(usize, bool, String)> {
    let links = (links.iter()).filter(|l| l.by != crate::owner::markdown::By::Definition);
    links
        .filter_map(|l| Some((l.at, l.md(), l.note()?)))
        .collect()
}

/// The code spans, code blocks and math in `text`, where `%%` is no comment.
fn code_in(text: &str, gates: cst::Gates) -> Vec<Range<usize>> {
    let mut code = Vec::new();
    let mut from = 0;
    cst::read(text, gates, &mut |step| match step {
        Step::Enter(kind, at) if is_code(kind) => from = at,
        Step::Leave(kind, at) if is_code(kind) => code.push(from..at),
        _ => {}
    });
    code
}

fn is_code(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::CodeSpan | Kind::InlineMath | Kind::MathBlock | Kind::FencedCode | Kind::IndentedCode
    )
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
    /// The paragraph just read, which pulldown-cmark read on into a paragraph or `$$` math on
    /// the next line: where it ends, until the next block says whether it goes on. A callout's
    /// title starts one.
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
        if kind == Kind::MathBlock {
            self.code_at = at;
        }
        if let Some(end) = self.pending.take() {
            let goes_on = matches!(kind, Kind::Paragraph | Kind::MathBlock);
            if goes_on && self.lines.of(at) == self.lines.of(end) {
                return;
            }
            self.paragraph(end);
        }
        match kind {
            Kind::Paragraph | Kind::MathBlock | Kind::TableRow => self.begin(at, None, true),
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
            // `$$` math was a paragraph's text to pulldown-cmark.
            Kind::MathBlock => {
                if self.wants.typing {
                    self.pass.code.push(self.code_at..at);
                }
                self.pending = Some(at);
            }
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
                self.end();
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
            Kind::LinkDest if self.link && !self.image && self.wants.links => {
                let dest = match text.strip_prefix('<') {
                    Some(pointy) => pointy.split('>').next(),
                    None => text.split_whitespace().next(),
                };
                self.md_link(dest.unwrap_or_default());
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
                on: On::Heading,
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
            match markdown::block_id_pattern().captures(own_last) {
                Some(c) if last == block.line => {
                    (&first[..c.get(0).unwrap().start()], Some(c[1].to_string()))
                }
                Some(c) => (first, Some(c[1].to_string())),
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
                on: On::ListItem,
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
            .then(|| markdown::block_id_pattern().captures(lines.line(last)))
            .flatten();
        // A `^id` alone names the block before it.
        let alone = id
            .as_ref()
            .is_some_and(|c| last == block.line && c.get(0).unwrap().start() == 0);
        let (section, parent) = (self.names(), self.enclosing());
        let mut raw = None;
        if let Some(c) = &id {
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
                block_id: Some(c[1].to_string()),
                data: Data::Block,
                key: String::new(),
            });
        }
        if (self.wants.paragraphs || self.wants.inline) && !alone {
            let mut text = src[block.start..end].trim_end();
            if let Some(c) = markdown::block_id_pattern().captures(text)
                && id.is_some()
            {
                text = text[..c.get(0).unwrap().start()].trim_end();
            }
            let text = block.start..block.start + text.len();
            self.pass.candidates.push(Candidate {
                on: On::Paragraph,
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
            on: On::TableRow,
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

/// Matches the types over what a pass found, then gives each item its id.
fn finish(
    note: &str,
    text: &str,
    s: &Settings,
    source: &Source,
    lines: &Lines,
    pass: Pass,
) -> Vec<Item> {
    let Pass {
        mut raws,
        mut candidates,
        code,
        tables,
        tags,
        links,
        written,
    } = pass;
    let m = &s.matcher;
    if !m.is_empty() {
        let src = lines.src;
        let want_inline = m.reads(On::Inline);
        let mut found = matcher::Note::new(note, text, source, m.needs_links);
        for tag in &tags {
            found.add_tag(tag);
        }
        found.links.extend(links);
        found.written = written;
        let gate = m.gate(&found);
        let mut note_is: Vec<String> = Vec::new();
        if let Some((typed, key)) = m.note(&gate, &found) {
            note_is.push(typed.kind.clone());
            note_is.extend(typed.is.iter().cloned());
            raws.push(Raw {
                line: 1,
                end_line: found.frontmatter.as_ref().map_or(1, |f| f.end_line),
                section: Vec::new(),
                parent: None,
                block_id: None,
                data: Data::Typed(Box::new(typed)),
                key,
            });
        }
        // In document order, so a list item is typed before the items inside it ask.
        candidates.sort_by_key(|c| c.text.start);
        // (type, where) of each inline match, so a block inside a list item's lines (a quote,
        // a table) doesn't find it twice.
        let mut inline_seen: std::collections::HashSet<(String, usize)> = Default::default();
        for c in candidates {
            // What it sits inside, when some type asks.
            let ask = m.needs_inside;
            let mut inside = if ask { note_is.clone() } else { Vec::new() };
            if ask && c.in_quote {
                inside.push("quote".into());
            }
            if let Some(p) = c.parent.filter(|_| ask) {
                inside.push("list-item".into());
                inside.push(raws[p].data.kind().to_string());
                if let Data::Typed(t) = &raws[p].data {
                    inside.extend(t.is.iter().cloned());
                }
            }
            let matched = c
                .typeable
                .then(|| {
                    m.block(
                        &gate,
                        &found,
                        &matcher::Block {
                            on: c.on,
                            text: &src[c.text.clone()],
                            at: c.text.start,
                            section: &c.section,
                            header: tables.get(c.table).map_or(&[], Vec::as_slice),
                            cells: &c.cells,
                            inside: inside.iter().map(String::as_str).collect(),
                            code: &code,
                        },
                    )
                })
                .flatten();
            // The item the block is, which its inline items name as their parent.
            let mut host = c.raw;
            if let Some((typed, key)) = matched {
                match c.raw {
                    Some(i) => {
                        raws[i].data = Data::Typed(Box::new(typed));
                        raws[i].key = key;
                    }
                    None => {
                        host = Some(raws.len());
                        raws.push(Raw {
                            line: c.line,
                            end_line: c.end_line,
                            section: c.section.clone(),
                            parent: c.parent,
                            block_id: c.block_id.clone(),
                            data: Data::Typed(Box::new(typed)),
                            key,
                        });
                    }
                }
            }
            if !want_inline {
                continue;
            }
            if ask {
                inside.push(c.on.name().into());
            }
            if let Some(h) = host.filter(|_| ask) {
                inside.push(raws[h].data.kind().to_string());
                if let Data::Typed(t) = &raws[h].data {
                    inside.extend(t.is.iter().cloned());
                }
            }
            let block = matcher::Block {
                on: On::Inline,
                text: &src[c.own.clone()],
                at: c.own.start,
                section: &c.section,
                header: &[],
                cells: &[],
                inside: inside.iter().map(String::as_str).collect(),
                code: &code,
            };
            for (mut typed, key, r) in m.inline(&gate, &found, &block) {
                let written = &block.text[r.clone()];
                let start = c.own.start + r.start + (written.len() - written.trim_start().len());
                let end = c.own.start + r.start + written.trim_end().len();
                if !inline_seen.insert((typed.kind.clone(), start)) {
                    continue;
                }
                typed.place.span = source.file_range(start..end);
                let line = lines.of(start);
                raws.push(Raw {
                    line,
                    end_line: lines.of(end.saturating_sub(1)).max(line),
                    section: c.section.clone(),
                    parent: host,
                    block_id: None,
                    data: Data::Typed(Box::new(typed)),
                    key,
                });
            }
        }
    }
    identify(note, text, raws)
}

/// Gives each item its id and rev, in document order (a parent before its children).
fn identify(note: &str, text: &str, raws: Vec<Raw>) -> Vec<Item> {
    let lines: Vec<&str> = text.lines().collect();
    let mut keys: Vec<String> = Vec::with_capacity(raws.len());
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    let mut items: Vec<Item> = Vec::with_capacity(raws.len());
    // An item's own lines, joined, reused from item to item.
    let mut own = String::new();
    for raw in raws {
        let kind = raw.data.kind();
        let parent_key = raw.parent.map_or("", |p| keys[p].as_str());
        let anchor = match (&raw.block_id, &raw.data) {
            (Some(id), _) => format!("^{id}"),
            (None, Data::Heading { text, .. }) => {
                let mut path = raw.section.join("#");
                if !raw.section.is_empty() {
                    path.push('#');
                }
                path.push_str(text);
                path
            }
            (None, Data::List { text }) => format!("{parent_key}/{:016x}", fnv(text.as_bytes())),
            (None, Data::Typed(_)) if raw.key.is_empty() => parent_key.to_string(),
            (None, Data::Typed(_)) => format!("{parent_key}/{:016x}", fnv(raw.key.as_bytes())),
            (None, Data::Block) => String::new(),
        };
        let n = seen.entry((kind.to_string(), anchor.clone())).or_default();
        let key = match *n {
            0 => anchor,
            n => format!("{anchor}.{n}"),
        };
        *n += 1;
        let id = [note, "\0", kind, "\0", &key]
            .iter()
            .fold(FNV_START, |h, part| fnv_from(h, part.as_bytes()));
        own.clear();
        for (i, line) in (lines.get(raw.line - 1..raw.end_line.min(lines.len())))
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            if i > 0 {
                own.push('\n');
            }
            own.push_str(line);
        }
        items.push(Item {
            id: format!("{id:016x}"),
            note: note.to_string(),
            line: raw.line,
            end_line: raw.end_line,
            section: raw.section,
            parent: raw.parent.map(|p| items[p].id.clone()),
            block_id: raw.block_id,
            rev: tasks::line_id(&own),
            data: raw.data,
        });
        keys.push(key);
    }
    items.sort_by_key(|i| i.line);
    items
}

/// A heading's words in the source: ATX without its `#`s, setext without its underline.
fn heading_range(src: &str, range: Range<usize>) -> Range<usize> {
    let raw = &src[range.clone()];
    let first = raw.lines().next().unwrap_or_default();
    let indent = first.len() - first.trim_start().len();
    if first.trim_start().starts_with('#') {
        let after = first[indent..].trim_start_matches('#');
        let start = range.start + first.len() - after.trim_start().len();
        let mut words = after.trim();
        let closed = words.trim_end_matches('#');
        if closed.is_empty() || closed.ends_with([' ', '\t']) {
            words = closed.trim_end();
        }
        start..start + words.len()
    } else {
        let body = raw.trim_end();
        let end = body.rfind('\n').unwrap_or(0);
        let words = body[..end].trim();
        let start = range.start + body.find(words).unwrap_or(0);
        start..start + words.len()
    }
}

/// The cells of the table row starting at `start`, as pulldown-cmark split them: at each `|`
/// that isn't escaped, a wikilink's too, then cut to the table's `cols` or filled with empty
/// cells after the row's line break.
fn row_cells(src: &str, start: usize, cols: usize) -> Vec<Range<usize>> {
    let b = src.as_bytes();
    let eol = src[start..]
        .find(['\n', '\r'])
        .map_or(src.len(), |i| start + i);
    let after = match &b[eol..] {
        [b'\r', b'\n', ..] => eol + 2,
        [_, ..] => eol + 1,
        [] => eol,
    };
    let mut cells = Vec::new();
    let mut i = start;
    loop {
        if b.get(i) == Some(&b'|') {
            i += 1;
        }
        let cell = i;
        while i < eol && matches!(b[i], b' ' | b'\t') {
            i += 1;
        }
        if i >= eol {
            break;
        }
        while i < eol && b[i] != b'|' {
            let escape = b[i] == b'\\' && b.get(i + 1).is_some_and(u8::is_ascii_punctuation);
            i += match (escape, b.get(i + 1..i + 3)) {
                (true, Some(b"\\|")) => 3,
                (true, _) => 2,
                _ => 1,
            };
        }
        cells.push(cell..i);
    }
    cells.truncate(cols);
    cells.resize(cols, after..after);
    cells
}

/// A table cell without its padding and pipes.
fn trim_cell(src: &str, range: Range<usize>) -> Range<usize> {
    let pad = [' ', '\t', '|', '\r', '\n'];
    let cell = &src[range.clone()];
    let start = range.start + cell.len() - cell.trim_start_matches(pad).len();
    let end = range.end - (cell.len() - cell.trim_end_matches(pad).len());
    start..end.max(start)
}

/// Where a list item's words start on its first line: after its marker and the space.
fn list_text_start(first: &str) -> usize {
    let t = first.trim_start();
    let marker = t.strip_prefix(['-', '*', '+']).unwrap_or_else(|| {
        t.trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')'])
    });
    first.len() - marker.trim_start().len()
}

/// A heading's text as written: ATX without its `#`s, setext without its underline.
fn heading_text(raw: &str) -> String {
    let raw = raw.trim();
    let text = match raw.strip_prefix('#') {
        Some(_) => {
            let t = raw
                .lines()
                .next()
                .unwrap_or_default()
                .trim_start_matches('#');
            let closed = t.trim_end().trim_end_matches('#');
            if closed.is_empty() || closed.ends_with([' ', '\t']) {
                closed.to_string()
            } else {
                t.to_string()
            }
        }
        None => {
            let lines: Vec<&str> = raw.lines().collect();
            lines[..lines.len().saturating_sub(1)].join(" ")
        }
    };
    text.trim().to_string()
}

/// A list item's first line without its marker.
fn list_text(first: &str) -> String {
    let t = first.trim_start();
    let t = t
        .strip_prefix(['-', '*', '+'])
        .unwrap_or_else(|| t.trim_start_matches(|c: char| c.is_ascii_digit()))
        .trim_start_matches(['.', ')']);
    let mut text = String::with_capacity(t.len());
    matcher::push_words(&mut text, t);
    text
}

#[cfg(test)]
mod pulldown;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn settings(flavour: &str) -> Settings {
        let nowhere = Path::new("/nonexistent-cold-lava-workspace");
        Settings::new(
            Flavour::built_in(flavour).unwrap(),
            TasksSettings::load(nowhere),
            daily::Settings::load(nowhere),
            templates::Settings::default(),
            types::load([types::Layer::built_in()]),
        )
    }

    /// A typed item's text field.
    fn field<'a>(i: &'a Item, name: &str) -> &'a str {
        match &i.data {
            Data::Typed(t) => t.fields.get(name).and_then(|v| v.as_str()).unwrap_or(""),
            _ => "",
        }
    }

    fn items(text: &str) -> Vec<Item> {
        extract("Note.md", text, &settings("obsidian"))
    }

    /// (kind, line, end_line, short text)
    fn summary(items: &[Item]) -> Vec<(&'static str, usize, usize, String)> {
        items
            .iter()
            .map(|i| {
                let text = match &i.data {
                    Data::Typed(t) if t.kind == "task" => {
                        format!("[{}] {}", field(i, "status"), field(i, "text"))
                    }
                    Data::List { text } => text.clone(),
                    Data::Heading { text, .. } => text.clone(),
                    Data::Block => format!("^{}", i.block_id.as_deref().unwrap_or("")),
                    Data::Typed(t) => t.kind.clone(),
                };
                let kind = match &i.data {
                    Data::Typed(t) if t.kind == "task" => "task",
                    Data::Typed(_) => "typed",
                    d => KINDS.iter().find(|k| **k == d.kind()).unwrap(),
                };
                (kind, i.line, i.end_line, text)
            })
            .collect()
    }

    fn s(
        kind: &'static str,
        line: usize,
        end: usize,
        text: &str,
    ) -> (&'static str, usize, usize, String) {
        (kind, line, end, text.to_string())
    }

    fn by_text<'a>(items: &'a [Item], text: &str) -> &'a Item {
        items
            .iter()
            .find(|i| match &i.data {
                Data::List { text: t } | Data::Heading { text: t, .. } => t == text,
                Data::Typed(_) => field(i, "text") == text,
                Data::Block => false,
            })
            .unwrap_or_else(|| panic!("no item {text:?} in {:?}", summary(items)))
    }

    #[test]
    fn reads_headings_lists_tasks_and_blocks() {
        let note = "\
# Trip
Intro paragraph ^intro

## Packing
- [ ] Boots 📅 2026-11-12
  - socks
  - [x] laces
    continued
- Map ^map

| a | b |
|---|---|
| 1 | 2 |

^table

Setext
------
";
        let all = items(note);
        assert_eq!(
            summary(&all),
            [
                s("heading", 1, 1, "Trip"),
                s("block", 2, 2, "^intro"),
                s("heading", 4, 4, "Packing"),
                s("task", 5, 5, "[ ] Boots"),
                s("list", 6, 6, "socks"),
                s("task", 7, 8, "[x] laces"),
                s("list", 9, 9, "Map"),
                s("block", 11, 15, "^table"),
                s("heading", 17, 18, "Setext"),
            ]
        );
        let boots = by_text(&all, "Boots");
        assert_eq!(boots.section, ["Trip", "Packing"]);
        assert_eq!(boots.parent, None);
        assert_eq!(by_text(&all, "socks").parent.as_ref(), Some(&boots.id));
        assert_eq!(by_text(&all, "laces").parent.as_ref(), Some(&boots.id));
        assert_eq!(by_text(&all, "Map").block_id.as_deref(), Some("map"));
        assert_eq!(by_text(&all, "Packing").section, ["Trip"]);
        assert_eq!(
            by_text(&all, "Setext").section,
            ["Trip"],
            "a level 2 heading ends Packing"
        );
        let ids: std::collections::HashSet<&str> = all.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids.len(), all.len(), "ids are unique");
    }

    #[test]
    fn code_and_comments_hold_no_items_but_callouts_do() {
        let note = "\
~~~
- [ ] in a tilde fence
~~~
```
- [ ] in a backtick fence
```
%% - [ ] in a comment
- [ ] still in it %%
- [ ] real %%with a comment%%
> [!todo] Callout
> - [ ] in a callout 📅 2026-11-12
> > - [/] in a nested quote
";
        let all = items(note);
        assert_eq!(
            summary(&all),
            [
                s("task", 9, 9, "[ ] real"),
                s("task", 11, 11, "[ ] in a callout"),
                s("task", 12, 12, "[/] in a nested quote"),
            ]
        );
        assert_eq!(
            all[0].rev,
            tasks::line_id("- [ ] real %%with a comment%%"),
            "rev is of the line as written"
        );
    }

    #[test]
    fn custom_statuses_are_tasks() {
        let all = items("- [/] going\n- [-] dropped\n- [?] unsure\n- [ ] \n");
        let types: Vec<_> = all
            .iter()
            .map(|i| (i.data.kind() == "task").then(|| field(i, "status_type")))
            .collect();
        assert_eq!(
            types,
            [
                Some("in_progress"),
                Some("cancelled"),
                Some("non_task"),
                None,
            ],
            "an empty checkbox is a list item"
        );
    }

    #[test]
    fn the_flavour_decides_what_is_read() {
        let note = "- [ ] task ^b1\n\nPara ^p1\n";
        let kinds = |flavour: &str| {
            summary(&extract("Note.md", note, &settings(flavour)))
                .into_iter()
                .map(|s| (s.0, s.3))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds("obsidian"),
            [("task", "[ ] task".into()), ("block", "^p1".into())]
        );
        assert_eq!(kinds("github"), [("task", "[ ] task ^b1".into())]);
        assert_eq!(kinds("commonmark"), [("list", "[ ] task ^b1".into())]);
    }

    #[test]
    fn ids_survive_ticking_redating_indenting_and_lines_above() {
        let before =
            "- [ ] Pay rent 📅 2026-11-01\n  - [ ] Transfer\n- [ ] Pay rent 📅 2026-12-01\n";
        let id = |text: &str, which: &str, nth: usize| {
            let all = items(text);
            all.iter()
                .filter(|i| i.data.kind() == "task" && field(i, "text") == which)
                .nth(nth)
                .unwrap()
                .id
                .clone()
        };
        let rent = id(before, "Pay rent", 0);
        let second = id(before, "Pay rent", 1);
        let child = id(before, "Transfer", 0);
        assert_ne!(rent, second, "identical siblings get their own ids");
        for after in [
            "- [x] Pay rent 📅 2026-11-01 ✅ 2026-11-02\n  - [x] Transfer\n- [ ] Pay rent 📅 2026-12-01\n",
            "- [ ] Pay rent 📅 2026-11-08 ⏫\n  - [ ] Transfer 🛫 2026-11-07\n- [ ] Pay rent 📅 2026-12-01\n",
            "  - [ ] Pay rent 📅 2026-11-01\n\t- [ ] Transfer\n  - [ ] Pay rent 📅 2026-12-01\n",
            "# Bills\n\nSome words.\n\n- [ ] Pay rent 📅 2026-11-01\n  - [ ] Transfer\n- [ ] Pay rent 📅 2026-12-01\n",
        ] {
            assert_eq!(id(after, "Pay rent", 0), rent, "{after}");
            assert_eq!(id(after, "Pay rent", 1), second, "{after}");
            assert_eq!(id(after, "Transfer", 0), child, "{after}");
        }
        let edited =
            "- [ ] Pay the rent 📅 2026-11-01\n  - [ ] Transfer\n- [ ] Pay rent 📅 2026-12-01\n";
        assert_ne!(id(edited, "Pay the rent", 0), rent, "new words, new id");
        assert_ne!(
            id(edited, "Transfer", 0),
            child,
            "its parent's words changed"
        );
        let anchored = "- [ ] Pay rent ^rent\n";
        let renamed = "- [x] Pay the landlord ^rent\n";
        assert_eq!(
            items(anchored)[0].id,
            items(renamed)[0].id,
            "a block id anchors it"
        );
        assert_eq!(
            items(anchored)[0].id,
            extract("Note.md", anchored, &settings("obsidian"))[0].id,
            "ids don't depend on the process"
        );
        assert_ne!(
            items(anchored)[0].id,
            extract("Other.md", anchored, &settings("obsidian"))[0].id,
            "ids include the note"
        );
    }

    #[test]
    fn rev_is_the_line_id_of_a_one_line_task() {
        let line = "- [ ] Pay rent 📅 2026-11-01";
        let all = items(&format!("intro\n\n{line}\n"));
        assert_eq!(all[0].rev, tasks::line_id(line));
        let two = items("- [ ] Pay rent\n  by transfer\n");
        assert_eq!(two[0].rev, tasks::line_id("- [ ] Pay rent\n  by transfer"));
    }

    #[test]
    fn daily_notes_date_their_tasks() {
        let s = settings("obsidian");
        let all = extract("2026-11-05.md", "- [ ] Water plants\n", &s);
        assert_eq!(field(&all[0], "daily_note"), "2026-11-05");
    }

    #[test]
    fn item_json_is_pinned() {
        let all = items(
            "# Plan\n- [/] Call [[Ana]] 📅 2026-11-12 ^call\n  - bring notes\n\nAside ^aside\n",
        );
        let id = |n: usize| all[n].id.clone();
        let rev = |n: usize| all[n].rev.clone();
        let wire = [
            json!({
                "id": id(0), "note": "Note.md", "line": 1, "end_line": 1, "section": [],
                "parent": null, "block_id": null, "rev": rev(0),
                "kind": "heading", "level": 1, "text": "Plan"
            }),
            json!({
                "id": id(1), "note": "Note.md", "line": 2, "end_line": 2, "section": ["Plan"],
                "parent": null, "block_id": "call", "rev": rev(1),
                "kind": "task", "status": "/", "status_type": "in_progress",
                "text": "Call [[Ana]]", "due": "2026-11-12", "scheduled": null, "start": null,
                "created": null, "done": null, "cancelled": null, "recurrence": null,
                "priority": null, "tags": [], "links": ["Ana"], "daily_note": null
            }),
            json!({
                "id": id(2), "note": "Note.md", "line": 3, "end_line": 3, "section": ["Plan"],
                "parent": id(1), "block_id": null, "rev": rev(2),
                "kind": "list", "text": "bring notes"
            }),
            json!({
                "id": id(3), "note": "Note.md", "line": 5, "end_line": 5, "section": ["Plan"],
                "parent": null, "block_id": "aside", "rev": rev(3),
                "kind": "block"
            }),
        ];
        assert_eq!(all.len(), wire.len());
        for (item, json) in all.iter().zip(wire) {
            assert_eq!(serde_json::to_value(item).unwrap(), json);
            let back = serde_json::from_value::<Item>(json.clone()).unwrap();
            assert_eq!(serde_json::to_value(back).unwrap(), json, "it reads back");
        }
        assert!(all.iter().all(|i| i.id.len() == 16 && i.rev.len() == 12));
    }

    /// Types that read every kind of block, the note's tags and links, and what a block is in.
    const EVERY_TYPE: &str = r#"
[page]
on = "note"
where.file-type = ["md"]
fields.tags = { inlines = "tags", type = "tag" }
fields.links = { inlines = "links", type = "link" }

[para]
on = "paragraph"
fields.text = { rest = true }

[head]
on = "heading"
fields.title = { rest = true }

[line]
on = "list-item"
fields.what = { rest = true }

[row]
on = "table-row"
fields.links = { inlines = "links", type = "link" }

[word]
on = "inline"
match = '\bthe (?<w>\w+)'
fields.w = { capture = "w" }

[quoted]
on = "inline"
match = '\ba (?<w>\w+)'
where.inside = ["quote"]
fields.w = { capture = "w" }
"#;

    fn every_type(flavour: &str) -> Settings {
        let table: toml::Table = toml::from_str(EVERY_TYPE).unwrap();
        let found = types::load([
            types::Layer::built_in(),
            types::Layer::config(Path::new("config.toml"), table),
        ]);
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        let nowhere = Path::new("/nonexistent-cold-lava-workspace");
        Settings::new(
            Flavour::built_in(flavour).unwrap(),
            TasksSettings::load(nowhere),
            daily::Settings::load(nowhere),
            templates::Settings::default(),
            found,
        )
    }

    /// Markdown whose blocks are easy to read differently from pulldown-cmark.
    const TRICKY: &[&str] = &[
        // Lazy lines.
        "- [ ] task\nlazy line\n- [ ] next\n\n> quote\nlazy in quote ^q1\n\nPara\nlazy\n    still para ^p1\n",
        // Loose and tight lists.
        "- a\n\n- b\n  \n  more b\n- c\n\n\n- d after two blank lines\n\n1. one\n2. two\n\n   para in two ^two\n3) other delimiter\n",
        // Indented code.
        "Para\n\n    code ^not\n    - [ ] not a task\n\n- item\n\n      code in item\n- [ ] after code\n\n\tTabbed code\n",
        // HTML blocks.
        "<div>\n- [ ] in html\n\n- [ ] after html\n<!-- comment\n- [ ] in comment\n-->\n<details><summary>x</summary>\n\nInside details ^d1\n\n</details>\n<span>inline</span> text ^s1\n<custom-tag>\n- [ ] after a lone tag\n",
        // Block ids.
        "First para\n\n^alone\n\n- item ^i1\n  continued ^i2\n- item two\n  ^i3\n\n# Heading ^h1\n\n> quoted ^qq\n> ^quote-alone\n\n| a | b |\n|---|---|\n| 1 | 2 |\n^table-id\n\nText\n^glued\n\n***\n\n^after-rule\n```\ncode\n```\n^after-code\n",
        // Nested lists under headings.
        "# One\n## Two\n- a\n  - b\n    - [ ] c\n      - d\n  - e\n### Three\n- [x] f\n  1. g\n  2. h\n# Four\n- i\n- i\n",
        // Headings.
        "#NoSpace\n# Closed #\n## Trailing ##   \n#\n   ### Indented\nSetext one\nline two\n===\n> Quoted setext\n> ---\n- # In an item\n  Setext in an item\n  ---\n",
        // Tables.
        "| Name | VEI | Note |\n|---|:-:|--:|\n| [[Krakatoa]] the big | 6 | big |\n| a \\| b | 2 |\n| x | y | z | extra |\nno pipes row\n\n> | q | r |\n> |---|---|\n> | 1 | 2 |\n\n| [[A]] | c |\n| --- | --- |\n| d | [[E]] |\n|| empty |\n  | indented | row |\n",
        // Fences.
        "~~~md\n- [ ] in tilde\n~~~\n````\n```\n- [ ] in nested\n```\n````\n- item\n  ```\n  - [ ] in item fence\n  ```\n- [ ] after\n```\nunclosed\n- [ ] in unclosed\n",
        // Callouts.
        "> [!note] Title\n> body text ^c1\n\n> [!todo]- Folded\n> - [ ] in callout\n\n> [!info] Alone ^c2\n\n> [!NOTE]\n> GitHub alert body ^c3\n\n> [!tip]\n>\n> after blank ^c4\n\n> [!quote] the title\n> the body\n> > [!note] nested\n> > nested body ^c5\n",
        // Comments.
        "Text %%hidden%% more ^k1\n%%\n- [ ] commented\n%%\n- [ ] visible %%note%%\nPara\n%%inline only%%\nmore ^k2\n``%%code%%`` stays\n",
        // Math.
        "$$\nE = mc^2\n$$\n^eq1\n\nInline $x$ and $$y$$ VEI 4 the end\n\n$$\na\n$$ ^eq2\n",
        // Frontmatter, and the body's tags and links.
        "---\ntags: [a, b]\nlinks: \"[[X]]\"\n---\n# Body #tag\nSee [[Note]], [[Note#H|alias]], ![[Embed]], [md](Other%20Note.md), [title](Some.md \"t\"), [web](https://x.y), ![img](pic.md), [anchor](#local), `[[code]]` and #tag, #nested/tag, (#paren), #123, a#not ^b1\n",
        // Line breaks, tabs, empty items, markers.
        "# Title\r\n- [ ] crlf task\r\n  more\r\n\r\nPara ^crlf\r\n",
        "- [ ] last line without a break",
        "-\ttab item\n\t- nested by a tab\n*\tstar\n",
        "-\n  content below\n- \n-   \n\n1.\n",
        "- a\n+ b\n* c\n1. d\n2) e\n",
        "Para\n- [ ] item interrupts\nPara2\n2. no interruption\nPara3\n# heading interrupts\n",
        "> a\n> > b ^n1\n> c\n\n> - [ ] task in a quote\n>   - nested\n> lazy\n",
        "- a\n---\n* * *\n- b\n",
        "- [ ] \n- [x]\n- [ ]no space\n- [/] in progress\n  - [-] cancelled child\n- [  ] two spaces\n",
        // A table that starts with `|` ends the paragraph above it, not a lazy one after a list
        // item; one that doesn't start with `|` doesn't.
        "Caption line\n| a | b |\n|---|---|\n| 1 | the 2 |\n\n> quoted caption\n> | q | r |\n> |---|---|\n> | 3 | 4 |\n\n- item\n| x | y |\n|---|---|\n| 5 | 6 |\n\nNot heavy\nc | d\n---|---\n7 | 8\n\n> [!info] Stats\n> | a | b |\n> |---|---|\n> | 1 | 2 |\n",
        // Reference links: full, collapsed and shortcut, an image, a label in another case, a
        // label nothing defines, a label defined twice.
        "See [the note][n], [N][], [n] and ![pic][n]; [web][w], [missing][y] and [y].\n\n[n]: Note.md\n[w]: https://example.com \"Title\"\n[N]: Other.md\n",
        // Definitions in a quote and in a list item, and before a paragraph's text.
        "> [q]: Quoted.md\n> the text with [q]\n\n- [i]: Item.md\n- see [i] and [Q]\n\n[t]: <Top Note.md> 'title'\nSetext after a definition, [t]\n===\n",
        // Footnote definitions hold blocks: a second paragraph indented under a blank line,
        // a list, a task, a fenced block, and the paragraph after one, which isn't its.
        "Text[^1] here.\n\n[^1]: First paragraph. ^f1\n\n    Second paragraph the same note. ^f2\n\nAfter the note.\n",
        "[^1]: Note with a list:\n\n    - a\n    - b ^li\n\n[^t]: Tasks:\n\n    - [ ] a task in a footnote\n    - [x] done\n",
        "[^c]: Code:\n\n    ```\n    - [ ] not a task\n    ```\n\n    after the code ^ac\n",
        "[^1]: The note.\nlazy line of it ^lz\n\n[^2]: Another.\n\nUnindented paragraph the end. ^un\n",
        // Two definitions in a row, right under a paragraph, and definitions in a quote.
        "Para the first ^p\n[^1]: One. ^one\n[^2]: Two. ^two\n",
        "> [^q]: In a quote the note. ^q1\n>\n>     Second in the quote. ^q2\n\n> text\n> [^r]: after quoted text ^r1\n",
        // A definition right under a list item ends the list, and what is indented under it
        // is its, not the item's. A `===` or `---` under a definition's line is no setext
        // underline: the heading the tree read there before #212 never was one.
        "- item one\n- item two\n[^1]: right after a list item\n    - [ ] continuation task [[Tgt]]\n",
        "[^1]: def line\n===\n\n[^2]: def two\n---\n\nParagraph above. ^above\n[^n]: Note text ^on\n\n^standalone\n",
        // Tags right after the markers that open emphasis and a strikethrough.
        "Read **#bold** and _#slanted_, *#star*, __#under__, ***#both*** and ~~#gone~~, but a#b and http://x.y/#frag are no tags.\n",
    ];

    /// Markdown pulldown-cmark read otherwise than Obsidian shows it, where the items differ
    /// from the ones it gave.
    const DIFFERENT: &[&str] = &[
        // A frontmatter whose first line is blank, as the Kanban plugin writes it, was a rule
        // and Markdown.
        "---\n\nkanban-plugin: basic\n- [ ] YAML\n\n---\n\n- [ ] Pack\n",
        // A `---` line after a blank line, with a `---` or `...` line below it, opened YAML in
        // the middle of a note, hiding what was between.
        "Intro\n\n---\n- [ ] between the rules\n---\n\n- [ ] after\n",
        // A byte order mark made the first line text.
        "\u{feff}- [ ] first\n- [ ] second\n",
        // `[[#Heading]]` held the tag `Heading`.
        "See [[#Heading]].\n",
        // A highlight was text to pulldown-cmark, so the tag right after its `==` was none. It
        // is one in Obsidian.
        "Read ==#marked==.\n",
        // `[[A\|B]]` in a table cell, where the pipe before the alias is escaped so that it
        // doesn't end the cell, linked to `A\`. The note is `A`.
        "| [[A\\|B]] | c |\n| --- | --- |\n| d | [[E\\|F]] |\n",
    ];

    /// How the tree's items and pulldown-cmark's differ for one note, if they do.
    fn difference(name: &str, text: &str, s: &Settings) -> Option<String> {
        let (tree, old) = (extract(name, text, s), pulldown::extract(name, text, s));
        if tree == old {
            return None;
        }
        let short = |i: Option<&Item>| format!("{i:?}").chars().take(400).collect::<String>();
        let mut out = format!(
            "{name} ({}): {} items, pulldown-cmark {}",
            s.flavour.name,
            tree.len(),
            old.len()
        );
        let n = tree.len().max(old.len());
        for k in (0..n).filter(|&k| tree.get(k) != old.get(k)).take(3) {
            out += &format!(
                "\n  tree: {}\n  then: {}",
                short(tree.get(k)),
                short(old.get(k))
            );
        }
        Some(out)
    }

    /// The walk through the tree gives every note the items pulldown-cmark gave it: the example
    /// workspace as `cold-lava items` reads it, then every Markdown file of the repository and
    /// the tricky cases in each flavour, with types that read every kind of block.
    #[test]
    fn the_tree_reads_items_as_pulldown_cmark_did() {
        let mut differ = Vec::new();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let s = Settings::load(&workspace);
        for e in walkdir::WalkDir::new(&workspace).into_iter().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md") {
                let name = p.strip_prefix(&workspace).unwrap().to_string_lossy();
                let text = std::fs::read_to_string(p).unwrap();
                differ.extend(difference(&name, &text, &s));
            }
        }
        // The one way they differ: pulldown-cmark took a frontmatter whose first line is blank,
        // as the Kanban plugin writes it, for a rule and Markdown. The tree reads frontmatter,
        // as Obsidian does (`a_frontmatter_with_a_blank_first_line_is_frontmatter`).
        let corpus: Vec<(String, String)> = cst::corpus()
            .into_iter()
            .filter(|(_, text)| !text.starts_with("---\n\n"))
            .collect();
        for flavour in ["obsidian", "github", "commonmark"] {
            let s = every_type(flavour);
            for (name, text) in &corpus {
                differ.extend(difference(name, text, &s));
            }
            for (k, text) in TRICKY.iter().enumerate() {
                differ.extend(difference(&format!("Tricky {k}.md"), text, &s));
            }
        }
        let s = every_type("obsidian");
        for text in DIFFERENT {
            if difference("Different.md", text, &s).is_none() {
                differ.push(format!("reads alike now, so it isn't DIFFERENT: {text:?}"));
            }
        }
        assert!(
            differ.is_empty(),
            "{} read differently:\n{}",
            differ.len(),
            differ.join("\n")
        );
    }

    /// pulldown-cmark read it as a rule, then Markdown, so its YAML could hold items.
    #[test]
    fn a_frontmatter_with_a_blank_first_line_is_frontmatter() {
        let text = "---\n\nkanban-plugin: basic\n- [ ] YAML\n\n---\n\n## To do\n\n- [ ] Pack\n";
        assert_eq!(
            summary(&items(text)),
            [s("heading", 8, 8, "To do"), s("task", 10, 10, "[ ] Pack")]
        );
    }

    /// What reading every note's items costs, pulldown-cmark's pass against the tree's walk,
    /// with the workspace's types and with none (the pass and the ids alone). Run on
    /// `scripts/bench-items.py`'s workspace: `COLD_LAVA_BENCH_WS=DIR cargo test --release -p
    /// cold-lava-engine items_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn items_cost() {
        use std::time::{Duration, Instant};
        let dir = std::env::var("COLD_LAVA_BENCH_WS").expect("COLD_LAVA_BENCH_WS");
        let root = Path::new(&dir);
        let typed = Settings::load(root);
        let plain = Settings::load_with(root, types::load(Vec::<types::Layer>::new()));
        let notes: Vec<(String, String)> = walkdir::WalkDir::new(root)
            .into_iter()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .filter_map(|e| {
                let name = e.path().strip_prefix(root).ok()?;
                let name = name.to_string_lossy().to_string();
                Some((name, std::fs::read_to_string(e.path()).ok()?))
            })
            .collect();
        let bytes: usize = notes.iter().map(|n| n.1.len()).sum();
        type Read = fn(&str, &str, &Settings) -> Vec<Item>;
        let sides: [(&str, Read, &Settings); 4] = [
            ("pulldown-cmark, with the types", pulldown::extract, &typed),
            ("the tree, with the types", extract, &typed),
            ("pulldown-cmark, no types", pulldown::extract, &plain),
            ("the tree, no types", extract, &plain),
        ];
        let mut runs: Vec<Vec<Duration>> = vec![Vec::new(); sides.len()];
        let mut items = vec![0usize; sides.len()];
        // The sides take turns, so drift and caches favour none.
        for _ in 0..9 {
            for (k, (_, read, s)) in sides.iter().enumerate() {
                let t = Instant::now();
                items[k] = notes
                    .iter()
                    .map(|(name, text)| read(name, text, s).len())
                    .sum();
                runs[k].push(t.elapsed());
            }
        }
        println!(
            "items_cost: {} notes, {} KiB; median (min-max) of 9:",
            notes.len(),
            bytes / 1024
        );
        for (k, (what, ..)) in sides.iter().enumerate() {
            runs[k].sort();
            println!(
                "  {what}: {:?} ({:?}-{:?}), {} items",
                runs[k][4], runs[k][0], runs[k][8], items[k]
            );
        }
    }
}
