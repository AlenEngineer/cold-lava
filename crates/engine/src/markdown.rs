//! Markdown as a workspace's app reads it. A `Flavour` is a named set of syntax features, and
//! `parse` turns a note into a `Doc` for the reader to draw. Syntax the flavour lacks stays
//! plain text, so `==x==` is a highlight under `obsidian` and four equals signs under `github`.
//!
//! The `Doc` is built from the note's syntax tree (`cst`), the one the index and the items
//! read, so what the reader shows as a link, a tag or a task is what they count.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::cst::{self, Kind, Step};

mod entities;
#[cfg(test)]
pub(crate) mod pulldown;

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

/// Each syntax's name in `config.toml`.
const SYNTAX: &[(Syntax, &str)] = &[
    (S::Tables, "tables"),
    (S::TaskLists, "task-lists"),
    (S::Strikethrough, "strikethrough"),
    (S::Footnotes, "footnotes"),
    (S::Alerts, "alerts"),
    (S::WikiLinks, "wikilinks"),
    (S::Embeds, "embeds"),
    (S::Callouts, "callouts"),
    (S::Highlights, "highlights"),
    (S::Comments, "comments"),
    (S::Tags, "tags"),
    (S::BlockIds, "block-ids"),
    (S::Math, "math"),
    (S::Frontmatter, "frontmatter"),
];

impl Syntax {
    pub fn from_name(name: &str) -> Option<Syntax> {
        SYNTAX.iter().find(|r| r.1 == name).map(|r| r.0)
    }

    /// Every name, for error messages.
    pub fn names() -> String {
        SYNTAX.iter().map(|r| r.1).collect::<Vec<_>>().join(", ")
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

    /// The text the reader and the items read: without `%%comments%%` when the flavour has
    /// them. Line numbers stay those of the note, and `Source::file_range` maps byte offsets
    /// back to it. `code` finds the code spans, code blocks and math of a note that has a
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

/// A note's text as it is read, and where each byte of it is in the file.
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
    // The comments are taken out first, as for the items, so their lines read as blank.
    let gates = cst::Gates::of(flavour).set(S::Comments, false);
    let source = flavour.source_with(text, |text| cst::code(text, gates.plain()));
    let marks = Marks::new(&source, marks, flavour.has(S::Tags));
    let text: &str = &source.text;
    let (blocks, defs) = cst::blocks(text, gates);
    let mut steps = Vec::new();
    blocks.read(&defs, &mut |step| steps.push(step));
    let mut reader = Reader {
        src: text,
        flavour,
        footnotes: footnotes(&steps),
        steps,
        // Past the document's own step.
        at: 1,
        defs,
        newlines: text.match_indices('\n').map(|(i, _)| i).collect(),
        marks,
        hard: false,
        level: 0,
        open: Vec::new(),
        list: 0,
        lists: 0,
        in_link: false,
        top: 0,
        notes: Vec::new(),
        checkbox: None,
    };
    Doc {
        blocks: reader.blocks(),
        broken: reader.marks.broken,
    }
}

/// The labels of the footnotes the tree's `steps` define, as labels are compared.
fn footnotes(steps: &[Step]) -> Vec<String> {
    let mut found = Vec::new();
    let mut marker = false;
    for step in steps {
        match *step {
            Step::Enter(Kind::FootnoteMarker, _) => marker = true,
            Step::Leave(Kind::FootnoteMarker, _) => marker = false,
            Step::Token(Kind::FootnoteLabel, label, _) if marker => found.push(fold(label)),
            _ => {}
        }
    }
    found
}

/// A footnote's label (`^label`, as the tree holds it) as labels are compared: case and the
/// amount of whitespace don't count.
fn fold(label: &str) -> String {
    let name = label.strip_prefix('^').unwrap_or(label);
    let words: Vec<&str> = name.split_whitespace().collect();
    words.join(" ").to_lowercase()
}

/// What an inline is to a mark: text as written can be cut, a container holds its own marks,
/// anything else is placed whole or not at all.
#[derive(Clone, Copy, PartialEq)]
enum Piece {
    Written,
    Container,
    Atom,
}

/// An inline as it is read: where it is in the source, and what it is to a mark.
type Read = (Inline, Range<usize>, Piece);

/// Text shown as it is written at `range` of `src`.
fn written(src: &str, range: Range<usize>) -> Read {
    (
        Inline::Text(src[range.clone()].to_string()),
        range,
        Piece::Written,
    )
}

/// The marks `parse_marked` was given, and where they go among the inlines read.
struct Marks {
    /// Marks to place, in the read text's offsets (empty when broken already).
    marks: Vec<Range<usize>>,
    broken: Vec<bool>,
    /// The inline container being read (a link, emphasis), whose marks it doesn't place when
    /// they cover all of it: the level around it does.
    within: Option<Range<usize>>,
    /// Whether the flavour has tags, which a mark doesn't cut.
    tags: bool,
}

impl Marks {
    fn new(source: &Source, marks: &[Range<usize>], tags: bool) -> Marks {
        let mut broken = vec![false; marks.len()];
        let marks = marks
            .iter()
            .zip(&mut broken)
            .map(|(m, broken)| {
                source.text_range(m.clone()).unwrap_or_else(|| {
                    *broken = true;
                    0..0
                })
            })
            .collect();
        Marks {
            marks,
            broken,
            within: None,
            tags,
        }
    }

    /// Puts `Inline::Mark`s around the inlines each mark covers: text as written is cut at a
    /// mark's ends, a container it covers only in part keeps the marks its own inlines got, and
    /// anything else it covers only in part breaks the mark.
    fn place(&mut self, out: Vec<Inline>, at: Vec<(Range<usize>, Piece)>) -> Vec<Inline> {
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
        let tags = self.tags;
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
}

/// Builds the `Doc` from the note's syntax tree (`cst`), read as the steps of a walk through
/// it, by recursive descent: each node reads its children up to its end. The tree is the one
/// the index and the items read, so what the reader shows as a link, a tag or a task is what
/// they count.
struct Reader<'a> {
    src: &'a str,
    flavour: &'a Flavour,
    steps: Vec<Step<'a>>,
    /// The step to read next.
    at: usize,
    /// The note's link definitions, which say where a link by reference points.
    defs: cst::Definitions,
    /// The labels of the note's footnotes: a `[^label]` without one is text.
    footnotes: Vec<String>,
    newlines: Vec<usize>,
    marks: Marks,
    /// The line break being read is a hard one.
    hard: bool,
    /// The level of the heading being read.
    level: u8,
    /// The `[` that nothing closed yet in the block being read, counted as the tree counts
    /// them, since the `](destination)` of a link whose text started on an earlier line
    /// (`LinkEnd`) closes the last: the list of inlines each is in, which of them (none for a
    /// `[` that isn't shown as written), and where in its text.
    open: Vec<(usize, Option<usize>, usize)>,
    /// The list of inlines being read, and how many were started.
    list: usize,
    lists: usize,
    /// Inside a link's text, where the tree counts no brackets.
    in_link: bool,
    /// The list of the paragraph being read, where a footnote may start, and where its
    /// footnotes do: the inline each starts before, its label, and its `[^label]:`.
    top: usize,
    notes: Vec<(usize, String, Range<usize>)>,
    /// A list item's checkbox that is no task's to the reader (`[/]`), shown as it is written
    /// before the item's text.
    checkbox: Option<usize>,
}

impl<'a> Reader<'a> {
    fn line(&self, offset: usize) -> usize {
        self.newlines.partition_point(|&n| n < offset) + 1
    }

    fn next(&mut self) -> Option<Step<'a>> {
        let step = self.steps.get(self.at).copied();
        self.at += 1;
        step
    }

    /// Where the node just entered ends.
    fn end(&self) -> usize {
        let mut depth = 0usize;
        for step in self.steps.get(self.at..).unwrap_or_default() {
            match step {
                Step::Enter(..) => depth += 1,
                Step::Leave(_, at) if depth == 0 => return *at,
                Step::Leave(..) => depth -= 1,
                Step::Token(..) => {}
            }
        }
        self.src.len()
    }

    /// Past the node just entered.
    fn skip(&mut self) {
        self.only(Kind::Document);
    }

    /// Past the node just entered: the text of its tokens of `kind`.
    fn only(&mut self, kind: Kind) -> String {
        let mut found = String::new();
        let mut depth = 0usize;
        while let Some(step) = self.next() {
            match step {
                Step::Enter(..) => depth += 1,
                Step::Leave(..) if depth == 0 => break,
                Step::Leave(..) => depth -= 1,
                Step::Token(k, text, _) if k == kind => found.push_str(text),
                Step::Token(..) => {}
            }
        }
        found
    }

    /// A list of inlines starts: nothing of it is read yet.
    fn start_list(&mut self) -> usize {
        self.lists += 1;
        std::mem::replace(&mut self.list, self.lists)
    }

    /// The blocks of the container just entered, up to its end.
    fn blocks(&mut self) -> Vec<Block> {
        let mut out = Vec::new();
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                // A container's markers and the indentation of its lines.
                Step::Token(..) => {}
                Step::Enter(kind, at) => self.block(kind, at, &mut out),
            }
        }
        out
    }

    fn block(&mut self, kind: Kind, at: usize, out: &mut Vec<Block>) {
        let line = self.line(at);
        let kind = match kind {
            Kind::Paragraph | Kind::MathBlock => {
                self.paragraph(kind, at, out);
                return;
            }
            Kind::AtxHeading | Kind::SetextHeading => {
                let inlines = self.heading(kind == Kind::AtxHeading);
                BlockKind::Heading {
                    level: self.level,
                    inlines,
                }
            }
            Kind::ThematicBreak => {
                self.skip();
                BlockKind::Rule
            }
            Kind::BlockQuote => BlockKind::Quote(self.blocks()),
            Kind::Callout => self.callout(),
            Kind::List => self.list(),
            Kind::FencedCode => self.fenced(),
            Kind::IndentedCode => BlockKind::Code {
                lang: None,
                text: self.indented(),
            },
            Kind::HtmlBlock => BlockKind::Code {
                lang: Some("html".to_string()),
                text: self.html(),
            },
            Kind::Table => self.table(),
            // The frontmatter, a blank line, a link definition: nothing to show.
            _ => {
                self.skip();
                return;
            }
        };
        out.push(Block { line, kind });
    }

    /// The paragraph or the `$$` math just entered at `at`, with the paragraphs and the math
    /// that start on the lines right under it: no blank line parts them, so they are one
    /// paragraph on screen, as they always were. A footnote's `[^label]:` on one of its lines
    /// ends the paragraph there and starts the footnote.
    fn paragraph(&mut self, kind: Kind, at: usize, out: &mut Vec<Block>) {
        let mut read = Vec::new();
        self.start_list();
        self.top = self.list;
        self.notes.clear();
        if let Some(checkbox) = self.checkbox.take() {
            let rest = &self.src[at..];
            let text = at + rest.len() - rest.trim_start_matches([' ', '\t']).len();
            read.push(written(self.src, checkbox..text));
        }
        self.leaf(kind, at, &mut read);
        self.joined(&mut read);
        self.top = 0;
        let mut parts = Vec::new();
        for (k, label, marker) in std::mem::take(&mut self.notes).into_iter().rev() {
            parts.push((Some((label, marker)), read.split_off(k)));
        }
        parts.push((None, read));
        for (note, read) in parts.into_iter().rev() {
            // A footnote's text starts on the first of its lines that holds any.
            let text = read.iter().find(|r| !is_break(r)).map(|r| r.1.start);
            let inlines = self.finish(read, true);
            match note {
                None => out.extend(self.shown(inlines, at)),
                Some((label, marker)) => out.push(Block {
                    line: self.line(marker.start),
                    kind: BlockKind::FootnoteDef {
                        label,
                        blocks: (self.shown(inlines, text.unwrap_or(marker.end)).into_iter())
                            .collect(),
                    },
                }),
            }
        }
    }

    /// The text of the paragraph or the `$$` math just entered at `at`, onto `read`.
    fn leaf(&mut self, kind: Kind, at: usize, read: &mut Vec<Read>) {
        self.open.clear();
        if kind != Kind::MathBlock {
            return self.pieces(read);
        }
        // `$$`, the math, `$$`, and what its last line goes on with: the block's own text,
        // without its containers' markers.
        let end = self.end();
        let mut body = String::new();
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Token(Kind::MathText | Kind::Newline, text, _) => body.push_str(text),
                _ => {}
            }
        }
        let close = body.find("$$");
        let math = close.map_or(body.trim_end_matches(['\n', '\r']), |k| &body[..k]);
        read.push((Inline::Math(math.to_string()), at..end, Piece::Atom));
        let rest = close.map_or("", |k| &body[k + 2..]);
        let rest = rest.trim_end_matches(['\n', '\r', ' ', '\t']);
        if !rest.is_empty() {
            read.push((Inline::Text(rest.to_string()), end..end, Piece::Atom));
        }
        // Its line break, should a paragraph go on under it.
        read.push((Inline::SoftBreak, end..end, Piece::Atom));
    }

    /// Reads on, onto `read`, into the paragraphs and the `$$` math that start on the line
    /// right under the block just read. Whether any did.
    fn joined(&mut self, read: &mut Vec<Read>) -> bool {
        let mut any = false;
        loop {
            let mut k = self.at;
            while matches!(self.steps.get(k), Some(Step::Token(..))) {
                k += 1;
            }
            let Some(&Step::Enter(kind @ (Kind::Paragraph | Kind::MathBlock), at)) =
                self.steps.get(k)
            else {
                return any;
            };
            self.at = k + 1;
            self.leaf(kind, at, read);
            any = true;
        }
    }

    /// A paragraph's inlines as the block the reader shows, if they show anything; `start` is
    /// where its text starts.
    fn shown(&self, inlines: Vec<Inline>, start: usize) -> Option<Block> {
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

    /// The inlines of the heading just entered; its level is `self.level`.
    fn heading(&mut self, atx: bool) -> Vec<Inline> {
        let mut read = Vec::new();
        self.start_list();
        self.open.clear();
        self.pieces(&mut read);
        if atx {
            unclose(&mut read);
        }
        self.finish(read, true)
    }

    /// The callout just entered: its `[!type]`, its title, its blocks.
    fn callout(&mut self) -> BlockKind {
        let (mut kind, mut folded) = (String::new(), None);
        let mut title = Vec::new();
        let mut line = 0;
        while let Some(step) = self.next() {
            match step {
                Step::Token(Kind::CalloutMarker, marker, _) => {
                    let name = marker.trim_start_matches("[!");
                    let close = name.find(']').unwrap_or(name.len());
                    kind = name[..close].to_lowercase();
                    folded = match &name[close..] {
                        "]-" => Some(true),
                        "]+" => Some(false),
                        _ => None,
                    };
                }
                Step::Token(..) => {}
                Step::Enter(Kind::CalloutTitle, at) => {
                    line = self.line(at);
                    self.start_list();
                    self.open.clear();
                    self.pieces(&mut title);
                    break;
                }
                // A callout has its title before anything else.
                _ => break,
            }
        }
        let alerts = ["note", "tip", "important", "warning", "caution"];
        let untitled = title.iter().all(is_break);
        if self.flavour.has(S::Alerts) && alerts.contains(&kind.as_str()) && untitled {
            // A GitHub alert: `[!NOTE]` alone, then its blocks.
            return BlockKind::Callout {
                kind,
                title: Vec::new(),
                folded: None,
                blocks: self.blocks(),
            };
        }
        // The lines right under the title go on from it as one paragraph: they are the
        // callout's first block, and a `^id` at their end is the paragraph's.
        let mut body = Vec::new();
        let mut blocks = Vec::new();
        let ids = self.flavour.has(S::BlockIds);
        let mut title = match self.joined(&mut body) {
            true => {
                let title = self.finish(title, true);
                let mut body = self.finish(body, true);
                if ids {
                    block_id(&mut body);
                }
                if !body.iter().all(is_blank) {
                    blocks.push(Block {
                        line: line + 1,
                        kind: BlockKind::Paragraph(body),
                    });
                }
                title
            }
            false => {
                let mut title = self.finish(title, true);
                if ids {
                    block_id(&mut title);
                }
                title
            }
        };
        title.retain(|i| !matches!(i, Inline::Text(t) if t.is_empty()));
        blocks.extend(self.blocks());
        BlockKind::Callout {
            kind,
            title,
            folded,
            blocks,
        }
    }

    /// The list just entered.
    fn list(&mut self) -> BlockKind {
        let mut start = None;
        let mut items = Vec::new();
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Token(..) => {}
                Step::Enter(Kind::ListItem, _) => {
                    let mut task = None;
                    // The item's marker and its checkbox come before its blocks.
                    while let Some(&Step::Token(kind, text, at)) = self.steps.get(self.at) {
                        self.at += 1;
                        match (kind, text) {
                            (Kind::ListMarker, _) if items.is_empty() => {
                                let number = text.trim_end_matches(['.', ')']);
                                start = number.parse::<u64>().ok();
                            }
                            (Kind::TaskMarker, "[ ]") => task = Some(false),
                            (Kind::TaskMarker, "[x]" | "[X]") => task = Some(true),
                            // Any other status is the items' to read: here it is text.
                            (Kind::TaskMarker, _) => self.checkbox = Some(at),
                            _ => {}
                        }
                    }
                    let mut blocks = Vec::new();
                    // With no paragraph of the item's to start, it is the item's text.
                    let next = self.steps.get(self.at);
                    let starts = matches!(next, Some(Step::Enter(Kind::Paragraph, _)));
                    if let Some(at) = self.checkbox.take_if(|_| !starts) {
                        let text = self.src[at..].lines().next().unwrap_or_default();
                        let text = at..at + text.trim_end().len();
                        let inlines = self.finish(vec![written(self.src, text)], true);
                        blocks.push(Block {
                            line: self.line(at),
                            kind: BlockKind::Paragraph(inlines),
                        });
                    }
                    blocks.extend(self.blocks());
                    items.push(Item { task, blocks });
                }
                Step::Enter(..) => self.skip(),
            }
        }
        BlockKind::List { start, items }
    }

    /// The fenced code just entered: its language, and its lines without the fence's
    /// indentation.
    fn fenced(&mut self) -> BlockKind {
        let mut lang = None;
        let mut text = String::new();
        let (mut indent, mut fences, mut lines) = (0, 0, 0);
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Token(Kind::Whitespace, blanks, _) if fences == 0 => indent = blanks.len(),
                Step::Token(Kind::Fence, ..) => fences += 1,
                Step::Token(Kind::Info, info, _) => {
                    lang = info.split_whitespace().next().map(str::to_string)
                }
                Step::Token(Kind::CodeText, line, _) => {
                    let blanks = line.bytes().take(indent).take_while(|b| *b == b' ');
                    text.push_str(&line[blanks.count()..]);
                }
                Step::Token(Kind::Newline, ..) => {
                    // Not the opening fence's line break, nor the closing one's.
                    if lines > 0 && fences == 1 {
                        text.push('\n');
                    }
                    lines += 1;
                }
                _ => {}
            }
        }
        BlockKind::Code { lang, text }
    }

    /// The lines of the indented code just entered, without their indentation.
    fn indented(&mut self) -> String {
        let mut text = String::new();
        let mut depth = 0usize;
        while let Some(step) = self.next() {
            match step {
                // A blank line between its lines, whose blanks past the indentation are code.
                Step::Enter(..) => depth += 1,
                Step::Leave(..) if depth == 0 => break,
                Step::Leave(..) => depth -= 1,
                Step::Token(Kind::CodeText, line, _) => text.push_str(unindent(line)),
                Step::Token(Kind::Whitespace, blanks, _) if depth > 0 => {
                    text.push_str(unindent(blanks))
                }
                Step::Token(Kind::Newline, ..) => text.push('\n'),
                Step::Token(..) => {}
            }
        }
        text
    }

    /// The lines of the HTML block just entered, as written.
    fn html(&mut self) -> String {
        let mut text = String::new();
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Token(Kind::HtmlText, line, _) => text.push_str(line),
                Step::Token(Kind::Newline, ..) => text.push('\n'),
                _ => {}
            }
        }
        text
    }

    /// The table just entered: its header, then its rows, each with the header's cells.
    fn table(&mut self) -> BlockKind {
        let mut header = Vec::new();
        let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
        let mut head = true;
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Token(..) => {}
                Step::Enter(Kind::TableRow, _) if head => {
                    header = self.cells();
                    head = false;
                }
                Step::Enter(Kind::TableRow, _) => rows.push(self.cells()),
                // The delimiter row.
                Step::Enter(..) => self.skip(),
            }
        }
        for row in &mut rows {
            row.resize(header.len(), Vec::new());
        }
        BlockKind::Table { header, rows }
    }

    /// The cells of the table row just entered. The tree holds no node for a cell with
    /// nothing in it: two pipes with none between them had one.
    fn cells(&mut self) -> Vec<Vec<Inline>> {
        let mut cells = Vec::new();
        // Whether the row has started, and whether a cell was read since its last pipe.
        let (mut started, mut filled) = (false, false);
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Token(Kind::Pipe, ..) => {
                    if started && !filled {
                        cells.push(Vec::new());
                    }
                    (started, filled) = (true, false);
                }
                Step::Token(..) => {}
                Step::Enter(Kind::TableCell, _) => {
                    let mut read = Vec::new();
                    self.start_list();
                    self.open.clear();
                    self.pieces(&mut read);
                    cells.push(self.finish(read, true));
                    (started, filled) = (true, true);
                }
                Step::Enter(..) => self.skip(),
            }
        }
        cells
    }

    /// The inlines of the node just entered, up to its end, onto `out`.
    fn pieces(&mut self, out: &mut Vec<Read>) {
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => return,
                Step::Token(kind, text, at) => self.token(kind, text, at, out),
                Step::Enter(kind, at) => self.inline(kind, at, out),
            }
        }
    }

    fn token(&mut self, kind: Kind, text: &str, at: usize, out: &mut Vec<Read>) {
        let range = at..at + text.len();
        match kind {
            Kind::Text | Kind::InlineHtml => {
                self.brackets(text, Some(out.len()));
                out.push(written(self.src, range));
            }
            // A `^id` names its block only at the block's end (`block_id`), and what a field
            // is made of is shown as written. So is what a wikilink or a link by reference
            // shows of its alias or its label.
            Kind::BlockId
            | Kind::Alias
            | Kind::LinkLabel
            | Kind::FieldKey
            | Kind::FieldColons
            | Kind::LBracket
            | Kind::RBracket
            | Kind::LParen
            | Kind::RParen => out.push(written(self.src, range)),
            Kind::Escape => {
                self.brackets(&text[1..], None);
                out.push((Inline::Text(text[1..].to_string()), range, Piece::Atom));
            }
            Kind::Entity => match character(text) {
                Some(shown) => out.push((Inline::Text(shown), range, Piece::Atom)),
                None => out.push(written(self.src, range)),
            },
            Kind::Tag => out.push((Inline::Tag(text[1..].to_string()), range, Piece::Atom)),
            Kind::HardBreak => self.hard = true,
            Kind::Newline => {
                let line_break = match std::mem::take(&mut self.hard) {
                    true => Inline::HardBreak,
                    false => Inline::SoftBreak,
                };
                out.push((line_break, range, Piece::Atom));
            }
            Kind::HeadingMarker => self.level = text.len() as u8,
            Kind::SetextUnderline => self.level = if text.starts_with('=') { 1 } else { 2 },
            // Indentation, a container's markers, the markers of emphasis.
            _ => {}
        }
    }

    /// Counts the brackets of `text`, which is the block's own, as the tree counted them;
    /// `piece` is the inline that shows it as written.
    fn brackets(&mut self, text: &str, piece: Option<usize>) {
        if self.in_link {
            return;
        }
        for (k, b) in text.bytes().enumerate() {
            match b {
                b'[' => self.open.push((self.list, piece, k)),
                b']' => {
                    self.open.pop();
                }
                _ => {}
            }
        }
    }

    fn inline(&mut self, kind: Kind, at: usize, out: &mut Vec<Read>) {
        let range = at..self.end();
        match kind {
            Kind::Emphasis | Kind::Strong | Kind::Strikethrough | Kind::Highlight => {
                let inlines = self.contained(range.clone(), false);
                let inline = match kind {
                    Kind::Emphasis => Inline::Emph(inlines),
                    Kind::Strong => Inline::Strong(inlines),
                    Kind::Strikethrough => Inline::Strike(inlines),
                    _ => Inline::Highlight(inlines),
                };
                out.push((inline, range, Piece::Container));
            }
            Kind::CodeSpan => {
                let code = self.code_span();
                out.push((Inline::Code(code), range, Piece::Atom));
            }
            Kind::InlineMath => {
                let math = self.only(Kind::MathText);
                out.push((Inline::Math(math), range, Piece::Atom));
            }
            Kind::WikiLink | Kind::Embed => self.wikilink(kind, range, out),
            Kind::Link | Kind::Image => self.link(kind, range, out),
            Kind::LinkEnd => self.link_end(range, out),
            // A Dataview field is shown as written, its value read; the tree counts the
            // brackets in it apart.
            Kind::InlineField | Kind::LineField => {
                let open = std::mem::take(&mut self.open);
                let in_link = std::mem::replace(&mut self.in_link, false);
                self.pieces(out);
                (self.open, self.in_link) = (open, in_link);
            }
            Kind::FootnoteRef => {
                let label = self.only(Kind::FootnoteLabel);
                let name = label.strip_prefix('^').unwrap_or(&label).trim();
                match self.footnotes.contains(&fold(&label)) {
                    true => out.push((Inline::FootnoteRef(name.to_string()), range, Piece::Atom)),
                    false => out.push(written(self.src, range)),
                }
            }
            Kind::FootnoteMarker => {
                let label = self.only(Kind::FootnoteLabel);
                let name = label.strip_prefix('^').unwrap_or(&label).trim();
                match self.list == self.top {
                    true => self.notes.push((out.len(), name.to_string(), range)),
                    false => out.push(written(self.src, range)),
                }
            }
            Kind::Autolink => {
                let url = self.only(Kind::Url);
                self.brackets(&url, None);
                let shown = written(self.src, range.start + 1..range.end - 1);
                let text = self.held(vec![shown], range.clone());
                let link = Inline::Link {
                    target: url,
                    kind: LinkKind::Url,
                    text,
                };
                out.push((link, range, Piece::Container));
            }
            // A node the reader has nothing of its own for: what it holds.
            _ => self.pieces(out),
        }
    }

    /// The inlines of the node just entered, which is at `range`, as a list of their own;
    /// `link` when they are a link's text.
    fn contained(&mut self, range: Range<usize>, link: bool) -> Vec<Inline> {
        let mut read = Vec::new();
        let list = self.start_list();
        let in_link = self.in_link;
        self.in_link = in_link || link;
        self.pieces(&mut read);
        (self.list, self.in_link) = (list, in_link);
        self.held(read, range)
    }

    /// `read` as the inlines a container at `range` holds.
    fn held(&mut self, read: Vec<Read>, range: Range<usize>) -> Vec<Inline> {
        let outer = self.marks.within.replace(range);
        let inlines = self.finish(read, false);
        self.marks.within = outer;
        inlines
    }

    /// `read` as the reader has it: no blanks before a line break, nor around a `block`'s own
    /// text, the marks placed, text next to text made one.
    fn finish(&mut self, mut read: Vec<Read>, block: bool) -> Vec<Inline> {
        trim(&mut read, block);
        let (out, at): (Vec<Inline>, Vec<(Range<usize>, Piece)>) =
            read.into_iter().map(|(i, r, p)| (i, (r, p))).unzip();
        merge_text(self.marks.place(out, at))
    }

    /// The text of the code span just entered, as CommonMark has it: a line break is a
    /// space, and one space comes off each end when both have one.
    fn code_span(&mut self) -> String {
        let mut code = String::new();
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Token(Kind::CodeText, text, _) => code.push_str(text),
                Step::Token(Kind::Newline, ..) => code.push(' '),
                _ => {}
            }
        }
        let inner = code.strip_prefix(' ').and_then(|c| c.strip_suffix(' '));
        match inner {
            Some(inner) if !inner.trim_matches(' ').is_empty() => inner.to_string(),
            _ => code,
        }
    }

    /// The wikilink or the embed just entered.
    fn wikilink(&mut self, kind: Kind, range: Range<usize>, out: &mut Vec<Read>) {
        let mut target = String::new();
        // Where the target is written, and what the link shows in its place once its `|` is
        // read: its alias, read as text is.
        let mut named = range.start..range.start;
        let mut alias: Option<Vec<Read>> = None;
        let list = self.start_list();
        let in_link = std::mem::replace(&mut self.in_link, true);
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Token(Kind::Target | Kind::Anchor, text, at) => {
                    if target.is_empty() {
                        named.start = at;
                    }
                    target.push_str(text);
                    named.end = at + text.len();
                }
                Step::Token(Kind::Pipe, ..) => alias = Some(Vec::new()),
                Step::Token(kind, text, at) => match &mut alias {
                    Some(shown) if kind == Kind::Alias => {
                        shown.push(written(self.src, at..at + text.len()))
                    }
                    Some(shown) => self.token(kind, text, at, shown),
                    None => {}
                },
                Step::Enter(kind, at) => match &mut alias {
                    Some(shown) => self.inline(kind, at, shown),
                    None => self.skip(),
                },
            }
        }
        (self.list, self.in_link) = (list, in_link);
        // `[[]]` and `[[|alias]]` name nothing: they are text, and no link of the index's.
        if target.is_empty() {
            return out.push(written(self.src, range));
        }
        if kind == Kind::Embed {
            return out.push((Inline::Embed { target }, range, Piece::Container));
        }
        let shown = alias.unwrap_or_else(|| vec![written(self.src, named)]);
        let text = self.held(shown, range.clone());
        let link = Inline::Link {
            target,
            kind: LinkKind::Wiki,
            text,
        };
        out.push((link, range, Piece::Container));
    }

    /// The Markdown link or image just entered: inline, or by reference to a definition.
    fn link(&mut self, kind: Kind, range: Range<usize>, out: &mut Vec<Read>) {
        let mut text = None;
        let mut inline = false;
        let mut dest = "";
        // Where its first bracket ends and where the `]` that closes it is: between them,
        // `[label]` and `[label][]` hold the label they show, which is read as text is.
        let (mut open, mut close) = (None, None);
        let mut shown: Vec<Read> = Vec::new();
        // The label `[text][label]` names.
        let mut named = None;
        let list = self.start_list();
        let in_link = std::mem::replace(&mut self.in_link, true);
        while let Some(step) = self.next() {
            match step {
                Step::Leave(..) => break,
                Step::Enter(Kind::LinkText, _) => text = Some(self.contained(range.clone(), true)),
                Step::Enter(kind, at) if close.is_none() => self.inline(kind, at, &mut shown),
                Step::Enter(..) => self.skip(),
                Step::Token(Kind::LBracket | Kind::ImageOpen, bracket, at) => {
                    open = open.or(Some(at + bracket.len()));
                }
                Step::Token(Kind::RBracket, _, at) => close = close.or(Some(at)),
                Step::Token(Kind::LParen, ..) => inline = true,
                Step::Token(Kind::RParen, ..) => {}
                Step::Token(Kind::LinkDest, to, _) => dest = to,
                Step::Token(Kind::LinkLabel, name, at) if close.is_some() => {
                    named = Some(at..at + name.len())
                }
                Step::Token(kind, text, at) if close.is_none() => {
                    self.token(kind, text, at, &mut shown)
                }
                Step::Token(..) => {}
            }
        }
        (self.list, self.in_link) = (list, in_link);
        let label = match (&text, open, close) {
            (None, Some(open), Some(close)) => Some(open..close),
            _ => named,
        };
        let target = match (inline, label) {
            (true, _) => cst::link_destination(dest).map(|d| unescape(&dest[d])),
            (false, Some(label)) => self.defs.get(&self.src[label]).map(unescape),
            (false, None) => None,
        };
        // A destination CommonMark doesn't read (`[a](two words)`) makes no link: the index
        // counts none either.
        let Some(target) = target else {
            return out.push(written(self.src, range));
        };
        if kind == Kind::Image {
            return out.push((Inline::Embed { target }, range, Piece::Container));
        }
        let text = match text {
            Some(text) => text,
            None => self.held(shown, range.clone()),
        };
        let kind = match has_scheme(&target) {
            true => LinkKind::Url,
            false => LinkKind::Markdown,
        };
        out.push((Inline::Link { target, kind, text }, range, Piece::Container));
    }

    /// The `](destination)` just entered, which closes a link whose text started on an
    /// earlier line of the block: the link is made of what was read since its `[`.
    fn link_end(&mut self, range: Range<usize>, out: &mut Vec<Read>) {
        let dest = self.only(Kind::LinkDest);
        let target = cst::link_destination(&dest).map(|d| unescape(&dest[d]));
        let opener = match self.in_link {
            true => None,
            false => self.open.pop(),
        };
        // Its `[` is among the inlines being read, shown as written, with no footnote
        // starting after it.
        let found = match (target, opener) {
            (Some(target), Some((list, Some(piece), k)))
                if list == self.list
                    && piece < out.len()
                    && self.notes.iter().all(|n| n.0 <= piece) =>
            {
                Some((target, piece, k))
            }
            _ => None,
        };
        let Some((target, piece, k)) = found else {
            return out.push(written(self.src, range));
        };
        let from = out[piece].1.start + k;
        let mut read = out.split_off(piece + 1);
        let (_, opener, _) = out.pop().expect("the inline its `[` is in");
        if from + 1 < opener.end {
            read.insert(0, written(self.src, from + 1..opener.end));
        }
        if opener.start < from {
            out.push(written(self.src, opener.start..from));
        }
        let whole = from..range.end;
        let text = self.held(read, whole.clone());
        let kind = match has_scheme(&target) {
            true => LinkKind::Url,
            false => LinkKind::Markdown,
        };
        out.push((Inline::Link { target, kind, text }, whole, Piece::Container));
    }
}

fn is_break(read: &Read) -> bool {
    matches!(read.0, Inline::SoftBreak | Inline::HardBreak)
}

/// Takes the blanks off the end of a line's text before its line break and, for a `block`'s
/// own inlines, off its start and its end with the line breaks there. Text left empty goes.
fn trim(read: &mut Vec<Read>, block: bool) {
    let blank = [' ', '\t'];
    let trim_end = |read: &mut Read| {
        if let (Inline::Text(text), range, Piece::Written) = read {
            text.truncate(text.trim_end_matches(blank).len());
            range.end = range.start + text.len();
        }
    };
    for k in 1..read.len() {
        if is_break(&read[k]) {
            trim_end(&mut read[k - 1]);
        }
    }
    let empty = |read: &Read| matches!(&read.0, Inline::Text(text) if text.is_empty());
    read.retain(|r| !empty(r));
    if !block {
        return;
    }
    while let Some(last) = read.last_mut() {
        trim_end(last);
        if !(is_break(last) || empty(last)) {
            break;
        }
        read.pop();
    }
    let lead = read.iter().take_while(|r| is_break(r)).count();
    read.drain(..lead);
    if let Some((Inline::Text(text), range, Piece::Written)) = read.first_mut() {
        let blanks = text.len() - text.trim_start_matches(blank).len();
        text.drain(..blanks);
        range.start += blanks;
        if text.is_empty() {
            read.remove(0);
        }
    }
}

/// Takes an ATX heading's closing `#`s off its text: a run of them at its end, after a blank
/// or alone.
fn unclose(read: &mut [Read]) {
    let Some(k) = read.iter().rposition(|r| !is_break(r)) else {
        return;
    };
    let (Inline::Text(text), range, Piece::Written) = &mut read[k] else {
        return;
    };
    let blank = [' ', '\t'];
    let end = text.trim_end_matches(blank);
    let words = end.trim_end_matches('#');
    if words.len() < end.len() && (words.ends_with(blank) || (words.is_empty() && k == 0)) {
        text.truncate(words.trim_end_matches(blank).len());
        range.end = range.start + text.len();
    }
}

/// A line of indented code without its indentation of four columns.
fn unindent(line: &str) -> &str {
    let mut col = 0;
    for (k, b) in line.bytes().enumerate() {
        match b {
            _ if col >= 4 => return &line[k..],
            b' ' => col += 1,
            b'\t' => col += 4 - col % 4,
            _ => return &line[k..],
        }
    }
    ""
}

/// The characters a character reference as written (`&amp;`, `&#35;`, `&#x23;`) stands for,
/// if it names any.
fn character(written: &str) -> Option<String> {
    let name = written.strip_prefix('&')?.strip_suffix(';')?;
    let Some(number) = name.strip_prefix('#') else {
        return entities::named(name).map(str::to_string);
    };
    let code = match number.strip_prefix(['x', 'X']) {
        Some(hex) if hex.len() <= 6 => u32::from_str_radix(hex, 16).ok()?,
        None if number.len() <= 7 => number.parse::<u32>().ok()?,
        _ => return None,
    };
    // As CommonMark has it: what is no character, and zero, is U+FFFD.
    let shown = char::from_u32(code).filter(|c| *c != '\0');
    Some(shown.unwrap_or('\u{fffd}').to_string())
}

/// A link's destination as it reads: its escapes and its character references resolved.
fn unescape(written: &str) -> String {
    let b = written.as_bytes();
    let mut out = String::with_capacity(written.len());
    let (mut from, mut i) = (0, 0);
    while i < b.len() {
        let escaped = b[i] == b'\\' && b.get(i + 1).is_some_and(u8::is_ascii_punctuation);
        let reference = match b[i] {
            b'&' => {
                let end = b[i..].iter().take(34).position(|c| *c == b';');
                end.and_then(|k| Some((k + 1, character(&written[i..=i + k])?)))
            }
            _ => None,
        };
        if escaped {
            out.push_str(&written[from..i]);
            (from, i) = (i + 1, i + 2);
        } else if let Some((len, shown)) = reference {
            out.push_str(&written[from..i]);
            out.push_str(&shown);
            (from, i) = (i + len, i + len);
        } else {
            i += 1;
        }
    }
    out.push_str(&written[from..]);
    out
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

// Text: what the tree leaves to the reader.

/// `#tag`: after the start of the text, a space, `(` or `,`; at least one non-digit. The workspace
/// index finds tags with the same pattern.
pub const TAG: &str = r"(?:^|[\s(,])#([\p{L}\p{N}_/\-]+)";

/// `TAG` in a note's own text, where the markers that open emphasis, a strikethrough or a
/// highlight may stand between what a tag follows and its `#` (`**#tag**`). The syntax tree
/// finds tags with it, and checks the markers, which it reads back from the `#`, against the
/// flavour.
pub const MARKED_TAG: &str = r"(?:^|[\s(,])[*_~=]*#([\p{L}\p{N}_/\-]+)";

/// Text next to text is one text.
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
        // Without embeds, `![[x]]` is a `!` and a wikilink, as the items count it.
        let blocks = parse("![[img.png]]", &only_links).blocks;
        let link = I::Link {
            target: "img.png".into(),
            kind: LinkKind::Wiki,
            text: vec![t("img.png")],
        };
        assert_eq!(para(blocks), [t("!"), link]);
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
