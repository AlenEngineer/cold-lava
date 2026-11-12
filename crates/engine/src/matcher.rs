//! Matching loaded types against a note's blocks (docs/extensions.md, "Types"). `items::extract`
//! reads a note in one pass and hands each block here; a `Matcher` says which type it is, if
//! any, and reads its fields with the byte span of every value.
//!
//! A note is gated once (folder, tags, frontmatter), and a block is only checked against the
//! shapes for its primitive that passed. One type wins per note and per block: higher
//! `priority`, then a subtype over its base, then the shape with more conditions, then origin
//! (workspace, config, extension), then name. Inline types don't compete: every match of each is an
//! item.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::ops::Range;

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value as Json;

use crate::daily;
use crate::frontmatter::{self, Frontmatter};
use crate::markdown::{self, Flavour, Syntax};
use crate::owner::markdown as markdown_owner;
use crate::owner::markdown::attributes::{Flat, Readings};
use crate::types::{InlineKind, NotePart, On, Origin, SectionPart, Source, Type, Types, Value};

/// What this matcher reads; shapes on `verbatim` wait.
const READ: &[On] = &[
    On::Document,
    On::Section,
    On::Text,
    On::Item,
    On::Row,
    On::Aside,
    On::Inline,
];

pub use crate::fields::{Fields, Name, Spans};

/// A typed item's kind, the types it answers to, and its fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Typed {
    pub kind: String,
    /// Its bases, nearest first: an `idea` that extends `task` is also a `task`.
    pub is: Vec<String>,
    /// Every field of the schema, in its order: null (or [] for a list) when absent.
    pub fields: Fields,
    /// Where each value was read, in the note's bytes; a list field has one per value.
    pub spans: Spans,
    /// Values found that didn't parse as their field's type.
    pub unparsed: usize,
    /// Where it was read, for writing a field back (`edit.rs`); not on the wire.
    pub place: Place,
}

/// Where a typed item was read: what `edit::set_field` needs to put a value back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    /// Its shape, by index in its type's `from`.
    pub shape: usize,
    /// For each field that found something, which of its sources did (index in the shape's
    /// list for that field).
    pub sources: Vec<(Name, usize)>,
    /// The block's text in the file, where a missing marker, flag or field is appended.
    pub text: Range<usize>,
    /// The item's own text in the file: the block's text, an inline type's match, a note's
    /// body. A look with `whole` covers it.
    pub span: Range<usize>,
    /// A table row's cells in the file, under their header.
    pub cells: Vec<(String, Range<usize>)>,
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
    /// Some shape reads the tags, or the links, written in a block of its own (`Written`).
    pub reads_tags: bool,
    pub reads_links: bool,
    /// Some shape has `where.inside`.
    pub needs_inside: bool,
    daily: daily::Settings,
}

/// An attribute the note's structure gives a piece (docs/extensions.md, "The alphabet, as
/// built"), read off the block when a shape asks. The ones Markdown writes in a piece's text
/// (`checkbox`, `due`, …) are sources like any other by the time they are here: the owner's
/// table spelled them out (`owner::markdown::attributes`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Attr {
    /// A section's heading level.
    Level,
    /// An aside's callout: its kind, and whether it folds.
    Callout,
    Fold,
    /// A name nothing gives on this piece: it has none.
    Unknown,
}

impl Attr {
    fn of(name: &str) -> Attr {
        match name {
            "level" => Attr::Level,
            "callout" => Attr::Callout,
            "fold" => Attr::Fold,
            _ => Attr::Unknown,
        }
    }

    /// Whether the block carries it.
    fn on(self, b: &Block) -> bool {
        match (self, b.on) {
            (Attr::Level, On::Section) => b.given.level > 0,
            (Attr::Callout, On::Aside) => b.given.callout.is_some(),
            (Attr::Fold, On::Aside) => matches!(b.given.callout, Some((_, Some(_)))),
            _ => false,
        }
    }
}

struct Compiled {
    name: String,
    is: Vec<String>,
    priority: i32,
    origin: u8,
    schema: Vec<(Name, Value, bool)>,
    /// The schema's names, shared by its items' fields.
    names: crate::fields::Schema,
    shapes: Vec<Shape>,
}

struct Shape {
    /// Its index in the definition's `from`.
    index: usize,
    on: On,
    w: crate::types::Where,
    /// `where.attribute`: the block carries each, by one of its readings.
    attributes: Vec<Vec<Src>>,
    pattern: Option<Regex>,
    key: Vec<usize>,
    required: Vec<usize>,
    /// Sources by schema field index.
    fields: Vec<(usize, Vec<Src>)>,
    conditions: usize,
    /// Some field reads a group of `pattern`, so a match needs its captures.
    captures: bool,
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
    /// The leftover text, without these strings.
    Rest(Vec<String>),
    Checkbox,
    /// The `checkbox` attribute: a checkbox followed by whitespace or nothing, and, with a
    /// text, only on an item that holds that text after it. The text is not words.
    Box(Option<String>),
    Attribute(Attr),
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

/// A shape's conditions, with each role in `where.inside` under the name a block's `inside`
/// holds, the one it had before the roles were named (`On::alias`). A name some type has is
/// that type's: a type called `item` keeps meaning itself there.
fn with_inside(w: &crate::types::Where, types: &Types) -> crate::types::Where {
    let mut w = w.clone();
    for name in &mut w.inside {
        if types.types.iter().all(|t| t.name != *name)
            && let Some(on) = On::from_name(name)
        {
            *name = on.alias().to_string();
        }
    }
    w
}

fn slot(on: On) -> Option<usize> {
    READ.iter().position(|o| *o == on)
}

impl Matcher {
    /// `written` is how Markdown writes the attributes the types read.
    pub fn new(
        types: &Types,
        written: &Readings,
        flavour: &Flavour,
        daily: &daily::Settings,
    ) -> Matcher {
        let mut m = Matcher {
            daily: daily.clone(),
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
            let schema: Vec<(Name, Value, bool)> = d
                .fields
                .iter()
                .map(|(name, value)| {
                    let list = d
                        .from
                        .iter()
                        .filter_map(|s| s.fields.get(name))
                        .any(|s| s.0.iter().any(is_list));
                    (Name::from(name.as_str()), value.clone(), list)
                })
                .collect();
            let field = |name: &str| schema.iter().position(|f| &*f.0 == name);
            let mut shapes = Vec::new();
            for (index, shape) in d.from.iter().enumerate() {
                if slot(shape.on).is_none() {
                    continue;
                }
                let w = &shape.conditions;
                let whole_note = matches!(shape.on, On::Document | On::Inline);
                m.needs_tags |= whole_note && !w.tag.is_empty();
                m.reads_tags |= !whole_note && !w.tag.is_empty();
                m.needs_inside |= !w.inside.is_empty();
                let fields = shape
                    .fields
                    .iter()
                    .filter_map(|(name, sources)| {
                        let srcs = (written.flat(shape.on, &sources.0).iter())
                            .map(|f| compile_flat(f, flavour))
                            .collect();
                        Some((field(name)?, srcs))
                    })
                    .collect::<Vec<(usize, Vec<Src>)>>();
                for (_, srcs) in &fields {
                    for s in srcs {
                        let (tags, links) = (
                            matches!(s, Src::Inlines(InlineKind::Tags)),
                            matches!(s, Src::Inlines(InlineKind::Links)),
                        );
                        if shape.on == On::Document {
                            m.needs_tags |= tags;
                            m.needs_links |= links;
                        } else {
                            m.reads_tags |= tags;
                            m.reads_links |= links;
                        }
                    }
                }
                let conditions = w.conditions() + shape.pattern.is_some() as usize;
                shapes.push(Shape {
                    index,
                    on: shape.on,
                    w: with_inside(w, types),
                    attributes: (w.attribute.iter())
                        .map(|name| {
                            let asked = [Source::Attribute(name.clone())];
                            (written.flat(shape.on, &asked).iter())
                                .map(|f| compile_flat(f, flavour))
                                .collect()
                        })
                        .collect(),
                    pattern: shape
                        .pattern
                        .as_deref()
                        .and_then(|p| crate::types::compile(p).ok()),
                    key: shape.key.iter().filter_map(|k| field(k)).collect(),
                    required: shape.required.iter().filter_map(|k| field(k)).collect(),
                    captures: fields
                        .iter()
                        .any(|(_, s)| s.iter().any(|s| matches!(s, Src::Capture(_)))),
                    fields,
                    conditions,
                });
            }
            m.types.push(Compiled {
                name: t.name.clone(),
                is: bases(t, types),
                priority: priority(t, types),
                origin: match t.origin {
                    Origin::Workspace { .. } => 0,
                    Origin::Config { .. } => 1,
                    Origin::Extension { .. } => 2,
                    Origin::BuiltIn => 3,
                },
                names: schema
                    .iter()
                    .map(|(name, _, list)| (name.clone(), *list))
                    .collect(),
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

    /// An item of the type `kind` with these fields, as an owner gave them: each value read as
    /// its field's type (as it would be written, or as JSON), a list field from a list. A value
    /// that doesn't read is counted in `unparsed`; a field the type doesn't have is an error.
    pub fn given(
        &self,
        kind: &str,
        given: &serde_json::Map<String, Json>,
    ) -> Result<Typed, String> {
        let ty = (self.types.iter())
            .find(|t| t.name == kind)
            .ok_or_else(|| format!("no type called {kind}"))?;
        let mut typed = Typed {
            kind: ty.name.clone(),
            is: ty.is.clone(),
            fields: Fields::new(ty.names.clone()),
            spans: Spans::default(),
            unparsed: 0,
            place: Place::default(),
        };
        for (name, v) in given {
            let i = (ty.schema.iter())
                .position(|(n, ..)| **n == **name)
                .ok_or_else(|| format!("{kind} has no field {name}"))?;
            let (_, value, list) = &ty.schema[i];
            let given: Vec<&Json> = match v {
                Json::Array(vs) if *list => vs.iter().collect(),
                Json::Null => Vec::new(),
                v => vec![v],
            };
            let read: Vec<Json> = (given.iter())
                .filter_map(|v| match v {
                    Json::String(s) => parse(value, s, &self.daily.format),
                    v => parse(value, &v.to_string(), &self.daily.format),
                })
                .collect();
            typed.unparsed += given.len() - read.len();
            match (*list, read.first()) {
                (true, _) if !read.is_empty() => typed.fields.set(i, Json::Array(read)),
                (false, Some(one)) => typed.fields.set(i, one.clone()),
                _ => {}
            }
        }
        Ok(typed)
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
            on: On::Document,
            text: note.body.0,
            at: note.body.1,
            section: &[],
            header: &[],
            cells: &[],
            inside: Vec::new(),
            code: &[],
            given: Given::default(),
        };
        self.block(gate, note, &block)
    }

    /// The block's type and its key, if one matches.
    pub fn block(&self, gate: &[Vec<bool>], note: &Note, b: &Block) -> Option<(Typed, String)> {
        let slot = slot(b.on)?;
        let mut passing: Vec<(usize, usize, Option<regex::Captures>)> = Vec::new();
        for &(t, s) in &self.by_on[slot] {
            if !gate[t][s] {
                continue;
            }
            let shape = &self.types[t].shapes[s];
            let carries = |readings: &Vec<Src>| readings.iter().any(|s| self.carries(s, note, b));
            if !block_passes(&shape.w, note, b) || !shape.attributes.iter().all(carries) {
                continue;
            }
            let caps = match &shape.pattern {
                Some(re) if shape.captures => match re.captures(b.text) {
                    Some(c) => Some(c),
                    None => continue,
                },
                Some(re) if !re.is_match(b.text) => continue,
                _ => None,
            };
            passing.push((t, s, caps));
        }
        // The winner first; of equals, the one loaded first. One missing a required field
        // gives way to the next.
        passing.sort_by(|a, b| self.better((b.0, b.1), (a.0, a.1)));
        passing.into_iter().find_map(|(t, s, caps)| {
            let (typed, key) = self.read(t, s, caps, note, b);
            let has = |&f: &usize| match &typed.fields[f] {
                Json::Null => false,
                Json::Array(a) => !a.is_empty(),
                _ => true,
            };
            self.types[t].shapes[s]
                .required
                .iter()
                .all(has)
                .then_some((typed, key))
        })
    }

    /// Every match of an inline type in a block's text, in order, with its range in that text.
    /// A match that touches `code` (ranges in `src`: code spans, fences, math) doesn't count.
    pub fn inline(
        &self,
        gate: &[Vec<bool>],
        note: &Note,
        b: &Block,
    ) -> Vec<(Typed, String, Range<usize>)> {
        let mut found: Vec<(usize, Typed, String, Range<usize>)> = Vec::new();
        let Some(slot) = slot(On::Inline) else {
            return Vec::new();
        };
        for &(t, s) in &self.by_on[slot] {
            let shape = &self.types[t].shapes[s];
            if !gate[t][s] || !block_passes(&shape.w, note, b) {
                continue;
            }
            let Some(re) = &shape.pattern else { continue };
            for caps in re.captures_iter(b.text) {
                let whole = caps.get(0).expect("group 0 is the match").range();
                let again = found.iter().any(|f| f.0 == t && f.3 == whole);
                if whole.is_empty() || b.in_code(&whole) || again {
                    continue;
                }
                let (typed, key) = self.read(t, s, Some(caps), note, b);
                found.push((t, typed, key, whole));
            }
        }
        found.sort_by_key(|f| f.3.start);
        found.into_iter().map(|f| (f.1, f.2, f.3)).collect()
    }

    /// Whether one reading of an attribute finds it on the block (`where.attribute`).
    fn carries(&self, src: &Src, note: &Note, b: &Block) -> bool {
        match src {
            Src::Box(with) => b.on == On::Item && boxed(b.text, with.as_deref()).is_some(),
            Src::Attribute(attr) => attr.on(b),
            src => read_source(src, None, note, b, &self.daily, &mut Vec::new())
                .iter()
                .any(|p| p.value != Some(Json::Bool(false))),
        }
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
            fields: Fields::new(ty.names.clone()),
            spans: Spans::default(),
            unparsed: 0,
            place: Place {
                shape: shape.index,
                sources: Vec::new(),
                text: note.file(b.at..b.at + b.text.len()),
                span: note.file(b.at..b.at + b.text.len()),
                cells: b
                    .header
                    .iter()
                    .zip(b.cells)
                    .map(|(h, c)| (h.clone(), note.file(c.clone())))
                    .collect(),
            },
        };
        // Byte ranges of the block's text that sources read, for `rest`.
        let mut used: Vec<Range<usize>> = Vec::with_capacity(2 * shape.fields.len());
        // What an attribute the shape asked for was read from is the attribute's, not words:
        // a task's global filter.
        for src in shape.attributes.iter().flatten() {
            match src {
                Src::Box(with) => {
                    if let Some((_, end)) = boxed(b.text, with.as_deref()) {
                        used.push(0..end + 1);
                    }
                    if let Some(with) = with {
                        let held = b.text.match_indices(with.as_str());
                        used.extend(held.map(|(i, _)| i..i + with.len()));
                    }
                }
                Src::Attribute(_) => {}
                src => drop(read_source(src, None, note, b, &self.daily, &mut used)),
            }
        }
        let mut rest_field = None;
        for (f, sources) in &shape.fields {
            // The first source with a value gives it; a value the others find is still the
            // field's, and not words.
            let mut pieces = Vec::new();
            for (i, src) in sources.iter().enumerate() {
                if let Src::Rest(without) = src {
                    rest_field = Some((*f, without));
                    continue;
                }
                let before = used.len();
                let found = read_source(src, caps.as_ref(), note, b, &self.daily, &mut used);
                if pieces.is_empty() {
                    if !found.is_empty() {
                        pieces = found;
                        let name = ty.schema[*f].0.clone();
                        typed.place.sources.push((name, i));
                    }
                } else if !found.iter().all(|p| self.parses(&ty.schema[*f].1, p)) {
                    used.truncate(before);
                }
            }
            self.set(&mut typed, ty, *f, pieces);
        }
        if let Some((f, without)) = rest_field {
            // A list item's text comes without its block id; a `^x` left on its first line,
            // when more lines follow, names nothing and is words.
            if b.on != On::Item {
                used.extend(markdown::block_id_pattern().find(b.text).map(|m| m.range()));
            }
            for w in without {
                used.extend(
                    b.text
                        .match_indices(w.as_str())
                        .map(|(i, _)| i..i + w.len()),
                );
            }
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
                On::Row => b
                    .cells
                    .first()
                    .map(|c| note.src[c.clone()].to_string())
                    .unwrap_or_default(),
                On::Document => String::new(),
                On::Inline => caps
                    .as_ref()
                    .and_then(|c| c.get(0))
                    .map_or("", |m| m.as_str().trim())
                    .to_string(),
                _ => b.text.trim().to_string(),
            }
        } else {
            let mut key = String::new();
            for (n, &f) in shape.key.iter().enumerate() {
                if n > 0 {
                    key.push('\u{1f}');
                }
                match &typed.fields[f] {
                    // Text as itself, so a task keyed by its words keeps the id it always had.
                    Json::String(s) => key.push_str(s),
                    v => key.push_str(&v.to_string()),
                }
            }
            key
        };
        // Kept for as long as the note is: no room for values it won't get.
        typed.fields.shrink_to_fit();
        typed.spans.shrink_to_fit();
        typed.place.sources.shrink_to_fit();
        (typed, key)
    }

    fn parses(&self, value: &Value, p: &Piece) -> bool {
        p.value.is_some()
            || if p.verbatim {
                parse_as_written(value, &p.text, &self.daily.format).is_some()
            } else {
                parse(value, &p.text, &self.daily.format).is_some()
            }
    }

    /// Parses what a field's source found and puts it on the item.
    fn set(&self, typed: &mut Typed, ty: &Compiled, f: usize, pieces: Vec<Piece>) {
        let (name, value, list) = &ty.schema[f];
        if pieces.is_empty() {
            return;
        }
        if pieces.iter().any(|p| p.span.is_some()) {
            let spans = pieces.iter().filter_map(|p| p.span.clone());
            typed.spans.insert(name.clone(), spans);
        }
        let format = &self.daily.format;
        let parse = |p: Piece| match p.value {
            Some(v) => Some(v),
            None if p.verbatim => parse_as_written(value, &p.text, format),
            None => parse(value, &p.text, format),
        };
        let n = pieces.len();
        let json = if *list {
            let parsed: Vec<Json> = pieces.into_iter().filter_map(parse).collect();
            typed.unparsed += n - parsed.len();
            Json::Array(parsed)
        } else if n == 1 {
            let parsed = pieces.into_iter().next().and_then(parse);
            typed.unparsed += parsed.is_none() as usize;
            parsed.unwrap_or(Json::Null)
        } else {
            // One value was wanted and several were written.
            typed.unparsed += n;
            Json::Null
        };
        // The schema's order is the fields' order.
        typed.fields.set(f, json);
    }
}

/// `t`'s priority: its own, else its nearest base's that has one, else 0.
fn priority(t: &Type, types: &Types) -> i32 {
    let mut t = t;
    for _ in 0..types.types.len() {
        if let Some(p) = t.definition.priority {
            return p;
        }
        match t
            .definition
            .extends
            .as_ref()
            .and_then(|b| types.types.iter().find(|x| x.name == *b))
        {
            Some(base) => t = base,
            None => break,
        }
    }
    0
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

/// One source of a field, as the owner's table spelled it out. A checkbox that stands for
/// the `checkbox` attribute is the flavour's: without task lists no item has one.
fn compile_flat(f: &Flat, flavour: &Flavour) -> Src {
    match (&*f.source, f.attribute) {
        (Source::Checkbox(_), Some((_, with))) => match flavour.has(Syntax::TaskLists) {
            true => Src::Box((!with.is_empty()).then(|| with.to_string())),
            false => Src::Later,
        },
        (s, _) => compile_source(s),
    }
}

/// An item's checkbox as the attribute: one character between brackets, then whitespace or
/// nothing, and with `with`, only when the text holds it after that.
fn boxed(text: &str, with: Option<&str>) -> Option<(char, usize)> {
    let (symbol, end) = markdown_owner::checkbox(text)?;
    if let Some(with) = with {
        let after = &text[end + 1..];
        let space = after.chars().next().map_or(0, char::len_utf8);
        if !after[space..].contains(with) {
            return None;
        }
    }
    Some((symbol, end))
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
                    r"(?m)\[{k}::[ \t]*([^\]\n]*)\]|\({k}::[ \t]*([^)\n]*)\)|^[ \t]*{k}::[ \t]*([^\n]*)"
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
        Source::Rest(r) => Src::Rest(r.without.clone()),
        Source::Checkbox(_) => Src::Checkbox,
        Source::Attribute(name) => Src::Attribute(Attr::of(name)),
        Source::Children(_) | Source::Info(_) => Src::Later,
    }
}

/// What the matcher knows about the note a block is in.
pub struct Note<'a> {
    /// Workspace-relative, with `.md`; for a piece of a file another owner reads, that file's
    /// path, with its extension.
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
    /// Where its tags and links are written, for a block's own.
    pub written: Written,
    /// The date of the daily note it is, read once its blocks ask.
    date: std::cell::OnceCell<Option<chrono::NaiveDate>>,
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
            path: path.split('\0').next().unwrap_or(path),
            src,
            source,
            body: (&src[body_at..], body_at),
            tags: Vec::new(),
            links: Vec::new(),
            written: Written::default(),
            frontmatter,
            date: Default::default(),
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
                note.links.extend(crate::owner::markdown::line_links(line));
            }
        }
        note
    }

    pub fn add_tag(&mut self, tag: &str) {
        if !tag.is_empty() && !self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            self.tags.push(tag.to_string());
        }
    }

    fn has_tag(&self, wanted: &str) -> bool {
        self.tags.iter().any(|t| tag_under(t, wanted))
    }

    fn file(&self, src: Range<usize>) -> Range<usize> {
        self.source.file_range(src)
    }
}

/// The tags and the links written in a note, as its syntax tree reads them, by where they are
/// in `src`, in order: a block's own `tags` and `links` are the ones on its lines.
#[derive(Default)]
pub struct Written {
    /// Each tag's name, without its `#`.
    pub tags: Vec<Range<usize>>,
    /// Each link: where its target is written, whether it is a Markdown link, and the note it
    /// names.
    pub links: Vec<(usize, bool, String)>,
}

impl Written {
    fn tags_in(&self, block: Range<usize>) -> &[Range<usize>] {
        let from = self.tags.partition_point(|t| t.start < block.start);
        let to = self.tags.partition_point(|t| t.start < block.end);
        &self.tags[from..to]
    }

    fn links_in(&self, block: Range<usize>) -> &[(usize, bool, String)] {
        let from = self.links.partition_point(|l| l.0 < block.start);
        let to = self.links.partition_point(|l| l.0 < block.end);
        &self.links[from..to]
    }
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
    /// a whole table row; for inline types, the text they are looked for in. And where it starts
    /// in `src`.
    pub text: &'a str,
    pub at: usize,
    pub section: &'a [String],
    /// A table row's header and its cells' ranges in `src`.
    pub header: &'a [String],
    pub cells: &'a [Range<usize>],
    /// What it sits inside: primitives, kinds and types.
    pub inside: Vec<&'a str>,
    /// The note's code spans, code blocks and math (ranges in `src`, in order), where no source
    /// reads and no inline type matches.
    pub code: &'a [Range<usize>],
    /// The attributes its owner read off the file's structure, not off its text.
    pub given: Given,
}

/// The attributes of a piece that its text doesn't hold, as the walk through the note found
/// them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Given {
    /// A section's heading level; 0 on any other piece.
    pub level: u8,
    /// An aside's callout: where its kind is written in `src` (`warning` in `[!warning]`), and
    /// whether it folds: closed (`-`), open (`+`) or not at all.
    pub callout: Option<(Range<usize>, Option<bool>)>,
}

impl Block<'_> {
    /// Where its text is in `src`.
    fn range(&self) -> Range<usize> {
        self.at..self.at + self.text.len()
    }

    /// Whether this range of its text touches code.
    fn in_code(&self, r: &Range<usize>) -> bool {
        let (start, end) = (self.at + r.start, self.at + r.end);
        let i = self.code.partition_point(|c| c.end <= start);
        self.code.get(i).is_some_and(|c| c.start < end)
    }
}

fn note_passes(w: &crate::types::Where, note: &Note) -> bool {
    passes(w, note.path, note.frontmatter.as_ref())
}

/// Whether the note at `path` (workspace-relative, with `.md`), with `text` and `tags` (as the
/// scan found them, frontmatter's and body's), meets `w` as a note shape's `where` would:
/// folder, file type, frontmatter properties and tags. What a claim to show notes is matched
/// with (docs/extensions.md, "Owners on top of Markdown, as built").
pub fn note_meets(w: &crate::types::Where, path: &str, text: &str, tags: &[String]) -> bool {
    passes(w, path, frontmatter::read(text).as_ref())
        && (w.tag.is_empty() || w.tag.iter().any(|t| tags.iter().any(|x| tag_under(x, t))))
}

fn passes(w: &crate::types::Where, path: &str, fm: Option<&Frontmatter>) -> bool {
    let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
    let kind = file.rsplit_once('.').map_or("", |(_, x)| x);
    (w.file_type.is_empty()
        || (w.file_type.iter()).any(|x| x.trim_start_matches('.').eq_ignore_ascii_case(kind)))
        && (w.folder.is_empty()
            || w.folder.iter().any(|f| {
                let f = f.trim_matches('/');
                f.is_empty() || folder == f || folder.starts_with(&format!("{f}/"))
            }))
        && w.property
            .iter()
            .all(|(k, v)| fm.is_some_and(|fm| fm.values(k).iter().any(|w| w.text == v)))
}

/// The conditions on the block itself; a note's tags are the note's, a block's its own, and an
/// inline match, which has no text to tag, goes by the note's.
fn block_passes(w: &crate::types::Where, note: &Note, b: &Block) -> bool {
    (w.tag.is_empty()
        || w.tag.iter().any(|t| match b.on {
            On::Document | On::Inline => note.has_tag(t),
            _ => (note.written.tags_in(b.range()).iter())
                .any(|tag| tag_under(&note.src[tag.clone()], t)),
        }))
        && w.header.iter().all(|h| b.header.iter().any(|x| x == h))
        && (w.section.is_empty() || b.section.iter().any(|s| w.section.contains(s)))
        && (w.inside.is_empty() || b.inside.iter().any(|i| w.inside.iter().any(|x| x == i)))
}

/// One value a source found: the text as written, where it is in the file, and its value when
/// the source already knows it (a flag's true or false).
struct Piece<'a> {
    text: Cow<'a, str>,
    span: Option<Range<usize>>,
    value: Option<Json>,
    /// Its text is the value as written, spaces and all (a checkbox's ` `).
    verbatim: bool,
}

impl<'a> Piece<'a> {
    fn text(text: impl Into<Cow<'a, str>>, span: Option<Range<usize>>) -> Piece<'a> {
        Piece {
            text: text.into(),
            span,
            value: None,
            verbatim: false,
        }
    }
}

/// What one source reads in a block; empty when it finds nothing. `used` collects the block's
/// text it read, for `rest`.
fn read_source<'a>(
    src: &'a Src,
    caps: Option<&regex::Captures<'a>>,
    note: &'a Note,
    b: &Block<'a>,
    daily: &daily::Settings,
    used: &mut Vec<Range<usize>>,
) -> Vec<Piece<'a>> {
    let text = b.text;
    let at = |r: Range<usize>| note.file(b.at + r.start..b.at + r.end);
    let found = |r: Range<usize>, used: &mut Vec<Range<usize>>, whole: Range<usize>| {
        used.push(whole);
        let value = text[r.clone()].trim();
        if value.is_empty() {
            return Vec::new();
        }
        let start = r.start + (text[r.clone()].len() - text[r.clone()].trim_start().len());
        vec![Piece::text(value, Some(at(start..start + value.len())))]
    };
    match src {
        Src::Capture(g) => match caps.and_then(|c| Some((c.name(g)?, c.get(0)?))) {
            Some((m, whole)) => found(m.range(), used, whole.range()),
            None => Vec::new(),
        },
        Src::Pattern(re) if !re.is_match(text) => Vec::new(),
        Src::Pattern(re) => {
            let c = re
                .captures_iter(text)
                .find(|c| !b.in_code(&c.get(0).unwrap().range()));
            match c {
                Some(c) => match c.get(1) {
                    Some(m) => found(m.range(), used, c.get(0).unwrap().range()),
                    None => Vec::new(),
                },
                None => Vec::new(),
            }
        }
        Src::Marker(glyph) => {
            // Every one written is the field's; the first gives the value, as the Tasks plugin
            // reads them.
            let mut first = Vec::new();
            for i in glyph_at(text, glyph) {
                // A variation selector after an emoji is part of the glyph: `⏳️` is `⏳`.
                let after = i + glyph.len();
                let after = after + text[after..].strip_prefix('\u{FE0F}').map_or(0, |_| 3);
                let lead = text[after..].len() - text[after..].trim_start().len();
                let start = after + lead;
                let len = text[start..]
                    .find(char::is_whitespace)
                    .unwrap_or(text.len() - start);
                if b.in_code(&(i..start + len)) {
                    continue;
                }
                let pieces = found(start..start + len, used, i..start + len);
                if first.is_empty() {
                    first = pieces;
                }
            }
            first
        }
        // Every form has `key::`; most blocks have none.
        Src::Inline(re) if !text.contains("::") || !re.is_match(text) => Vec::new(),
        Src::Inline(re) => {
            let mut first = Vec::new();
            for c in re.captures_iter(text) {
                let whole = c.get(0).unwrap().range();
                if b.in_code(&whole) {
                    continue;
                }
                let m = (1..=3).find_map(|i| c.get(i)).expect("one form matched");
                let pieces = found(m.range(), used, whole);
                if first.is_empty() {
                    first = pieces;
                }
            }
            first
        }
        Src::Flag(glyph) => {
            let at_glyph = glyph_at(text, glyph).find(|&i| !b.in_code(&(i..i + glyph.len())));
            if let Some(i) = at_glyph {
                used.push(i..i + glyph.len());
            }
            vec![Piece {
                text: Cow::Borrowed(""),
                span: at_glyph.map(|i| at(i..i + glyph.len())),
                value: Some(Json::Bool(at_glyph.is_some())),
                verbatim: false,
            }]
        }
        Src::Flags(map) => {
            // Every glyph of the map is the source's text, though only the first one found
            // gives the value.
            let outside = |glyph: &str| {
                glyph_at(text, glyph)
                    .map(|i| i..i + glyph.len())
                    .filter(|r| !b.in_code(r))
                    .collect::<Vec<_>>()
            };
            for glyph in map.keys() {
                used.extend(outside(glyph));
            }
            let first = map
                .iter()
                .find_map(|(glyph, name)| Some((outside(glyph).first()?.start, glyph, name)));
            match first {
                Some((i, glyph, name)) => vec![Piece {
                    text: Cow::Borrowed(name),
                    span: Some(at(i..i + glyph.len())),
                    value: Some(Json::String(name.clone())),
                    verbatim: false,
                }],
                None => Vec::new(),
            }
        }
        Src::Column(name) => {
            let Some(i) = b.header.iter().position(|h| h == name) else {
                return Vec::new();
            };
            match b.cells.get(i) {
                Some(cell) if !cell.is_empty() => vec![Piece::text(
                    &note.src[cell.clone()],
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
                    vec![Piece::text(w.text, Some(w.span.clone()))]
                }
                Some(frontmatter::Value::List(l)) => l
                    .iter()
                    .map(|w| Piece::text(w.text, Some(w.span.clone())))
                    .collect(),
                _ => Vec::new(),
            }
        }
        Src::Inlines(InlineKind::Tags) if b.on == On::Document => note
            .tags
            .iter()
            .map(|t| Piece::text(t.as_str(), None))
            .collect(),
        Src::Inlines(InlineKind::Links) if b.on == On::Document => note
            .links
            .iter()
            .map(|l| Piece {
                text: Cow::Borrowed(l),
                span: None,
                value: Some(Json::String(l.clone())),
                verbatim: false,
            })
            .collect(),
        Src::Inlines(InlineKind::Tags) => (note.written.tags_in(b.range()).iter())
            .map(|tag| Piece::text(&note.src[tag.clone()], Some(note.file(tag.clone()))))
            .collect(),
        Src::Inlines(InlineKind::Links) => {
            // Wikilinks, then Markdown links, as in the note's own.
            let links = note.written.links_in(b.range());
            let written = |md: bool| links.iter().filter(move |l| l.1 == md);
            (written(false).chain(written(true)))
                .map(|l| Piece {
                    text: Cow::Borrowed(l.2.as_str()),
                    span: None,
                    value: Some(Json::String(l.2.clone())),
                    verbatim: false,
                })
                .collect()
        }
        Src::Note(NotePart::Date) => match *note.date.get_or_init(|| daily.date_of(note.path)) {
            Some(d) => vec![Piece {
                value: Some(Json::String(d.format("%Y-%m-%d").to_string())),
                ..Piece::text(d.to_string(), None)
            }],
            None => Vec::new(),
        },
        Src::Note(part) => {
            let (folder, file) = note.path.rsplit_once('/').unwrap_or(("", note.path));
            let value = match part {
                NotePart::Name | NotePart::Date => file.strip_suffix(".md").unwrap_or(file),
                NotePart::Path => note.path,
                NotePart::Folder => folder,
            };
            vec![Piece::text(value, None)]
        }
        Src::Checkbox => {
            let Some(symbol) = text
                .strip_prefix('[')
                .and_then(|t| t.chars().next())
                .filter(|&c| c != '\n' && text[1 + c.len_utf8()..].starts_with(']'))
            else {
                return Vec::new();
            };
            let end = 1 + symbol.len_utf8();
            used.push(0..end + 1);
            vec![Piece {
                verbatim: true,
                ..Piece::text(symbol.to_string(), Some(at(1..end)))
            }]
        }
        Src::Box(with) => {
            let found = (b.on == On::Item).then(|| boxed(text, with.as_deref()));
            let Some((symbol, end)) = found.flatten() else {
                return Vec::new();
            };
            if let Some(with) = with {
                let held = text.match_indices(with.as_str());
                used.extend(held.map(|(i, _)| i..i + with.len()));
            }
            used.push(0..end + 1);
            vec![Piece {
                verbatim: true,
                ..Piece::text(symbol.to_string(), Some(at(1..end)))
            }]
        }
        Src::Attribute(attr) => match (*attr, b.on) {
            (Attr::Level, On::Section) if b.given.level > 0 => {
                vec![Piece::text(b.given.level.to_string(), None)]
            }
            (Attr::Callout, On::Aside) => match &b.given.callout {
                // Callout kinds ignore case: `[!NOTE]` is `note`.
                Some((kind, _)) => vec![Piece::text(
                    note.src[kind.clone()].to_lowercase(),
                    Some(note.file(kind.clone())),
                )],
                None => Vec::new(),
            },
            (Attr::Fold, On::Aside) => match b.given.callout {
                Some((_, Some(closed))) => {
                    vec![Piece::text(if closed { "closed" } else { "open" }, None)]
                }
                _ => Vec::new(),
            },
            _ => Vec::new(),
        },
        Src::Section(SectionPart::Heading) => b
            .section
            .last()
            .map(|h| Piece::text(h.as_str(), None))
            .into_iter()
            .collect(),
        Src::Section(SectionPart::Path) => b
            .section
            .iter()
            .map(|h| Piece::text(h.as_str(), None))
            .collect(),
        Src::Rest(_) | Src::Later => Vec::new(),
    }
}

/// Where `glyph` is written in `text`. Most blocks have few of the glyphs a type looks for, and
/// its first character, looked for alone, rules the rest out quickly.
fn glyph_at<'t>(text: &'t str, glyph: &'t str) -> impl Iterator<Item = usize> + 't {
    let written = glyph.chars().next().is_some_and(|c| text.contains(c));
    written
        .then(|| text.match_indices(glyph).map(|(i, _)| i))
        .into_iter()
        .flatten()
}

/// The text no source read, with its spaces collapsed.
fn rest_of(text: &str, used: &mut [Range<usize>]) -> String {
    used.sort_by_key(|r| r.start);
    let mut out = String::new();
    let mut at = 0;
    for r in used.iter() {
        if r.start > at {
            push_words(&mut out, &text[at..r.start]);
        }
        at = at.max(r.end);
    }
    push_words(&mut out, &text[at.min(text.len())..]);
    out
}

/// Adds `text`'s words to `out`, one space between each.
pub(crate) fn push_words(out: &mut String, text: &str) {
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
}

/// A written value as its field's type; `None` when it doesn't parse.
pub fn parse(value: &Value, text: &str, daily_format: &str) -> Option<Json> {
    parse_as_written(value, text.trim(), daily_format)
}

/// `parse` without trimming: a checkbox's ` ` is a status.
pub fn parse_as_written(value: &Value, text: &str, daily_format: &str) -> Option<Json> {
    if text.is_empty() {
        return None;
    }
    Some(match value {
        Value::Text => Json::String(text.to_string()),
        Value::Number => number(text.parse::<f64>().ok().filter(|n| n.is_finite())?),
        // ISO, the format of nearly every date in a note, without the moment.js machinery.
        Value::Date { format: None } => {
            let iso = text.len() == 10
                && text.bytes().enumerate().all(|(i, b)| match i {
                    4 | 7 => b == b'-',
                    _ => b.is_ascii_digit(),
                });
            if !iso {
                return None;
            }
            let n = |r: Range<usize>| text[r].parse::<u32>().ok();
            chrono::NaiveDate::from_ymd_opt(n(0..4)? as i32, n(5..7)?, n(8..10)?)?;
            Json::String(text.to_string())
        }
        Value::Date {
            format: Some(format),
        } => {
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
            crate::owner::markdown::line_links(text)
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
                .map()
                .get(text)
                .cloned()
                .or_else(|| values.other().map(String::from))?,
        ),
        Value::Duration => number(duration(text)?),
    })
}

/// A number as items carry it: whole numbers without a fraction.
pub fn number(n: f64) -> Json {
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
    use crate::types::{Layer, load};
    use serde_json::json;
    use std::path::Path;

    fn settings_with(layers: Vec<Layer>) -> Settings {
        let found = load(layers);
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        let nowhere = Path::new("/nonexistent-cold-lava-workspace");
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
        settings_with(vec![
            Layer::built_in(),
            Layer::config(Path::new("config.toml"), table),
        ])
    }

    fn typed(items: &[Item]) -> Vec<&Typed> {
        items
            .iter()
            .filter_map(|i| match &i.data {
                Data::Typed(t) => Some(t.as_ref()),
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
            Data::Typed(t) => Json::Object(
                t.fields
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
            ),
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

    const ESTIMATE: &str = r#"
[estimate]
on = "inline"
match = '(?:^|\s)~(?<v>{duration})\b'
fields.minutes = { capture = "v", type = "duration" }
fields.under = { section = "heading" }
"#;

    const SPRINT: &str = "\
---
note: ~9h in frontmatter
---
# Plan ~1h
- [ ] Dig ~30m and haul ~2h ^dig
- Sweep %%x%% ~15m
  then ~5m on its second line
  - nested ~1m

Survey the rim ~3h, not `~4h`, not~6h.
| Step | Time |
| --- | --- |
| Map | ~45m |

```
~8h
```
> Quoted ~10m
";

    #[test]
    fn inline_types_match_anywhere_in_text_any_number_per_block() {
        let s = settings(ESTIMATE);
        let all = extract("Sprint.md", SPRINT, &s);
        let found: Vec<(usize, Json, Vec<&str>)> = all
            .iter()
            .filter(|i| i.data.kind() == "estimate")
            .map(|i| {
                (
                    i.line,
                    fields(i)["minutes"].clone(),
                    spans(SPRINT, i, "minutes"),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (4, json!(60), vec!["1h"]),
                (5, json!(30), vec!["30m"]),
                (5, json!(120), vec!["2h"]),
                (6, json!(15), vec!["15m"]),
                (7, json!(5), vec!["5m"]),
                (8, json!(1), vec!["1m"]),
                (10, json!(180), vec!["3h"]),
                (13, json!(45), vec!["45m"]),
                (18, json!(10), vec!["10m"]),
            ],
            "not in frontmatter, a code span, a fence, or glued to a word"
        );
        let item = |line: usize| {
            all.iter()
                .find(|i| i.line == line && i.data.kind() != "estimate")
        };
        let parent = |line: usize, nth: usize| {
            all.iter()
                .filter(|i| i.line == line && i.data.kind() == "estimate")
                .nth(nth)
                .unwrap()
                .parent
                .clone()
        };
        assert_eq!(parent(4, 0), item(4).map(|h| h.id.clone()), "a heading");
        let dig = item(5).unwrap();
        assert_eq!(dig.data.kind(), "task");
        assert_eq!(parent(5, 0), Some(dig.id.clone()), "a task");
        assert_eq!(parent(5, 1), Some(dig.id.clone()), "two in one task");
        assert_eq!(
            parent(7, 0),
            item(6).map(|l| l.id.clone()),
            "a list item's lines"
        );
        assert_eq!(parent(8, 0), item(8).map(|l| l.id.clone()), "a nested item");
        assert_eq!(parent(10, 0), None, "a paragraph is no item");
        assert_eq!(
            all.iter()
                .find(|i| i.line == 4 && i.data.kind() == "estimate")
                .map(fields),
            Some(json!({ "minutes": 60, "under": null })),
        );
        let haul = all
            .iter()
            .find(|i| i.data.kind() == "estimate" && fields(i)["minutes"] == 120)
            .unwrap();
        assert_eq!(fields(haul)["under"], "Plan ~1h");
        assert_eq!(haul.section, ["Plan ~1h"]);
        assert_eq!(
            haul.rev,
            crate::tasks::line_id(SPRINT.lines().nth(4).unwrap())
        );
    }

    #[test]
    fn inline_ids_follow_their_block_and_their_text() {
        let s = settings(ESTIMATE);
        let ids = |text: &str| -> Vec<String> {
            extract("Sprint.md", text, &s)
                .into_iter()
                .filter(|i| i.data.kind() == "estimate")
                .map(|i| i.id)
                .collect()
        };
        let before = ids(SPRINT);
        let after = ids(&SPRINT.replace("# Plan ~1h\n", "# Plan ~1h\n\nA new line.\n"));
        assert_eq!(before, after, "lines added above keep every id");
        let ticked = ids(&SPRINT.replace("- [ ] Dig", "- [x] Dig"));
        assert_eq!(before, ticked, "ticking the task keeps its estimates' ids");
        let retimed = ids(&SPRINT.replace("~30m", "~40m"));
        assert_ne!(before[1], retimed[1], "a new value is a new item");
        assert_eq!(before[2], retimed[2], "its neighbour keeps its id");
        let twice = ids("~5m and ~5m\n");
        assert_eq!(twice.len(), 2);
        assert_ne!(twice[0], twice[1], "the nth identical one");
    }

    #[test]
    fn inline_where_gates_by_note_section_and_block() {
        let text =
            "---\ntags: [work]\n---\n# Today\n- [ ] Dig ~30m\n- Rest ~1h\n# Later\nRead ~2h\n";
        let minutes = |toml: &str, path: &str, text: &str| -> Vec<Json> {
            extract(path, text, &settings(&format!("{ESTIMATE}{toml}")))
                .iter()
                .filter(|i| i.data.kind() == "estimate")
                .map(|i| fields(i)["minutes"].clone())
                .collect()
        };
        assert_eq!(
            minutes("", "N.md", text),
            [json!(30), json!(60), json!(120)]
        );
        assert_eq!(minutes("where.tag = [\"work\"]\n", "N.md", text).len(), 3);
        assert!(minutes("where.tag = [\"home\"]\n", "N.md", text).is_empty());
        assert_eq!(
            minutes("where.folder = [\"Work\"]\n", "Work/N.md", text).len(),
            3
        );
        assert!(minutes("where.folder = [\"Work\"]\n", "N.md", text).is_empty());
        assert_eq!(
            minutes("where.section = [\"Later\"]\n", "N.md", text),
            [json!(120)]
        );
        assert_eq!(
            minutes("where.inside = [\"task\"]\n", "N.md", text),
            [json!(30)]
        );
        assert_eq!(
            minutes("where.inside = [\"paragraph\"]\n", "N.md", text),
            [json!(120)]
        );
        assert_eq!(
            minutes("where.inside = [\"list-item\"]\n", "N.md", text),
            [json!(30), json!(60)]
        );
        assert_eq!(
            minutes("where.inside = [\"text\"]\n", "N.md", text),
            [json!(120)],
            "a role reads as its older name"
        );
        assert_eq!(
            minutes("where.inside = [\"item\"]\n", "N.md", text),
            [json!(30), json!(60)]
        );
        assert_eq!(
            minutes(
                "where.inside = [\"item\"]\n[item]\non = \"paragraph\"\n",
                "N.md",
                text
            ),
            [json!(120)],
            "a type called item is what its name means there, not the role"
        );
    }

    /// `heading` in `where.inside` is a pattern in a heading's own words, as it always was. The
    /// body of a section is `where.section`, and `section` is refused there (`types.rs`).
    #[test]
    fn inside_a_heading_is_its_own_words_and_under_it_is_where_section() {
        let text = "# Title ~7m\n\nBody under title ~20m\n\n- item under title ~3m\n";
        let minutes = |toml: &str| -> Vec<Json> {
            extract("N.md", text, &settings(&format!("{ESTIMATE}{toml}")))
                .iter()
                .filter(|i| i.data.kind() == "estimate")
                .map(|i| fields(i)["minutes"].clone())
                .collect()
        };
        assert_eq!(minutes(""), [json!(7), json!(20), json!(3)]);
        assert_eq!(minutes("where.inside = [\"heading\"]\n"), [json!(7)]);
        assert_eq!(
            minutes("where.section = [\"Title ~7m\"]\n"),
            [json!(20), json!(3)]
        );
        let refused = load([
            Layer::built_in(),
            Layer::config(
                Path::new("config.toml"),
                toml::from_str(&format!("{ESTIMATE}where.inside = [\"section\"]\n")).unwrap(),
            ),
        ]);
        assert_eq!(refused.types.len(), 1, "only the task loads");
        assert!(
            refused.errors[0].contains("where.inside names \"section\", which where.inside can't"),
            "{:?}",
            refused.errors
        );
    }

    #[test]
    fn inline_items_sit_in_typed_blocks_and_keep_their_spans_through_comments() {
        let s = settings(&format!(
            "{ESTIMATE}\n[step]\non = \"table-row\"\nwhere.header = [\"Step\"]\n\
             fields.name = {{ column = \"Step\" }}\n"
        ));
        let text = "| Step | Time |\n| --- | --- |\n| Map %%x%% | ~45m |\n";
        let all = extract("N.md", text, &s);
        let step = one(&all, "step");
        let estimate = one(&all, "estimate");
        assert_eq!(estimate.parent.as_ref(), Some(&step.id), "a typed row");
        assert_eq!(spans(text, estimate, "minutes"), ["45m"]);
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
            values: crate::types::EnumValues::Written(
                [
                    ("to read".into(), "todo".into()),
                    ("read".into(), "done".into()),
                ]
                .into(),
            ),
        };
        assert_eq!(p(status.clone(), "to read"), Some(json!("todo")));
        assert_eq!(p(status, "skimmed"), None, "a form the table doesn't have");
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

        let ext = [(
            "x.thought".to_string(),
            toml::from_str("on = \"list-item\"").unwrap(),
        )];
        let table: toml::Table = toml::from_str("[zzz]\non = \"list-item\"\n").unwrap();
        let s = settings_with(vec![
            Layer::extension("x", ext),
            Layer::config(Path::new("config.toml"), table),
        ]);
        let all = extract("Note.md", "- think\n", &s);
        assert_eq!(all[0].data.kind(), "zzz", "config beats an extension");
    }

    #[test]
    fn an_owners_type_matches_its_own_files_below_the_users_types() {
        let row = toml::from_str(
            "on = \"table-row\"\nwhere.file-type = [\"csv\"]\npriority = -1\n\
             fields.name = { pattern = '^\\|\\s*(.*?)\\s*\\|' }\nfields.path = { note = \"path\" }\n",
        )
        .unwrap();
        let mine: toml::Table = toml::from_str(
            "[eruption]\non = \"table-row\"\nwhere.header = [\"Volcano\", \"VEI\"]\n\
             fields.vei = { column = \"VEI\", type = \"number\" }\n",
        )
        .unwrap();
        let s = settings_with(vec![
            Layer::built_in(),
            Layer::extension("csv-table", [("csv-table.row".to_string(), row)]),
            Layer::config(Path::new("config.toml"), mine),
        ]);
        let row = |file: &str, header: &str, cells: &str| {
            let text = format!("| {header} |\n| --- | --- |\n| {cells} |\n");
            extract(&format!("{file}\0{cells}"), &text, &s)
        };
        let erupted = row("Data/Eruptions.csv", "Volcano | VEI", "Krakatoa | 6");
        assert_eq!(erupted[0].data.kind(), "eruption", "the user's type wins");
        let watched = row(
            "Data/Observatories.csv",
            "Observatory | Volcano",
            "HVO | Kīlauea",
        );
        let [hvo] = &watched[..] else {
            panic!("{watched:?}")
        };
        assert_eq!(
            hvo.data.kind(),
            "csv-table.row",
            "no type of the user's matches"
        );
        let json = serde_json::to_value(hvo).unwrap();
        assert_eq!(
            (json["name"].clone(), json["path"].clone()),
            ("HVO".into(), "Data/Observatories.csv".into()),
            "a piece's note is its file"
        );
        let text = "| Observatory | Volcano |\n| --- | --- |\n| HVO | Kīlauea |\n";
        assert!(
            extract("Data/Observatories.md", text, &s).is_empty(),
            "a table in a note is no CSV row"
        );
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
    fn required_fields_settings_checkboxes_and_rest_without() {
        // CommonMark has no task lists, so its checkboxes are list items' text.
        let types = |toml: &str| {
            load([Layer::config(
                Path::new("config.toml"),
                toml::from_str(toml).unwrap(),
            )])
        };
        let nowhere = Path::new("/nonexistent-cold-lava-workspace");
        let s = Settings::new(
            Flavour::built_in("commonmark").unwrap(),
            TasksSettings::load(nowhere),
            daily::Settings::load(nowhere),
            crate::templates::Settings::default(),
            types(
                r#"
            [todo]
            on = "list-item"
            match = '^\[[^\n]\]\s'
            required = ["words"]
            fields.state = { checkbox = true, type = "enum", values = { " " = "open", x = "done" } }
            fields.symbol = { checkbox = true }
            fields.words = { rest = { without = ["@work"] } }
            fields.day = { note = "date", type = "date" }

            [line]
            on = "list-item"
            fields.text = { rest = true }
            "#,
            ),
        );
        let all = extract(
            "2026-11-05.md",
            "- [ ] Call Ana @work\n- [x] @work\n- plain\n",
            &s,
        );
        assert_eq!(
            fields(&all[0]),
            json!({"state": "open", "symbol": " ", "words": "Call Ana", "day": "2026-11-05"}),
            "a space is a status, and the daily note's date is the note's"
        );
        assert_eq!(
            all[1].data.kind(),
            "line",
            "without words it isn't a todo, and the next type takes it"
        );
        assert_eq!(all[2].data.kind(), "line");

        let mut types = types(
            r#"
            [flagged]
            on = "paragraph"
            match = '${tasks.global-filter}\s'
            fields.text = { rest = { without = "${tasks.global-filter}" } }
            "#,
        );
        types.fill(&crate::types::Provided {
            global_filter: "#t(1)".into(),
            ..Default::default()
        });
        let shape = &types.types[0].definition.from[0];
        assert_eq!(shape.pattern.as_deref(), Some(r"\#t\(1\)\s"));
        assert_eq!(
            serde_json::to_value(&shape.fields["text"]).unwrap(),
            json!([{"rest": {"without": ["#t(1)"]}}])
        );
    }

    /// What a typed note's own `links` hold, kept as it was when #206 looked at it: the
    /// frontmatter's wikilinks, then the body's links to notes as written, by wikilink (in
    /// the flavours that have them), by Markdown link or by reference. Not an embed, not an
    /// image, and not a link whose text started on an earlier line.
    #[test]
    fn a_notes_own_links_leave_out_embeds_images_and_a_link_over_two_lines() {
        let s = settings(
            "[page]\non = \"note\"\nwhere.file-type = [\"md\"]\nfields.links = { inlines = \"links\", type = \"link\" }\n",
        );
        let text = "---\nup: \"[[Parent]]\"\n---\nSee [[Wiki]], ![[Embedded]], [md](Md.md \"title\"), [pointy](<Pointy Note.md>), ![img](Image.md), [by ref][r] and [a text that\ngoes on](Wrapped.md).\n\n| a |\n| --- |\n| [[Cell\\|alias]] |\n\n[r]: Reference.md\n";
        let all = extract("Page.md", text, &s);
        assert_eq!(
            fields(one(&all, "page"))["links"],
            json!(["Parent", "Wiki", "Md", "Pointy Note", "Reference", "Cell"])
        );
    }

    #[test]
    fn a_tasks_tags_and_links_are_the_ones_the_tree_reads_on_its_lines() {
        let s = settings_with(vec![Layer::built_in()]);
        let text = "- [ ] **#urgent** call [Ana][a], see [md](Plans/Trip.md) and [[Plan|the plan]], not `#code [[Code]]`\n- [ ] plain #later\n\n[a]: People/Ana.md\n";
        let all = extract("Note.md", text, &s);
        let tasks: Vec<&Item> = all.iter().filter(|i| i.data.kind() == "task").collect();
        let wire = |item: &Item| serde_json::to_value(item).unwrap();
        assert_eq!(wire(tasks[0])["tags"], json!(["urgent"]));
        assert_eq!(
            wire(tasks[0])["links"],
            json!(["Plan", "People/Ana", "Plans/Trip"]),
            "wikilinks, then Markdown links, a link by reference among them"
        );
        assert_eq!(spans(text, tasks[0], "tags"), ["urgent"]);
        assert_eq!(wire(tasks[1])["tags"], json!(["later"]));
        assert_eq!(wire(tasks[1])["links"], json!([]));
    }

    #[test]
    fn a_block_type_gated_by_a_tag_reads_the_tags_on_its_lines() {
        let s = settings(
            "[chore]\non = \"list-item\"\nwhere.tag = [\"chore\"]\nfields.what = { rest = true }\n",
        );
        let all = extract(
            "Note.md",
            "- ==#chore== sweep\n- `#chore` code\n- rest\n",
            &s,
        );
        let kinds: Vec<&str> = all.iter().map(|i| i.data.kind()).collect();
        assert_eq!(kinds, ["chore", "list", "list"]);
    }

    #[test]
    fn no_types_no_work() {
        let s = settings_with(Vec::new());
        assert!(s.matcher.is_empty());
        let all = extract("Note.md", "| a |\n| - |\n| 1 |\n\nWords.\n", &s);
        assert!(all.is_empty());
    }

    #[test]
    fn a_whole_note_meets_a_where_by_folder_property_and_tag() {
        let w = |toml: &str| -> crate::types::Where { toml::from_str(toml).unwrap() };
        let board = "---\nkanban-plugin: basic\n---\n\n## Doing\n\n- [ ] Survey #field\n";
        let tags = ["field".to_string()];
        let kanban = w("property = { kanban-plugin = \"basic\" }");
        assert!(note_meets(&kanban, "Projects/Board.md", board, &tags));
        assert!(!note_meets(&kanban, "Plain.md", "## Doing\n", &[]));
        let other = w("property = { kanban-plugin = \"board\" }");
        assert!(!note_meets(&other, "Projects/Board.md", board, &tags));
        let tagged = w("tag = [\"field\"]\nfolder = [\"Projects\"]");
        assert!(note_meets(&tagged, "Projects/Board.md", board, &tags));
        assert!(!note_meets(&tagged, "Elsewhere/Board.md", board, &tags));
        assert!(!note_meets(&tagged, "Projects/Board.md", board, &[]));
        assert_eq!((kanban.conditions(), tagged.conditions()), (1, 2));
    }

    /// Types of a person's own that read what a piece is, not how Markdown writes it.
    const ATTRIBUTES: &str = r#"
[box]
on = "item"
priority = 2
where.attribute = "checkbox"
fields.state = { attribute = "checkbox" }
fields.words = { rest = true }

[chapter]
on = "section"
where.attribute = "level"
fields.depth = { attribute = "level", type = "number" }
fields.title = { rest = true }

[note-box]
on = "aside"
where.attribute = "callout"
fields.callout = { attribute = "callout" }
fields.fold = { attribute = "fold" }
fields.title = { rest = true }

[said]
on = "aside"
fields.words = { rest = true }
"#;

    const PIECES: &str = "\
# One
## Two ##
- [x] done thing
- [ ]\ttabbed
- [/]
- [x]no space
- plain

> [!WARNING]- Hot lava
> body

> [!note]
> alone

> said first
> more
";

    /// (kind, line, end_line, fields) of each typed item.
    fn read(items: &[Item]) -> Vec<(&str, usize, usize, Json)> {
        (items.iter())
            .filter(|i| matches!(i.data, Data::Typed(_)))
            .map(|i| (i.data.kind(), i.line, i.end_line, fields(i)))
            .collect()
    }

    #[test]
    fn a_type_reads_the_attributes_of_an_item_a_section_and_an_aside() {
        let s = settings(ATTRIBUTES);
        let all = extract("Note.md", PIECES, &s);
        assert_eq!(
            read(&all),
            [
                ("chapter", 1, 1, json!({"depth": 1, "title": "One"})),
                ("chapter", 2, 2, json!({"depth": 2, "title": "Two"})),
                ("box", 3, 3, json!({"state": "x", "words": "done thing"})),
                ("box", 4, 4, json!({"state": " ", "words": "tabbed"})),
                ("box", 5, 5, json!({"state": "/", "words": null})),
                (
                    "note-box",
                    9,
                    10,
                    json!({"callout": "warning", "fold": "closed", "title": "Hot lava"})
                ),
                (
                    "note-box",
                    12,
                    13,
                    json!({"callout": "note", "fold": null, "title": null})
                ),
                ("said", 15, 16, json!({"words": "said first"})),
            ]
        );
        let kinds: Vec<&str> = all.iter().map(|i| i.data.kind()).collect();
        assert_eq!(
            kinds.iter().filter(|k| **k == "list").count(),
            2,
            "a bracket with no space after it is no checkbox, and a plain item has none: {kinds:?}"
        );
        let done = all.iter().find(|i| i.line == 3).unwrap();
        assert_eq!(spans(PIECES, done, "state"), ["x"]);
        let hot = all.iter().find(|i| i.data.kind() == "note-box").unwrap();
        assert_eq!(
            spans(PIECES, hot, "callout"),
            ["WARNING"],
            "the kind where it is written"
        );
    }

    /// An attribute is the owner's reading in the workspace's flavour: one without task lists
    /// has no checkboxes, so nothing carries `checkbox` there, and a heading has its level in
    /// every flavour.
    #[test]
    fn attributes_follow_the_flavour() {
        let table: toml::Table = toml::from_str(ATTRIBUTES).unwrap();
        let kinds = |flavour: &str| {
            let found = load(vec![
                Layer::built_in(),
                Layer::config(Path::new("config.toml"), table.clone()),
            ]);
            let nowhere = Path::new("/nonexistent-cold-lava-workspace");
            let s = Settings::new(
                Flavour::built_in(flavour).unwrap(),
                TasksSettings::load(nowhere),
                daily::Settings::load(nowhere),
                crate::templates::Settings::default(),
                found,
            );
            let all = extract("Note.md", "# One\n- [x] done thing\n", &s);
            all.iter()
                .map(|i| i.data.kind().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds("obsidian"), ["chapter", "box"]);
        assert_eq!(kinds("github"), ["chapter", "box"]);
        assert_eq!(kinds("commonmark"), ["chapter", "list"]);
    }

    /// A type with a shape on a quote beside another shape matched nothing in quotes before
    /// asides were read. Now that it does, the items it already had keep their ids: a typed
    /// aside's key is set apart, so it takes no place among the type's items with the same
    /// words. The second paragraph's id is the one it had before asides matched.
    #[test]
    fn a_typed_aside_moves_no_id_that_was_there_before() {
        let s = settings(
            "[said]\nfields = { words = \"text\" }\n\
             [[said.from]]\non = \"paragraph\"\nmatch = '^foo'\nfields.words = { rest = true }\n\
             [[said.from]]\non = \"quote\"\nmatch = '^foo'\nfields.words = { rest = true }\n",
        );
        let all = extract("R.md", "foo one\n\n> foo one\n\nfoo one\n", &s);
        let said: Vec<(usize, &str)> = (all.iter())
            .filter(|i| i.data.kind() == "said")
            .map(|i| (i.line, i.id.as_str()))
            .collect();
        let lines: Vec<usize> = said.iter().map(|s| s.0).collect();
        assert_eq!(
            lines,
            [1, 3, 5],
            "two paragraphs and the quote between them"
        );
        assert_eq!(said[2].1, "e68456f9ae56127e", "the paragraph on line 5");
        let without = settings(
            "[said]\nfields = { words = \"text\" }\n\
             [[said.from]]\non = \"paragraph\"\nmatch = '^foo'\nfields.words = { rest = true }\n",
        );
        let before = extract("R.md", "foo one\n\n> foo one\n\nfoo one\n", &without);
        let before: Vec<&str> = (before.iter())
            .filter(|i| i.data.kind() == "said")
            .map(|i| i.id.as_str())
            .collect();
        assert_eq!(
            before,
            [said[0].1, said[2].1],
            "the ids the paragraphs have with no shape on a quote"
        );
        assert!(!before.contains(&said[1].1), "the aside's id is its own");
    }

    /// An aside's text is the first words in it: a quote inside it adds no marker to them.
    #[test]
    fn an_aside_s_text_starts_at_its_first_words() {
        let s = settings("[said]\non = \"aside\"\nfields.words = { rest = true }\n");
        let all = extract("Note.md", "> > deep foo\n> shallow\n", &s);
        let words: Vec<Json> = (all.iter())
            .filter(|i| i.data.kind() == "said")
            .map(fields)
            .collect();
        assert_eq!(
            words,
            [json!({"words": "deep foo"}), json!({"words": "deep foo"})],
            "the outer quote and the one inside it"
        );
    }
}
