//! Markdown as a workspace's app reads it. A `Flavour` is a named set of syntax features, and
//! `parse` turns a note into a `Doc` for the reader to draw. Syntax the flavour lacks stays
//! plain text, so `==x==` is a highlight under `obsidian` and four equals signs under `github`.
//!
//! pulldown-cmark parses CommonMark and the features it has options for. The Obsidian syntax it
//! lacks is added by the rules in "Extras" below, each gated by its `Syntax`.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::iter::Peekable;
use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use pulldown_cmark::{
    BlockQuoteKind, CodeBlockKind, Event, LinkType, OffsetIter, Options, Parser, Tag,
};
use regex::Regex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Syntax {
    Tables,
    TaskLists,
    Strikethrough,
    Footnotes,
    /// GitHub's `> [!NOTE]`, five fixed kinds.
    Alerts,
    /// `[[Note#Heading|alias]]`.
    WikiLinks,
    /// `![[Note]]`, `![[image.png]]`.
    Embeds,
    /// Obsidian's `> [!type]± Title`, any type, foldable.
    Callouts,
    /// `==x==`.
    Highlights,
    /// `%%x%%`, hidden, also across lines.
    Comments,
    /// `#tag`, `#nested/tag`.
    Tags,
    /// A trailing ` ^id` naming the block.
    BlockIds,
    /// `$x$`, `$$x$$`.
    Math,
    /// Leading YAML between `---` lines, hidden from the document.
    Frontmatter,
}

use Syntax as S;

/// Each syntax's name in `config.toml` and the pulldown-cmark option behind it (empty for the
/// ones added by the extras).
const SYNTAX: &[(Syntax, &str, Options)] = &[
    (S::Tables, "tables", Options::ENABLE_TABLES),
    (S::TaskLists, "task-lists", Options::ENABLE_TASKLISTS),
    (
        S::Strikethrough,
        "strikethrough",
        Options::ENABLE_STRIKETHROUGH,
    ),
    (S::Footnotes, "footnotes", Options::ENABLE_FOOTNOTES),
    (S::Alerts, "alerts", Options::ENABLE_GFM),
    (S::WikiLinks, "wikilinks", Options::ENABLE_WIKILINKS),
    (S::Embeds, "embeds", Options::ENABLE_WIKILINKS),
    (S::Callouts, "callouts", Options::empty()),
    (S::Highlights, "highlights", Options::empty()),
    (S::Comments, "comments", Options::empty()),
    (S::Tags, "tags", Options::empty()),
    (S::BlockIds, "block-ids", Options::empty()),
    (S::Math, "math", Options::ENABLE_MATH),
    (
        S::Frontmatter,
        "frontmatter",
        Options::ENABLE_YAML_STYLE_METADATA_BLOCKS,
    ),
];

impl Syntax {
    pub fn from_name(name: &str) -> Option<Syntax> {
        SYNTAX.iter().find(|r| r.1 == name).map(|r| r.0)
    }

    /// Every name, for error messages.
    pub fn names() -> String {
        SYNTAX.iter().map(|r| r.1).collect::<Vec<_>>().join(", ")
    }

    fn options(self) -> Options {
        SYNTAX
            .iter()
            .find(|r| r.0 == self)
            .expect("every syntax has a row")
            .2
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flavour {
    pub name: String,
    pub syntax: BTreeSet<Syntax>,
}

const GITHUB: &[Syntax] = &[
    S::Tables,
    S::TaskLists,
    S::Strikethrough,
    S::Footnotes,
    S::Alerts,
    S::Frontmatter,
];

// Obsidian reads `> [!NOTE]` as a callout, so it has no `Alerts`.
const OBSIDIAN: &[Syntax] = &[
    S::Tables,
    S::TaskLists,
    S::Strikethrough,
    S::Footnotes,
    S::Frontmatter,
    S::WikiLinks,
    S::Embeds,
    S::Callouts,
    S::Highlights,
    S::Comments,
    S::Tags,
    S::BlockIds,
    S::Math,
];

pub const BUILT_IN: &[(&str, &[Syntax])] = &[
    ("commonmark", &[S::Frontmatter]),
    ("github", GITHUB),
    ("obsidian", OBSIDIAN),
];

impl Flavour {
    pub fn built_in(name: &str) -> Option<Flavour> {
        BUILT_IN.iter().find(|f| f.0 == name).map(|f| Flavour {
            name: name.to_string(),
            syntax: f.1.iter().copied().collect(),
        })
    }

    pub fn has(&self, syntax: Syntax) -> bool {
        self.syntax.contains(&syntax)
    }

    /// pulldown-cmark's options for this flavour.
    pub fn options(&self) -> Options {
        self.syntax
            .iter()
            .fold(Options::empty(), |o, s| o | s.options())
    }

    /// The text pulldown-cmark reads: without `%%comments%%` when the flavour has them. Line
    /// numbers stay those of the note, and `Source::file_range` maps byte offsets back to it.
    pub fn source<'t>(&self, text: &'t str) -> Source<'t> {
        self.source_with(text, |text| {
            Parser::new_ext(text, self.options())
                .into_offset_iter()
                .filter(|(e, _)| {
                    matches!(
                        e,
                        Event::Code(_)
                            | Event::InlineMath(_)
                            | Event::DisplayMath(_)
                            | Event::Start(Tag::CodeBlock(_))
                    )
                })
                .map(|(_, r)| r)
                .collect()
        })
    }

    /// `source`, where `code` finds the code spans, code blocks and math of a note that has a
    /// `%%`: a `%%` in code doesn't open or close a comment.
    pub fn source_with<'t>(
        &self,
        text: &'t str,
        code: impl FnOnce(&str) -> Vec<Range<usize>>,
    ) -> Source<'t> {
        if self.has(S::Comments) && text.contains("%%") {
            strip_comments(text, &code(text))
        } else {
            Source {
                text: Cow::Borrowed(text),
                pieces: Vec::new(),
            }
        }
    }
}

/// A note's text as pulldown-cmark reads it, and where each byte of it is in the file.
pub struct Source<'t> {
    pub text: Cow<'t, str>,
    /// `(offset here, offset in the file)` where each copied piece starts; empty when nothing
    /// was removed.
    pieces: Vec<(usize, usize)>,
}

impl Source<'_> {
    /// Where `range` of this text is in the file. Text removed inside the range stays inside.
    pub fn file_range(&self, range: Range<usize>) -> Range<usize> {
        let at = |offset: usize, end: bool| {
            let i = self
                .pieces
                .partition_point(|p| if end { p.0 < offset } else { p.0 <= offset });
            match i.checked_sub(1).map(|i| self.pieces[i]) {
                Some((here, file)) => file + offset - here,
                None => offset,
            }
        };
        let start = at(range.start, false);
        let end = if range.is_empty() {
            start
        } else {
            at(range.end, true)
        };
        start..end
    }

    /// Where `file` of the file is in this text; `None` when either end is in removed text.
    pub fn text_range(&self, file: Range<usize>) -> Option<Range<usize>> {
        let at = |offset: usize| {
            if self.pieces.is_empty() {
                return Some(offset);
            }
            let k = self
                .pieces
                .partition_point(|p| p.1 <= offset)
                .checked_sub(1)?;
            let (here, from) = self.pieces[k];
            let mapped = here + offset - from;
            match self.pieces.get(k + 1) {
                Some(&(next, _)) if mapped > next => None,
                _ => Some(mapped),
            }
        };
        Some(at(file.start)?..at(file.end)?)
    }
}

static CONFIGURED: OnceLock<Option<Flavour>> = OnceLock::new();

/// The flavour `config.toml` chose, which the frontend reads and sets once at startup.
pub fn set_configured(flavour: Option<Flavour>) {
    let _ = CONFIGURED.set(flavour);
}

/// The flavour set in `config.toml`, else `obsidian` for a folder Obsidian has opened (it has
/// `.obsidian/`), else `github`.
pub fn flavour_for(workspace_root: &Path) -> Flavour {
    CONFIGURED.get().cloned().flatten().unwrap_or_else(|| {
        let name = if workspace_root.join(".obsidian").is_dir() {
            "obsidian"
        } else {
            "github"
        };
        Flavour::built_in(name).expect("a built-in flavour")
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Doc {
    pub blocks: Vec<Block>,
    /// For each mark `parse_marked` was given: whether it couldn't be placed exactly (it cuts
    /// through text the source doesn't show as written, a tag or code, or a comment), so a
    /// look on it is skipped rather than drawn in the wrong place.
    pub broken: Vec<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// 1-based line of the note where the block starts.
    pub line: usize,
    pub kind: BlockKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockKind {
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    List {
        start: Option<u64>,
        items: Vec<Item>,
    },
    Quote(Vec<Block>),
    /// An Obsidian callout or a GitHub alert (`title` is empty for alerts).
    Callout {
        kind: String,
        title: Vec<Inline>,
        /// `None`: not foldable; `Some(true)`: starts folded (`-`); `Some(false)`: open (`+`).
        folded: Option<bool>,
        blocks: Vec<Block>,
    },
    /// Fenced, indented or HTML (`lang` is `html`), shown as written.
    Code {
        lang: Option<String>,
        text: String,
    },
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
    Math(String),
    FootnoteDef {
        label: String,
        blocks: Vec<Block>,
    },
    /// An embed or image alone in its paragraph.
    Embed {
        target: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// `Some(done)` for a task.
    pub task: Option<bool>,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// `[[target|text]]`: resolve `target` (minus `#…`) with the workspace's resolver.
    Wiki,
    /// `[text](target)` to a file in the workspace.
    Markdown,
    /// A URL with a scheme, to open outside.
    Url,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text(String),
    Strong(Vec<Inline>),
    Emph(Vec<Inline>),
    Strike(Vec<Inline>),
    Highlight(Vec<Inline>),
    Code(String),
    Link {
        target: String,
        kind: LinkKind,
        text: Vec<Inline>,
    },
    /// `![[target]]` or an image.
    Embed {
        target: String,
    },
    /// Without the `#`, as written.
    Tag(String),
    Math(String),
    FootnoteRef(String),
    BlockId(String),
    SoftBreak,
    HardBreak,
    /// Where the text of mark `id` (`parse_marked`) starts (`open`) or ends. A mark cut by a
    /// container (a link it starts outside of) is opened and closed once on each side.
    Mark {
        id: usize,
        open: bool,
    },
}

pub fn parse(text: &str, flavour: &Flavour) -> Doc {
    parse_marked(text, flavour, &[])
}

/// `parse`, with each of `marks` (byte ranges of the file) shown as `Inline::Mark`s around the
/// text it covers.
pub fn parse_marked(text: &str, flavour: &Flavour, marks: &[Range<usize>]) -> Doc {
    let options = flavour.options();
    let source = flavour.source(text);
    let mut broken = vec![false; marks.len()];
    let marks: Vec<Range<usize>> = marks
        .iter()
        .zip(&mut broken)
        .map(|(m, broken)| {
            source.text_range(m.clone()).unwrap_or_else(|| {
                *broken = true;
                0..0
            })
        })
        .collect();
    let text = source.text;
    let mut reader = Reader {
        src: &text,
        flavour,
        events: Parser::new_ext(&text, options)
            .into_offset_iter()
            .peekable(),
        newlines: text.match_indices('\n').map(|(i, _)| i).collect(),
        tasks: Vec::new(),
        marks,
        broken,
        within: None,
    };
    Doc {
        blocks: reader.blocks(),
        broken: reader.broken,
    }
}

/// Builds the `Doc` from pulldown-cmark's events by recursive descent: each container reads
/// its children up to its `End` event.
struct Reader<'a> {
    src: &'a str,
    flavour: &'a Flavour,
    events: Peekable<OffsetIter<'a>>,
    newlines: Vec<usize>,
    /// One slot per open list item, for its task marker. Nested items open after the outer
    /// item's marker, so the marker always belongs to the last slot.
    tasks: Vec<Option<bool>>,
    /// Marks to place, in this text's offsets (empty when broken already).
    marks: Vec<Range<usize>>,
    broken: Vec<bool>,
    /// The inline container being read (a link, emphasis), whose marks it doesn't place when
    /// they cover all of it: the level around it does.
    within: Option<Range<usize>>,
}

/// What an inline is to a mark: text as written can be cut, a container holds its own marks,
/// anything else is placed whole or not at all.
#[derive(Clone, Copy, PartialEq)]
enum Piece {
    Written,
    Container,
    Atom,
}

fn is_inline(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

impl Reader<'_> {
    fn line(&self, offset: usize) -> usize {
        self.newlines.partition_point(|&n| n < offset) + 1
    }

    /// Blocks up to the enclosing container's end (consumed) or the end of the note.
    fn blocks(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        while let Some((event, range)) = self.events.peek() {
            let start = range.start;
            match event {
                Event::End(_) => {
                    self.events.next();
                    break;
                }
                Event::Start(tag) if !is_inline(tag) => {
                    let Some((Event::Start(tag), range)) = self.events.next() else {
                        unreachable!()
                    };
                    self.block(tag, range, &mut blocks);
                }
                Event::Rule => {
                    self.events.next();
                    blocks.push(Block {
                        line: self.line(start),
                        kind: BlockKind::Rule,
                    });
                }
                // A tight list item holds its text without a paragraph.
                _ => {
                    let inlines = self.inlines(false);
                    blocks.extend(self.paragraph(inlines, start));
                }
            }
        }
        blocks
    }

    fn block(&mut self, tag: Tag, range: Range<usize>, out: &mut Vec<Block>) {
        let line = self.line(range.start);
        let kind = match tag {
            Tag::Paragraph => {
                let inlines = self.inlines(true);
                out.extend(self.paragraph(inlines, range.start));
                return;
            }
            Tag::Heading { level, .. } => BlockKind::Heading {
                level: level as u8,
                inlines: self.inlines(true),
            },
            Tag::BlockQuote(Some(alert)) => BlockKind::Callout {
                kind: alert_kind(alert).to_string(),
                title: Vec::new(),
                folded: None,
                blocks: self.blocks(),
            },
            Tag::BlockQuote(None) => {
                let blocks = self.blocks();
                if self.flavour.has(S::Callouts) {
                    callout(blocks)
                } else {
                    BlockKind::Quote(blocks)
                }
            }
            Tag::CodeBlock(kind) => BlockKind::Code {
                lang: match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_string)
                    }
                    CodeBlockKind::Indented => None,
                },
                text: self.text(),
            },
            Tag::HtmlBlock => BlockKind::Code {
                lang: Some("html".to_string()),
                text: self.text(),
            },
            Tag::List(start) => BlockKind::List {
                start,
                items: self.items(),
            },
            Tag::FootnoteDefinition(label) => BlockKind::FootnoteDef {
                label: label.to_string(),
                blocks: self.blocks(),
            },
            Tag::Table(_) => self.table(),
            Tag::MetadataBlock(_) => {
                self.text();
                return;
            }
            // Containers of options no flavour turns on (definition lists): keep their content.
            _ => {
                out.extend(self.blocks());
                return;
            }
        };
        out.push(Block { line, kind });
    }

    fn paragraph(&self, inlines: Vec<Inline>, start: usize) -> Option<Block> {
        let mut inlines = inlines;
        if self.flavour.has(S::BlockIds) {
            block_id(&mut inlines);
        }
        let mut content = inlines
            .iter()
            .filter(|i| !is_blank(i) && !matches!(i, Inline::Mark { .. }));
        let only = match (content.next(), content.next()) {
            (None, _) => return None,
            (Some(only), None) => Some(only),
            _ => None,
        };
        let kind = match only {
            Some(Inline::Embed { target }) => BlockKind::Embed {
                target: target.clone(),
            },
            Some(Inline::Math(m)) if self.src[start..].trim_start().starts_with("$$") => {
                BlockKind::Math(m.clone())
            }
            _ => BlockKind::Paragraph(inlines),
        };
        Some(Block {
            line: self.line(start),
            kind,
        })
    }

    fn items(&mut self) -> Vec<Item> {
        let mut items = Vec::new();
        while let Some((Event::Start(Tag::Item), _)) = self.events.next() {
            self.tasks.push(None);
            let blocks = self.blocks();
            items.push(Item {
                task: self.tasks.pop().flatten(),
                blocks,
            });
        }
        items
    }

    fn table(&mut self) -> BlockKind {
        let mut header = Vec::new();
        let mut rows = Vec::new();
        loop {
            match self.events.next() {
                Some((Event::Start(Tag::TableHead), _)) => header = self.cells(),
                Some((Event::Start(Tag::TableRow), _)) => rows.push(self.cells()),
                _ => break,
            }
        }
        BlockKind::Table { header, rows }
    }

    fn cells(&mut self) -> Vec<Vec<Inline>> {
        let mut cells = Vec::new();
        while let Some((Event::Start(Tag::TableCell), _)) = self.events.next() {
            cells.push(self.inlines(true));
        }
        cells
    }

    /// The raw text up to the container's end.
    fn text(&mut self) -> String {
        let mut text = String::new();
        for (event, _) in self.events.by_ref() {
            match event {
                Event::Text(t) | Event::Html(t) | Event::Code(t) => text.push_str(&t),
                Event::End(_) => break,
                _ => {}
            }
        }
        text
    }

    /// Inlines up to the container's end, which is consumed when `explicit`. Otherwise (a tight
    /// list item's text) reading stops before the next block or the item's end.
    fn inlines(&mut self, explicit: bool) -> Vec<Inline> {
        let mut out = Vec::new();
        // Where each of `out` is in the source, and what it is to a mark.
        let mut at: Vec<(Range<usize>, Piece)> = Vec::new();
        loop {
            match self.events.peek() {
                None => break,
                Some((Event::End(_), _)) => {
                    if explicit {
                        self.events.next();
                    }
                    break;
                }
                Some((Event::Start(tag), _)) if !is_inline(tag) => break,
                Some((Event::Rule, _)) => break,
                _ => {}
            }
            let Some((event, range)) = self.events.next() else {
                break;
            };
            let before = out.len();
            let container = matches!(event, Event::Start(_));
            match event {
                Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => {
                    out.push(Inline::Text(t.into_string()))
                }
                Event::Code(t) => out.push(Inline::Code(t.into_string())),
                Event::InlineMath(t) | Event::DisplayMath(t) => {
                    out.push(Inline::Math(t.into_string()))
                }
                Event::FootnoteReference(label) => {
                    out.push(Inline::FootnoteRef(label.into_string()))
                }
                Event::SoftBreak => out.push(Inline::SoftBreak),
                Event::HardBreak => out.push(Inline::HardBreak),
                Event::TaskListMarker(done) => {
                    if let Some(slot) = self.tasks.last_mut() {
                        *slot = Some(done);
                    }
                }
                Event::Start(tag) => {
                    let outer = self.within.replace(range.clone());
                    self.inline(tag, range.clone(), &mut out);
                    self.within = outer;
                }
                Event::End(_) | Event::Rule => unreachable!("handled above"),
            }
            for inline in &out[before..] {
                let piece = match inline {
                    Inline::Text(t) if self.src.get(range.clone()) == Some(t.as_str()) => {
                        Piece::Written
                    }
                    _ if container => Piece::Container,
                    _ => Piece::Atom,
                };
                at.push((range.clone(), piece));
            }
        }
        let out = self.place_marks(out, at);
        extras(out, self.flavour)
    }

    /// Puts `Inline::Mark`s around the inlines each mark covers: text as written is cut at a
    /// mark's ends, a container it covers only in part keeps the marks its own inlines got, and
    /// anything else it covers only in part breaks the mark.
    fn place_marks(&mut self, out: Vec<Inline>, at: Vec<(Range<usize>, Piece)>) -> Vec<Inline> {
        let (Some(first), Some(last)) = (at.first(), at.last()) else {
            return out;
        };
        let span = first.0.start..last.0.end.max(first.0.end);
        let here: Vec<usize> = (0..self.marks.len())
            .filter(|&i| {
                let m = &self.marks[i];
                let around = self
                    .within
                    .as_ref()
                    .is_some_and(|w| m.start <= w.start && w.end <= m.end);
                !self.broken[i] && !around && m.start < span.end && span.start < m.end
            })
            .collect();
        if here.is_empty() {
            return out;
        }
        let tags = self.flavour.has(S::Tags);
        let mut pieces: Vec<(Inline, Range<usize>, Piece)> = Vec::with_capacity(out.len());
        for (inline, (range, piece)) in out.into_iter().zip(at) {
            let Inline::Text(text) = &inline else {
                pieces.push((inline, range, piece));
                continue;
            };
            if piece != Piece::Written {
                pieces.push((inline, range, piece));
                continue;
            }
            let mut cuts = BTreeSet::new();
            for &i in &here {
                for edge in [self.marks[i].start, self.marks[i].end] {
                    if range.start < edge && edge < range.end {
                        if safe_cut(text, edge - range.start, tags) {
                            cuts.insert(edge - range.start);
                        } else {
                            self.broken[i] = true;
                        }
                    }
                }
            }
            let mut from = 0;
            for cut in cuts.into_iter().chain([text.len()]) {
                pieces.push((
                    Inline::Text(text[from..cut].to_string()),
                    range.start + from..range.start + cut,
                    Piece::Written,
                ));
                from = cut;
            }
        }
        let mut placed = Vec::with_capacity(pieces.len() + 2 * here.len());
        let mut open: Vec<usize> = Vec::new();
        for (inline, range, piece) in pieces {
            let mut covers = Vec::new();
            for &i in &here {
                let m = &self.marks[i];
                if m.start <= range.start && range.end <= m.end {
                    covers.push(i);
                } else if m.start < range.end && range.start < m.end && piece != Piece::Container {
                    self.broken[i] = true;
                }
            }
            for &id in open.iter().filter(|i| !covers.contains(i)) {
                placed.push(Inline::Mark { id, open: false });
            }
            open.retain(|i| covers.contains(i));
            for id in covers {
                if !open.contains(&id) {
                    placed.push(Inline::Mark { id, open: true });
                    open.push(id);
                }
            }
            placed.push(inline);
        }
        placed.extend(open.into_iter().map(|id| Inline::Mark { id, open: false }));
        placed
    }

    fn inline(&mut self, tag: Tag, range: Range<usize>, out: &mut Vec<Inline>) {
        let wiki = |t: LinkType| matches!(t, LinkType::WikiLink { .. });
        match tag {
            Tag::Emphasis => out.push(Inline::Emph(self.inlines(true))),
            Tag::Strong => out.push(Inline::Strong(self.inlines(true))),
            Tag::Strikethrough => out.push(Inline::Strike(self.inlines(true))),
            // `ENABLE_WIKILINKS` is on for either syntax, so each one checks its own.
            Tag::Link { link_type, .. } if wiki(link_type) && !self.flavour.has(S::WikiLinks) => {
                self.literal(range, out)
            }
            Tag::Image { link_type, .. } if wiki(link_type) && !self.flavour.has(S::Embeds) => {
                self.literal(range, out)
            }
            Tag::Link {
                link_type,
                dest_url,
                ..
            } => {
                let kind = if wiki(link_type) {
                    LinkKind::Wiki
                } else if matches!(link_type, LinkType::Autolink | LinkType::Email)
                    || has_scheme(&dest_url)
                {
                    LinkKind::Url
                } else {
                    LinkKind::Markdown
                };
                out.push(Inline::Link {
                    target: dest_url.into_string(),
                    kind,
                    text: self.inlines(true),
                });
            }
            Tag::Image { dest_url, .. } => {
                self.inlines(true);
                out.push(Inline::Embed {
                    target: dest_url.into_string(),
                });
            }
            _ => out.extend(self.inlines(true)),
        }
    }

    fn literal(&mut self, range: Range<usize>, out: &mut Vec<Inline>) {
        self.inlines(true);
        out.push(Inline::Text(self.src[range].to_string()));
    }
}

fn alert_kind(kind: BlockQuoteKind) -> &'static str {
    match kind {
        BlockQuoteKind::Note => "note",
        BlockQuoteKind::Tip => "tip",
        BlockQuoteKind::Important => "important",
        BlockQuoteKind::Warning => "warning",
        BlockQuoteKind::Caution => "caution",
    }
}

fn has_scheme(url: &str) -> bool {
    static SCHEME: OnceLock<Regex> = OnceLock::new();
    SCHEME
        .get_or_init(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:").unwrap())
        .is_match(url)
}

fn is_blank(inline: &Inline) -> bool {
    match inline {
        Inline::Text(t) => t.trim().is_empty(),
        Inline::SoftBreak | Inline::HardBreak => true,
        _ => false,
    }
}

// Extras: the syntax pulldown-cmark doesn't parse. They work on `Inline`s, where code is its
// own variant, so none of them reaches inside code.

/// `#tag`: after the start of the text, a space, `(` or `,`; at least one non-digit. The workspace
/// index finds tags with the same pattern.
pub const TAG: &str = r"(?:^|[\s(,])#([\p{L}\p{N}_/\-]+)";

/// `TAG` in a note's own text, where the markers that open emphasis, a strikethrough or a
/// highlight may stand between what a tag follows and its `#` (`**#tag**`). The syntax tree
/// finds tags with it, and checks the markers against the flavour. The markers are no group
/// of their own: a match's groups are allocated, and a second one made the scan slower.
pub const MARKED_TAG: &str = r"(?:^|[\s(,])[*_~=]*#([\p{L}\p{N}_/\-]+)";

/// Delimited spans that may wrap other inlines.
type Wrap = fn(Vec<Inline>) -> Inline;

const SPANS: &[(Syntax, &str, Wrap)] = &[(S::Highlights, "==", Inline::Highlight)];

/// The first match in `text` at or after `from`: its byte range and what it becomes.
type Token = fn(text: &str, from: usize) -> Option<(Range<usize>, Inline)>;

/// Words inside text.
const TOKENS: &[(Syntax, Token)] = &[(S::Tags, tag)];

fn extras(inlines: Vec<Inline>, flavour: &Flavour) -> Vec<Inline> {
    let tokens: Vec<Token> = TOKENS
        .iter()
        .filter(|t| flavour.has(t.0))
        .map(|t| t.1)
        .collect();
    let mut out: Vec<Inline> = merge_text(inlines)
        .into_iter()
        .flat_map(|i| match i {
            Inline::Text(t) if !tokens.is_empty() => split_tokens(&t, &tokens),
            i => vec![i],
        })
        .collect();
    for (syntax, delim, wrap) in SPANS {
        if flavour.has(*syntax) {
            out = spans(out, delim, *wrap);
        }
    }
    out
}

/// pulldown-cmark splits text at every character that might start syntax.
fn merge_text(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for i in inlines {
        match (out.last_mut(), i) {
            (Some(Inline::Text(a)), Inline::Text(b)) => a.push_str(&b),
            (_, i) => out.push(i),
        }
    }
    out
}

fn split_tokens(text: &str, tokens: &[Token]) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some((range, inline)) = tokens
        .iter()
        .filter_map(|find| find(text, at))
        .min_by_key(|(r, _)| r.start)
    {
        if range.start > at {
            out.push(Inline::Text(text[at..range.start].to_string()));
        }
        out.push(inline);
        at = range.end;
    }
    if at < text.len() {
        out.push(Inline::Text(text[at..].to_string()));
    }
    out
}

/// Whether `text` can be cut at `at` and still read the same: not inside a character, a `==`
/// or a tag, and no new tag where the second part starts.
fn safe_cut(text: &str, at: usize, tags: bool) -> bool {
    let tag_spans = |text: &str, shift: usize| {
        let mut found = Vec::new();
        let mut from = 0;
        while let Some((range, _)) = tag(text, from) {
            from = range.end;
            found.push(range.start + shift..range.end + shift);
        }
        found
    };
    text.is_char_boundary(at)
        && !(text[..at].ends_with('=') && text[at..].starts_with('='))
        && (!tags || {
            let mut parts = tag_spans(&text[..at], 0);
            parts.extend(tag_spans(&text[at..], at));
            parts == tag_spans(text, 0)
        })
}

fn tag(text: &str, from: usize) -> Option<(Range<usize>, Inline)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(TAG).unwrap());
    let mut from = from;
    while let Some(c) = re.captures_at(text, from) {
        let name = c.get(1).unwrap();
        if !name.as_str().chars().all(|c| c.is_ascii_digit()) {
            return Some((
                name.start() - 1..name.end(),
                Inline::Tag(name.as_str().to_string()),
            ));
        }
        from = name.end();
    }
    None
}

/// Pairs `delim`s left to right into `wrap`ped spans, like `**`: an opener is followed by
/// something other than a space, a closer follows something other than a space. Unpaired
/// delimiters stay text.
fn spans(inlines: Vec<Inline>, delim: &str, wrap: Wrap) -> Vec<Inline> {
    enum Piece {
        Inline(Inline),
        Delim,
    }
    let mut pieces = Vec::new();
    for i in inlines {
        match i {
            Inline::Text(t) if t.contains(delim) => {
                for (k, part) in t.split(delim).enumerate() {
                    if k > 0 {
                        pieces.push(Piece::Delim);
                    }
                    if !part.is_empty() {
                        pieces.push(Piece::Inline(Inline::Text(part.to_string())));
                    }
                }
            }
            i => pieces.push(Piece::Inline(i)),
        }
    }
    let can_open: Vec<bool> = (0..pieces.len())
        .map(|k| match pieces.get(k + 1) {
            Some(Piece::Inline(Inline::Text(t))) => !t.starts_with(char::is_whitespace),
            Some(Piece::Inline(_)) => true,
            _ => false,
        })
        .collect();

    let mut out: Vec<Inline> = Vec::new();
    let mut open: Option<usize> = None;
    for (k, piece) in pieces.into_iter().enumerate() {
        if let Piece::Inline(i) = piece {
            out.push(i);
            continue;
        }
        let can_close = match out.last() {
            Some(Inline::Text(t)) => !t.ends_with(char::is_whitespace),
            Some(_) => true,
            None => false,
        };
        match open {
            Some(at) if can_close && out.len() > at => {
                let inner = out.split_off(at);
                out.pop();
                out.push(wrap(merge_text(inner)));
                open = None;
            }
            _ => {
                out.push(Inline::Text(delim.to_string()));
                if can_open[k] {
                    open = Some(out.len());
                }
            }
        }
    }
    merge_text(out)
}

/// A trailing `^id`, after a space or alone: the block's id is group 1.
pub fn block_id_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?:^|\s)\^([A-Za-z0-9-]+)\s*$").unwrap())
}

/// A trailing `^id` (after a space, or alone on the paragraph's last line) names the block.
fn block_id(inlines: &mut Vec<Inline>) {
    let marks = inlines
        .iter()
        .rev()
        .take_while(|i| matches!(i, Inline::Mark { .. }))
        .count();
    let tail = inlines.split_off(inlines.len() - marks);
    trailing_block_id(inlines);
    inlines.extend(tail);
}

fn trailing_block_id(inlines: &mut Vec<Inline>) {
    let Some(Inline::Text(text)) = inlines.last_mut() else {
        return;
    };
    let Some(c) = block_id_pattern().captures(text) else {
        return;
    };
    let id = c[1].to_string();
    let start = c.get(0).unwrap().start();
    text.truncate(start);
    while inlines.last().is_some_and(is_blank) {
        inlines.pop();
    }
    inlines.push(Inline::BlockId(id));
}

/// `> [!type]± Title` on the quote's first line makes it a callout.
fn callout(mut blocks: Vec<Block>) -> BlockKind {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^\[!([^\]\s]+)\]([+-]?)[ \t]*").unwrap());
    let Some(Block {
        line,
        kind: BlockKind::Paragraph(inlines),
    }) = blocks.first_mut()
    else {
        return BlockKind::Quote(blocks);
    };
    let Some(Inline::Text(first)) = inlines
        .iter_mut()
        .find(|i| !matches!(i, Inline::Mark { .. }))
    else {
        return BlockKind::Quote(blocks);
    };
    let Some(c) = re.captures(first) else {
        return BlockKind::Quote(blocks);
    };
    let kind = c[1].to_lowercase();
    let folded = match &c[2] {
        "-" => Some(true),
        "+" => Some(false),
        _ => None,
    };
    let marker = c.get(0).unwrap().end();
    first.drain(..marker);
    let line = *line;
    let mut title = std::mem::take(inlines);
    let body = match title
        .iter()
        .position(|i| matches!(i, Inline::SoftBreak | Inline::HardBreak))
    {
        Some(at) => title.split_off(at).split_off(1),
        None => Vec::new(),
    };
    title.retain(|i| !matches!(i, Inline::Text(t) if t.is_empty()));
    if body.iter().all(is_blank) {
        blocks.remove(0);
    } else {
        blocks[0] = Block {
            line: line + 1,
            kind: BlockKind::Paragraph(body),
        };
    }
    BlockKind::Callout {
        kind,
        title,
        folded,
        blocks,
    }
}

/// Removes `%%…%%` outside code, keeping the line breaks so lines still match the file.
fn strip_comments<'t>(text: &'t str, code: &[Range<usize>]) -> Source<'t> {
    let delims: Vec<usize> = text
        .match_indices("%%")
        .map(|(i, _)| i)
        .filter(|i| !code.iter().any(|r| r.contains(i)))
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut pieces = vec![(0, 0)];
    let mut at = 0;
    for [open, close] in delims.as_chunks::<2>().0 {
        out.push_str(&text[at..*open]);
        for (i, _) in text[*open..*close].match_indices('\n') {
            pieces.push((out.len(), open + i));
            out.push('\n');
        }
        at = close + 2;
        pieces.push((out.len(), at));
    }
    out.push_str(&text[at..]);
    Source {
        text: Cow::Owned(out),
        pieces,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BlockKind as K;
    use Inline as I;

    fn obsidian(text: &str) -> Vec<Block> {
        parse(text, &Flavour::built_in("obsidian").unwrap()).blocks
    }

    fn github(text: &str) -> Vec<Block> {
        parse(text, &Flavour::built_in("github").unwrap()).blocks
    }

    fn kinds(blocks: Vec<Block>) -> Vec<BlockKind> {
        blocks.into_iter().map(|b| b.kind).collect()
    }

    fn one(blocks: Vec<Block>) -> [BlockKind; 1] {
        kinds(blocks).try_into().unwrap()
    }

    fn t(s: &str) -> Inline {
        I::Text(s.to_string())
    }

    /// The inlines of a note that is one paragraph.
    fn para(blocks: Vec<Block>) -> Vec<Inline> {
        match kinds(blocks).as_slice() {
            [K::Paragraph(i)] => i.clone(),
            other => panic!("not one paragraph: {other:?}"),
        }
    }

    #[test]
    fn headings_paragraphs_code_and_rules() {
        let blocks =
            github("# Title\n\nSome *soft* **bold** `x`\n\n```rust\nfn x() {}\n```\n\n---\n");
        let lines: Vec<usize> = blocks.iter().map(|b| b.line).collect();
        assert_eq!(lines, [1, 3, 5, 9]);
        assert_eq!(
            kinds(blocks),
            [
                K::Heading {
                    level: 1,
                    inlines: vec![t("Title")]
                },
                K::Paragraph(vec![
                    t("Some "),
                    I::Emph(vec![t("soft")]),
                    t(" "),
                    I::Strong(vec![t("bold")]),
                    t(" "),
                    I::Code("x".into()),
                ]),
                K::Code {
                    lang: Some("rust".into()),
                    text: "fn x() {}\n".into()
                },
                K::Rule,
            ]
        );
    }

    #[test]
    fn lists_and_tasks() {
        let blocks = github("- [ ] todo\n- [x] done\n  - plain\n\n3. three\n");
        let [list, numbered] = kinds(blocks).try_into().unwrap();
        let K::List { start: None, items } = list else {
            panic!("{list:?}")
        };
        assert_eq!(items[0].task, Some(false));
        assert_eq!(items[0].blocks[0].kind, K::Paragraph(vec![t("todo")]));
        assert_eq!(items[1].task, Some(true));
        let K::List { items: nested, .. } = &items[1].blocks[1].kind else {
            panic!("{:?}", items[1].blocks)
        };
        assert_eq!(nested[0].task, None);
        assert_eq!(nested[0].blocks[0].line, 3);
        assert!(matches!(numbered, K::List { start: Some(3), .. }));
    }

    #[test]
    fn loose_task_list() {
        let blocks = github("- [x] one\n\n- [ ] two\n");
        let [K::List { items, .. }] = one(blocks) else {
            panic!()
        };
        assert_eq!(items[0].task, Some(true));
        assert_eq!(items[1].task, Some(false));
        assert_eq!(items[1].blocks[0].kind, K::Paragraph(vec![t("two")]));
    }

    #[test]
    fn tables_and_quotes() {
        let blocks = github("| a | b |\n| - | - |\n| 1 | **2** |\n\n> quoted\n");
        assert_eq!(
            kinds(blocks),
            [
                K::Table {
                    header: vec![vec![t("a")], vec![t("b")]],
                    rows: vec![vec![vec![t("1")], vec![I::Strong(vec![t("2")])]]],
                },
                K::Quote(vec![Block {
                    line: 5,
                    kind: K::Paragraph(vec![t("quoted")])
                }]),
            ]
        );
    }

    #[test]
    fn wikilinks() {
        assert_eq!(
            para(obsidian("See [[Note#H|alias]] and [[Other]].")),
            [
                t("See "),
                I::Link {
                    target: "Note#H".into(),
                    kind: LinkKind::Wiki,
                    text: vec![t("alias")]
                },
                t(" and "),
                I::Link {
                    target: "Other".into(),
                    kind: LinkKind::Wiki,
                    text: vec![t("Other")]
                },
                t("."),
            ]
        );
        let commonmark = Flavour::built_in("commonmark").unwrap();
        let blocks = parse("See [[Note|alias]] ![[x.png]]", &commonmark).blocks;
        assert_eq!(para(blocks), [t("See [[Note|alias]] ![[x.png]]")]);
    }

    #[test]
    fn markdown_links_and_urls() {
        let link = |target: &str, kind| I::Link {
            target: target.into(),
            kind,
            text: vec![t("x")],
        };
        assert_eq!(
            para(github("[x](Notes/A.md) [x](https://a.b/c#d)")),
            [
                link("Notes/A.md", LinkKind::Markdown),
                t(" "),
                link("https://a.b/c#d", LinkKind::Url),
            ]
        );
    }

    #[test]
    fn embeds() {
        assert_eq!(
            kinds(obsidian("![[img.png]]\n\nText ![[Note]] here")),
            [
                K::Embed {
                    target: "img.png".into()
                },
                K::Paragraph(vec![
                    t("Text "),
                    I::Embed {
                        target: "Note".into()
                    },
                    t(" here")
                ]),
            ]
        );
        let only_links = Flavour {
            name: "links".into(),
            syntax: [S::WikiLinks].into(),
        };
        let blocks = parse("![[img.png]]", &only_links).blocks;
        assert_eq!(para(blocks), [t("![[img.png]]")]);
    }

    #[test]
    fn highlights() {
        assert_eq!(
            para(obsidian("a ==hi== and ==**b** c==")),
            [
                t("a "),
                I::Highlight(vec![t("hi")]),
                t(" and "),
                I::Highlight(vec![I::Strong(vec![t("b")]), t(" c")]),
            ]
        );
        assert_eq!(para(github("a ==hi==")), [t("a ==hi==")]);
        assert_eq!(
            para(obsidian("x == y and z == w")),
            [t("x == y and z == w")]
        );
        assert_eq!(para(obsidian("`==no==`")), [I::Code("==no==".into())]);
    }

    #[test]
    fn comments_are_hidden() {
        assert_eq!(para(obsidian("shown %%secret%% too")), [t("shown  too")]);
        let blocks = obsidian("before\n\n%%\nhidden\n\nlines\n%%\n\nafter `%%code%%`\n");
        let lines: Vec<usize> = blocks.iter().map(|b| b.line).collect();
        assert_eq!(lines, [1, 9]);
        assert_eq!(
            kinds(blocks),
            [
                K::Paragraph(vec![t("before")]),
                K::Paragraph(vec![t("after "), I::Code("%%code%%".into())]),
            ]
        );
        let fenced = obsidian("```\n%% kept %%\n```\n");
        assert!(matches!(&kinds(fenced)[..], [K::Code { text, .. }] if text == "%% kept %%\n"));
        assert_eq!(para(github("a %%b%%")), [t("a %%b%%")]);
    }

    #[test]
    fn tags() {
        assert_eq!(
            para(obsidian("#tag and #nested/tag, #123 (#x)")),
            [
                I::Tag("tag".into()),
                t(" and "),
                I::Tag("nested/tag".into()),
                t(", #123 ("),
                I::Tag("x".into()),
                t(")"),
            ]
        );
        assert_eq!(
            kinds(obsidian("# Heading\n\n`#code` http://x#frag a#b")),
            [
                K::Heading {
                    level: 1,
                    inlines: vec![t("Heading")]
                },
                K::Paragraph(vec![I::Code("#code".into()), t(" http://x#frag a#b")]),
            ]
        );
        assert_eq!(para(github("#tag")), [t("#tag")]);
    }

    #[test]
    fn obsidian_callouts() {
        let blocks = obsidian("> [!warning]- Watch **out**\n> body\n>\n> more\n");
        let [
            K::Callout {
                kind,
                title,
                folded,
                blocks,
            },
        ] = one(blocks)
        else {
            panic!()
        };
        assert_eq!(kind, "warning");
        assert_eq!(folded, Some(true));
        assert_eq!(title, [t("Watch "), I::Strong(vec![t("out")])]);
        assert_eq!(
            blocks,
            [
                Block {
                    line: 2,
                    kind: K::Paragraph(vec![t("body")])
                },
                Block {
                    line: 4,
                    kind: K::Paragraph(vec![t("more")])
                },
            ]
        );

        let [
            K::Callout {
                kind,
                title,
                folded,
                blocks,
            },
        ] = one(obsidian("> [!NOTE]\n> hi\n"))
        else {
            panic!()
        };
        assert_eq!((kind.as_str(), folded), ("note", None));
        assert!(title.is_empty());
        assert_eq!(blocks[0].kind, K::Paragraph(vec![t("hi")]));

        assert!(matches!(
            &kinds(github("> [!custom] x\n"))[..],
            [K::Quote(_)]
        ));
    }

    #[test]
    fn github_alerts() {
        assert_eq!(
            kinds(github("> [!NOTE]\n> hi\n")),
            [K::Callout {
                kind: "note".into(),
                title: vec![],
                folded: None,
                blocks: vec![Block {
                    line: 2,
                    kind: K::Paragraph(vec![t("hi")])
                }],
            }]
        );
        let commonmark = Flavour::built_in("commonmark").unwrap();
        assert!(matches!(
            &kinds(parse("> [!NOTE]\n> hi\n", &commonmark).blocks)[..],
            [K::Quote(_)]
        ));
    }

    #[test]
    fn frontmatter_is_hidden() {
        let blocks = github("---\ntags: [a]\n---\n# Title\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].line, 4);
        assert!(matches!(blocks[0].kind, K::Heading { level: 1, .. }));
    }

    #[test]
    fn block_ids() {
        assert_eq!(
            para(obsidian("A claim. ^claim-1")),
            [t("A claim."), I::BlockId("claim-1".into())]
        );
        assert_eq!(
            para(obsidian("A claim\n^c2")),
            [t("A claim"), I::BlockId("c2".into())]
        );
        assert_eq!(para(github("A claim. ^c")), [t("A claim. ^c")]);
    }

    #[test]
    fn marks_wrap_the_text_they_cover() {
        let m = |id, open| I::Mark { id, open };
        let marked = |text: &str, marks: &[Range<usize>]| {
            let doc = parse_marked(text, &Flavour::built_in("obsidian").unwrap(), marks);
            (para(doc.blocks), doc.broken)
        };
        // File offsets map through a comment the reader never shows.
        let text = "a %%note%% VEI 6 b\n";
        let at = text.find("VEI").unwrap();
        assert_eq!(
            marked(text, std::slice::from_ref(&(at..at + 5))),
            (
                vec![t("a  "), m(0, true), t("VEI 6"), m(0, false), t(" b")],
                vec![false]
            )
        );
        // A whole link is marked around it, not again inside; a mark inside its text, inside.
        let text = "see [[Lava]] now";
        let link = |text: Vec<Inline>| I::Link {
            target: "Lava".into(),
            kind: LinkKind::Wiki,
            text,
        };
        assert_eq!(
            marked(text, &[4..12, 6..8]),
            (
                vec![
                    t("see "),
                    m(0, true),
                    link(vec![m(1, true), t("La"), m(1, false), t("va")]),
                    m(0, false),
                    t(" now"),
                ],
                vec![false, false]
            )
        );
        // Cutting through an entity, which isn't shown as written, can't be placed.
        assert_eq!(marked("a &amp; b", std::slice::from_ref(&(3..5))).1, [true]);
        assert_eq!(
            marked("a &amp; b", std::slice::from_ref(&(2..7))).1,
            [false]
        );
    }

    #[test]
    fn math() {
        assert_eq!(
            kinds(obsidian("Euler: $e^{i\\pi}$\n\n$$\nx^2\n$$\n")),
            [
                K::Paragraph(vec![t("Euler: "), I::Math("e^{i\\pi}".into())]),
                K::Math("\nx^2\n".into()),
            ]
        );
        assert_eq!(para(github("$x$")), [t("$x$")]);
    }

    #[test]
    fn footnotes_and_strikethrough() {
        assert_eq!(
            kinds(github("~~old~~ new[^1]\n\n[^1]: Why.\n")),
            [
                K::Paragraph(vec![
                    I::Strike(vec![t("old")]),
                    t(" new"),
                    I::FootnoteRef("1".into())
                ]),
                K::FootnoteDef {
                    label: "1".into(),
                    blocks: vec![Block {
                        line: 3,
                        kind: K::Paragraph(vec![t("Why.")])
                    }],
                },
            ]
        );
    }

    #[test]
    fn every_syntax_name_round_trips() {
        let all: BTreeSet<Syntax> = GITHUB.iter().chain(OBSIDIAN).copied().collect();
        assert_eq!(all.len(), SYNTAX.len());
        for s in all {
            let row = SYNTAX.iter().find(|r| r.0 == s).unwrap();
            assert_eq!(Syntax::from_name(row.1), Some(s));
        }
    }

    #[test]
    fn flavour_is_detected_from_the_workspace() {
        let dir = std::env::temp_dir().join(format!("cold-lava-flavour-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(flavour_for(&dir).name, "github");
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        assert_eq!(flavour_for(&dir).name, "obsidian");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn example_workspace_notes_parse() {
        fn links(blocks: &[Block]) -> usize {
            fn inl(i: &[Inline]) -> usize {
                i.iter()
                    .map(|i| match i {
                        I::Link {
                            kind: LinkKind::Wiki,
                            ..
                        } => 1,
                        I::Strong(c) | I::Emph(c) | I::Strike(c) | I::Highlight(c) => inl(c),
                        _ => 0,
                    })
                    .sum()
            }
            blocks
                .iter()
                .map(|b| match &b.kind {
                    K::Paragraph(i) | K::Heading { inlines: i, .. } => inl(i),
                    K::Quote(b)
                    | K::Callout { blocks: b, .. }
                    | K::FootnoteDef { blocks: b, .. } => links(b),
                    K::List { items, .. } => items.iter().map(|it| links(&it.blocks)).sum(),
                    _ => 0,
                })
                .sum()
        }
        let flavour = Flavour::built_in("obsidian").unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let mut notes = 0;
        let mut found = 0;
        for entry in walkdir::WalkDir::new(&root) {
            let path = entry.unwrap().into_path();
            if path.extension().is_some_and(|e| e == "md") {
                let text = std::fs::read_to_string(&path).unwrap();
                found += links(&parse(&text, &flavour).blocks);
                notes += 1;
            }
        }
        assert!(notes > 10, "{notes} notes");
        assert!(found > 0);
    }
}
