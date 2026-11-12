//! What an owner in another process gives for one of its files (`file/read`), and how the engine
//! reads that into the workspace's items and links. There are three ways to give an item:
//!
//! - a **piece** (`Piece`): a role, attributes, plain text in runs and where it stands, in the
//!   alphabet's terms (docs/extensions.md, "Pieces, as built"). The types match it as they
//!   match a note's block, reading the attributes the owner gave, and no character of its
//!   text is ever read as Markdown;
//! - a Markdown piece, which goes through the Markdown owner, so the built-in `task` and the
//!   user's types match in it exactly as in a note;
//! - a typed item, checked against its type.
//!
//! Each item's id comes from its file, its kind and the owner's place for it, so it is the same
//! in every process and after a restart.

use crate::alphabet::{Mark, Role};
use crate::gateway;
use crate::items::{Data, Item, Settings};
use crate::matcher::{Asked, Carried};
use crate::owner::markdown;
use crate::tasks::{FNV_START, StatusType, TasksSettings, fnv_from, line_id};
use crate::types::On;
use crate::workspace::{Ref, clean_target};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use std::collections::HashMap;
use std::ops::Range;

/// The answer to `file/read`: the file's items and links.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Given {
    #[serde(default)]
    pub items: Vec<GivenItem>,
    #[serde(default)]
    pub links: Vec<GivenLink>,
}

/// One item, at a place in the file the owner keeps stable (a row's key, a card's id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum GivenItem {
    /// Markdown text, read as a note is: a task in it is a `task`.
    Markdown { place: String, text: String },
    /// An item of a loaded type, its fields by name.
    Typed {
        place: String,
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        fields: Map<String, Json>,
    },
    /// A piece in the alphabet's terms: never Markdown.
    Piece(Piece),
}

/// How deep pieces may nest (`in`), and how much one may hold. An owner's answer is another
/// program's: a piece past a bound is reported and left out.
pub const MAX_DEPTH: usize = 64;
pub const MAX_RUNS: usize = 4096;
pub const MAX_ATTRIBUTES: usize = 256;
/// The bytes a name may have: an attribute's, a text field's, a label's, a cell's.
pub const MAX_NAME: usize = 256;
/// The bytes a place may have, and a link's target.
pub const MAX_PLACE: usize = 1024;
pub const MAX_TARGET: usize = 4096;

/// One unit of what a file holds, in the alphabet's terms (docs/neutral-model.md): what it is
/// (`role`), what it carries beyond its text (`attributes`), its text, and where it stands
/// (`in`, a row's `cells`, and its position among the answer's items).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Piece {
    /// The owner's own name for this spot in the file. The engine hands it back with a change
    /// and never writes it anywhere.
    pub place: String,
    pub role: Role,
    /// Text, a number, `true` or `false` by name; `null` is the same as leaving the name out.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attributes: Map<String, Json>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<Run>,
    /// *Contains*: the place of the piece it is inside, which comes before it in the answer.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub inside: Option<String>,
    /// *Beside*: a row's cells, in order. A row has cells and no text of its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<Cell>,
}

/// One cell of a row: the name of its column, and what it says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub name: String,
    #[serde(default)]
    pub text: Vec<Run>,
}

/// One run of a piece's text. A string is plain characters, whatever they are: `#42`, `$5`,
/// `[[x]]` and `**x**` are those characters and nothing else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Run {
    Plain(String),
    Marked(Marked),
    Link(LinkRun),
    Label(Label),
    Field(FieldRun),
}

/// `{"text": "hot", "marks": ["strong"]}`: characters with marks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marked {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
}

/// `{"link": "Krakatoa", "text": "the 1883 one"}`: a link, its target as `[[target]]` would
/// name it (a file of the workspace, or an address on the web), and the words that show. With
/// no words, the target shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRun {
    pub link: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
}

/// `{"label": "volcano"}`: a label, which Markdown writes as a tag. Its name has no `#`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Label {
    pub label: String,
}

/// `{"field": "rating", "value": "5"}`: a named value written in the text. It is the piece's
/// attribute of that name unless `attributes` has one, and it is not among the piece's words.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldRun {
    pub field: String,
    pub value: String,
}

/// What a `checkbox` attribute says, in the model's words, and the Tasks plugin's status type
/// each one is. A type reads the status symbol the workspace writes for that type (`x` for
/// `done`), so an owner's task has the `status` and `status_type` a note's has.
pub const CHECKBOX: [(&str, StatusType); 4] = [
    ("open", StatusType::Todo),
    ("done", StatusType::Done),
    ("in-progress", StatusType::InProgress),
    ("cancelled", StatusType::Cancelled),
];

/// The status symbol a type reads for the `checkbox` word an owner gave.
fn checkbox_symbol(word: &str, tasks: &TasksSettings) -> Result<char, String> {
    let known = CHECKBOX.iter().find(|(w, _)| *w == word);
    known.map(|(_, t)| tasks.symbol_of(*t)).ok_or_else(|| {
        let words: Vec<&str> = CHECKBOX.iter().map(|(w, _)| *w).collect();
        format!("its checkbox is {word:?}: one of {}", words.join(", "))
    })
}

impl Piece {
    /// Every run it holds: its text's, then each cell's.
    pub fn runs(&self) -> impl Iterator<Item = &Run> {
        let cells = self.cells.iter().flat_map(|c| c.text.iter());
        self.text.iter().chain(cells)
    }

    /// The target of each link in its text, as the owner wrote it.
    pub fn link_targets(&self) -> Vec<&str> {
        let mut targets = Vec::new();
        for run in self.runs() {
            if let Run::Link(l) = run {
                targets.push(l.link.as_str());
            }
        }
        targets
    }

    /// Whether it is within the bounds, and says nothing two ways.
    fn check(&self) -> Result<(), String> {
        if self.place.is_empty() || self.place.len() > MAX_PLACE {
            return Err(format!(
                "its place is empty or longer than {MAX_PLACE} bytes"
            ));
        }
        let long =
            |r: &Run| matches!(r, Run::Link(l) if l.link.is_empty() || l.link.len() > MAX_TARGET);
        if self.runs().any(long) {
            return Err(format!(
                "a link's target is empty or longer than {MAX_TARGET} bytes"
            ));
        }
        if self.runs().count() > MAX_RUNS {
            return Err(format!("its text has more than {MAX_RUNS} runs"));
        }
        if self.attributes.len() > MAX_ATTRIBUTES {
            return Err(format!("it has more than {MAX_ATTRIBUTES} attributes"));
        }
        if !self.cells.is_empty() && (self.role != Role::Row || !self.text.is_empty()) {
            return Err("only a row has cells, and a row's text is its cells'".into());
        }
        let cells = self.cells.iter().map(|c| ("cell", c.name.as_str()));
        let attributes = self.attributes.keys().map(|k| ("attribute", k.as_str()));
        let runs = self.runs().filter_map(|r| match r {
            Run::Label(l) => Some(("label", l.label.as_str())),
            Run::Field(f) => Some(("field", f.field.as_str())),
            _ => None,
        });
        for (what, name) in cells.chain(attributes).chain(runs) {
            if name.is_empty() || name.len() > MAX_NAME {
                return Err(format!(
                    "a {what}'s name is empty or longer than {MAX_NAME} bytes"
                ));
            }
            if what == "label" && (name.starts_with('#') || name.contains(char::is_whitespace)) {
                return Err(format!(
                    "the label {name:?} has a # or a space: a label is its name"
                ));
            }
        }
        Ok(())
    }
}

/// A piece's text as the matcher reads it: its words as plain characters, and where its
/// labels, links and cells are in them. A label shows as `#name` and a link as its words, or
/// its target when it has none; a field is a value, not words.
#[derive(Default)]
struct Flat {
    words: String,
    labels: Vec<Range<usize>>,
    links: Vec<(usize, String)>,
    header: Vec<String>,
    cells: Vec<Range<usize>>,
    fields: Vec<(String, String)>,
}

impl Flat {
    fn of(p: &Piece) -> Flat {
        let mut flat = Flat::default();
        flat.push(&p.text);
        for (i, cell) in p.cells.iter().enumerate() {
            if i > 0 {
                flat.words.push('\t');
            }
            let start = flat.words.len();
            flat.push(&cell.text);
            flat.header.push(cell.name.clone());
            flat.cells.push(start..flat.words.len());
        }
        flat
    }

    fn push(&mut self, runs: &[Run]) {
        for run in runs {
            match run {
                Run::Plain(text) => self.words.push_str(text),
                Run::Marked(m) => self.words.push_str(&m.text),
                Run::Link(l) => {
                    let at = self.words.len();
                    self.words.push_str(match l.text.is_empty() {
                        true => &l.link,
                        false => &l.text,
                    });
                    self.links
                        .extend(clean_target(&l.link).map(|target| (at, target)));
                }
                Run::Label(l) => {
                    self.words.push('#');
                    let at = self.words.len();
                    self.words.push_str(&l.label);
                    self.labels.push(at..self.words.len());
                }
                Run::Field(f) => self.fields.push((f.field.clone(), f.value.clone())),
            }
        }
    }
}

/// The attributes a piece carries, each as text a field's type reads: what `attributes` says,
/// then the fields written in its text under names `attributes` doesn't have.
fn carried(p: &Piece, flat: &Flat, tasks: &TasksSettings) -> Result<Vec<Carried>, String> {
    let mut out: Vec<Carried> = Vec::new();
    for (name, value) in &p.attributes {
        let (value, there) = match value {
            Json::Null => continue,
            Json::Bool(b) => (b.to_string(), *b),
            Json::Number(n) => (n.to_string(), true),
            Json::String(text) => (text.trim().to_string(), true),
            Json::Array(_) | Json::Object(_) => {
                return Err(format!(
                    "its attribute {name} is a list or a table: an attribute is text, a number, \
                     true or false"
                ));
            }
        };
        // A checkbox says one of its words: a blank one is not "no checkbox", it is a mistake.
        if !value.is_empty() || name == "checkbox" {
            let name = name.clone();
            out.push(Carried { name, value, there });
        }
    }
    for (name, value) in &flat.fields {
        let value = value.trim().to_string();
        let says = !value.is_empty() || name == "checkbox";
        if says && !out.iter().any(|c| c.name == *name) {
            let (name, there) = (name.clone(), true);
            out.push(Carried { name, value, there });
        }
    }
    for c in out.iter_mut().filter(|c| c.name == "checkbox") {
        c.value = checkbox_symbol(&c.value, tasks)?.to_string();
    }
    Ok(out)
}

/// A place for a message: its first characters, since an owner may give one of any length.
fn short(place: &str) -> String {
    cut(place, 80)
}

/// The first `most` characters of what an owner gave, for a message. Every problem with a
/// piece goes through this, so no value an owner sends is repeated whole.
fn cut(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// The id of the item a piece is: its file, its kind and the owner's place for it, set apart
/// from a typed item's id at the same place, so neither can take the other's.
pub fn piece_id(file: &str, kind: &str, place: &str) -> String {
    let id = [file, "\0", kind, "\0", place, "\0", "piece"]
        .iter()
        .fold(FNV_START, |h, part| fnv_from(h, part.as_bytes()));
    format!("{id:016x}")
}

/// The id of a typed item an owner gave: its file, its type and the owner's place for it.
pub fn typed_id(file: &str, kind: &str, place: &str) -> String {
    let id = [file, "\0", kind, "\0", place]
        .iter()
        .fold(FNV_START, |h, part| fnv_from(h, part.as_bytes()));
    format!("{id:016x}")
}

/// The pieces of an answer read so far, for the ones that say they are `in` one of them.
#[derive(Default)]
struct Before<'a> {
    by_place: HashMap<&'a str, usize>,
    seen: Vec<Seen>,
}

struct Seen {
    role: Role,
    /// Its words, one space between each: a section's title.
    words: String,
    parent: Option<usize>,
    depth: usize,
    /// The item it is: its id, and its kind with the types that one extends.
    item: Option<(String, Vec<String>)>,
}

/// Read one piece into `out`: the item it is, if it is one, and its links. An item is of the
/// type that matches the piece, as for a note's block; else an `item` is a `list` and a
/// `section` a `heading`, as in a note, and any other piece is no item.
fn read_piece<'a>(
    file: &str,
    p: &'a Piece,
    s: &Settings,
    before: &mut Before<'a>,
    out: &mut Read,
) -> Result<(), String> {
    p.check()?;
    if before.by_place.contains_key(p.place.as_str()) {
        return Err("another piece is at this place".into());
    }
    let parent = match &p.inside {
        Some(place) => match before.by_place.get(place.as_str()) {
            Some(i) => Some(*i),
            None => {
                return Err(format!(
                    "it is in {}, and no piece before it is at that place",
                    short(place)
                ));
            }
        },
        None => None,
    };
    let depth = parent.map_or(0, |i| before.seen[i].depth + 1);
    if depth > MAX_DEPTH {
        return Err(format!("it is more than {MAX_DEPTH} pieces deep"));
    }
    let flat = Flat::of(p);
    let carried = carried(p, &flat, &s.tasks)?;
    // What it is inside, nearest first.
    let mut ancestors: Vec<&Seen> = Vec::new();
    let mut at = parent;
    while let Some(i) = at {
        ancestors.push(&before.seen[i]);
        at = before.seen[i].parent;
    }
    let sections = ancestors.iter().rev().filter(|a| a.role == Role::Section);
    let section: Vec<String> = sections.map(|a| a.words.clone()).collect();
    let mut inside: Vec<String> = Vec::new();
    for a in &ancestors {
        inside.extend(a.role.alias().map(String::from));
        inside.extend(a.item.iter().flat_map(|(_, names)| names.iter().cloned()));
    }
    let mut items = ancestors.iter().filter(|a| a.role == Role::Item);
    let parent_item = items.find_map(|a| a.item.as_ref().map(|(id, _)| id.clone()));
    let mut words = String::new();
    crate::matcher::push_words(&mut words, &flat.words);
    let on = match p.role {
        Role::Section => Some(On::Section),
        Role::Text => Some(On::Text),
        Role::Item => Some(On::Item),
        Role::Row => Some(On::Row),
        Role::Aside => Some(On::Aside),
        Role::Document | Role::Verbatim | Role::Embed | Role::Opaque => None,
    };
    let typed = on.and_then(|on| {
        let asked = Asked {
            on,
            path: file,
            words: &flat.words,
            labels: &flat.labels,
            links: &flat.links,
            header: &flat.header,
            cells: &flat.cells,
            section: &section,
            inside: &inside,
            carried: &carried,
        };
        s.matcher.piece(&s.types, &asked)
    });
    let data = match (typed, p.role) {
        (Some(typed), _) => Some(Data::Typed(Box::new(typed))),
        (None, Role::Item) => Some(Data::List {
            text: words.clone(),
        }),
        (None, Role::Section) => {
            let given = carried.iter().find(|c| c.name == "level");
            let given = given.and_then(|c| c.value.parse::<u8>().ok());
            let level = given.filter(|l| (1..=6).contains(l));
            Some(Data::Heading {
                level: level.unwrap_or((section.len() + 1).min(6) as u8),
                text: words.clone(),
            })
        }
        (None, _) => None,
    };
    let item = data.map(|data| {
        let id = piece_id(file, data.kind(), &p.place);
        let mut names = vec![data.kind().to_string()];
        if let Data::Typed(t) = &data {
            names.extend(t.is.iter().cloned());
        }
        out.places.insert(id.clone(), p.place.clone());
        out.items.push(Item {
            id: id.clone(),
            note: file.to_string(),
            line: 1,
            end_line: 1,
            section: section.clone(),
            parent: parent_item,
            block_id: None,
            rev: line_id(&serde_json::to_string(p).unwrap_or_default()),
            data,
        });
        (id, names)
    });
    for (_, target) in &flat.links {
        let r = Ref {
            target: target.clone(),
            line: 1,
            context: words.chars().take(240).collect(),
        };
        out.links.push((r, p.place.clone()));
    }
    before.by_place.insert(&p.place, before.seen.len());
    before.seen.push(Seen {
        role: p.role,
        words,
        parent,
        depth,
        item,
    });
    Ok(())
}

/// A link the owner says beside its items: *links to*, from one of its places.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum GivenLink {
    /// To a file. A link outside the Markdown pieces (those are read with them): its target
    /// as written, as `[[target]]` would name it.
    File {
        target: String,
        place: String,
        /// What it says around the link, for backlinks.
        #[serde(default)]
        text: String,
    },
    /// From one piece of the file to another, by their places, with what the link says: a
    /// board's arrow and its label. Carried and never interpreted. It joins two pieces of one
    /// file, not two files, so it is no backlink and no edge of the graph.
    Between {
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        label: String,
    },
}

impl GivenLink {
    /// The target of a link to a file, as written.
    pub fn target(&self) -> Option<&str> {
        match self {
            GivenLink::File { target, .. } => Some(target),
            GivenLink::Between { .. } => None,
        }
    }
}

/// One change `file/edit` asks of an owner, at one of the places it gave.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Change {
    /// A Markdown piece's new text, which the engine made as it edits a note.
    Text { place: String, text: String },
    /// A typed item's field, in the form items carry it or as text its type reads; `null`
    /// removes it.
    Field {
        place: String,
        field: String,
        value: Json,
    },
    /// A link the owner gave, among the answer's `links` or as a run of a piece's text: its
    /// target as written, and what it becomes (a rename).
    Link {
        place: String,
        link: String,
        to: String,
    },
    /// An attribute of a piece: text, a number, `true` or `false`; `null` takes it off. A
    /// `checkbox` is one of its words (`CHECKBOX`): ticking a task is `checkbox = done`.
    Attribute {
        place: String,
        attribute: String,
        value: Json,
    },
    /// A piece's new text, as runs, or with `cell` the new text of that cell of a row. A
    /// `text` that is a list is this change; one that is a string is a Markdown piece's.
    Runs {
        place: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cell: Option<String>,
        text: Vec<Run>,
    },
}

/// Whether a value asked for a typed item's field takes the field off: `null`, or nothing but
/// space. One character is a value as written, since a status symbol may be a space.
pub fn clears(value: &Json) -> bool {
    let blank = |text: &str| text.trim().is_empty() && text.chars().count() != 1;
    value.is_null() || value.as_str().is_some_and(blank)
}

/// What the owner should give for the file once `changes` are made to what it gave.
pub fn apply(given: &Given, changes: &[Change]) -> Given {
    let mut out = given.clone();
    for c in changes {
        match c {
            Change::Text { place, text } => {
                for item in &mut out.items {
                    if let GivenItem::Markdown { place: p, text: t } = item
                        && p == place
                    {
                        *t = text.clone();
                    }
                }
            }
            Change::Field {
                place,
                field,
                value,
            } => {
                for item in &mut out.items {
                    if let GivenItem::Typed {
                        place: p, fields, ..
                    } = item
                        && p == place
                    {
                        match clears(value) {
                            true => fields.remove(field),
                            false => fields.insert(field.clone(), value.clone()),
                        };
                    }
                }
            }
            Change::Link { place, link, to } => {
                for l in &mut out.links {
                    if let GivenLink::File {
                        place: p, target, ..
                    } = l
                        && p == place
                        && target == link
                    {
                        *target = to.clone();
                    }
                }
                for p in pieces_at(&mut out, place) {
                    let cells = p.cells.iter_mut().flat_map(|c| c.text.iter_mut());
                    for run in p.text.iter_mut().chain(cells) {
                        if let Run::Link(l) = run
                            && l.link == *link
                        {
                            l.link = to.clone();
                        }
                    }
                }
            }
            Change::Attribute {
                place,
                attribute,
                value,
            } => {
                for p in pieces_at(&mut out, place) {
                    // Wherever the owner wrote it, it now says this once.
                    let named = |r: &Run| matches!(r, Run::Field(f) if f.field == *attribute);
                    p.text.retain(|r| !named(r));
                    match value.is_null() {
                        true => p.attributes.remove(attribute),
                        false => p.attributes.insert(attribute.clone(), value.clone()),
                    };
                }
            }
            Change::Runs { place, cell, text } => {
                for p in pieces_at(&mut out, place) {
                    match cell {
                        None => p.text = text.clone(),
                        Some(name) => {
                            for c in p.cells.iter_mut().filter(|c| c.name == *name) {
                                c.text = text.clone();
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// The pieces an owner gave at `place`.
fn pieces_at<'a>(given: &'a mut Given, place: &'a str) -> impl Iterator<Item = &'a mut Piece> {
    given.items.iter_mut().filter_map(move |item| match item {
        GivenItem::Piece(p) if p.place == place => Some(p),
        _ => None,
    })
}

/// The words of some runs as a field shows them: plain characters, a label as `#name`, a link
/// by its words, no field, and one space between words.
pub fn words(runs: &[Run]) -> String {
    let mut flat = Flat::default();
    flat.push(runs);
    let mut out = String::new();
    crate::matcher::push_words(&mut out, &flat.words);
    out
}

/// A piece's new text when its words become `new`: every link, label and marked run whose
/// own words are still there, in order, stays what it was, the characters between them are
/// plain, and its fields stay. A label or a link typed into `new` is characters: only the
/// owner, or runs, can say one.
pub fn reword(old: &[Run], new: &str) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut at = 0;
    for run in old {
        let shown = match run {
            Run::Plain(_) => continue,
            Run::Field(_) => {
                out.push(run.clone());
                continue;
            }
            Run::Marked(m) => m.text.clone(),
            Run::Link(l) if l.text.is_empty() => l.link.clone(),
            Run::Link(l) => l.text.clone(),
            Run::Label(l) => format!("#{}", l.label),
        };
        // A label ends where its name does: `#home` is not the start of `#homework`.
        let whole = |end: usize| {
            let next = new[end..].chars().next();
            !matches!(run, Run::Label(_)) || next.is_none_or(char::is_whitespace)
        };
        let found = (new[at..].match_indices(shown.as_str()))
            .map(|(i, _)| at + i)
            .find(|i| whole(i + shown.len()));
        if let Some(i) = found.filter(|_| !shown.is_empty()) {
            if i > at {
                out.push(Run::Plain(new[at..i].to_string()));
            }
            out.push(run.clone());
            at = i + shown.len();
        }
    }
    if at < new.len() {
        out.push(Run::Plain(new[at..].to_string()));
    }
    out
}

/// What a piece says, for comparing two answers: its role, its attributes wherever they are
/// written (`attributes` or a field of its text) as text, and its text and cells with
/// neighbouring plain runs joined. Not its place, which can be part of what changed: `read`
/// puts where it stands beside this (its position in the answer, and the position of the
/// piece it is in).
fn piece_key(p: &Piece) -> String {
    let mut attributes: std::collections::BTreeMap<&str, String> = Default::default();
    for (name, value) in &p.attributes {
        let text = match value {
            Json::Null => continue,
            Json::String(text) => text.trim().to_string(),
            Json::Number(n) => match n.as_f64() {
                Some(n) => crate::matcher::number(n).to_string(),
                None => n.to_string(),
            },
            other => other.to_string(),
        };
        if !text.is_empty() {
            attributes.insert(name.as_str(), text);
        }
    }
    let mut fields: Vec<(&str, &str)> = Vec::new();
    let text = normal(&p.text, &mut fields);
    let mut cells: Vec<(&str, Vec<Run>)> = Vec::new();
    for c in &p.cells {
        cells.push((c.name.as_str(), normal(&c.text, &mut fields)));
    }
    for (name, value) in fields {
        if !value.is_empty() && !attributes.contains_key(name) {
            attributes.insert(name, value.to_string());
        }
    }
    serde_json::to_string(&(p.role, attributes, text, cells)).unwrap_or_default()
}

/// Runs as two answers must agree on them: neighbouring plain runs joined, a marked run with
/// no mark plain, and the fields set aside into `fields`, since they are attributes.
fn normal<'a>(runs: &'a [Run], fields: &mut Vec<(&'a str, &'a str)>) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for run in runs {
        let text = match run {
            Run::Field(f) => {
                fields.push((f.field.as_str(), f.value.trim()));
                continue;
            }
            Run::Plain(text) => text,
            Run::Marked(m) if m.marks.is_empty() => &m.text,
            other => {
                out.push(other.clone());
                continue;
            }
        };
        match out.last_mut() {
            _ if text.is_empty() => {}
            Some(Run::Plain(last)) => last.push_str(text),
            _ => out.push(Run::Plain(text.clone())),
        }
    }
    out
}

/// Whether what an owner gave reads as what was asked of it: the same items (their kinds and
/// fields, wherever they are and whatever their ids), the same link targets, and the same
/// pieces in the same order, each saying what was asked and inside the same piece. So a piece
/// the change wasn't for says what it said and stands where it stood, and a change that
/// lands on another piece that says the same is caught by where it is. Places may differ,
/// since an owner's place can be part of what changed (a CSV row's first cell). Else what
/// differs, for a message.
pub fn same(asked: &ReadBack, gave: &ReadBack) -> Result<(), String> {
    let (want_pieces, got_pieces) = (&asked.pieces, &gave.pieces);
    let (asked, gave) = (&asked.read, &gave.read);
    let items = |r: &Read| -> Vec<String> {
        let mut v: Vec<String> = r.items.iter().map(data_key).collect();
        v.sort();
        v
    };
    let links = |r: &Read| -> Vec<String> {
        let mut v: Vec<String> = r.links.iter().map(|(l, _)| l.target.clone()).collect();
        v.sort();
        v
    };
    let only = |a: &[String], b: &[String]| -> Vec<String> {
        let mut b = b.to_vec();
        let mut out = Vec::new();
        for x in a {
            match b.iter().position(|y| y == x) {
                Some(i) => drop(b.remove(i)),
                None => out.push(x.clone()),
            }
        }
        out
    };
    let short = |v: Vec<String>| -> String {
        let s = v.join(", ");
        match s.chars().count() > 200 {
            true => format!("{}…", s.chars().take(200).collect::<String>()),
            false => s,
        }
    };
    let (want, got) = (items(asked), items(gave));
    if want != got {
        let (missing, extra) = (only(&want, &got), only(&got, &want));
        return Err(match (missing.is_empty(), extra.is_empty()) {
            (false, false) => format!("{} where {} was asked", short(extra), short(missing)),
            (false, true) => format!("{} is missing", short(missing)),
            _ => format!("{} wasn't asked for", short(extra)),
        });
    }
    let (want, got) = (links(asked), links(gave));
    if want != got {
        let (missing, extra) = (only(&want, &got), only(&got, &want));
        return Err(format!(
            "links to {} where links to {} were asked",
            short(extra),
            short(missing)
        ));
    }
    // No change makes, moves or relabels a link between two pieces: they come back as they
    // were, in order, each between the pieces at the same positions.
    if asked.between != gave.between {
        return Err(format!(
            "the links between pieces are {} where {} were asked",
            short(gave.between.clone()),
            short(asked.between.clone())
        ));
    }
    // Pieces are compared in the order given: *follows* is a relation, and no change to an
    // attribute, a text or a link moves a piece.
    let (want, got) = (want_pieces, got_pieces);
    let says = |pieces: &[(String, String)]| -> Vec<String> {
        pieces.iter().map(|(_, says)| says.clone()).collect()
    };
    if says(want) != says(got) {
        let differs = want.iter().zip(got).position(|(a, b)| a.1 != b.1);
        return Err(match differs {
            Some(i) => format!(
                "the piece {} of {} is {} where {} was asked",
                i + 1,
                got.len(),
                short(vec![got[i].1.clone()]),
                short(vec![want[i].1.clone()])
            ),
            None => format!(
                "the pieces are {} where {} were asked",
                got.len(),
                want.len()
            ),
        });
    }
    // A place that is in both answers holds the piece that was asked for there. Two pieces
    // that say the same can't be told apart by order alone: this is what catches a change
    // made to the twin, with the two then named as each other. An owner that renamed the
    // place leaves nothing to check it against.
    for (place, asked) in want {
        let there = got.iter().find(|(p, _)| p == place);
        if let Some((_, gave)) = there.filter(|(_, gave)| gave != asked) {
            return Err(format!(
                "the piece at {} is {} where {} was asked",
                cut(place, 80),
                short(vec![gave.clone()]),
                short(vec![asked.clone()])
            ));
        }
    }
    Ok(())
}

/// Whether two items say the same: their kinds and fields, not where they are.
pub fn same_data(a: &Item, b: &Item) -> bool {
    data_key(a) == data_key(b)
}

/// An item's kind and fields as JSON, without where they were read.
fn data_key(item: &Item) -> String {
    serde_json::to_string(&item.data).unwrap_or_default()
}

/// A file's items and links, read from what its owner gave, with the owner's place for each.
#[derive(Default)]
pub struct Read {
    pub items: Vec<Item>,
    /// By item id.
    pub places: HashMap<String, String>,
    pub links: Vec<(Ref, String)>,
    /// Every link between two pieces, in the order given: which pieces it joins, by their
    /// positions among the answer's pieces, and its label. The read-back check compares them.
    pub between: Vec<String>,
    /// What didn't read, one line each: a type that isn't loaded, a field it doesn't have.
    pub problems: Vec<String>,
}

/// An answer read for the read-back check (`same`): its items and links, and what each of its
/// pieces says. Reading a file for the workspace (`read`) makes none of the second, since only
/// an edit compares pieces.
pub struct ReadBack {
    pub read: Read,
    /// Every piece in the order given, whether or not it is an item: its place, and what it
    /// says (`piece_key`) with the position of the piece it is in.
    pieces: Vec<(String, String)>,
}

/// Read an answer to compare it with another: what the owner should give, or what it gave.
pub fn read_back(file: &str, given: &Given, s: &Settings) -> ReadBack {
    let mut position: HashMap<&str, usize> = HashMap::new();
    let mut pieces: Vec<(String, String)> = Vec::new();
    for item in &given.items {
        let GivenItem::Piece(p) = item else {
            continue;
        };
        let inside = match p.inside.as_deref().map(|place| position.get(place)) {
            None => "in nothing".to_string(),
            Some(Some(i)) => format!("in piece {}", i + 1),
            Some(None) => "in a place no piece is at".to_string(),
        };
        position.entry(&p.place).or_insert(pieces.len());
        let says = format!("{inside}: {}", piece_key(p));
        pieces.push((p.place.clone(), says));
    }
    let read = read(file, given, s);
    ReadBack { read, pieces }
}

/// Read what the owner gave for the file at workspace-relative `file`.
pub fn read(file: &str, given: &Given, s: &Settings) -> Read {
    let mut out = Read::default();
    let mut before = Before::default();
    // Where each piece stands among the answer's pieces, by its place.
    let mut position: HashMap<&str, usize> = HashMap::new();
    let mut pieces = 0;
    for item in &given.items {
        match item {
            GivenItem::Piece(p) => {
                position.entry(&p.place).or_insert(pieces);
                pieces += 1;
                if let Err(why) = read_piece(file, p, s, &mut before, &mut out) {
                    // One short line, whatever the owner sent: a reason may quote a value.
                    out.problems
                        .push(format!("{}: {}", short(&p.place), cut(&why, 200)));
                }
            }
            GivenItem::Markdown { place, text } => {
                let key = format!("{file}\0{place}");
                let lines: Vec<&str> = text.lines().collect();
                for mut item in markdown::items(&key, text, s) {
                    let line = lines.get(item.line - 1).copied().unwrap_or_default();
                    item = gateway::answer_line(s, line, &item).into_owned();
                    item.note = file.to_string();
                    out.places.insert(item.id.clone(), place.clone());
                    out.items.push(item);
                }
                let refs = markdown::read(text).refs.into_iter();
                out.links.extend(refs.map(|r| (r, place.clone())));
            }
            GivenItem::Typed {
                place,
                kind,
                fields,
            } => match s.matcher.given(kind, fields) {
                Ok(typed) => {
                    let id = typed_id(file, kind, place);
                    out.places.insert(id.clone(), place.clone());
                    out.items.push(Item {
                        id,
                        note: file.to_string(),
                        line: 1,
                        end_line: 1,
                        section: Vec::new(),
                        parent: None,
                        block_id: None,
                        rev: line_id(&Json::Object(fields.clone()).to_string()),
                        data: Data::Typed(Box::new(typed)),
                    });
                }
                Err(e) => out.problems.push(format!("{place}: {e}")),
            },
        }
    }
    for l in &given.links {
        match l {
            GivenLink::File {
                target,
                place,
                text,
            } => {
                let target = crate::workspace::clean_target(target);
                let r = target.map(|target| Ref {
                    target,
                    line: 1,
                    context: text.trim().chars().take(240).collect(),
                });
                out.links.extend(r.map(|r| (r, place.clone())));
            }
            GivenLink::Between { from, to, label } => {
                // Its ends are pieces of this answer that read: a link to a place no piece is
                // at joins nothing, and is reported and left out.
                let at = |place: &String| position.get(place.as_str()).copied();
                let read = |place: &String| before.by_place.contains_key(place.as_str());
                match (at(from), at(to)) {
                    (Some(a), Some(b)) if read(from) && read(to) => {
                        let label = Json::String(label.clone());
                        let says = format!("piece {} to piece {}: {label}", a + 1, b + 1);
                        out.between.push(says);
                    }
                    _ => {
                        let missing = if read(from) { to } else { from };
                        out.problems.push(format!(
                            "a link from {} to {}: no piece is at {}",
                            short(from),
                            short(to),
                            short(missing)
                        ));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_an_owner_gives_is_json() {
        let wire = json!({
            "items": [
                {"place": "2", "text": "- [ ] Read the report 📅 2026-11-20"},
                {"place": "3", "type": "eruption", "fields": {"vei": 6}},
            ],
            "links": [{"target": "Krakatoa", "place": "3", "text": "[[Krakatoa]],1883,6"}],
        });
        let given: Given = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&given).unwrap(), wire);
        assert!(matches!(&given.items[1], GivenItem::Typed { kind, .. } if kind == "eruption"));
        let wrong = json!({"items": [{"place": "1", "text": "x", "colour": "red"}]});
        assert!(serde_json::from_value::<Given>(wrong).is_err());
    }

    /// The workspace's settings with the built-in types and config.toml's `[types]` as `toml`.
    fn settings(toml: &str) -> Settings {
        use crate::types::{Layer, load};
        let table: toml::Table = toml::from_str(toml).unwrap();
        let config = Layer::config(std::path::Path::new("config.toml"), table);
        let found = load(vec![Layer::built_in(), config]);
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        let nowhere = std::path::Path::new("/nonexistent-cold-lava-workspace");
        Settings::new(
            crate::markdown::Flavour::built_in("obsidian").unwrap(),
            TasksSettings::load(nowhere),
            crate::daily::Settings::load(nowhere),
            crate::templates::Settings::default(),
            found,
        )
    }

    fn given(wire: Json) -> Given {
        serde_json::from_value(wire).unwrap()
    }

    fn fields(item: &Item) -> Json {
        let Data::Typed(t) = &item.data else {
            panic!("{item:?} is not typed");
        };
        let fields = t.fields.iter();
        Json::Object(fields.map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    /// The pinned JSON of a piece: its role, attributes, text in runs, and the relations a
    /// piece says itself (`in`, a row's `cells`). What isn't said is left out.
    #[test]
    fn a_piece_is_json() {
        let wire = json!({
            "items": [
                {"place": "chores", "role": "section", "attributes": {"level": 1}, "text": ["Chores"]},
                {
                    "place": "t1",
                    "role": "item",
                    "attributes": {"task": true, "checkbox": "open", "due": "2026-11-20"},
                    "text": [
                        "Call the plumber about ",
                        {"text": "the leak", "marks": ["strong", "emphasis"]},
                        " ",
                        {"link": "Krakatoa", "text": "see the note"},
                        {"link": "https://example.com"},
                        {"label": "home"},
                        {"field": "cost", "value": "$5"},
                    ],
                    "in": "chores",
                },
                {
                    "place": "r1",
                    "role": "row",
                    "cells": [
                        {"name": "Volcano", "text": ["Krakatoa"]},
                        {"name": "VEI", "text": ["6"]},
                    ],
                },
                {"place": "card", "role": "text", "attributes": {"x": 40, "y": 120.5}},
            ],
            "links": [],
        });
        let read: Given = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&read).unwrap(), wire);
        let GivenItem::Piece(task) = &read.items[1] else {
            panic!("{:?}", read.items[1]);
        };
        assert_eq!(task.role, Role::Item);
        assert_eq!(task.inside.as_deref(), Some("chores"));
        assert!(matches!(&task.text[0], Run::Plain(t) if t == "Call the plumber about "));
        assert!(
            matches!(&task.text[1], Run::Marked(m) if m.marks == [Mark::Strong, Mark::Emphasis])
        );
        assert!(matches!(&task.text[3], Run::Link(l) if l.link == "Krakatoa"));
        assert!(matches!(&task.text[5], Run::Label(l) if l.label == "home"));
        assert!(matches!(&task.text[6], Run::Field(f) if f.field == "cost" && f.value == "$5"));
        assert_eq!(task.link_targets(), ["Krakatoa", "https://example.com"]);

        // The older answers still read as themselves: a text that is a string is Markdown.
        let older = given(json!({"items": [
            {"place": "1", "text": "- [ ] one"},
            {"place": "2", "type": "task", "fields": {"text": "two"}},
        ]}));
        assert!(matches!(&older.items[0], GivenItem::Markdown { .. }));
        assert!(matches!(&older.items[1], GivenItem::Typed { .. }));

        // The alphabet is closed, and a piece says nothing the engine doesn't know.
        for wrong in [
            json!({"place": "1", "role": "slide"}),
            json!({"place": "1", "role": "item", "colour": "red"}),
            json!({"place": "1", "role": "item", "text": "a string is Markdown's"}),
            json!({"place": "1", "role": "item", "text": [{"text": "x", "marks": ["underline"]}]}),
            json!({"place": "1", "role": "item", "text": [{"label": "x", "colour": "red"}]}),
            json!({"place": "1", "role": "row", "cells": [{"text": ["no name"]}]}),
        ] {
            let answer = json!({"items": [wrong.clone()]});
            assert!(serde_json::from_value::<Given>(answer).is_err(), "{wrong}");
        }
    }

    /// The test of step 3: an owner of a format that isn't Markdown gives its tasks as pieces,
    /// and they are `task`s, read from the attributes it gave. Nothing in their text is read
    /// as Markdown: `#42`, `$5`, `[[x]]`, `**x**`, a checkbox and a due date written there are
    /// characters.
    #[test]
    fn an_owners_pieces_are_tasks_with_no_markdown_anywhere() {
        let s = settings("");
        let literal = "Pay #42 for $5 to [[x]] and **x**, not - [ ] this 📅 2026-01-01 %%or%% that";
        let answer = given(json!({"items": [
            {"place": "chores", "role": "section", "text": ["Chores ", {"label": "house"}]},
            {
                "place": "t1",
                "role": "item",
                "in": "chores",
                "attributes": {"task": true, "checkbox": "open", "due": "2026-11-20", "priority": "high"},
                "text": [
                    "Call the plumber about ",
                    {"text": "the leak", "marks": ["strong"]},
                    ", ",
                    {"link": "Krakatoa", "text": "as noted"},
                    " ",
                    {"label": "home"},
                    {"field": "scheduled", "value": "2026-11-18"},
                ],
            },
            {
                "place": "t2",
                "role": "item",
                "in": "chores",
                "attributes": {"task": true, "checkbox": "done"},
                "text": [literal],
            },
            {"place": "n1", "role": "item", "in": "t1", "text": ["bring a torch"]},
            {"place": "box", "role": "item", "attributes": {"checkbox": "open"}, "text": ["a box, no task"]},
            {"place": "p", "role": "text", "text": ["Prose with [[a link]] in it"]},
            {"place": "o", "role": "opaque"},
        ]}));
        let read = read("Home.chores", &answer, &s);
        assert_eq!(read.problems, Vec::<String>::new());
        let kinds: Vec<&str> = read.items.iter().map(|i| i.data.kind()).collect();
        assert_eq!(kinds, ["heading", "task", "task", "list", "list"]);

        let [title, open, done, nested, boxed] = &read.items[..] else {
            panic!("{:?}", read.items);
        };
        assert_eq!(
            title.data,
            Data::Heading {
                level: 1,
                text: "Chores #house".into()
            }
        );
        assert_eq!(
            fields(open),
            json!({
                "status": " ", "status_type": "todo",
                "text": "Call the plumber about the leak, as noted #home",
                "due": "2026-11-20", "scheduled": "2026-11-18", "start": null, "created": null,
                "done": null, "cancelled": null, "recurrence": null, "priority": "high",
                "tags": ["home"], "links": ["Krakatoa"], "daily_note": null,
            })
        );
        assert_eq!(open.section, ["Chores #house"]);
        assert_eq!(
            (open.line, open.end_line, &open.note),
            (1, 1, &"Home.chores".to_string())
        );
        assert_eq!(
            fields(done),
            json!({
                "status": "x", "status_type": "done", "text": literal,
                "due": null, "scheduled": null, "start": null, "created": null,
                "done": null, "cancelled": null, "recurrence": null, "priority": null,
                "tags": [], "links": [], "daily_note": null,
            }),
            "no tag, no link, no mark, no checkbox and no date from plain text"
        );
        assert_eq!(
            nested.data,
            Data::List {
                text: "bring a torch".into()
            }
        );
        assert_eq!(nested.parent.as_deref(), Some(open.id.as_str()));
        assert_eq!(nested.section, ["Chores #house"]);
        assert_eq!(
            boxed.data,
            Data::List {
                text: "a box, no task".into()
            },
            "a checkbox is not a task: its owner says which items are"
        );

        // Only the link the owner said is one: the prose's `[[a link]]` is characters.
        let links: Vec<(&str, &str)> = (read.links.iter())
            .map(|(r, place)| (r.target.as_str(), place.as_str()))
            .collect();
        assert_eq!(links, [("Krakatoa", "t1")]);
        assert_eq!(read.places[&open.id], "t1");
    }

    /// An id is the file, the kind and the owner's place, as before, and a piece's is set apart
    /// from the older answers' at the same place. Pinned: an id must not move.
    #[test]
    fn ids_are_pinned_for_every_way_to_give_an_item() {
        let s = settings(
            "[eruption]\non = \"row\"\nfields.vei = { column = \"VEI\", type = \"number\" }\n",
        );
        let answer = given(json!({"items": [
            {"place": "p1", "text": "- [ ] Read the report 📅 2026-11-20"},
            {"place": "3", "type": "eruption", "fields": {"vei": 6}},
            {"place": "3", "role": "row", "cells": [{"name": "VEI", "text": ["6"]}]},
        ]}));
        let read = read("B.csv", &answer, &s);
        assert_eq!(read.problems, Vec::<String>::new());
        let ids: Vec<(&str, &str)> = (read.items.iter())
            .map(|i| (i.data.kind(), i.id.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("task", "a31f47d3d345839c"),
                ("eruption", "63d323788612c4e8"),
                ("eruption", piece_id("B.csv", "eruption", "3").as_str()),
            ]
        );
        assert_eq!(piece_id("Home.chores", "task", "t1"), "e5b0fa4e80ce14e0");
        assert_ne!(ids[1].1, ids[2].1, "a piece never takes a typed item's id");
        assert_eq!(fields(&read.items[1]), fields(&read.items[2]));
        // A piece's version is all it says: any change to it is another rev.
        let moved = given(json!({"items": [
            {"place": "3", "role": "row", "attributes": {"x": 1}, "cells": [{"name": "VEI", "text": ["6"]}]},
        ]}));
        let moved = super::read("B.csv", &moved, &s).items.remove(0);
        assert_eq!(moved.id, ids[2].1);
        assert_ne!(moved.rev, read.items[2].rev);
    }

    /// A row's cells are read by their names, and what a cell says is characters.
    #[test]
    fn a_rows_cells_are_read_by_name() {
        let s = settings(
            "[eruption]\non = \"row\"\nwhere.header = [\"VEI\"]\n\
             fields.vei = { column = \"VEI\", type = \"number\" }\n\
             fields.name = { column = \"Volcano\", type = \"text\" }\n\
             fields.tags = { inlines = \"tags\", type = \"tag\" }\n\
             fields.links = { inlines = \"links\", type = \"link\" }\n",
        );
        let answer = given(json!({"items": [
            {"place": "r1", "role": "row", "cells": [
                {"name": "Volcano", "text": [{"link": "Krakatoa"}, " #1 of $5"]},
                {"name": "VEI", "text": ["6"]},
                {"name": "Notes", "text": [{"label": "sunda"}]},
            ]},
            {"place": "r2", "role": "row", "cells": [{"name": "Volcano", "text": ["No VEI column"]}]},
        ]}));
        let read = read("Eruptions.sheet", &answer, &s);
        assert_eq!(read.problems, Vec::<String>::new());
        assert_eq!(
            read.items.len(),
            1,
            "a row no type matches is no item, as in a note"
        );
        assert_eq!(
            fields(&read.items[0]),
            json!({"vei": 6, "name": "Krakatoa #1 of $5", "tags": ["sunda"], "links": ["Krakatoa"]})
        );
    }

    /// A place and a link's target are bounded like the rest, a blank `checkbox` is a mistake
    /// and not "no checkbox", and `task` is carried by any value but `false`, `null` and
    /// nothing.
    #[test]
    fn places_targets_and_a_blank_checkbox_are_checked() {
        let s = settings("");
        let task = |place: &str, attributes: Json| json!({"place": place, "role": "item", "attributes": attributes, "text": [place]});
        let long = "L".repeat(MAX_PLACE + 1);
        let far = "t".repeat(MAX_TARGET + 1);
        let answer = given(json!({"items": [
            task("", json!({})),
            task(&long, json!({})),
            task(&"p".repeat(MAX_PLACE), json!({})),
            {"place": "link", "role": "item", "text": [{"link": ""}]},
            {"place": "far", "role": "item", "text": [{"link": far}]},
            task("blank", json!({"task": true, "checkbox": " "})),
            task("empty", json!({"task": true, "checkbox": ""})),
            {"place": "run", "role": "item", "attributes": {"task": true},
             "text": ["x", {"field": "checkbox", "value": ""}]},
            task("none", json!({"task": true, "checkbox": null})),
            task("zero", json!({"task": 0, "checkbox": "open"})),
            task("yes", json!({"task": "yes", "checkbox": "open"})),
            task("no", json!({"task": false, "checkbox": "open"})),
            task("nothing", json!({"task": "", "checkbox": "open"})),
            task("huge", json!({"task": true, "checkbox": "€".repeat(300_000)})),
            {"place": "deep", "role": "item", "in": "€".repeat(5000), "text": ["x"]},
        ]}));
        let read = read("Home.chores", &answer, &s);
        let places: Vec<&str> = (read.problems.iter())
            .map(|p| p.split(':').next().unwrap())
            .collect();
        let shown = format!("{}…", "L".repeat(80));
        assert_eq!(
            places,
            [
                "",
                shown.as_str(),
                "link",
                "far",
                "blank",
                "empty",
                "run",
                "huge",
                "deep"
            ],
            "{:?}",
            read.problems
        );
        assert!(
            read.problems.iter().all(|p| p.chars().count() < 300),
            "a problem is one short line, whatever value it quotes"
        );
        assert!(read.problems[7].ends_with('…'), "{}", read.problems[7]);
        assert!(read.problems[4].contains("one of open, done, in-progress, cancelled"));
        let kinds: Vec<(&str, &str)> = (read.items.iter())
            .map(|i| (read.places[&i.id].as_str(), i.data.kind()))
            .collect();
        assert_eq!(
            &kinds[1..],
            [
                ("none", "task"),
                ("zero", "task"),
                ("yes", "task"),
                ("no", "list"),
                ("nothing", "list")
            ],
            "task is carried by 0 and by any text, not by false or nothing; with no checkbox it has no status"
        );
        assert_eq!(kinds[0].1, "list");
    }

    /// A link-typed field takes what an owner gave as the target as written: `[[x]]` in an
    /// attribute or a cell is characters, never a Markdown link to read.
    #[test]
    fn a_link_field_takes_an_owners_value_as_written() {
        let s = settings(
            "[chore]\non = \"item\"\nwhere.attribute = \"chore\"\n\
             fields.text = { rest = true }\n\
             fields.ref = { attribute = \"ref\", type = \"link\" }\n",
        );
        let answer = given(json!({"items": [
            {"place": "a", "role": "item", "attributes": {"chore": true, "ref": "[[Secret]]"},
             "text": ["one"]},
            {"place": "b", "role": "item", "text": ["two"],
             "attributes": {"chore": true, "ref": "[x](../../etc/passwd)"}},
            {"place": "c", "role": "item", "attributes": {"chore": true, "ref": "Secret"},
             "text": ["three"]},
        ]}));
        let read = read("Home.chores", &answer, &s);
        assert_eq!(read.problems, Vec::<String>::new());
        let refs: Vec<Json> = read
            .items
            .iter()
            .map(|i| fields(i)["ref"].clone())
            .collect();
        assert_eq!(
            refs,
            [
                json!("[[Secret]]"),
                json!("[x](../../etc/passwd)"),
                json!("Secret")
            ]
        );
        assert!(
            read.links.is_empty(),
            "an attribute is a value, not a link of the file"
        );
    }

    /// An owner's answer is another program's: what is outside the bounds or says something
    /// two ways is reported and left out, and the rest is read.
    #[test]
    fn a_piece_out_of_bounds_is_reported_and_left_out() {
        let s = settings("");
        let mut items = vec![
            json!({"place": "ok", "role": "item", "text": ["kept"]}),
            json!({"place": "ok", "role": "item", "text": ["the same place again"]}),
            json!({"place": "orphan", "role": "item", "in": "later", "text": ["x"]}),
            json!({"place": "later", "role": "item", "text": ["also kept"]}),
            json!({"place": "list", "role": "item", "attributes": {"due": ["2026-11-20"]}}),
            json!({"place": "tick", "role": "item", "attributes": {"checkbox": "x"}}),
            json!({"place": "cells", "role": "item", "cells": [{"name": "a", "text": []}]}),
            json!({"place": "tag", "role": "item", "text": [{"label": "#home"}]}),
        ];
        // A chain one deeper than the bound.
        items.push(json!({"place": "d0", "role": "item", "text": ["d0"]}));
        for depth in 1..=MAX_DEPTH + 1 {
            let (place, parent) = (format!("d{depth}"), format!("d{}", depth - 1));
            items.push(json!({"place": place, "role": "item", "in": parent, "text": ["deep"]}));
        }
        let read = read("Home.chores", &given(json!({"items": items})), &s);
        let places: Vec<&str> = (read.problems.iter())
            .map(|p| p.split(':').next().unwrap())
            .collect();
        let deepest = format!("d{}", MAX_DEPTH + 1);
        assert_eq!(
            places,
            [
                "ok",
                "orphan",
                "list",
                "tick",
                "cells",
                "tag",
                deepest.as_str()
            ],
            "{:?}",
            read.problems
        );
        assert_eq!(read.items.len(), 2 + MAX_DEPTH + 1);
        assert!(read.problems[3].contains("one of open, done, in-progress, cancelled"));
    }

    /// The pinned JSON of every change `file/edit` can ask: the three it could before, and
    /// the two in the model's terms. A `text` that is a list is runs; a string is Markdown.
    /// The pinned JSON of a link between two pieces, beside a link to a file in `links`: the
    /// two places, and a label when it has one. It is carried and compared, and it is no link
    /// of the file: no backlink, no item.
    #[test]
    fn a_link_between_two_pieces_is_json() {
        let wire = json!({
            "items": [
                {"place": "a", "role": "text", "attributes": {"x": 40, "y": 120}},
                {"place": "b", "role": "embed"},
            ],
            "links": [
                {"target": "Volcanoes/Lava.md", "place": "b", "text": "Volcanoes/Lava.md"},
                {"from": "a", "to": "b", "label": "feeds"},
                {"from": "b", "to": "b"},
            ],
        });
        let answer = given(wire.clone());
        assert_eq!(serde_json::to_value(&answer).unwrap(), wire);
        let s = settings("");
        let read = read("Board.canvas", &answer, &s);
        assert!(read.problems.is_empty(), "{:?}", read.problems);
        assert!(read.items.is_empty(), "a text no type matches is no item");
        let targets: Vec<&str> = read.links.iter().map(|(r, _)| r.target.as_str()).collect();
        assert_eq!(targets, ["Volcanoes/Lava"], "only the link to a file");
        assert_eq!(
            read.between,
            ["piece 1 to piece 2: \"feeds\"", "piece 2 to piece 2: \"\""]
        );
        for wrong in [
            json!({"from": "a", "to": "b", "colour": "red"}),
            json!({"from": "a"}),
            json!({"from": "a", "to": "b", "place": "a"}),
        ] {
            let wire = json!({"links": [wrong]});
            assert!(serde_json::from_value::<Given>(wire).is_err());
        }

        // An end no piece is at: reported in one short line and left out, the rest read.
        let long = "z".repeat(5000);
        let dangling = given(json!({
            "items": [{"place": "a", "role": "text"}],
            "links": [{"from": "a", "to": long}, {"from": "nope", "to": "a"}, {"from": "a", "to": "a"}],
        }));
        let read = super::read("Board.canvas", &dangling, &s);
        assert_eq!(read.between, ["piece 1 to piece 1: \"\""]);
        assert_eq!(read.problems.len(), 2, "{:?}", read.problems);
        assert!(read.problems.iter().all(|p| p.chars().count() < 300));
        assert!(read.problems[1].ends_with("no piece is at nope"));

        // The read-back: a link relabelled, turned round, dropped or added is not what was
        // asked, and a rename's change leaves it as it was.
        let asked = read_back("Board.canvas", &answer, &s);
        let differs = |links: Json| {
            let mut wire = wire.clone();
            wire["links"] = links;
            same(&asked, &read_back("Board.canvas", &given(wire), &s)).is_err()
        };
        let file =
            json!({"target": "Volcanoes/Lava.md", "place": "b", "text": "Volcanoes/Lava.md"});
        let arrow = json!({"from": "a", "to": "b", "label": "feeds"});
        let own = json!({"from": "b", "to": "b"});
        assert!(
            !differs(json!([arrow, file, own])),
            "among the file's links"
        );
        assert!(differs(
            json!([file, {"from": "a", "to": "b", "label": "starves"}, own])
        ));
        assert!(differs(
            json!([file, {"from": "b", "to": "a", "label": "feeds"}, own])
        ));
        assert!(differs(json!([file, arrow])));
        assert!(differs(json!([file, own, arrow])));
        let moved = Change::Link {
            place: "b".into(),
            link: "Volcanoes/Lava.md".into(),
            to: "Hot/Lava.md".into(),
        };
        let after = apply(&answer, &[moved]);
        assert_eq!(after.links[0].target(), Some("Hot/Lava.md"));
        assert_eq!(after.links[1..], answer.links[1..]);
    }

    #[test]
    fn a_change_is_json() {
        let wire = json!([
            {"place": "1#task", "text": "- [x] Read the report"},
            {"place": "3", "field": "vei", "value": 7},
            {"place": "3", "link": "Krakatoa", "to": "Krakatau"},
            {"place": "plumber", "attribute": "checkbox", "value": "done"},
            {"place": "plumber", "attribute": "due", "value": null},
            {"place": "plumber", "text": ["Call the electrician ", {"label": "home"}]},
            {"place": "r1", "cell": "VEI", "text": ["7"]},
        ]);
        let changes: Vec<Change> = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&changes).unwrap(), wire);
        assert!(matches!(&changes[0], Change::Text { .. }));
        assert!(matches!(&changes[1], Change::Field { .. }));
        assert!(matches!(&changes[2], Change::Link { .. }));
        assert!(
            matches!(&changes[3], Change::Attribute { attribute, .. } if attribute == "checkbox")
        );
        assert!(matches!(
            &changes[4],
            Change::Attribute {
                value: Json::Null,
                ..
            }
        ));
        assert!(matches!(&changes[5], Change::Runs { cell: None, text, .. } if text.len() == 2));
        assert!(matches!(&changes[6], Change::Runs { cell: Some(c), .. } if c == "VEI"));
        for wrong in [
            json!({"place": "1", "attribute": "due"}),
            json!({"place": "1", "attribute": "due", "value": 1, "field": "due"}),
            json!({"place": "1", "text": ["x"], "colour": "red"}),
        ] {
            assert!(
                serde_json::from_value::<Change>(wrong.clone()).is_err(),
                "{wrong}"
            );
        }
    }

    /// The read-back check compares pieces. What the owner gives for its new text must say
    /// what it gave before with the changes made, piece by piece, however it writes it: an
    /// attribute as a field of the text, plain runs cut elsewhere, another place.
    #[test]
    fn the_read_back_compares_pieces() {
        let s = settings("");
        let other = json!({"place": "t2", "role": "item", "text": ["Another"],
                           "attributes": {"task": true, "checkbox": "open"}});
        let before = given(json!({"items": [
            {"place": "t1", "role": "item", "attributes": {"task": true, "checkbox": "open"},
             "text": ["Call ", {"link": "Krakatoa"}, " ", {"label": "home"},
                      {"field": "due", "value": "2026-11-20"}]},
            other,
            {"place": "o", "role": "opaque", "attributes": {"x": 40}},
        ]}));
        let changes: Vec<Change> = serde_json::from_value(json!([
            {"place": "t1", "attribute": "checkbox", "value": "done"},
            {"place": "t1", "attribute": "due", "value": "2026-11-21"},
            {"place": "t1", "link": "Krakatoa", "to": "Krakatau"},
        ]))
        .unwrap();
        let expect = apply(&before, &changes);
        assert_eq!(
            serde_json::to_value(&expect).unwrap(),
            json!({"items": [
                {"place": "t1", "role": "item",
                 "attributes": {"task": true, "checkbox": "done", "due": "2026-11-21"},
                 "text": ["Call ", {"link": "Krakatau"}, " ", {"label": "home"}]},
                other,
                {"place": "o", "role": "opaque", "attributes": {"x": 40}},
            ], "links": []})
        );
        let asked = read_back("Home.chores", &expect, &s);
        let answer = |t1: Json, t2: Json, o: Json| {
            let gave = given(json!({"items": [t1, t2, o]}));
            same(&asked, &read_back("Home.chores", &gave, &s))
        };
        let t1 = json!({"place": "moved", "role": "item",
            "attributes": {"task": true, "checkbox": "done"},
            "text": ["Ca", "ll ", {"link": "Krakatau"}, {"text": " "}, {"label": "home"},
                     {"field": "due", "value": " 2026-11-21"}]});
        let o = json!({"place": "o", "role": "opaque", "attributes": {"x": "40"}});
        assert_eq!(answer(t1.clone(), other.clone(), o.clone()), Ok(()));

        // The change not made, another task changed with it, and a piece no type reads
        // changed: each is caught.
        let unticked = json!({"place": "t1", "role": "item",
            "attributes": {"task": true, "checkbox": "open", "due": "2026-11-21"},
            "text": ["Call ", {"link": "Krakatau"}, " ", {"label": "home"}]});
        assert!(answer(unticked, other.clone(), o.clone()).is_err());
        let too = json!({"place": "t2", "role": "item", "text": ["Another"],
                         "attributes": {"task": true, "checkbox": "done"}});
        assert!(answer(t1.clone(), too, o.clone()).is_err());
        let moved = json!({"place": "o", "role": "opaque", "attributes": {"x": 41}});
        let why = answer(t1.clone(), other.clone(), moved).unwrap_err();
        assert!(why.starts_with("the piece "), "{why}");
        let marked = json!({"place": "t2", "role": "item",
            "text": [{"text": "Another", "marks": ["strong"]}],
            "attributes": {"task": true, "checkbox": "open"}});
        let why = answer(t1.clone(), marked, o.clone()).unwrap_err();
        assert!(
            why.starts_with("the piece 2 of 3 "),
            "a mark no item shows: {why}"
        );

        // Where a piece stands is compared too: the same pieces in another order, one moved
        // into another, and one that left the piece it was in are each caught.
        let why = answer(t1.clone(), o.clone(), other.clone()).unwrap_err();
        assert!(why.starts_with("the piece 2 of 3 "), "swapped: {why}");
        let mut inside = other.clone();
        inside["in"] = json!("moved");
        let why = answer(t1.clone(), inside, o.clone()).unwrap_err();
        assert!(
            why.contains("in piece 1") && why.contains("in nothing"),
            "{why}"
        );
        let gave = given(json!({"items": [t1, other]}));
        let why = same(&asked, &read_back("Home.chores", &gave, &s)).unwrap_err();
        assert!(why.starts_with("the pieces are 2 where 3 "), "{why}");
    }

    /// A change that lands on another piece that says the same is caught by where it is,
    /// though both answers hold the same pieces: places need not be stable for that.
    #[test]
    fn the_read_back_catches_a_change_made_to_a_twin() {
        let s = settings("");
        let twin = |place: &str, word: &str, inside: Json| {
            json!({"place": place, "role": "item", "in": inside, "text": ["Water plants"],
                   "attributes": {"task": true, "checkbox": word}})
        };
        let section = json!({"place": "sec", "role": "section", "text": ["House"]});
        let before = given(json!({"items": [
            section, twin("a", "open", json!("sec")), twin("b", "open", json!("sec")),
        ]}));
        let tick: Vec<Change> = serde_json::from_value(json!([
            {"place": "a", "attribute": "checkbox", "value": "done"},
        ]))
        .unwrap();
        let asked = read_back("Home.chores", &apply(&before, &tick), &s);
        let gives = |a: Json, b: Json| {
            let gave = given(json!({"items": [section, a, b]}));
            same(&asked, &read_back("Home.chores", &gave, &s))
        };
        let (sec, top) = (json!("sec"), Json::Null);
        assert_eq!(
            gives(
                twin("a", "done", sec.clone()),
                twin("b", "open", sec.clone())
            ),
            Ok(())
        );
        // New places are fine: a place can be part of what changed.
        assert_eq!(
            gives(
                twin("a2", "done", sec.clone()),
                twin("b2", "open", sec.clone())
            ),
            Ok(())
        );
        let wrong = gives(
            twin("a", "open", sec.clone()),
            twin("b", "done", sec.clone()),
        );
        assert!(wrong.unwrap_err().starts_with("the piece 2 of 3 "));
        let out = gives(twin("a", "done", sec.clone()), twin("b", "open", top));
        assert!(out.unwrap_err().starts_with("the piece 3 of 3 "));
        // The twin ticked, and the two named as each other: every piece is where it was
        // asked, but the place that was asked to be done is open.
        let named = gives(
            twin("b", "done", sec.clone()),
            twin("a", "open", sec.clone()),
        );
        assert!(named.unwrap_err().starts_with("the piece at a is "));
    }

    /// New words replace a piece's text whole, so they are refused when another field has its
    /// value from those words: the date after a marker and the number a pattern finds would be
    /// dropped without a word. A piece that reads nothing from its words is reworded, and a
    /// link-typed attribute is set as written.
    #[test]
    fn rewording_is_refused_when_another_field_is_read_from_the_words() {
        use crate::gateway::{Op, Reason};
        use crate::owner::change;
        let s = settings(
            r#"
[chore]
on = "item"
where.attribute = "chore"
fields.text = { rest = true }
fields.due = { marker = "📅", type = "date" }
fields.cost = { pattern = '\$(\d+)', type = "number" }
fields.ref = { attribute = "ref", type = "link" }

[twosrc]
fields = { text = "text", due = "date" }
[[twosrc.from]]
on = "item"
where.attribute = "twosrc"
fields.text = { rest = true }
fields.due = [{ attribute = "due" }, { marker = "📅" }]

[without]
on = "item"
where.attribute = "without"
fields.text = { rest = { without = ["(urgent)", "!!"] } }

[starred]
on = "item"
where.attribute = "starred"
fields.text = { rest = true }
fields.star = { flag = "⭐", type = "bool" }
"#,
        );
        let answer = given(json!({"items": [
            {"place": "c1", "role": "item", "attributes": {"chore": true},
             "text": ["Fix the roof 📅 2026-03-04 for $50 ", {"label": "home"}]},
            {"place": "c2", "role": "item", "attributes": {"chore": true}, "text": ["Third"]},
            {"place": "two", "role": "item", "text": ["Roof 📅 2026-03-04 job"],
             "attributes": {"twosrc": true, "due": "2026-01-01"}},
            {"place": "wo", "role": "item", "attributes": {"without": true},
             "text": ["(urgent) Fix the sink !!"]},
            {"place": "s1", "role": "item", "attributes": {"starred": true},
             "text": ["a ⭐ starred one"]},
            {"place": "s0", "role": "item", "attributes": {"starred": true},
             "text": ["no star  here ", {"label": "home"}, {"field": "due", "value": "x"}]},
        ]}));
        let read = read("Home.chores", &answer, &s);
        assert_eq!(read.problems, Vec::<String>::new());
        let piece = |n: usize| match &answer.items[n] {
            GivenItem::Piece(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            fields(&read.items[0]),
            json!({"text": "Fix the roof for #home", "due": "2026-03-04", "cost": 50, "ref": null})
        );
        let set = |field: &str, value: &str| Op::SetField {
            field: field.into(),
            value: json!(value),
        };
        let words = set("text", "Fix the whole roof");
        let refused = change::piece(piece(0), &read.items[0], &words, &s)
            .err()
            .unwrap();
        assert_eq!(refused.reason, Reason::Refused);
        assert!(
            refused
                .message
                .contains(r#"new words would drop "📅 2026-03-04 $50""#),
            "{refused}"
        );
        // A date a second source read and didn't use, words a `without` took out, and a flag
        // that is there: none shows in the field, so each would be dropped unseen.
        assert_eq!(
            fields(&read.items[2]),
            json!({"text": "Roof job", "due": "2026-01-01"})
        );
        for (n, lost) in [(2, "📅 2026-03-04"), (3, "(urgent) !!"), (4, "⭐")] {
            let refused = change::piece(piece(n), &read.items[n], &words, &s)
                .err()
                .unwrap();
            assert_eq!(refused.reason, Reason::Refused);
            let says = format!("new words would drop {lost:?}");
            assert!(refused.message.contains(&says), "{n}: {refused}");
        }
        // Every word shows: a flag that isn't there, a label and a field take nothing out.
        let made = change::piece(piece(5), &read.items[5], &words, &s)
            .ok()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&made.changes).unwrap(),
            json!([{"place": "s0", "text": [{"field": "due", "value": "x"}, "Fix the whole roof"]}])
        );
        let made = change::piece(piece(1), &read.items[1], &words, &s)
            .ok()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&made.changes).unwrap(),
            json!([{"place": "c2", "text": ["Fix the whole roof"]}])
        );
        let link = set("ref", " [[Other]] ");
        let made = change::piece(piece(1), &read.items[1], &link, &s)
            .ok()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&made.changes).unwrap(),
            json!([{"place": "c2", "attribute": "ref", "value": "[[Other]]"}])
        );
    }

    /// Rewording keeps each link, label and marked run whose words are still there, in order,
    /// and the fields. What is typed is characters.
    #[test]
    fn rewording_keeps_the_runs_whose_words_are_still_there() {
        let old: Vec<Run> = serde_json::from_value(json!([
            "Call the plumber ",
            {"label": "home"},
            " about ",
            {"text": "the leak", "marks": ["strong"]},
            {"field": "due", "value": "2026-11-20"},
            " ",
            {"link": "Krakatoa", "text": "as noted"},
        ]))
        .unwrap();
        let runs = |new: &str| serde_json::to_value(reword(&old, new)).unwrap();
        assert_eq!(
            runs("Call the electrician #home about the leak today, as noted"),
            json!([
                "Call the electrician ",
                {"label": "home"},
                " about ",
                {"text": "the leak", "marks": ["strong"]},
                {"field": "due", "value": "2026-11-20"},
                " today, ",
                {"link": "Krakatoa", "text": "as noted"},
            ])
        );
        assert_eq!(
            runs("Call nobody #homework [[x]]"),
            json!([{"field": "due", "value": "2026-11-20"}, "Call nobody #homework [[x]]"]),
            "`#homework` is not the label `home`, and a typed link is characters"
        );
        assert_eq!(runs(""), json!([{"field": "due", "value": "2026-11-20"}]));
    }
}
