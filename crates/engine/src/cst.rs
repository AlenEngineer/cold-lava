//! A note as a lossless concrete syntax tree of Obsidian-flavoured Markdown, for the editor
//! (docs/engine.md, "The lossless Markdown tree"). Every byte of the note belongs to a token, the
//! blank lines and indentation too, so printing the tree gives the note back exactly.
//!
//! The tree is `rowan`'s: an immutable "green" tree of kinds and text with shared subtrees, and
//! a "red" layer (`SyntaxNode`) made on demand for parents and offsets. `Tree::edit` reparses
//! only the top-level blocks an edit can change and splices them into the old green tree.
//!
//! Parsing is in two parts: `block` finds the blocks line by line (CommonMark's containers and
//! leaves) as a flat list of events, and `inline` reads a line of a block's text into code spans,
//! links, tags and the rest while the events are built into the tree. What the flavour lacks
//! (`Syntax`) stays text.

mod block;
mod inline;
#[cfg(test)]
mod tests;

use std::ops::Range;

use rowan::{GreenNode, GreenNodeBuilder, GreenNodeData, NodeOrToken};

use crate::markdown::{Flavour, Syntax};

macro_rules! kinds {
    ($($(#[$doc:meta])* $name:ident,)*) => {
        /// What a node or a token of the tree is.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u16)]
        pub enum Kind {
            $($(#[$doc])* $name,)*
        }
        const KINDS: &[Kind] = &[$(Kind::$name,)*];
    };
}

kinds! {
    // Tokens.
    /// Spaces and tabs: indentation, the space after a marker, trailing blanks.
    Whitespace,
    /// `\n` or `\r\n`.
    Newline,
    /// A byte order mark at the start of the file.
    Bom,
    Text,
    /// `---` opening or closing the frontmatter.
    FrontmatterFence,
    /// A frontmatter line's text, around its wikilinks.
    Yaml,
    /// The `#`s of an ATX heading.
    HeadingMarker,
    /// The `===` or `---` under a setext heading.
    SetextUnderline,
    /// `***`, `---`, `___`.
    Rule,
    /// `>`.
    QuoteMarker,
    /// `[!type]`, `[!type]+` or `[!type]-` on a callout's first line.
    CalloutMarker,
    /// `-`, `*`, `+`, `1.`, `1)`.
    ListMarker,
    /// `[ ]`, `[x]`, or any status character between the brackets.
    TaskMarker,
    /// A code fence's backticks or tildes.
    Fence,
    /// What follows an opening fence: the language and the rest.
    Info,
    /// A line of a code block, or the inside of a code span.
    CodeText,
    /// A line of an HTML block, opaque.
    HtmlText,
    /// `$` or `$$`.
    MathMarker,
    MathText,
    /// `%%`.
    CommentMarker,
    CommentText,
    /// `|` between table cells, or before a wikilink's alias.
    Pipe,
    /// A cell of a table's delimiter row (`:---:`).
    Delimiter,
    /// The backticks around a code span.
    Backticks,
    /// `[[`.
    WikiOpen,
    /// `![[`.
    EmbedOpen,
    /// `]]`.
    WikiClose,
    /// What a wikilink names, before `#`, `^` or `|`.
    Target,
    /// `#Heading` or `^block` after a wikilink's target.
    Anchor,
    /// What a wikilink shows instead of its target.
    Alias,
    /// `[`.
    LBracket,
    /// `![`.
    ImageOpen,
    /// `]`.
    RBracket,
    /// `(`.
    LParen,
    /// `)`.
    RParen,
    /// What a Markdown link or image points to, with its title if it has one.
    LinkDest,
    /// `#tag`, `#nested/tag`.
    Tag,
    /// `^id` at the end of a line.
    BlockId,
    /// A Dataview field's name.
    FieldKey,
    /// `::`.
    FieldColons,
    /// What a reference link names between its brackets: `ref` in `[text][ref]`, `[ref][]` and
    /// `[ref]`. The note's `Definitions` say where it points.
    LinkLabel,
    /// A link definition's label, between its brackets.
    DefinitionLabel,
    /// `:` after a link definition's label.
    Colon,
    /// What a link definition's label points to, with its title if it has one.
    DefinitionDest,

    // Nodes.
    Document,
    Frontmatter,
    /// `[label]: destination "title"`, where a paragraph would start.
    LinkDefinition,
    /// A line with nothing but whitespace.
    BlankLine,
    Paragraph,
    AtxHeading,
    SetextHeading,
    ThematicBreak,
    BlockQuote,
    /// A block quote whose first line is `[!type]`.
    Callout,
    /// The rest of a callout's first line.
    CalloutTitle,
    List,
    ListItem,
    FencedCode,
    IndentedCode,
    HtmlBlock,
    /// `$$` … `$$` as a block.
    MathBlock,
    /// `%%` … `%%` across lines.
    CommentBlock,
    Table,
    TableRow,
    TableDelimiterRow,
    TableCell,
    CodeSpan,
    WikiLink,
    Embed,
    Link,
    Image,
    /// The text of a link or an image, between the brackets.
    LinkText,
    /// Dataview's `[key:: value]` or `(key:: value)`.
    InlineField,
    /// Dataview's `key:: value` taking a whole line.
    LineField,
    /// `%%hidden%%` within a line.
    Comment,
    /// `$x$` or `$$x$$` within a line.
    InlineMath,
}

impl From<Kind> for rowan::SyntaxKind {
    fn from(kind: Kind) -> Self {
        rowan::SyntaxKind(kind as u16)
    }
}

impl Kind {
    fn of(raw: rowan::SyntaxKind) -> Kind {
        KINDS[raw.0 as usize]
    }
}

/// The Markdown language for `rowan`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Markdown {}

impl rowan::Language for Markdown {
    type Kind = Kind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Kind {
        Kind::of(raw)
    }

    fn kind_to_raw(kind: Kind) -> rowan::SyntaxKind {
        kind.into()
    }
}

pub type SyntaxNode = rowan::SyntaxNode<Markdown>;
pub type SyntaxToken = rowan::SyntaxToken<Markdown>;

/// The syntax a tree is parsed with: a flavour's, as bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gates(u32);

impl Gates {
    pub fn of(flavour: &Flavour) -> Gates {
        Gates(flavour.syntax.iter().fold(0, |g, s| g | (1 << *s as u32)))
    }

    /// What reading a note's links and tags sees, whatever the workspace's flavour, as it
    /// always has: wikilinks, embeds and tags, outside code and the frontmatter's text. Links in
    /// comments and math still count. A tag may follow a strikethrough's or a highlight's
    /// markers, and a footnote's `[^label]:` defines no link, as in Obsidian.
    pub(crate) const INDEX: Gates = Gates(
        (1 << Syntax::Frontmatter as u32)
            | (1 << Syntax::Strikethrough as u32)
            | (1 << Syntax::Highlights as u32)
            | (1 << Syntax::Footnotes as u32)
            | (1 << Syntax::WikiLinks as u32)
            | (1 << Syntax::Embeds as u32)
            | (1 << Syntax::Tags as u32)
            | (1 << Syntax::Tables as u32)
            | (1 << Syntax::TaskLists as u32)
            | (1 << Syntax::Callouts as u32)
            | (1 << Syntax::BlockIds as u32),
    );

    pub fn has(self, syntax: Syntax) -> bool {
        self.0 & (1 << syntax as u32) != 0
    }

    /// These gates with `syntax` on or off.
    pub(crate) fn set(self, syntax: Syntax, on: bool) -> Gates {
        let bit = 1 << syntax as u32;
        Gates(if on { self.0 | bit } else { self.0 & !bit })
    }
}

/// A note's link definitions (`[label]: destination`), in the order written. A reference link
/// (`[text][label]`, `[label][]`, `[label]`) points where its label's definition says. Labels
/// match as in CommonMark: case and the amount of whitespace don't count, and a label's first
/// definition wins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Definitions(Vec<(String, String)>);

impl Definitions {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Where `label`, as a link writes it between its brackets, points: the destination as
    /// written, without its title or its `<>`.
    pub fn get(&self, label: &str) -> Option<&str> {
        if self.0.is_empty() {
            return None;
        }
        let label = fold(label);
        let found = self.0.iter().find(|d| d.0 == label);
        found.map(|d| d.1.as_str())
    }

    /// Add the definition written with this label and this destination (with its title).
    fn add(&mut self, label: &str, dest: &str) {
        let dest = match dest.strip_prefix('<') {
            Some(pointy) => pointy.split('>').next(),
            None => dest.split_whitespace().next(),
        };
        let dest = dest.unwrap_or_default().to_string();
        self.0.push((fold(label), dest));
    }

    /// Add the definitions written in the block `node`: itself, or the blocks a container holds.
    fn add_from(&mut self, node: &GreenNodeData) {
        match Kind::of(node.kind()) {
            Kind::LinkDefinition => {
                let label = token_of(node, Kind::DefinitionLabel);
                self.add(label, token_of(node, Kind::DefinitionDest));
            }
            Kind::BlockQuote | Kind::Callout | Kind::List | Kind::ListItem => {
                for child in node.children() {
                    if let NodeOrToken::Node(n) = child {
                        self.add_from(n);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A label as labels are compared.
fn fold(label: &str) -> String {
    let words: Vec<&str> = label.split_whitespace().collect();
    words.join(" ").to_lowercase()
}

/// The text of `node`'s first token of `kind`.
fn token_of(node: &GreenNodeData, kind: Kind) -> &str {
    for child in node.children() {
        if let NodeOrToken::Token(t) = child
            && Kind::of(t.kind()) == kind
        {
            return t.text();
        }
    }
    ""
}

/// A note's syntax tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    green: GreenNode,
    gates: Gates,
    defs: Definitions,
}

impl Tree {
    /// `text` read in `flavour`.
    pub fn parse(text: &str, flavour: &Flavour) -> Tree {
        Tree::with(text, Gates::of(flavour))
    }

    pub fn with(text: &str, gates: Gates) -> Tree {
        let (green, defs) = block::document(text, gates);
        Tree { green, gates, defs }
    }

    /// The note's link definitions, which say where its reference links point.
    pub fn definitions(&self) -> &Definitions {
        &self.defs
    }

    pub fn green(&self) -> &GreenNode {
        &self.green
    }

    /// The red tree's root, with parents and offsets.
    pub fn root(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// The note's text, as the tree holds it.
    pub fn text(&self) -> String {
        self.green.to_string()
    }

    pub fn len(&self) -> usize {
        u32::from(self.green.text_len()) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The tree of `text`, which is this tree's text with the bytes `replaced` replaced by
    /// `inserted` bytes. Only the top-level blocks the edit can change are parsed again; the
    /// others are this tree's, shared. An edit that changes the note's link definitions changes
    /// how `[label]` reads in every block, so the note is then parsed whole.
    pub fn edit(&self, text: &str, replaced: Range<usize>, inserted: usize) -> Tree {
        debug_assert_eq!(text.len() + replaced.len(), self.len() + inserted);
        match self.reparse(text, &replaced, inserted) {
            Some(green) => Tree {
                green,
                gates: self.gates,
                defs: self.defs.clone(),
            },
            None => Tree::with(text, self.gates),
        }
    }

    /// `edit` without a full parse; `None` when the edit reaches the top of the note, where the
    /// frontmatter is decided, or changes the note's link definitions.
    fn reparse(&self, text: &str, replaced: &Range<usize>, inserted: usize) -> Option<GreenNode> {
        // Line 1 decides whether there is a frontmatter, and an unclosed `---` there may be
        // closed by any later line.
        if !text[..replaced.start].contains('\n') {
            return None;
        }
        let kids: Vec<(Kind, usize)> = (self.green.children())
            .map(|c| (Kind::of(c.kind()), u32::from(c.text_len()) as usize))
            .collect();
        let has_frontmatter = kids.iter().any(|k| k.0 == Kind::Frontmatter);
        if self.gates.has(Syntax::Frontmatter)
            && !has_frontmatter
            && text.lines().next().is_some_and(|l| l.trim_end() == "---")
        {
            return None;
        }
        let mut starts = Vec::with_capacity(kids.len());
        let mut at = 0;
        let mut body = 0;
        for (i, (kind, len)) in kids.iter().enumerate() {
            starts.push(at);
            at += len;
            if matches!(kind, Kind::Bom | Kind::Frontmatter) {
                body = i + 1;
            }
        }
        let body_start = starts.get(body).copied()?;
        if replaced.start < body_start {
            return None;
        }
        // The block the edit starts in, and back to the block before it that isn't blank: how
        // a block ends depends on the first line that isn't blank after it.
        let i = starts.partition_point(|&s| s <= replaced.start) - 1;
        let mut k = i;
        if k > body {
            k -= 1;
            while k > body && kids[k].0 == Kind::BlankLine {
                k -= 1;
            }
        }
        let from = starts[k];
        let new_end = replaced.start + inserted;
        let old = |p: usize| (p + replaced.len()).checked_sub(inserted);
        let (events, end, defs) = block::region(text, from, self.gates, |p| {
            p >= new_end && old(p).is_some_and(|o| starts.binary_search(&o).is_ok())
        });
        let j = match end == text.len() {
            true => kids.len(),
            false => starts.binary_search(&old(end)?).ok()?,
        };
        // The blocks parsed again must define what the blocks they replace defined: every
        // other block was read with those definitions.
        let mut were = Definitions::default();
        for child in self.green.children().skip(k).take(j - k) {
            if let NodeOrToken::Node(n) = child {
                were.add_from(n);
            }
        }
        if defs != were {
            return None;
        }
        let children = block::nodes(text, &events, self.gates, &self.defs);
        Some(self.green.splice_children(k..j, children))
    }
}

/// Where a parse goes: the green tree's builder, or a reader going forwards (`read`).
trait Sink<'t> {
    fn start(&mut self, kind: Kind);
    fn finish(&mut self);
    fn token(&mut self, kind: Kind, text: &'t str);
}

impl<'t> Sink<'t> for GreenNodeBuilder<'_> {
    fn start(&mut self, kind: Kind) {
        self.start_node(kind.into());
    }

    fn finish(&mut self) {
        self.finish_node();
    }

    fn token(&mut self, kind: Kind, text: &'t str) {
        GreenNodeBuilder::token(self, kind.into(), text);
    }
}

/// A parse as the steps `walk` would take through its tree.
struct Steps<F> {
    at: usize,
    open: Vec<Kind>,
    f: F,
}

impl<'t, F: FnMut(Step<'t>)> Sink<'t> for Steps<F> {
    fn start(&mut self, kind: Kind) {
        (self.f)(Step::Enter(kind, self.at));
        self.open.push(kind);
    }

    fn finish(&mut self) {
        let kind = self.open.pop().expect("a node to finish");
        (self.f)(Step::Leave(kind, self.at));
    }

    fn token(&mut self, kind: Kind, text: &'t str) {
        (self.f)(Step::Token(kind, text, self.at));
        self.at += text.len();
    }
}

/// `text` read in `gates`, step by step as `walk` goes through its tree, without making the
/// tree: for a reader that needs a note once, as the workspace's scan does.
pub fn read<'t>(text: &'t str, gates: Gates, f: &mut impl FnMut(Step<'t>)) {
    let (blocks, defs) = blocks(text, gates);
    blocks.read(&defs, f);
}

/// A note whose blocks are found, before `read` goes through its inlines.
pub struct Blocks<'t> {
    text: &'t str,
    gates: Gates,
    events: Vec<block::Ev>,
}

/// The blocks of `text` read in `gates`, and the note's link definitions, which are known once
/// the blocks are: for a reader that follows reference links, which asks the definitions
/// where each `LinkLabel` points while it reads.
pub fn blocks(text: &str, gates: Gates) -> (Blocks<'_>, Definitions) {
    let (events, defs) = block::events(text, gates);
    let blocks = Blocks {
        text,
        gates,
        events,
    };
    (blocks, defs)
}

impl<'t> Blocks<'t> {
    /// The note, step by step as `walk` goes through its tree; `defs` are its definitions.
    /// It takes the blocks, so they are dropped when the walk ends, before the reader goes
    /// on with what it read.
    pub fn read(self, defs: &Definitions, f: &mut impl FnMut(Step<'t>)) {
        let mut steps = Steps {
            at: 0,
            open: Vec::new(),
            f,
        };
        steps.start(Kind::Document);
        inline::build(self.text, &self.events, self.gates, defs, &mut steps);
        steps.finish();
    }
}

/// One step of a walk through a green tree, with its byte offset.
pub enum Step<'a> {
    Enter(Kind, usize),
    Leave(Kind, usize),
    Token(Kind, &'a str, usize),
}

/// Walk `node`'s descendants in order, starting at byte `at`. Reading the green tree straight
/// makes no red nodes, which is what a reader that only goes forwards needs.
pub fn walk<'a>(node: &'a GreenNodeData, at: usize, f: &mut impl FnMut(Step<'a>)) -> usize {
    let mut at = at;
    for child in node.children() {
        match child {
            NodeOrToken::Node(n) => {
                let kind = Kind::of(n.kind());
                f(Step::Enter(kind, at));
                at = walk(n, at, f);
                f(Step::Leave(kind, at));
            }
            NodeOrToken::Token(t) => {
                f(Step::Token(Kind::of(t.kind()), t.text(), at));
                at += t.text().len();
            }
        }
    }
    at
}

/// How many block events `text` makes, for timing the block pass alone.
#[cfg(test)]
pub(crate) fn block_events(text: &str, gates: Gates) -> usize {
    block::events(text, gates).0.len()
}

/// Every Markdown file of the repository: the example workspace, the docs, the fixtures.
#[cfg(test)]
pub(crate) fn corpus() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let skip = |e: &walkdir::DirEntry| {
        let name = e.file_name().to_string_lossy();
        !(name == "target" || (name.starts_with('.') && e.depth() > 0) || name == "node_modules")
    };
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_entry(skip)
        .flatten()
    {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "md")
            && let Ok(text) = std::fs::read_to_string(p)
        {
            out.push((p.display().to_string(), text));
        }
    }
    assert!(out.len() >= 40, "found {} notes", out.len());
    out
}
