//! Matching the loaded types against a piece an owner gave (`owner::given::Piece`): the same
//! shapes, conditions, precedence and value parsing as for a note's block, read from what the
//! owner said instead of from Markdown.
//!
//! What differs is where a field's value comes from. In a note, `attribute = "due"` is spelled
//! out into the Markdown owner's readings of the text (`Readings::flat`). A piece carries its
//! attributes, so here the source is read as written: `attribute = "due"` is the value the
//! owner gave under that name. `checkbox = true` and `inline = "due"`, which name how Markdown
//! writes two of them, read the attribute too. Every other source reads the piece's words as
//! plain characters (`pattern`, `marker`, `flag`, `flags`, `rest`), its labels and links
//! (`inlines`), its cells (`column`), its file (`note`) or its sections (`section`).
//!
//! One kind of piece is matched more narrowly: one with no words of its own that stands at the
//! place of a Markdown piece of the same answer, as a board's card does (`Asked::
//! words_elsewhere`). What is written there is the Markdown piece's, and the types already
//! match it there. So a shape matches such a piece only when its `where.attribute` names an
//! attribute the piece carries: `where.attribute = "x"` lists a board's cards, and no shape
//! that reads text, or reads an attribute without testing for it, gains an empty item per
//! card. Every other piece, with words or without, matches as before.
//!
//! Nothing here runs for a note: `Matcher::block` and `read_source` are as they were, and the
//! shapes are compiled for pieces the first time an owner gives one.

use std::borrow::Cow;
use std::ops::Range;

use serde_json::Value as Json;

use super::{
    Block, Fields, Given, Matcher, Note, Place, Spans, Src, Typed, Written, block_passes,
    compile_source, note_passes, read_source, rest_of, slot,
};
use crate::markdown;
use crate::types::{On, Source, Types, Value};

/// A piece, as the matcher is asked about it: its words as plain text, and where its labels,
/// links and cells are in them.
pub struct Asked<'a> {
    pub on: On,
    /// The file it is in, workspace-relative, with its extension.
    pub path: &'a str,
    /// Its text, flattened: no character of it is syntax.
    pub words: &'a str,
    /// Each label's name in `words`, in order.
    pub labels: &'a [Range<usize>],
    /// Each link in `words`, in order: where it starts, and the file it names.
    pub links: &'a [(usize, String)],
    /// A row's cells: their names, and where each one's words are in `words`.
    pub header: &'a [String],
    pub cells: &'a [Range<usize>],
    /// The titles of the sections it is in, outermost first.
    pub section: &'a [String],
    /// What it sits inside: roles under their older names, kinds and types.
    pub inside: &'a [String],
    pub carried: &'a [Carried],
    /// It has no text and no cells, and the answer gives a Markdown piece at its place: its
    /// words are that piece's. Only a shape with `where.attribute` matches it.
    pub words_elsewhere: bool,
}

/// One attribute a piece carries, as text a field's type reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Carried {
    pub name: String,
    pub value: String,
    /// Whether `where.attribute` counts it: a `false` is a value, and not the attribute.
    pub there: bool,
}

/// How each shape of each compiled type reads a piece, by the matcher's own indexes; `None`
/// for a shape whose definition isn't among the types asked with.
pub(super) type Shapes = Vec<Vec<Option<Reads>>>;

pub(super) struct Reads {
    /// Sources by schema field index, in the order written.
    fields: Vec<(usize, Vec<Reading>)>,
}

enum Reading {
    /// The attribute of this name, as the piece's owner gave it.
    Carried(String),
    /// A reading of the piece's words, or of what stands around it.
    Words(Src),
}

fn compile(m: &Matcher, types: &Types) -> Shapes {
    // The matcher's types are the usable ones of `types`, in its order.
    let mut definitions = types.types.iter();
    (m.types.iter())
        .map(|ty| {
            let definition =
                (definitions.by_ref().find(|t| t.name == ty.name)).map(|t| &t.definition);
            let field = |name: &str| ty.schema.iter().position(|f| &*f.0 == name);
            (ty.shapes.iter())
                .map(|shape| {
                    let written = definition?.from.get(shape.index)?;
                    let fields = (written.fields.iter())
                        .filter_map(|(name, sources)| {
                            let sources = sources.0.iter().map(reading).collect();
                            Some((field(name)?, sources))
                        })
                        .collect();
                    Some(Reads { fields })
                })
                .collect()
        })
        .collect()
}

fn reading(source: &Source) -> Reading {
    match source {
        Source::Attribute(name) | Source::Inline(name) => Reading::Carried(name.clone()),
        Source::Checkbox(_) => Reading::Carried("checkbox".to_string()),
        other => Reading::Words(compile_source(other)),
    }
}

impl Matcher {
    /// The type of a piece an owner gave, if one matches. `types` are the ones this matcher
    /// was made from.
    pub fn piece(&self, types: &Types, asked: &Asked) -> Option<Typed> {
        let slot = slot(asked.on)?;
        let shapes = self.pieces.get_or_init(|| compile(self, types));
        let source = markdown::Source::as_written(asked.words);
        let links = asked.links.iter();
        let note = Note {
            path: asked.path,
            src: asked.words,
            source: &source,
            frontmatter: None,
            body: (asked.words, 0),
            tags: Vec::new(),
            links: Vec::new(),
            written: Written {
                tags: asked.labels.to_vec(),
                links: links.map(|(at, to)| (*at, false, to.clone())).collect(),
            },
            date: Default::default(),
        };
        let block = Block {
            on: asked.on,
            text: asked.words,
            at: 0,
            section: asked.section,
            header: asked.header,
            cells: asked.cells,
            inside: asked.inside.iter().map(String::as_str).collect(),
            code: &[],
            given: Given::default(),
        };
        let carries = |name: &String| asked.carried.iter().any(|c| c.there && c.name == *name);
        let mut passing: Vec<(usize, usize, &Reads, Option<regex::Captures>)> = Vec::new();
        for &(t, s) in &self.by_on[slot] {
            let shape = &self.types[t].shapes[s];
            let Some(reads) = shapes.get(t).and_then(|ty| ty.get(s)?.as_ref()) else {
                continue;
            };
            // Its words are a Markdown piece's: only a test for an attribute, which it must
            // carry (below), makes it this shape's.
            if asked.words_elsewhere && shape.w.attribute.is_empty() {
                continue;
            }
            if !note_passes(&shape.w, &note)
                || !block_passes(&shape.w, &note, &block)
                || !shape.w.attribute.iter().all(carries)
            {
                continue;
            }
            let caps = match &shape.pattern {
                Some(re) if shape.captures => match re.captures(block.text) {
                    Some(c) => Some(c),
                    None => continue,
                },
                Some(re) if !re.is_match(block.text) => continue,
                _ => None,
            };
            passing.push((t, s, reads, caps));
        }
        passing.sort_by(|a, b| self.better((b.0, b.1), (a.0, a.1)));
        passing.into_iter().find_map(|(t, s, reads, caps)| {
            let typed = self.read_piece((t, s), reads, caps, &note, &block, asked.carried);
            let has = |&f: &usize| match &typed.fields[f] {
                Json::Null => false,
                Json::Array(a) => !a.is_empty(),
                _ => true,
            };
            let required = &self.types[t].shapes[s].required;
            required.iter().all(has).then_some(typed)
        })
    }

    /// `read`, for a piece: the same order of precedence among a field's sources, and the
    /// words no source read are the `rest`.
    fn read_piece(
        &self,
        (t, s): (usize, usize),
        reads: &Reads,
        caps: Option<regex::Captures>,
        note: &Note,
        b: &Block,
        carried: &[Carried],
    ) -> Typed {
        let ty = &self.types[t];
        let mut typed = Typed {
            kind: ty.name.clone(),
            is: ty.is.clone(),
            fields: Fields::new(ty.names.clone()),
            spans: Spans::default(),
            unparsed: 0,
            place: Place {
                shape: ty.shapes[s].index,
                ..Place::default()
            },
        };
        let mut used: Vec<Range<usize>> = Vec::new();
        let mut rest_field = None;
        for (f, sources) in &reads.fields {
            let mut pieces = Vec::new();
            for (i, source) in sources.iter().enumerate() {
                let before = used.len();
                let found = match source {
                    Reading::Words(Src::Rest(without)) => {
                        rest_field = Some((*f, without));
                        continue;
                    }
                    Reading::Words(src) => {
                        read_source(src, caps.as_ref(), note, b, &self.daily, &mut used)
                    }
                    Reading::Carried(name) => (carried.iter())
                        .find(|c| c.name == *name)
                        .map(|c| super::Piece {
                            text: Cow::Borrowed(c.value.as_str()),
                            span: None,
                            value: None,
                            verbatim: true,
                        })
                        .into_iter()
                        .collect(),
                };
                // A link's target is what the owner gave, as written: reading a value as a
                // link's type does would read `[[x]]` in it as Markdown.
                let mut found = found;
                if matches!(ty.schema[*f].1, Value::Link) {
                    for p in found.iter_mut().filter(|p| p.value.is_none()) {
                        p.value = Some(Json::String(p.text.trim().to_string()));
                    }
                }
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
            for w in without {
                let held = b.text.match_indices(w.as_str());
                used.extend(held.map(|(i, _)| i..i + w.len()));
            }
            let rest = rest_of(b.text, &mut used);
            let pieces = match rest.is_empty() {
                true => Vec::new(),
                false => vec![super::Piece::text(rest, None)],
            };
            self.set(&mut typed, ty, f, pieces);
        }
        // A span is a place in a note's bytes, and a piece's words are in no file.
        typed.spans = Spans::default();
        typed.fields.shrink_to_fit();
        typed.place.sources.shrink_to_fit();
        typed
    }
}
