//! Matching loaded types against a note's blocks (docs/extensions.md, "Types"). `items::extract`
//! reads a note in one pass and hands each block here; a `Matcher` says which type it is, if
//! any, and reads its fields with the byte span of every value.
//!
//! A note is gated once (folder, tags, frontmatter), and a block is only checked against the
//! shapes for its primitive that passed. One type wins per note and per block: higher
//! `priority`, then a subtype over its base, then the shape with more conditions, then origin
//! (vault, config, extension), then name.

use std::cmp::Ordering;
use std::ops::Range;
use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value as Json;

use crate::frontmatter::{self, Frontmatter};
use crate::markdown::{self, Flavour, Syntax};
use crate::types::{InlineKind, NotePart, On, Origin, SectionPart, Source, Type, Types, Value};
use crate::{daily, vault};

/// The primitives this matcher reads; shapes on the others (code, quote, inline) wait.
const READ: &[On] = &[
    On::Note,
    On::Heading,
    On::Paragraph,
    On::ListItem,
    On::TableRow,
];

/// A typed item's kind, the types it answers to, and its fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Typed {
    pub kind: String,
    /// Its bases, nearest first: an `idea` that extends `task` is also a `task`.
    pub is: Vec<String>,
    /// Every field of the schema, in its order: null (or [] for a list) when absent.
    pub fields: IndexMap<String, Json>,
    /// Where each value was read, in the note's bytes; a list field has one per value.
    pub spans: IndexMap<String, Vec<Range<usize>>>,
    /// Values found that didn't parse as their field's type.
    pub unparsed: usize,
}

/// The loaded types, compiled for matching.
#[derive(Default)]
pub struct Matcher {
    types: Vec<Compiled>,
    /// (type, shape) for each primitive in `READ`.
    by_on: [Vec<(usize, usize)>; READ.len()],
    /// Some shape needs the note's tags, or its links.
    pub needs_tags: bool,
    pub needs_links: bool,
    daily_format: String,
}

struct Compiled {
    name: String,
    is: Vec<String>,
    priority: i32,
    origin: u8,
    schema: Vec<(String, Value, bool)>,
    shapes: Vec<Shape>,
}

struct Shape {
    on: On,
    w: crate::types::Where,
    pattern: Option<Regex>,
    key: Vec<usize>,
    /// Sources by schema field index.
    fields: Vec<(usize, Vec<Src>)>,
    conditions: usize,
}

enum Src {
    Capture(String),
    Pattern(Regex),
    Marker(String),
    Inline(Regex),
    Flag(String),
    Flags(IndexMap<String, String>),
    Column(String),
    Property(String),
    Inlines(InlineKind),
    Note(NotePart),
    Section(SectionPart),
    Rest,
    /// `children` and `info`, not read yet.
    Later,
}

/// A field's source gives a list: tags, links, children, the section path.
fn is_list(s: &Source) -> bool {
    matches!(
        s,
        Source::Inlines(_) | Source::Children(_) | Source::Section(SectionPart::Path)
    )
}

fn slot(on: On) -> Option<usize> {
    READ.iter().position(|o| *o == on)
}

impl Matcher {
    pub fn new(types: &Types, flavour: &Flavour, daily: &daily::Settings) -> Matcher {
        let mut m = Matcher {
            daily_format: daily.format.clone(),
            ..Matcher::default()
        };
        for t in &types.types {
            let d = &t.definition;
            let usable = d
                .requires
                .iter()
                .all(|r| Syntax::from_name(r).is_some_and(|s| flavour.has(s)));
            if !usable {
                continue;
            }
            let schema: Vec<(String, Value, bool)> = d
                .fields
                .iter()
                .map(|(name, value)| {
                    let list = d
                        .from
                        .iter()
                        .filter_map(|s| s.fields.get(name))
                        .any(|s| s.0.iter().any(is_list));
                    (name.clone(), value.clone(), list)
                })
                .collect();
            let field = |name: &str| schema.iter().position(|f| f.0 == name);
            let mut shapes = Vec::new();
            for shape in &d.from {
                if slot(shape.on).is_none() {
                    continue;
                }
                let w = &shape.conditions;
                m.needs_tags |= shape.on == On::Note && !w.tag.is_empty();
                let fields = shape
                    .fields
                    .iter()
                    .filter_map(|(name, sources)| {
                        let srcs = sources.0.iter().map(compile_source).collect();
                        Some((field(name)?, srcs))
                    })
                    .collect::<Vec<(usize, Vec<Src>)>>();
                for (_, srcs) in &fields {
                    for s in srcs {
                        if shape.on == On::Note {
                            m.needs_tags |= matches!(s, Src::Inlines(InlineKind::Tags));
                            m.needs_links |= matches!(s, Src::Inlines(InlineKind::Links));
                        }
                    }
                }
                let conditions = w.header.len()
                    + w.property.len()
                    + [&w.tag, &w.folder, &w.section, &w.language, &w.inside]
                        .iter()
                        .filter(|l| !l.is_empty())
                        .count()
                    + shape.pattern.is_some() as usize;
                shapes.push(Shape {
                    on: shape.on,
                    w: w.clone(),
                    pattern: shape
                        .pattern
                        .as_deref()
                        .and_then(|p| crate::types::compile(p).ok()),
                    key: shape.key.iter().filter_map(|k| field(k)).collect(),
                    fields,
                    conditions,
                });
            }
            m.types.push(Compiled {
                name: t.name.clone(),
                is: bases(t, types),
                priority: d.priority,
                origin: match t.origin {
                    Origin::Vault { .. } => 0,
                    Origin::Config { .. } => 1,
                    Origin::Extension { .. } => 2,
                },
                schema,
                shapes,
            });
        }
        for (t, c) in m.types.iter().enumerate() {
            for (s, shape) in c.shapes.iter().enumerate() {
                if let Some(slot) = slot(shape.on) {
                    m.by_on[slot].push((t, s));
                }
            }
        }
        m
    }

    /// No type reads this primitive, so the pass needn't collect it.
    pub fn reads(&self, on: On) -> bool {
        slot(on).is_some_and(|s| !self.by_on[s].is_empty())
    }

    pub fn is_empty(&self) -> bool {
        self.by_on.iter().all(Vec::is_empty)
    }

    /// The names of the types that can match, for `items KIND`.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.types.iter().map(|t| t.name.as_str())
    }

    /// Which shapes the note passes: folder, frontmatter properties and tags. Checked once per
    /// note, before any block.
    pub fn gate(&self, note: &Note) -> Vec<Vec<bool>> {
        self.types
            .iter()
            .map(|t| t.shapes.iter().map(|s| note_passes(&s.w, note)).collect())
            .collect()
    }

    /// The note's own type, if one matches.
    pub fn note(&self, gate: &[Vec<bool>], note: &Note) -> Option<(Typed, String)> {
        let block = Block {
            on: On::Note,
            text: note.body.0,
            at: note.body.1,
            section: &[],
            header: &[],
            cells: &[],
            inside: Vec::new(),
        };
        self.block(gate, note, &block)
    }

    /// The block's type and its key, if one matches.
    pub fn block(&self, gate: &[Vec<bool>], note: &Note, b: &Block) -> Option<(Typed, String)> {
        let slot = slot(b.on)?;
        let mut best: Option<(usize, usize, Option<regex::Captures>)> = None;
        for &(t, s) in &self.by_on[slot] {
            if !gate[t][s] {
                continue;
            }
            let shape = &self.types[t].shapes[s];
            if !block_passes(&shape.w, note, b) {
                continue;
            }
            let caps = match &shape.pattern {
                Some(re) => match re.captures(b.text) {
                    Some(c) => Some(c),
                    None => continue,
                },
                None => None,
            };
            let wins = match &best {
                None => true,
                Some((bt, bs, _)) => self.better((t, s), (*bt, *bs)) == Ordering::Greater,
            };
            if wins {
                best = Some((t, s, caps));
            }
        }
        let (t, s, caps) = best?;
        Some(self.read(t, s, caps, note, b))
    }

    /// Which of two matching (type, shape)s wins.
    fn better(&self, a: (usize, usize), b: (usize, usize)) -> Ordering {
        let (ta, tb) = (&self.types[a.0], &self.types[b.0]);
        ta.priority
            .cmp(&tb.priority)
            .then_with(|| {
                if ta.is.contains(&tb.name) {
                    Ordering::Greater
                } else if tb.is.contains(&ta.name) {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| {
                let (sa, sb) = (&ta.shapes[a.1], &tb.shapes[b.1]);
                sa.conditions.cmp(&sb.conditions)
            })
            .then_with(|| tb.origin.cmp(&ta.origin))
            .then_with(|| tb.name.cmp(&ta.name))
    }

    fn read(
        &self,
        t: usize,
        s: usize,
        caps: Option<regex::Captures>,
        note: &Note,
        b: &Block,
    ) -> (Typed, String) {
        let ty = &self.types[t];
        let shape = &ty.shapes[s];
        let mut typed = Typed {
            kind: ty.name.clone(),
            is: ty.is.clone(),
            fields: ty
                .schema
                .iter()
                .map(|(name, _, list)| {
                    let empty = if *list {
                        Json::Array(Vec::new())
                    } else {
                        Json::Null
                    };
                    (name.clone(), empty)
                })
                .collect(),
            spans: IndexMap::new(),
            unparsed: 0,
        };
        // Byte ranges of the block's text that sources read, for `rest`.
        let mut used: Vec<Range<usize>> = Vec::new();
        let mut rest_field = None;
        for (f, sources) in &shape.fields {
            let mut pieces = Vec::new();
            for src in sources {
                if matches!(src, Src::Rest) {
                    rest_field = Some(*f);
                    continue;
                }
                pieces = read_source(src, caps.as_ref(), note, b, &mut used);
                if !pieces.is_empty() {
                    break;
                }
            }
            self.set(&mut typed, ty, *f, pieces);
        }
        if let Some(f) = rest_field {
            used.extend(markdown::block_id_pattern().find(b.text).map(|m| m.range()));
            let rest = rest_of(b.text, &mut used);
            let pieces = if rest.is_empty() {
                Vec::new()
            } else {
                vec![Piece::text(rest, None)]
            };
            self.set(&mut typed, ty, f, pieces);
        }
        let key = if shape.key.is_empty() {
            match b.on {
                On::TableRow => b
                    .cells
                    .first()
                    .map(|c| note.src[c.clone()].to_string())
                    .unwrap_or_default(),
                On::Note => String::new(),
                _ => b.text.trim().to_string(),
            }
        } else {
            shape
                .key
                .iter()
                .map(|&f| typed.fields[f].to_string())
                .collect::<Vec<_>>()
                .join("\u{1f}")
        };
        (typed, key)
    }

    /// Parses what a field's source found and puts it on the item.
    fn set(&self, typed: &mut Typed, ty: &Compiled, f: usize, pieces: Vec<Piece>) {
        let (name, value, list) = &ty.schema[f];
        if pieces.is_empty() {
            return;
        }
        let spans: Vec<Range<usize>> = pieces.iter().filter_map(|p| p.span.clone()).collect();
        let mut parsed = Vec::new();
        for p in &pieces {
            match p
                .value
                .clone()
                .or_else(|| parse(value, &p.text, &self.daily_format))
            {
                Some(v) => parsed.push(v),
                None => typed.unparsed += 1,
            }
        }
        let json = if *list {
            Json::Array(parsed)
        } else if pieces.len() == 1 {
            parsed.pop().unwrap_or(Json::Null)
        } else {
            // One value was wanted and several were written.
            typed.unparsed += parsed.len();
            Json::Null
        };
        typed.fields[name.as_str()] = json;
        if !spans.is_empty() {
            typed.spans.insert(name.clone(), spans);
        }
    }
}

/// `t`'s bases, nearest first, ending with a native kind if the chain reaches one.
fn bases(t: &Type, types: &Types) -> Vec<String> {
    let mut is = Vec::new();
    let mut next = t.definition.extends.clone();
    while let Some(n) = next {
        if is.contains(&n) {
            break;
        }
        next = types
            .types
            .iter()
            .find(|b| b.name == n)
            .and_then(|b| b.definition.extends.clone());
        is.push(n);
    }
    is
}

fn compile_source(s: &Source) -> Src {
    match s {
        Source::Capture(g) => Src::Capture(g.clone()),
        Source::Pattern(p) => match crate::types::compile(p) {
            Ok(re) => Src::Pattern(re),
            Err(_) => Src::Later,
        },
        Source::Marker(g) => Src::Marker(g.clone()),
        Source::Inline(key) => {
            let k = regex::escape(key);
            Src::Inline(
                Regex::new(&format!(
                    r"(?m)\[{k}::[ \t]*([^\]\n]*)\]|\({k}::[ \t]*([^)\n]*)\)|(?:^|[ \t]){k}::[ \t]*([^\n]*)"
                ))
                .expect("an escaped key makes a valid pattern"),
            )
        }
        Source::Flag(g) => Src::Flag(g.clone()),
        Source::Flags(map) => Src::Flags(map.clone()),
        Source::Column(c) => Src::Column(c.clone()),
        Source::Property(p) => Src::Property(p.clone()),
        Source::Inlines(k) => Src::Inlines(*k),
        Source::Note(p) => Src::Note(*p),
        Source::Section(p) => Src::Section(*p),
        Source::Rest(_) => Src::Rest,
        Source::Children(_) | Source::Info(_) => Src::Later,
    }
}

/// What the matcher knows about the note a block is in.
pub struct Note<'a> {
    /// Vault-relative, with `.md`.
    pub path: &'a str,
    /// The text pulldown-cmark read, and its map back to the file.
    pub src: &'a str,
    pub source: &'a markdown::Source<'a>,
    pub frontmatter: Option<Frontmatter<'a>>,
    /// The text after the frontmatter, and its offset in `src`.
    pub body: (&'a str, usize),
    /// Its tags, frontmatter's then the body's, without `#`, each once.
    pub tags: Vec<String>,
    pub links: Vec<String>,
}

impl<'a> Note<'a> {
    /// The note at `path` with this text, read as `source`; `links` says whether some type
    /// reads its links, which then start with the frontmatter's.
    pub fn new(
        path: &'a str,
        text: &'a str,
        source: &'a markdown::Source<'a>,
        links: bool,
    ) -> Note<'a> {
        let frontmatter = frontmatter::read(text);
        let src: &str = &source.text;
        let body_at = match &frontmatter {
            Some(fm) => src
                .match_indices('\n')
                .nth(fm.end_line - 1)
                .map_or(src.len(), |(i, _)| i + 1),
            None => 0,
        };
        let mut note = Note {
            path,
            src,
            source,
            body: (&src[body_at..], body_at),
            tags: Vec::new(),
            links: Vec::new(),
            frontmatter,
        };
        let (tags, end_line) = match &note.frontmatter {
            Some(fm) => (
                ["tags", "tag"]
                    .iter()
                    .flat_map(|k| fm.values(k))
                    .map(|w| w.text.trim_start_matches('#').to_string())
                    .collect(),
                fm.end_line,
            ),
            None => (Vec::new(), 0),
        };
        for t in tags {
            note.add_tag(&t);
        }
        if links {
            for line in text.lines().take(end_line) {
                note.links.extend(vault::line_links(line));
            }
        }
        note
    }

    pub fn add_tag(&mut self, tag: &str) {
        if !tag.is_empty() && !self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            self.tags.push(tag.to_string());
        }
    }

    /// The tags in a piece of text, as `markdown::TAG` finds them.
    pub fn tags_in(text: &str) -> impl Iterator<Item = (&str, Range<usize>)> {
        tag_pattern().captures_iter(text).filter_map(|c| {
            let m = c.get(1)?;
            (!m.as_str().chars().all(|c| c.is_ascii_digit())).then(|| (m.as_str(), m.range()))
        })
    }

    fn has_tag(&self, wanted: &str) -> bool {
        self.tags.iter().any(|t| tag_under(t, wanted))
    }

    fn file(&self, src: Range<usize>) -> Range<usize> {
        self.source.file_range(src)
    }
}

fn tag_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(markdown::TAG).unwrap())
}

/// `volcano/caldera` is under `volcano`; `volcano-type` is not. Tags ignore case, as in Obsidian.
fn tag_under(tag: &str, wanted: &str) -> bool {
    let (tag, wanted) = (
        tag.to_lowercase(),
        wanted.trim_start_matches('#').to_lowercase(),
    );
    tag == wanted
        || tag
            .strip_prefix(&wanted)
            .is_some_and(|r| r.starts_with('/'))
}

/// A block the matcher is asked about.
pub struct Block<'a> {
    pub on: On,
    /// Its text: a list item's first line without its marker, a heading's words, a paragraph,
    /// a whole table row; and where it starts in `src`.
    pub text: &'a str,
    pub at: usize,
    pub section: &'a [String],
    /// A table row's header and its cells' ranges in `src`.
    pub header: &'a [String],
    pub cells: &'a [Range<usize>],
    /// What it sits inside: primitives, kinds and types.
    pub inside: Vec<&'a str>,
}

fn note_passes(w: &crate::types::Where, note: &Note) -> bool {
    let folder = note.path.rsplit_once('/').map_or("", |(f, _)| f);
    (w.folder.is_empty()
        || w.folder.iter().any(|f| {
            let f = f.trim_matches('/');
            f.is_empty() || folder == f || folder.starts_with(&format!("{f}/"))
        }))
        && w.property.iter().all(|(k, v)| {
            note.frontmatter
                .as_ref()
                .is_some_and(|fm| fm.values(k).iter().any(|w| w.text == v))
        })
}

/// The conditions on the block itself; a note's tags are the note's, a block's its own.
fn block_passes(w: &crate::types::Where, note: &Note, b: &Block) -> bool {
    (w.tag.is_empty()
        || w.tag.iter().any(|t| match b.on {
            On::Note => note.has_tag(t),
            _ => Note::tags_in(b.text).any(|(tag, _)| tag_under(tag, t)),
        }))
        && w.header.iter().all(|h| b.header.iter().any(|x| x == h))
        && (w.section.is_empty() || b.section.iter().any(|s| w.section.contains(s)))
        && (w.inside.is_empty() || b.inside.iter().any(|i| w.inside.iter().any(|x| x == i)))
}

/// One value a source found: the text as written, where it is in the file, and its value when
/// the source already knows it (a flag's true or false).
struct Piece {
    text: String,
    span: Option<Range<usize>>,
    value: Option<Json>,
}

impl Piece {
    fn text(text: String, span: Option<Range<usize>>) -> Piece {
        Piece {
            text,
            span,
            value: None,
        }
    }
}

/// What one source reads in a block; empty when it finds nothing. `used` collects the block's
/// text it read, for `rest`.
fn read_source(
    src: &Src,
    caps: Option<&regex::Captures>,
    note: &Note,
    b: &Block,
    used: &mut Vec<Range<usize>>,
) -> Vec<Piece> {
    let text = b.text;
    let at = |r: Range<usize>| note.file(b.at + r.start..b.at + r.end);
    let found = |r: Range<usize>, used: &mut Vec<Range<usize>>, whole: Range<usize>| {
        used.push(whole);
        let value = text[r.clone()].trim();
        if value.is_empty() {
            return Vec::new();
        }
        let start = r.start + (text[r.clone()].len() - text[r.clone()].trim_start().len());
        vec![Piece::text(
            value.to_string(),
            Some(at(start..start + value.len())),
        )]
    };
    match src {
        Src::Capture(g) => match caps.and_then(|c| Some((c.name(g)?, c.get(0)?))) {
            Some((m, whole)) => found(m.range(), used, whole.range()),
            None => Vec::new(),
        },
        Src::Pattern(re) => match re.captures(text) {
            Some(c) => match c.get(1) {
                Some(m) => found(m.range(), used, c.get(0).unwrap().range()),
                None => Vec::new(),
            },
            None => Vec::new(),
        },
        Src::Marker(glyph) => {
            let Some(i) = text.find(glyph.as_str()) else {
                return Vec::new();
            };
            let after = i + glyph.len();
            let lead = text[after..].len() - text[after..].trim_start().len();
            let start = after + lead;
            let len = text[start..]
                .find(char::is_whitespace)
                .unwrap_or(text.len() - start);
            found(start..start + len, used, i..start + len)
        }
        Src::Inline(re) => match re.captures(text) {
            Some(c) => {
                let m = (1..=3).find_map(|i| c.get(i)).expect("one form matched");
                found(m.range(), used, c.get(0).unwrap().range())
            }
            None => Vec::new(),
        },
        Src::Flag(glyph) => {
            let at_glyph = text.find(glyph.as_str());
            if let Some(i) = at_glyph {
                used.push(i..i + glyph.len());
            }
            vec![Piece {
                text: String::new(),
                span: at_glyph.map(|i| at(i..i + glyph.len())),
                value: Some(Json::Bool(at_glyph.is_some())),
            }]
        }
        Src::Flags(map) => {
            for (glyph, name) in map {
                if let Some(i) = text.find(glyph.as_str()) {
                    used.push(i..i + glyph.len());
                    return vec![Piece {
                        text: name.clone(),
                        span: Some(at(i..i + glyph.len())),
                        value: Some(Json::String(name.clone())),
                    }];
                }
            }
            Vec::new()
        }
        Src::Column(name) => {
            let Some(i) = b.header.iter().position(|h| h == name) else {
                return Vec::new();
            };
            match b.cells.get(i) {
                Some(cell) if !cell.is_empty() => vec![Piece::text(
                    note.src[cell.clone()].to_string(),
                    Some(note.file(cell.clone())),
                )],
                _ => Vec::new(),
            }
        }
        Src::Property(key) => {
            let Some(fm) = &note.frontmatter else {
                return Vec::new();
            };
            match fm.get(key) {
                Some(frontmatter::Value::Scalar(w)) => {
                    vec![Piece::text(w.text.to_string(), Some(w.span.clone()))]
                }
                Some(frontmatter::Value::List(l)) => l
                    .iter()
                    .map(|w| Piece::text(w.text.to_string(), Some(w.span.clone())))
                    .collect(),
                _ => Vec::new(),
            }
        }
        Src::Inlines(InlineKind::Tags) if b.on == On::Note => note
            .tags
            .iter()
            .map(|t| Piece::text(t.clone(), None))
            .collect(),
        Src::Inlines(InlineKind::Links) if b.on == On::Note => note
            .links
            .iter()
            .map(|l| Piece {
                text: l.clone(),
                span: None,
                value: Some(Json::String(l.clone())),
            })
            .collect(),
        Src::Inlines(InlineKind::Tags) => Note::tags_in(text)
            .map(|(t, r)| Piece::text(t.to_string(), Some(at(r))))
            .collect(),
        Src::Inlines(InlineKind::Links) => vault::line_links(text)
            .into_iter()
            .map(|l| Piece {
                value: Some(Json::String(l.clone())),
                text: l,
                span: None,
            })
            .collect(),
        Src::Note(part) => {
            let (folder, file) = note.path.rsplit_once('/').unwrap_or(("", note.path));
            let value = match part {
                NotePart::Name => file.strip_suffix(".md").unwrap_or(file),
                NotePart::Path => note.path,
                NotePart::Folder => folder,
            };
            vec![Piece::text(value.to_string(), None)]
        }
        Src::Section(SectionPart::Heading) => b
            .section
            .last()
            .map(|h| Piece::text(h.clone(), None))
            .into_iter()
            .collect(),
        Src::Section(SectionPart::Path) => b
            .section
            .iter()
            .map(|h| Piece::text(h.clone(), None))
            .collect(),
        Src::Rest | Src::Later => Vec::new(),
    }
}

/// The text no source read, with its spaces collapsed.
fn rest_of(text: &str, used: &mut [Range<usize>]) -> String {
    used.sort_by_key(|r| r.start);
    let mut out = String::new();
    let mut at = 0;
    for r in used.iter() {
        if r.start > at {
            out.push_str(&text[at..r.start]);
            out.push(' ');
        }
        at = at.max(r.end);
    }
    out.push_str(&text[at.min(text.len())..]);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A written value as its field's type; `None` when it doesn't parse.
pub fn parse(value: &Value, text: &str, daily_format: &str) -> Option<Json> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(match value {
        Value::Text => Json::String(text.to_string()),
        Value::Number => number(text.parse::<f64>().ok().filter(|n| n.is_finite())?),
        Value::Date { format } => {
            let format = format.as_deref().unwrap_or("YYYY-MM-DD");
            let format = format.replace("${daily.format}", daily_format);
            let date = daily::parse_date(text, &format)?;
            Json::String(date.format("%Y-%m-%d").to_string())
        }
        Value::Datetime { format: Some(_) } => return None,
        Value::Datetime { format: None } => {
            let t = [
                "%Y-%m-%dT%H:%M:%S",
                "%Y-%m-%dT%H:%M",
                "%Y-%m-%d %H:%M:%S",
                "%Y-%m-%d %H:%M",
            ]
            .iter()
            .find_map(|f| chrono::NaiveDateTime::parse_from_str(text, f).ok())?;
            Json::String(t.format("%Y-%m-%dT%H:%M:%S").to_string())
        }
        Value::Bool => match text.to_lowercase().as_str() {
            "true" | "yes" => Json::Bool(true),
            "false" | "no" => Json::Bool(false),
            _ => return None,
        },
        Value::Link => Json::String(
            vault::line_links(text)
                .into_iter()
                .next()
                .unwrap_or_else(|| text.to_string()),
        ),
        Value::Tag => {
            let t = text.strip_prefix('#').unwrap_or(text);
            if t.is_empty() || t.contains(char::is_whitespace) {
                return None;
            }
            Json::String(t.to_string())
        }
        Value::Enum { values } => Json::String(
            values
                .get(text)
                .cloned()
                .unwrap_or_else(|| "other".to_string()),
        ),
        Value::Duration => number(duration(text)?),
    })
}

fn number(n: f64) -> Json {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        Json::from(n as i64)
    } else {
        Json::from(n)
    }
}

/// `30m`, `1h30m`, `2d`, `90s` in minutes.
fn duration(text: &str) -> Option<f64> {
    let mut minutes = 0.0;
    let mut rest = text;
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .filter(|&i| i > 0)?;
        let n: f64 = rest[..digits].parse().ok()?;
        let unit = rest[digits..].chars().next()?;
        minutes += n * match unit {
            'd' => 1440.0,
            'h' => 60.0,
            'm' => 1.0,
            's' => 1.0 / 60.0,
            _ => return None,
        };
        rest = &rest[digits + 1..];
    }
    Some(minutes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{Data, Item, Settings, extract};
    use crate::tasks::TasksSettings;
    use crate::types::{Definitions, Layer, load};
    use serde_json::json;
    use std::path::Path;

    fn settings_with(layers: Vec<Layer>) -> Settings {
        let found = load(layers);
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        let nowhere = Path::new("/nonexistent-cold-lava-vault");
        Settings::new(
            Flavour::built_in("obsidian").unwrap(),
            TasksSettings::load(nowhere),
            daily::Settings::load(nowhere),
            crate::templates::Settings::default(),
            found,
        )
    }

    /// Settings with config.toml's `[types]` table written as `toml`.
    fn settings(toml: &str) -> Settings {
        let table: toml::Table = toml::from_str(toml).unwrap();
        settings_with(vec![Layer::config(Path::new("config.toml"), table)])
    }

    fn typed(items: &[Item]) -> Vec<&Typed> {
        items
            .iter()
            .filter_map(|i| match &i.data {
                Data::Typed(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    fn one<'a>(items: &'a [Item], kind: &str) -> &'a Item {
        let found: Vec<&Item> = items.iter().filter(|i| i.data.kind() == kind).collect();
        assert_eq!(found.len(), 1, "one {kind} in {items:#?}");
        found[0]
    }

    fn fields(item: &Item) -> Json {
        match &item.data {
            Data::Typed(t) => serde_json::to_value(&t.fields).unwrap(),
            d => panic!("not typed: {d:?}"),
        }
    }

    fn spans<'a>(text: &'a str, item: &Item, field: &str) -> Vec<&'a str> {
        let Data::Typed(t) = &item.data else { panic!() };
        t.spans
            .get(field)
            .map(|s| s.iter().map(|r| &text[r.clone()]).collect())
            .unwrap_or_default()
    }

    const VOLCANO: &str = r#"
[volcano]
on = "note"
where.tag = ["volcano"]
fields.name = { note = "name" }
fields.path = { note = "path" }
fields.folder = { note = "folder" }
fields.country = { property = "country" }
fields.elevation = { property = "elevation_m", type = "number" }
fields.tags = { inlines = "tags", type = "tag" }
fields.links = { inlines = "links", type = "link" }
fields.status = { inline = "status" }
fields.first = { capture = "w" }
match = '(?<w>\w+) eruption'
"#;

    const KRAKATOA: &str = "---\n\
        tags: [volcano/caldera]\n\
        country: Indonesia\n\
        ---\n\
        # Krakatoa\n\
        Its 1883 eruption (VEI 6, see [[Volcanic Explosivity Index]]) #eruption\n\
        %%a comment%% status:: dormant\n";

    #[test]
    fn a_note_reads_its_properties_name_tags_links_and_body() {
        let s = settings(VOLCANO);
        let all = extract("Volcanoes/Krakatoa.md", KRAKATOA, &s);
        let k = one(&all, "volcano");
        assert_eq!(
            (k.line, k.end_line),
            (1, 4),
            "a note item is its frontmatter"
        );
        assert_eq!(
            fields(k),
            json!({
                "name": "Krakatoa", "path": "Volcanoes/Krakatoa.md", "folder": "Volcanoes",
                "country": "Indonesia", "elevation": null,
                "tags": ["volcano/caldera", "eruption"],
                "links": ["Volcanic Explosivity Index"], "status": "dormant",
                "first": "1883"
            })
        );
        assert_eq!(
            all.iter().filter(|i| i.data.kind() == "heading").count(),
            1,
            "a note's type coexists with the blocks in it"
        );
        for (field, written) in [
            ("country", "Indonesia"),
            ("status", "dormant"),
            ("first", "1883"),
        ] {
            assert_eq!(spans(KRAKATOA, k, field), [written], "{field}");
        }
    }

    #[test]
    fn nested_tags_count_and_lookalikes_dont() {
        let s = settings("[volcano]\non = \"note\"\nwhere.tag = [\"volcano\"]\n");
        let tagged = |tags: &str| {
            let text = format!("---\ntags: [{tags}]\n---\nbody\n");
            !typed(&extract("V.md", &text, &s)).is_empty()
        };
        assert!(tagged("volcano"));
        assert!(tagged("Volcano/Strato"));
        assert!(!tagged("volcano-type"));
        assert!(!tagged("concept"));
        assert!(
            typed(&extract("V.md", "```\n#volcano\n```\n", &s)).is_empty(),
            "a tag in code is not a tag"
        );
        assert_eq!(
            typed(&extract("V.md", "Lava #volcano/shield here\n", &s)).len(),
            1,
            "a tag in the body counts"
        );
    }

    const ERUPTION: &str = r#"
[eruption]
fields = { vei = "number", example = "text", volcanoes = "link", tags = "tag",
           year = "number", decade = "number", under = "text", path = "text" }

[[eruption.from]]
on = "table-row"
where.header = ["VEI", "Example"]
key = ["vei"]
match = '(?<y>\d{4})'
fields = { vei = { column = "VEI" }, example = { column = "Example" },
           volcanoes = { inlines = "links" }, tags = { inlines = "tags" },
           year = { capture = "y" }, decade = { pattern = '(\d{3})\d' },
           under = { section = "heading" }, path = { section = "path" } }
"#;

    const VEI: &str = "\
# Index
## Scale

| VEI | Example |
| --- | --- |
| 4 | [[Eyjafjallajökull]] 2010 #iceland |
| 5 | %%x%% [[Mount St. Helens]] 1980, [[Vesuvius]] AD 79 |
| six | [[Krakatoa]] 1883 |
| 7 | no year |

| Name | Other |
| --- | --- |
| 1 | 1999 |
";

    #[test]
    fn table_rows_read_columns_captures_patterns_and_sections() {
        let s = settings(ERUPTION);
        let all = extract("VEI.md", VEI, &s);
        let rows: Vec<&Item> = all.iter().filter(|i| i.data.kind() == "eruption").collect();
        assert_eq!(
            rows.iter().map(|r| r.line).collect::<Vec<_>>(),
            [6, 7, 8],
            "a row without the match, and another table's rows, are not eruptions"
        );
        assert_eq!(
            fields(rows[0]),
            json!({
                "vei": 4, "example": "[[Eyjafjallajökull]] 2010 #iceland",
                "volcanoes": ["Eyjafjallajökull"], "tags": ["iceland"], "year": 2010,
                "decade": 201, "under": "Scale", "path": ["Index", "Scale"]
            })
        );
        assert_eq!(fields(rows[2])["vei"], Json::Null, "six is not a number");
        let Data::Typed(t) = &rows[2].data else {
            panic!()
        };
        assert_eq!(t.unparsed, 1);
        assert_eq!(spans(VEI, rows[0], "vei"), ["4"]);
        assert_eq!(spans(VEI, rows[0], "year"), ["2010"]);
        assert_eq!(spans(VEI, rows[0], "tags"), ["iceland"]);
        assert_eq!(
            spans(VEI, rows[1], "example"),
            ["[[Mount St. Helens]] 1980, [[Vesuvius]] AD 79"],
            "a span is of the file, after the comment before it"
        );
        assert_eq!(spans(VEI, rows[1], "year"), ["1980"], "after a comment");
        assert_eq!(
            rows[0].rev,
            crate::tasks::line_id(VEI.lines().nth(5).unwrap())
        );
    }

    #[test]
    fn row_ids_survive_other_rows_changing() {
        let s = settings(ERUPTION);
        let ids = |text: &str| -> Vec<(Json, String, String)> {
            extract("VEI.md", text, &s)
                .into_iter()
                .filter(|i| i.data.kind() == "eruption")
                .map(|i| (fields(&i)["vei"].clone(), i.id, i.rev))
                .collect()
        };
        let before = ids(VEI);
        let edited = VEI
            .replace(
                "| 4 | [[Eyjafjallajökull]] 2010",
                "| 4 | [[Eyjafjallajökull]] 2011",
            )
            .replace("# Index\n", "# Index\n\nA new line.\n");
        let after = ids(&edited);
        assert_eq!(before[0].1, after[0].1, "its own edit keeps the key");
        assert_ne!(before[0].2, after[0].2, "and changes the rev");
        assert_eq!(before[1..], after[1..], "other rows keep their id and rev");
    }

    const LISTS: &str = r#"
[chore]
on = "list-item"
where.section = ["Chores"]
fields.due = { marker = "📅", type = "date" }
fields.who = { inline = "who" }
fields.urgent = { flag = "❗", type = "bool" }
fields.size = { flags = { "🐘" = "big", "🐭" = "small" } }
fields.minutes = { pattern = '~(\S+)', type = "duration" }
fields.what = { rest = true }

[chapter]
on = "heading"
match = '^Chapter (?<n>\d+)'
fields.n = { capture = "n", type = "number" }
fields.title = { rest = true }
"#;

    #[test]
    fn list_items_and_headings_read_markers_fields_flags_and_the_rest() {
        let s = settings(LISTS);
        let text = "\
# Chapter 3 The Ash
## Chores
- Sweep ash 📅 2026-11-12 [who:: Ana] ❗ 🐘 ~1h30m ^sweep
- Rake 📅 someday
- [ ] Tasks stay tasks 📅 2026-11-12
## Other
- Not a chore
";
        let all = extract("Note.md", text, &s);
        let sweep = all.iter().find(|i| i.line == 3).unwrap();
        assert_eq!(sweep.data.kind(), "chore");
        assert_eq!(sweep.block_id.as_deref(), Some("sweep"));
        assert_eq!(
            fields(sweep),
            json!({
                "due": "2026-11-12", "who": "Ana", "urgent": true, "size": "big",
                "minutes": 90, "what": "Sweep ash"
            })
        );
        assert_eq!(spans(text, sweep, "due"), ["2026-11-12"]);
        assert_eq!(spans(text, sweep, "who"), ["Ana"]);
        let rake = all.iter().find(|i| i.line == 4).unwrap();
        assert_eq!(
            fields(rake),
            json!({
                "due": null, "who": null, "urgent": false, "size": null, "minutes": null,
                "what": "Rake"
            })
        );
        let Data::Typed(t) = &rake.data else { panic!() };
        assert_eq!(t.unparsed, 1, "someday is not a date");
        assert_eq!(all[4].data.kind(), "task", "tasks win over list-item types");
        assert_eq!(all[6].data.kind(), "list", "outside its section");
        let chapter = one(&all, "chapter");
        assert_eq!(
            fields(chapter),
            json!({ "n": 3, "title": "The Ash" }),
            "the rest is what the match didn't read"
        );
        assert!(
            all.iter()
                .all(|i| i.data.kind() != "heading" || i.line != 1),
            "a typed block replaces its structural kind"
        );
    }

    #[test]
    fn paragraphs_match_too() {
        let s = settings(
            "[quote]\non = \"paragraph\"\nwhere.tag = [\"quote\"]\n\
             fields.by = { inline = \"by\" }\nfields.text = { rest = true }\n",
        );
        let text = "Lava is hot. (by:: Ana) #quote ^q1\n\nPlain words.\n";
        let all = extract("Note.md", text, &s);
        let q = one(&all, "quote");
        assert_eq!(q.block_id.as_deref(), Some("q1"));
        assert_eq!(
            fields(q),
            json!({ "by": "Ana", "text": "Lava is hot. #quote" })
        );
        assert_eq!(all.len(), 1, "an untyped paragraph is no item");
    }

    #[test]
    fn values_parse_by_type() {
        let p = |v: Value, text: &str| parse(&v, text, "DD.MM.YYYY");
        let date = |f: &str| Value::Date {
            format: Some(f.into()),
        };
        assert_eq!(p(Value::Number, "1,651"), None);
        assert_eq!(p(Value::Number, "-2.5"), Some(json!(-2.5)));
        assert_eq!(
            p(date("D MMMM YYYY"), "18 May 1980"),
            Some(json!("1980-05-18"))
        );
        assert_eq!(
            p(date("${daily.format}"), "05.11.2026"),
            Some(json!("2026-11-05"))
        );
        assert_eq!(p(Value::Date { format: None }, "2026-13-01"), None);
        assert_eq!(p(Value::Bool, "Yes"), Some(json!(true)));
        assert_eq!(p(Value::Bool, "maybe"), None);
        assert_eq!(p(Value::Link, "[[Lava|hot rock]]"), Some(json!("Lava")));
        assert_eq!(p(Value::Link, "Lava"), Some(json!("Lava")));
        assert_eq!(
            p(Value::Tag, "#volcano/strato"),
            Some(json!("volcano/strato"))
        );
        assert_eq!(p(Value::Tag, "two words"), None);
        let status = Value::Enum {
            values: [
                ("to read".into(), "todo".into()),
                ("read".into(), "done".into()),
            ]
            .into(),
        };
        assert_eq!(p(status.clone(), "to read"), Some(json!("todo")));
        assert_eq!(p(status, "skimmed"), Some(json!("other")));
        assert_eq!(p(Value::Duration, "30m"), Some(json!(30)));
        assert_eq!(p(Value::Duration, "2d"), Some(json!(2880)));
        assert_eq!(p(Value::Duration, "1h30"), None);
        assert_eq!(
            p(Value::Datetime { format: None }, "2026-11-05 14:30"),
            Some(json!("2026-11-05T14:30:00"))
        );
    }

    #[test]
    fn one_type_per_block_by_priority_subtype_shape_origin_then_name() {
        let winner = |toml: &str| {
            let s = settings(toml);
            let all = extract("Note.md", "# Ideas\n- [ ] a task\n- think #idea\n", &s);
            all.iter()
                .find(|i| i.line == 3)
                .map(|i| (i.data.kind().to_string(), i.data.clone()))
                .unwrap()
        };
        let base = "[note]\non = \"list-item\"\n";
        let tagged = "[idea]\non = \"list-item\"\nwhere.tag = [\"idea\"]\n";
        assert_eq!(
            winner(&format!("{base}{tagged}")).0,
            "idea",
            "more conditions"
        );
        assert_eq!(
            winner(&format!("{base}priority = 1\n{tagged}")).0,
            "note",
            "priority first"
        );
        let sub = "[idea]\nextends = \"note\"\non = \"list-item\"\n";
        let (kind, data) = winner(&format!("{base}{sub}"));
        assert_eq!(kind, "idea", "a subtype beats its base");
        assert!(data.answers_to("note") && data.answers_to("idea"));
        let Data::Typed(t) = data else { panic!() };
        assert_eq!(t.is, ["note"]);
        assert_eq!(
            winner("[b]\non = \"list-item\"\n[a]\non = \"list-item\"\n").0,
            "a",
            "then by name"
        );

        let ext: Definitions = [(
            "thought".to_string(),
            toml::from_str("on = \"list-item\"").unwrap(),
        )]
        .into();
        let table: toml::Table = toml::from_str("[zzz]\non = \"list-item\"\n").unwrap();
        let s = settings_with(vec![
            Layer::extension("x", &ext),
            Layer::config(Path::new("config.toml"), table),
        ]);
        let all = extract("Note.md", "- think\n", &s);
        assert_eq!(all[0].data.kind(), "zzz", "config beats an extension");
    }

    #[test]
    fn note_ids_survive_body_edits_and_revs_follow_the_frontmatter() {
        let s = settings(VOLCANO);
        let k = |text: &str| {
            let all = extract("Volcanoes/Krakatoa.md", text, &s);
            let k = one(&all, "volcano");
            (k.id.clone(), k.rev.clone())
        };
        let before = k(KRAKATOA);
        let body = k(&KRAKATOA.replace("Its 1883", "Its famous 1883"));
        assert_eq!(before, body, "editing the body changes neither");
        let fm = k(&KRAKATOA.replace("country: Indonesia", "country: Indonesië"));
        assert_eq!(before.0, fm.0);
        assert_ne!(before.1, fm.1, "editing the frontmatter changes the rev");
        assert_ne!(
            before.0,
            extract("Volcanoes/Other.md", KRAKATOA, &s)
                .iter()
                .find(|i| i.data.kind() == "volcano")
                .unwrap()
                .id
        );
    }

    #[test]
    fn typed_item_json_is_pinned() {
        let s = settings(
            "[eruption]\nextends = \"block\"\non = \"table-row\"\n\
             fields.vei = { column = \"VEI\", type = \"number\" }\n\
             fields.place = { column = \"Where\" }\n\
             fields.links = { inlines = \"links\" }\n",
        );
        let all = extract("Note.md", "| VEI | Name |\n| --- | --- |\n| 4 | x |\n", &s);
        let item = one(&all, "eruption");
        let wire = json!({
            "id": item.id, "note": "Note.md", "line": 3, "end_line": 3, "section": [],
            "parent": null, "block_id": null, "rev": crate::tasks::line_id("| 4 | x |"),
            "kind": "eruption", "is": ["block"], "vei": 4, "place": null, "links": []
        });
        assert_eq!(serde_json::to_value(item).unwrap(), wire);
        let keys = serde_json::to_string(item).unwrap();
        let keys: Vec<&str> = keys
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|k| wire.get(k).is_some())
            .collect();
        assert_eq!(
            keys,
            [
                "id", "note", "line", "end_line", "section", "parent", "block_id", "rev", "kind",
                "is", "vei", "place", "links"
            ],
            "keys in order: the envelope, kind, is, then the schema's"
        );
        let back: Item = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), wire);
        assert!(
            serde_json::from_value::<Item>(json!({
                "id": "x", "note": "N.md", "line": 1, "end_line": 1, "section": [],
                "parent": null, "block_id": null, "rev": "r", "kind": "list", "text": "t"
            }))
            .is_ok_and(|i| matches!(i.data, Data::List { .. })),
            "native kinds still read as themselves"
        );
    }

    #[test]
    fn no_types_no_work() {
        let s = settings("");
        assert!(s.matcher.is_empty());
        let all = extract("Note.md", "| a |\n| - |\n| 1 |\n\nWords.\n", &s);
        assert!(all.is_empty());
    }
}
