//! Markdown as a vault's app reads it. A `Flavour` is a named set of syntax features, and
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
}

/// The flavour set in `config.toml`, else `obsidian` for a folder Obsidian has opened (it has
/// `.obsidian/`), else `github`.
pub fn flavour_for(vault_root: &Path) -> Flavour {
    crate::config::get().flavour.clone().unwrap_or_else(|| {
        let name = if vault_root.join(".obsidian").is_dir() {
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
    /// `[[target|text]]`: resolve `target` (minus `#…`) with the vault's resolver.
    Wiki,
    /// `[text](target)` to a file in the vault.
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
}

pub fn parse(text: &str, flavour: &Flavour) -> Doc {
    let options = flavour
        .syntax
        .iter()
        .fold(Options::empty(), |o, s| o | s.options());
    let text = if flavour.has(S::Comments) {
        strip_comments(text, options)
    } else {
        Cow::Borrowed(text)
    };
    let mut reader = Reader {
        src: &text,
        flavour,
        events: Parser::new_ext(&text, options)
            .into_offset_iter()
            .peekable(),
        newlines: text.match_indices('\n').map(|(i, _)| i).collect(),
        tasks: Vec::new(),
    };
    Doc {
        blocks: reader.blocks(),
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
        let mut content = inlines.iter().filter(|i| !is_blank(i));
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
                Event::Start(tag) => self.inline(tag, range, &mut out),
                Event::End(_) | Event::Rule => unreachable!("handled above"),
            }
        }
        extras(out, self.flavour)
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

/// `#tag`: after the start of the text, a space, `(` or `,`; at least one non-digit. The vault
/// index finds tags with the same pattern.
pub const TAG: &str = r"(?:^|[\s(,])#([\p{L}\p{N}_/\-]+)";

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

/// A trailing `^id` (after a space, or alone on the paragraph's last line) names the block.
fn block_id(inlines: &mut Vec<Inline>) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?:^|\s)\^([A-Za-z0-9-]+)\s*$").unwrap());
    let Some(Inline::Text(text)) = inlines.last_mut() else {
        return;
    };
    let Some(c) = re.captures(text) else {
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
    let Some(Inline::Text(first)) = inlines.first_mut() else {
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
fn strip_comments(text: &str, options: Options) -> Cow<'_, str> {
    if !text.contains("%%") {
        return Cow::Borrowed(text);
    }
    let code: Vec<Range<usize>> = Parser::new_ext(text, options)
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
        .collect();
    let delims: Vec<usize> = text
        .match_indices("%%")
        .map(|(i, _)| i)
        .filter(|i| !code.iter().any(|r| r.contains(i)))
        .collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for [open, close] in delims.as_chunks::<2>().0 {
        out.push_str(&text[at..*open]);
        out.extend(text[*open..*close].chars().filter(|&c| c == '\n'));
        at = close + 2;
    }
    out.push_str(&text[at..]);
    Cow::Owned(out)
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
    fn flavour_is_detected_from_the_vault() {
        let dir = std::env::temp_dir().join(format!("cold-lava-flavour-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(flavour_for(&dir).name, "github");
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        assert_eq!(flavour_for(&dir).name, "obsidian");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn example_vault_notes_parse() {
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("example-vault");
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
