//! Type definitions: `[types.NAME]` tables that give blocks a meaning, declared in TOML by you, a
//! vault or an extension (docs/extensions.md, "Types"). This module reads, checks and lists
//! them; `matcher.rs` matches them against notes.
//!
//! A `Definition` is one type, normalised: its schema (`fields` and their value types) and its
//! shapes (`from`), each saying which primitive it is found on and where each field is read. It
//! reads the TOML people write, the one-shape shorthand included, and serialises to JSON that
//! reads back the same, for extension manifests and `types/list`. `load` takes the definitions
//! from every origin in load order and keeps the ones that check out.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::{Regex, RegexBuilder};
use serde::de::{self, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::daily;
use crate::markdown::Syntax;

/// The only `version` a types file can have today.
pub const VERSION: i64 = 1;

/// Where a vault keeps its own types.
pub const VAULT_FILE: &str = ".cold-lava/types.toml";

/// Kinds the engine reads natively (`items.rs`). They can be extended but not redefined, until
/// they are written as definitions themselves.
pub const NATIVE: &[&str] = &["task", "list", "heading", "block"];

/// Every item has these, so no type can name a field after them.
pub const RESERVED_FIELDS: &[&str] = &[
    "id", "note", "line", "end_line", "section", "parent", "block_id", "rev", "kind", "is",
];

/// `${…}` settings a definition can follow, read from the vault's own settings.
pub const PROVIDERS: &[&str] = &[
    "tasks.statuses",
    "tasks.global-filter",
    "tasks.format",
    "daily.format",
];

/// `{…}` placeholders in a `match` and the patterns they stand for.
pub const PLACEHOLDERS: &[(&str, &str)] = &[
    ("date", r"\d{4}-\d{2}-\d{2}"),
    ("number", r"-?\d+(?:\.\d+)?"),
    ("duration", r"(?:\d+(?:\.\d+)?[dhms])+"),
];

/// Native behaviour a type can name in `actions`.
pub const ACTIONS: &[&str] = &["tasks.toggle"];

/// What one pattern may compile to, so no definition can make matching slow.
const REGEX_LIMIT: usize = 256 * 1024;

/// Definitions by name, as an extension's manifest carries them.
pub type Definitions = IndexMap<String, Definition>;

/// One type: its schema and the shapes it shows up in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Written")]
pub struct Definition {
    /// The schema: each field's value type, in the order written.
    pub fields: IndexMap<String, Value>,
    /// The ways the type shows up in a note; at least one.
    pub from: Vec<Shape>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub priority: i32,
    /// Syntax the vault's flavour must have for the type to match.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// How each field looks, by field name.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub style: IndexMap<String, Look>,
    /// Native behaviour by name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    /// How an inline type is written when one is added: `" ~{minutes}"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write: Option<String>,
}

fn is_zero(n: &i32) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One way a type shows up: a primitive, cheap conditions, an optional pattern, and where each
/// field is read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    pub on: On,
    #[serde(default, rename = "where", skip_serializing_if = "Where::is_empty")]
    pub conditions: Where,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// The fields that tell two items of this shape apart, for their ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key: Vec<String>,
    /// Where each field is read, by field name; the first source with a value wins.
    #[serde(default)]
    pub fields: IndexMap<String, Sources>,
}

/// The primitive a shape is found on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum On {
    /// A whole note, read from its frontmatter, name and tags.
    Note,
    Heading,
    Paragraph,
    ListItem,
    TableRow,
    Code,
    Quote,
    /// A pattern anywhere in a block's text.
    Inline,
}

use On as O;

const TEXT: &[On] = &[O::Heading, O::Paragraph, O::ListItem, O::Quote];
const BLOCKS: &[On] = &[
    O::Heading,
    O::Paragraph,
    O::ListItem,
    O::TableRow,
    O::Code,
    O::Quote,
];
const ALL: &[On] = &[
    O::Note,
    O::Heading,
    O::Paragraph,
    O::ListItem,
    O::TableRow,
    O::Code,
    O::Quote,
    O::Inline,
];

impl On {
    pub fn name(self) -> &'static str {
        match self {
            O::Note => "note",
            O::Heading => "heading",
            O::Paragraph => "paragraph",
            O::ListItem => "list-item",
            O::TableRow => "table-row",
            O::Code => "code",
            O::Quote => "quote",
            O::Inline => "inline",
        }
    }

    fn from_name(name: &str) -> Option<On> {
        ALL.iter().copied().find(|o| o.name() == name)
    }
}

/// Cheap conditions a block meets before any pattern runs. Empty lists don't constrain.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Where {
    /// A table's header has all of these columns (table-row only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub header: Vec<String>,
    /// The note's frontmatter has these values.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub property: IndexMap<String, String>,
    /// The note or block has one of these tags (or a tag nested under one).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tag: Vec<String>,
    /// The note is under one of these folders.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub folder: Vec<String>,
    /// The block is under a heading with one of these names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub section: Vec<String>,
    /// The fence's language is one of these (code only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub language: Vec<String>,
    /// The block sits inside one of these: a type, a native kind or a primitive.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inside: Vec<String>,
}

impl Where {
    pub fn is_empty(&self) -> bool {
        self == &Where::default()
    }
}

/// A field's value type.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Value {
    Text,
    Number,
    /// A date, written in a moment.js format (`YYYY-MM-DD` when none is given).
    Date {
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    Datetime {
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    Bool,
    Link,
    Tag,
    /// Written forms and the names they stand for: `{ "to read" = "todo" }`.
    Enum {
        values: IndexMap<String, String>,
    },
    /// `1h30m`.
    Duration,
}

const VALUE_TYPES: &[&str] = &[
    "text", "number", "date", "datetime", "bool", "link", "tag", "enum", "duration",
];

impl Value {
    pub fn name(&self) -> &'static str {
        match self {
            Value::Text => "text",
            Value::Number => "number",
            Value::Date { .. } => "date",
            Value::Datetime { .. } => "datetime",
            Value::Bool => "bool",
            Value::Link => "link",
            Value::Tag => "tag",
            Value::Enum { .. } => "enum",
            Value::Duration => "duration",
        }
    }

    /// `type`, `values` and `format` as written; the type defaults to text.
    fn from_parts(
        ty: Option<String>,
        mut values: Option<IndexMap<String, String>>,
        mut format: Option<String>,
    ) -> Result<Value, String> {
        let value = match ty.as_deref().unwrap_or("text") {
            "text" => Value::Text,
            "number" => Value::Number,
            "date" => Value::Date {
                format: format.take(),
            },
            "datetime" => Value::Datetime {
                format: format.take(),
            },
            "bool" => Value::Bool,
            "link" => Value::Link,
            "tag" => Value::Tag,
            "enum" => Value::Enum {
                values: values
                    .take()
                    .filter(|v| !v.is_empty())
                    .ok_or("an enum needs values = { written = \"name\", … }")?,
            },
            "duration" => Value::Duration,
            other => {
                return Err(format!(
                    "{other:?} is not a value type ({})",
                    VALUE_TYPES.join(", ")
                ));
            }
        };
        if values.is_some() {
            return Err("values go with type = \"enum\"".into());
        }
        if format.is_some() {
            return Err("format goes with type = \"date\" or \"datetime\"".into());
        }
        Ok(value)
    }
}

/// Where a field's value is read in a shape, and written back when the source can.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// A named group of the shape's `match`.
    Capture(String),
    /// A pattern of the field's own over the block's text; the value is its first group.
    Pattern(String),
    /// A glyph and the value after it: `📅 2026-11-12`.
    Marker(String),
    /// A Dataview field: `due:: x`, `[due:: x]` or `(due:: x)`.
    Inline(String),
    /// Whether a glyph is there: true or false.
    Flag(String),
    /// Which of these glyphs is there, by the name each stands for.
    Flags(IndexMap<String, String>),
    /// A table row's cell under this header.
    Column(String),
    /// A frontmatter property.
    Property(String),
    /// A code fence's info string, or part of it.
    Info(InfoPart),
    /// The tags or links in the text.
    Inlines(InlineKind),
    /// The items of this kind or type under it.
    Children(String),
    /// Something about the note it is in.
    Note(NotePart),
    /// The headings it sits under.
    Section(SectionPart),
    /// The text no other source read.
    Rest(Yes),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InfoPart {
    /// `rust` in ```` ```rust title=x ````.
    Language,
    /// `title=x`, what follows the language.
    Arguments,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InlineKind {
    Tags,
    Links,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotePart {
    Name,
    Path,
    Folder,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SectionPart {
    /// The nearest heading.
    Heading,
    /// Every heading above it, outermost first.
    Path,
}

/// `true`, the only thing `rest = …` can say.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Yes;

impl Serialize for Yes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for Yes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Yes, D::Error> {
        if bool::deserialize(d)? {
            Ok(Yes)
        } else {
            Err(de::Error::custom("rest = false: leave it out instead"))
        }
    }
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Capture(_) => "capture",
            Source::Pattern(_) => "pattern",
            Source::Marker(_) => "marker",
            Source::Inline(_) => "inline",
            Source::Flag(_) => "flag",
            Source::Flags(_) => "flags",
            Source::Column(_) => "column",
            Source::Property(_) => "property",
            Source::Info(_) => "info",
            Source::Inlines(_) => "inlines",
            Source::Children(_) => "children",
            Source::Note(_) => "note",
            Source::Section(_) => "section",
            Source::Rest(_) => "rest",
        }
    }

    /// Whether a value set on the field can be put back through this source: only its span
    /// changes. The others are derived from text the source doesn't own.
    pub fn writable(&self) -> bool {
        !matches!(
            self,
            Source::Inlines(_)
                | Source::Children(_)
                | Source::Note(_)
                | Source::Section(_)
                | Source::Rest(_)
        )
    }

    /// The primitives it can be read on.
    pub fn on(&self) -> &'static [On] {
        match self {
            Source::Capture(_) | Source::Note(_) => ALL,
            Source::Pattern(_) => BLOCKS,
            Source::Marker(_) | Source::Flag(_) | Source::Flags(_) | Source::Rest(_) => TEXT,
            Source::Inline(_) => &[O::Note, O::Heading, O::Paragraph, O::ListItem, O::Quote],
            Source::Column(_) => &[O::TableRow],
            Source::Property(_) => &[O::Note],
            Source::Info(_) => &[O::Code],
            Source::Inlines(_) => &[
                O::Note,
                O::Heading,
                O::Paragraph,
                O::ListItem,
                O::TableRow,
                O::Quote,
            ],
            Source::Children(_) => &[O::Note, O::Heading, O::ListItem],
            Source::Section(_) => &[
                O::Heading,
                O::Paragraph,
                O::ListItem,
                O::TableRow,
                O::Code,
                O::Quote,
                O::Inline,
            ],
        }
    }
}

/// A field's sources in one shape, in the order written. TOML writes them as one table
/// (`{ property = "title", note = "name" }`); JSON as a list, since not every JSON reader keeps
/// an object's order. Both read.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Sources(pub Vec<Source>);

impl<'de> Deserialize<'de> for Sources {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Sources, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Sources;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("sources like { column = \"Title\" }")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Sources, A::Error> {
                let mut out = Vec::new();
                while let Some(s) = seq.next_element()? {
                    out.push(s);
                }
                Ok(Sources(out))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Sources, A::Error> {
                let mut out = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    out.push(source(key, &mut map)?);
                }
                Ok(Sources(out))
            }
        }
        d.deserialize_any(V)
    }
}

/// The source `key = value` names, read off a table that may hold more than one.
fn source<'de, A: MapAccess<'de>>(key: String, map: &mut A) -> Result<Source, A::Error> {
    Source::deserialize(de::value::MapAccessDeserializer::new(OneEntry {
        key: Some(key),
        map,
    }))
}

/// One `key = value` of a larger table, as a table of its own.
struct OneEntry<'a, A> {
    key: Option<String>,
    map: &'a mut A,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for OneEntry<'_, A> {
    type Error = A::Error;
    fn next_key_seed<K: de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        self.key
            .take()
            .map(|k| seed.deserialize(k.into_deserializer()))
            .transpose()
    }
    fn next_value_seed<V: de::DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, A::Error> {
        self.map.next_value_seed(seed)
    }
}

/// A field as written at the top of a type: its value type (`"date"`, or a table with `type`,
/// `values`, `format`) and, in the one-shape shorthand, its sources beside it.
struct FieldEntry {
    value: Value,
    sources: Vec<Source>,
}

impl<'de> Deserialize<'de> for FieldEntry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<FieldEntry, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = FieldEntry;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a value type like \"date\", or a table like { type = \"date\" }")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<FieldEntry, E> {
                let value =
                    Value::from_parts(Some(s.to_string()), None, None).map_err(E::custom)?;
                Ok(FieldEntry {
                    value,
                    sources: Vec::new(),
                })
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<FieldEntry, A::Error> {
                let (mut ty, mut values, mut format) = (None, None, None);
                let mut sources = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "type" => ty = Some(map.next_value()?),
                        "values" => values = Some(map.next_value()?),
                        "format" => format = Some(map.next_value()?),
                        _ => sources.push(source(key, &mut map)?),
                    }
                }
                let value = Value::from_parts(ty, values, format).map_err(de::Error::custom)?;
                Ok(FieldEntry { value, sources })
            }
        }
        d.deserialize_any(V)
    }
}

/// How a field looks, from a closed set every view draws the same way.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Look {
    #[serde(alias = "color", skip_serializing_if = "Option::is_none")]
    pub colour: Option<Colour>,
    #[serde(skip_serializing_if = "is_false")]
    pub badge: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub dim: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub strike: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub hide: bool,
    /// The value is a link to follow.
    #[serde(skip_serializing_if = "is_false")]
    pub link: bool,
    /// The look applies to the whole item, not only the field.
    #[serde(skip_serializing_if = "is_false")]
    pub whole: bool,
    /// Looks for an enum field's values, by name.
    #[serde(skip_serializing_if = "IndexMap::is_empty")]
    pub by_value: IndexMap<String, Look>,
}

/// Colours are names, so a light terminal is one table in the frontend: Obsidian's six canvas
/// colours, the accent and a muted one.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Colour {
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Purple,
    Accent,
    Muted,
}

/// A type as written: either one shape at the top level or `[[from]]` shapes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    #[serde(default)]
    fields: IndexMap<String, FieldEntry>,
    #[serde(default)]
    from: Vec<Shape>,
    on: Option<On>,
    #[serde(rename = "where")]
    conditions: Option<Where>,
    #[serde(rename = "match")]
    pattern: Option<String>,
    key: Option<Vec<String>>,
    extends: Option<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    style: IndexMap<String, Look>,
    #[serde(default)]
    actions: Vec<String>,
    write: Option<String>,
}

impl TryFrom<Written> for Definition {
    type Error = String;

    fn try_from(w: Written) -> Result<Definition, String> {
        let shorthand =
            w.on.is_some() || w.conditions.is_some() || w.pattern.is_some() || w.key.is_some();
        let mut fields = IndexMap::new();
        let mut from = w.from;
        if from.is_empty() {
            let on =
                w.on.ok_or("no shape: give it `on`, or [[from]] shapes each with their `on`")?;
            let mut shape = Shape {
                on,
                conditions: w.conditions.unwrap_or_default(),
                pattern: w.pattern,
                key: w.key.unwrap_or_default(),
                fields: IndexMap::new(),
            };
            for (name, entry) in w.fields {
                if !entry.sources.is_empty() {
                    shape.fields.insert(name.clone(), Sources(entry.sources));
                }
                fields.insert(name, entry.value);
            }
            from.push(shape);
        } else {
            if shorthand {
                return Err(
                    "with [[from]] shapes, `on`, `where`, `match` and `key` go in each shape"
                        .into(),
                );
            }
            for (name, entry) in w.fields {
                if let Some(s) = entry.sources.first() {
                    return Err(format!(
                        "fields.{name}: `{}` says where it's read, which goes in a [[from]] shape's fields",
                        s.kind()
                    ));
                }
                fields.insert(name, entry.value);
            }
        }
        Ok(Definition {
            fields,
            from,
            extends: w.extends,
            priority: w.priority,
            requires: w.requires,
            style: w.style,
            actions: w.actions,
            write: w.write,
        })
    }
}

/// Where a type was defined.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Origin {
    /// An extension's manifest; its types are named `<extension>.<name>`.
    Extension { name: String },
    /// `[types.*]` in config.toml.
    Config { path: PathBuf },
    /// The vault's `.cold-lava/types.toml`.
    Vault { path: PathBuf },
}

impl Origin {
    /// What an error line starts with: the file, or the extension.
    fn place(&self) -> String {
        match self {
            Origin::Extension { name } => format!("extension {name}"),
            Origin::Config { path } | Origin::Vault { path } => path.display().to_string(),
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Origin::Extension { name } => write!(f, "extension {name}"),
            Origin::Config { path } => write!(f, "config {}", path.display()),
            Origin::Vault { path } => write!(f, "vault {}", path.display()),
        }
    }
}

/// A loaded type.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Type {
    pub name: String,
    pub origin: Origin,
    #[serde(flatten)]
    pub definition: Definition,
}

/// Definitions from one origin, read but not yet checked.
#[derive(Clone)]
pub struct Layer {
    origin: Origin,
    definitions: Vec<(String, Result<Definition, String>)>,
    /// Problems with the file as a whole.
    errors: Vec<String>,
}

impl Layer {
    pub fn extension(name: &str, definitions: &Definitions) -> Layer {
        Layer {
            origin: Origin::Extension {
                name: name.to_string(),
            },
            definitions: definitions
                .iter()
                .map(|(n, d)| (format!("{name}.{n}"), Ok(d.clone())))
                .collect(),
            errors: Vec::new(),
        }
    }

    /// config.toml's `[types]` table, which the frontend reads and hands over.
    pub fn config(path: &Path, table: toml::Table) -> Layer {
        Layer::from_table(
            Origin::Config {
                path: path.to_path_buf(),
            },
            table,
        )
    }

    /// The vault's `.cold-lava/types.toml`; `None` when it has none.
    pub fn vault(root: &Path) -> Option<Layer> {
        let path = root.join(VAULT_FILE);
        let text = match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            r => r,
        };
        let origin = Origin::Vault { path };
        let place = origin.place();
        let file_error = |e: String| {
            Some(Layer {
                errors: vec![format!("{place}: {e}")],
                origin: origin.clone(),
                definitions: Vec::new(),
            })
        };
        let mut file: toml::Table = match text.map_err(|e| e.to_string()).and_then(|t| {
            toml::from_str(&t).map_err(|e| e.to_string().replace('\n', " ").trim().to_string())
        }) {
            Ok(f) => f,
            Err(e) => return file_error(format!("can't read it: {e}")),
        };
        match file.remove("version") {
            Some(toml::Value::Integer(VERSION)) => {}
            Some(v) => {
                return file_error(format!(
                    "version = {v} is not one this cold-lava reads (it reads version = {VERSION}); the file is skipped"
                ));
            }
            None => {
                return file_error(format!(
                    "no version: start the file with version = {VERSION}; the file is skipped"
                ));
            }
        }
        let types = match file.remove("types") {
            Some(toml::Value::Table(t)) => t,
            Some(v) => return file_error(format!("types: expected [types.NAME] tables, got {v}")),
            None => toml::Table::new(),
        };
        let mut layer = Layer::from_table(origin, types);
        for key in file.keys() {
            layer.errors.push(format!(
                "{place}: {key:?} is not a setting (only version and [types.NAME])"
            ));
        }
        Some(layer)
    }

    fn from_table(origin: Origin, table: toml::Table) -> Layer {
        let definitions = table
            .into_iter()
            .map(|(name, value)| {
                let parsed = value
                    .try_into::<Definition>()
                    .map_err(|e| e.to_string().replace('\n', " ").trim().to_string());
                (name, parsed)
            })
            .collect();
        Layer {
            origin,
            definitions,
            errors: Vec::new(),
        }
    }
}

/// Every loaded type, and what was wrong with the rest.
#[derive(Debug, Default, Serialize)]
pub struct Types {
    /// Kinds read natively, not by definitions.
    pub native: Vec<String>,
    /// In load order; a type replaced by a later origin takes the later one's place.
    pub types: Vec<Type>,
    /// One line per mistake: where, which type, what. The type is dropped.
    pub errors: Vec<String>,
    /// Things worth knowing that aren't mistakes, such as a type replacing an earlier one.
    pub notes: Vec<String>,
}

static CONFIGURED: OnceLock<Vec<Layer>> = OnceLock::new();

/// The layers every vault gets before its own: extensions' types and config.toml's, which the
/// frontend reads and sets once at startup.
pub fn set_configured(layers: Vec<Layer>) {
    let _ = CONFIGURED.set(layers);
}

/// The types a vault's notes are read with: the configured layers, then the vault's own file.
pub fn for_vault(root: &Path) -> Types {
    let configured = CONFIGURED.get().cloned().unwrap_or_default();
    load(configured.into_iter().chain(Layer::vault(root)))
}

/// Loads layers in order: built in, extensions, config.toml, the vault. A later definition of a
/// name replaces an earlier one, and says so in a note. Broken definitions are dropped.
pub fn load(layers: impl IntoIterator<Item = Layer>) -> Types {
    let mut errors = Vec::new();
    let mut notes = Vec::new();
    let mut kept: IndexMap<String, Type> = IndexMap::new();
    let mut dropped = HashSet::new();
    for layer in layers {
        errors.extend(layer.errors);
        let place = layer.origin.place();
        for (name, parsed) in layer.definitions {
            let problems = match &parsed {
                Err(e) => vec![e.clone()],
                Ok(d) => check(&name, d),
            };
            let definition = match parsed {
                Ok(d) if problems.is_empty() => d,
                _ => {
                    errors.extend(
                        problems
                            .iter()
                            .map(|p| format!("{place}: [types.{name}] {p}")),
                    );
                    dropped.insert(name);
                    continue;
                }
            };
            if let Some(old) = kept.shift_remove(&name) {
                notes.push(format!(
                    "{place}: [types.{name}] replaces the one from {}",
                    old.origin
                ));
            }
            dropped.remove(&name);
            let origin = layer.origin.clone();
            kept.insert(
                name.clone(),
                Type {
                    name,
                    origin,
                    definition,
                },
            );
        }
    }
    // References between types, until no type names one that was dropped.
    loop {
        let broken: Vec<(String, String)> = kept
            .values()
            .filter_map(|t| {
                references(t, &kept, &dropped)
                    .err()
                    .map(|e| (t.name.clone(), e))
            })
            .collect();
        if broken.is_empty() {
            break;
        }
        for (name, e) in broken {
            if let Some(t) = kept.shift_remove(&name) {
                errors.push(format!("{}: [types.{name}] {e}", t.origin.place()));
            }
            dropped.insert(name);
        }
    }
    Types {
        native: NATIVE.iter().map(|n| n.to_string()).collect(),
        types: kept.into_values().collect(),
        errors,
        notes,
    }
}

/// `extends`, `children` and `where.inside` name types that exist, and `extends` doesn't go
/// round in a circle.
fn references(
    t: &Type,
    kept: &IndexMap<String, Type>,
    dropped: &HashSet<String>,
) -> Result<(), String> {
    let missing = |what: &str, name: &str| {
        if dropped.contains(name) {
            format!("{what} {name:?}, which was dropped for its errors")
        } else {
            format!("{what} {name:?}, which is not a type")
        }
    };
    let known = |n: &str| NATIVE.contains(&n) || kept.contains_key(n);
    if let Some(base) = &t.definition.extends {
        if !known(base) {
            return Err(missing("extends", base));
        }
        let mut chain = vec![t.name.as_str()];
        let mut next = Some(base.as_str());
        while let Some(n) = next {
            let circle = chain.contains(&n);
            chain.push(n);
            if circle {
                return Err(format!(
                    "extends goes round in a circle: {}",
                    chain.join(" → ")
                ));
            }
            next = kept.get(n).and_then(|b| b.definition.extends.as_deref());
        }
    }
    for shape in &t.definition.from {
        for inside in &shape.conditions.inside {
            if !known(inside) && On::from_name(inside).is_none() {
                return Err(missing("where.inside names", inside));
            }
        }
        for sources in shape.fields.values() {
            for s in &sources.0 {
                if let Source::Children(kind) = s
                    && !known(kind)
                {
                    return Err(missing("children names", kind));
                }
            }
        }
    }
    Ok(())
}

/// Everything wrong with one definition on its own, one line each.
fn check(name: &str, d: &Definition) -> Vec<String> {
    if NATIVE.contains(&name) {
        return vec![format!(
            "{name} is built in: it can be extended (extends = \"{name}\") but not redefined"
        )];
    }
    let mut p = Vec::new();
    if !is_name(name, true) {
        p.push(format!(
            "{name:?} is not a type name: a letter, then letters, digits, - or _"
        ));
    }
    for (field, value) in &d.fields {
        if RESERVED_FIELDS.contains(&field.as_str()) {
            p.push(format!(
                "field {field:?} is reserved: every item has it ({})",
                RESERVED_FIELDS.join(", ")
            ));
        } else if !is_name(field, false) {
            p.push(format!(
                "{field:?} is not a field name: a letter, then letters, digits, - or _"
            ));
        }
        if let Value::Date { format: Some(f) } | Value::Datetime { format: Some(f) } = value
            && let Err(e) = check_format(f)
        {
            p.push(format!("fields.{field}: {e}"));
        }
    }
    for syntax in &d.requires {
        if Syntax::from_name(syntax).is_none() {
            p.push(format!(
                "requires {syntax:?}, which is not a syntax ({})",
                Syntax::names()
            ));
        }
    }
    for action in &d.actions {
        if !ACTIONS.contains(&action.as_str()) {
            p.push(format!(
                "actions: {action:?} is not a native action ({})",
                ACTIONS.join(", ")
            ));
        }
    }
    if let Some(write) = &d.write {
        if d.from.iter().any(|s| s.on != On::Inline) {
            p.push("write is for inline types (on = \"inline\")".into());
        }
        for field in placeholders(write) {
            if !d.fields.contains_key(field) {
                p.push(format!("write uses {{{field}}}, which is not a field"));
            }
        }
    }
    for (field, look) in &d.style {
        let Some(value) = d.fields.get(field) else {
            p.push(format!("style.{field}: {field:?} is not a field"));
            continue;
        };
        for (name, inner) in &look.by_value {
            if !inner.by_value.is_empty() {
                p.push(format!(
                    "style.{field}.by-value.{name}: by-value can't nest"
                ));
            }
            match value {
                Value::Enum { values } if values.values().any(|v| v == name) => {}
                Value::Enum { values } => {
                    let names: Vec<&str> = values.values().map(String::as_str).collect();
                    p.push(format!(
                        "style.{field}.by-value: {name:?} is not one of its values ({})",
                        names.join(", ")
                    ));
                }
                _ => p.push(format!(
                    "style.{field}.by-value: {field} is {}, not an enum",
                    value.name()
                )),
            }
        }
    }
    let many = d.from.len() > 1;
    for (i, shape) in d.from.iter().enumerate() {
        let at = if many {
            format!("shape {}: ", i + 1)
        } else {
            String::new()
        };
        check_shape(d, shape, &at, &mut p);
    }
    p
}

fn check_shape(d: &Definition, shape: &Shape, at: &str, p: &mut Vec<String>) {
    let on = shape.on;
    let w = &shape.conditions;
    for (set, key, ok) in [
        (!w.header.is_empty(), "header", on == On::TableRow),
        (!w.language.is_empty(), "language", on == On::Code),
        (!w.section.is_empty(), "section", on != On::Note),
        (!w.inside.is_empty(), "inside", on != On::Note),
    ] {
        if set && !ok {
            p.push(format!("{at}where.{key} doesn't apply on {}", on.name()));
        }
    }
    let groups: Option<Vec<String>> = match &shape.pattern {
        Some(m) => match compile(m) {
            Ok(re) => Some(re.capture_names().flatten().map(String::from).collect()),
            Err(e) => {
                p.push(format!("{at}match '{m}': {e}"));
                None
            }
        },
        None => {
            if on == On::Inline {
                p.push(format!("{at}an inline type needs a match"));
            }
            Some(Vec::new())
        }
    };
    for k in &shape.key {
        if !d.fields.contains_key(k) {
            p.push(format!("{at}key {k:?} is not a field"));
        }
    }
    let mut rest = 0;
    for (field, sources) in &shape.fields {
        let at = format!("{at}fields.{field}: ");
        let Some(value) = d.fields.get(field) else {
            p.push(format!("{at}not one of the type's fields"));
            continue;
        };
        for s in &sources.0 {
            if !s.on().contains(&on) {
                let allowed: Vec<&str> = s.on().iter().map(|o| o.name()).collect();
                p.push(format!(
                    "{at}{} can't be read on {} (only on {})",
                    s.kind(),
                    on.name(),
                    allowed.join(", ")
                ));
            }
            match s {
                Source::Capture(g) => match (&shape.pattern, &groups) {
                    (None, _) => p.push(format!("{at}capture {g:?} needs a match")),
                    (Some(_), Some(gs)) if !gs.contains(g) => {
                        p.push(format!("{at}the match has no group named {g:?}"))
                    }
                    _ => {}
                },
                Source::Pattern(re) => match compile(re) {
                    Err(e) => p.push(format!("{at}pattern '{re}': {e}")),
                    Ok(r) if r.captures_len() < 2 => p.push(format!(
                        "{at}pattern '{re}' needs a group (…) around the value"
                    )),
                    Ok(_) => {}
                },
                Source::Marker(t)
                | Source::Inline(t)
                | Source::Flag(t)
                | Source::Column(t)
                | Source::Property(t)
                    if t.trim().is_empty() =>
                {
                    p.push(format!("{at}{} is empty", s.kind()))
                }
                Source::Flags(glyphs) if glyphs.is_empty() => {
                    p.push(format!("{at}flags needs at least one glyph"))
                }
                Source::Rest(_) => rest += 1,
                _ => {}
            }
            if matches!(s, Source::Flag(_)) && *value != Value::Bool {
                p.push(format!(
                    "{at}flag gives true or false, so the field's type is bool, not {}",
                    value.name()
                ));
            }
        }
    }
    if rest > 1 {
        p.push(format!("{at}more than one field reads the rest"));
    }
}

fn is_name(s: &str, dots: bool) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || (dots && c == '.'))
}

/// A `match` or `pattern` with its placeholders and settings filled in, compiled under the size
/// limit. Settings stand for an empty group here: their values come from the vault.
pub(crate) fn compile(pattern: &str) -> Result<Regex, String> {
    let expanded = expand(pattern)?;
    RegexBuilder::new(&expanded)
        .size_limit(REGEX_LIMIT)
        .dfa_size_limit(REGEX_LIMIT)
        .build()
        .map_err(|e| match e {
            regex::Error::CompiledTooBig(_) => format!(
                "too big: a pattern compiles to at most {} KiB",
                REGEX_LIMIT / 1024
            ),
            e => {
                let text = e.to_string();
                text.lines()
                    .rev()
                    .find_map(|l| l.strip_prefix("error: "))
                    .unwrap_or(&text)
                    .to_string()
            }
        })
}

fn expand(pattern: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(c) = rest.chars().next() {
        if c == '\\' {
            // An escape, with `\p{…}` kept whole so its braces aren't read as a placeholder.
            let mut end = 1 + rest[1..].chars().next().map_or(0, char::len_utf8);
            if matches!(&rest[1..end], "p" | "P") && rest[end..].starts_with('{') {
                end += rest[end..].find('}').map_or(rest.len() - end, |i| i + 1);
            }
            out.push_str(&rest[..end]);
            rest = &rest[end..];
        } else if let Some(r) = rest.strip_prefix("${") {
            let end = r.find('}').ok_or("`${` has no closing `}`")?;
            let name = &r[..end];
            if !PROVIDERS.contains(&name) {
                return Err(unknown_provider(name));
            }
            out.push_str("(?:)");
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix('{')
            && r.starts_with(|c: char| c.is_ascii_alphabetic())
        {
            let end = r.find('}').ok_or("`{` has no closing `}`")?;
            let name = &r[..end];
            let Some((_, re)) = PLACEHOLDERS.iter().find(|p| p.0 == name) else {
                let known: Vec<String> = PLACEHOLDERS
                    .iter()
                    .map(|p| format!("{{{}}}", p.0))
                    .collect();
                return Err(format!(
                    "{{{name}}} is not a placeholder ({})",
                    known.join(", ")
                ));
            };
            out.push_str(&format!("(?:{re})"));
            rest = &r[end + 1..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    Ok(out)
}

fn unknown_provider(name: &str) -> String {
    let known: Vec<String> = PROVIDERS.iter().map(|p| format!("${{{p}}}")).collect();
    format!("${{{name}}} is not a setting ({})", known.join(", "))
}

/// A date format: settings it names exist, and every token is one `daily` reads.
fn check_format(format: &str) -> Result<(), String> {
    let mut plain = String::new();
    let mut rest = format;
    while let Some(i) = rest.find("${") {
        plain.push_str(&rest[..i]);
        let r = &rest[i + 2..];
        let end = r.find('}').ok_or("format: `${` has no closing `}`")?;
        if !PROVIDERS.contains(&&r[..end]) {
            return Err(format!("format: {}", unknown_provider(&r[..end])));
        }
        rest = &r[end + 1..];
    }
    plain.push_str(rest);
    let unknown = daily::unknown_tokens(&plain);
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "format {format:?}: {} not a date token cold-lava reads (YYYY, MM, DD, …); put literal text in [brackets]",
            unknown
                .iter()
                .map(|t| format!("{t:?}"))
                .collect::<Vec<_>>()
                .join(", ")
                + if unknown.len() == 1 { " is" } else { " are" }
        ))
    }
}

/// The `{name}`s in a `write` template.
fn placeholders(template: &str) -> impl Iterator<Item = &str> {
    template
        .split('{')
        .skip(1)
        .filter_map(|s| s.split_once('}').map(|(name, _)| name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn definition(text: &str) -> Definition {
        toml::from_str(text).unwrap_or_else(|e| panic!("{text}\n{e}"))
    }

    fn config(text: &str) -> Layer {
        Layer::config(Path::new("config.toml"), toml::from_str(text).unwrap())
    }

    fn names(t: &Types) -> Vec<&str> {
        t.types.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn every_source_kind_reads_in_the_order_written() {
        let d = definition(
            r#"
            fields = { a = "text", b = "number", c = "date", d = "bool", e = "text", f = "tag",
                       g = "text", h = "text", i = "text", j = "text" }
            [[from]]
            on = "list-item"
            match = '~(?<v>{duration})'
            fields = { a = { capture = "v" }, b = { pattern = 'n(\d+)' },
                       c = { marker = "📅", inline = "due" }, d = { flag = "⏫" },
                       e = { flags = { "⏫" = "high", "🔼" = "medium" } }, f = { inlines = "tags" },
                       g = { children = "task" }, h = { note = "folder" },
                       i = { section = "path" }, j = { rest = true } }
            [[from]]
            on = "table-row"
            fields = { a = { column = "A" } }
            [[from]]
            on = "note"
            fields = { a = { property = "a", note = "name" } }
            [[from]]
            on = "code"
            fields = { a = { info = "language" } }
            "#,
        );
        assert_eq!(check("x", &d), Vec::<String>::new());
        let list = &d.from[0].fields;
        assert_eq!(
            list["c"].0,
            [Source::Marker("📅".into()), Source::Inline("due".into())]
        );
        assert_eq!(list["j"].0, [Source::Rest(Yes)]);
        let kinds: Vec<&str> = list.values().flat_map(|s| &s.0).map(Source::kind).collect();
        assert_eq!(
            kinds,
            [
                "capture", "pattern", "marker", "inline", "flag", "flags", "inlines", "children",
                "note", "section", "rest"
            ]
        );
        assert_eq!(d.from[1].fields["a"].0, [Source::Column("A".into())]);
        assert_eq!(
            d.from[2].fields["a"].0,
            [Source::Property("a".into()), Source::Note(NotePart::Name)]
        );
        assert_eq!(d.from[3].fields["a"].0, [Source::Info(InfoPart::Language)]);
        let writable: Vec<bool> = list.values().map(|s| s.0[0].writable()).collect();
        assert_eq!(
            writable,
            [
                true, true, true, true, true, false, false, false, false, false
            ]
        );
    }

    #[test]
    fn one_shape_can_sit_at_the_top() {
        let lone = definition(
            r#"
            on = "inline"
            match = '(?:^|\s)~(?<v>{duration})\b'
            fields.minutes = { capture = "v", type = "duration" }
            fields.note-name = "text"
            write = " ~{minutes}"
            "#,
        );
        let long = definition(
            r#"
            fields = { minutes = "duration", note-name = "text" }
            write = " ~{minutes}"
            [[from]]
            on = "inline"
            match = '(?:^|\s)~(?<v>{duration})\b'
            fields = { minutes = { capture = "v" } }
            "#,
        );
        assert_eq!(lone, long);
        assert_eq!(check("estimate", &lone), Vec::<String>::new());
    }

    #[test]
    fn shape_mistakes_are_one_line() {
        for (text, needle) in [
            (
                "on = \"note\"\n[[x.from]]\non = \"note\"",
                "go in each shape",
            ),
            (
                "fields.a = { property = \"a\" }\n[[x.from]]\non = \"note\"",
                "fields.a: `property`",
            ),
            ("fields.a = \"text\"", "no shape"),
            ("on = \"note\"\nfields.a = { colum = \"A\" }", "colum"),
            (
                "on = \"note\"\nfields.a = \"txt\"",
                "\"txt\" is not a value type",
            ),
            (
                "on = \"note\"\nfields.a = { type = \"text\", values = { a = \"b\" } }",
                "values go with type = \"enum\"",
            ),
            (
                "on = \"note\"\nfields.a = { type = \"enum\" }",
                "an enum needs values",
            ),
            (
                "on = \"paragraph\"\nfields.a = { rest = false }",
                "rest = false",
            ),
            ("on = \"row\"", "row"),
            ("on = \"note\"\nsytle = 1", "sytle"),
        ] {
            let t = load([config(&format!("[x]\n{text}"))]);
            assert_eq!(t.errors.len(), 1, "{text}: {:?}", t.errors);
            assert!(
                t.errors[0].starts_with("config.toml: [types.x] "),
                "{:?}",
                t.errors
            );
            assert!(!t.errors[0].contains('\n'), "{:?}", t.errors);
            assert!(t.errors[0].contains(needle), "{text}: {:?}", t.errors);
            assert!(t.types.is_empty());
        }
    }

    #[test]
    fn each_mistake_drops_its_type_and_keeps_the_rest() {
        let good = "[good]\non = \"note\"\nfields.title = { property = \"title\" }\n";
        for (bad, needle) in [
            ("on = \"code\"\nmatch = '\\w{5000}'", "too big"),
            ("on = \"code\"\nmatch = '(x'", "match '(x': unclosed group"),
            (
                "on = \"list-item\"\nmatch = '(?<v>x)'\nfields.a = { capture = \"w\" }",
                "no group named \"w\"",
            ),
            (
                "on = \"list-item\"\nfields.a = { capture = \"v\" }",
                "needs a match",
            ),
            (
                "on = \"list-item\"\nmatch = '{when}'",
                "{when} is not a placeholder",
            ),
            (
                "on = \"list-item\"\nmatch = '${tasks.status}'",
                "is not a setting",
            ),
            (
                "on = \"note\"\nfields.a = { column = \"A\" }",
                "column can't be read on note",
            ),
            (
                "on = \"list-item\"\nfields.a = { property = \"a\" }",
                "property can't be read on list-item",
            ),
            (
                "on = \"paragraph\"\nfields.a = { info = \"language\" }",
                "info can't be read on paragraph",
            ),
            (
                "on = \"paragraph\"\nfields.a = { rest = true }\nfields.b = { rest = true }",
                "more than one field reads the rest",
            ),
            (
                "on = \"note\"\nfields.rev = \"text\"",
                "field \"rev\" is reserved",
            ),
            (
                "on = \"note\"\nfields.\"my field\" = \"text\"",
                "not a field name",
            ),
            (
                "on = \"note\"\nextends = \"thought\"",
                "extends \"thought\", which is not a type",
            ),
            (
                "on = \"note\"\nrequires = [\"tabels\"]",
                "\"tabels\", which is not a syntax",
            ),
            (
                "on = \"note\"\nfields.d = { type = \"date\", format = \"YYYY-Q\" }",
                "\"Q\" is not a date token",
            ),
            (
                "on = \"note\"\nfields.d = { type = \"date\", format = \"${daily.fmt}\" }",
                "${daily.fmt} is not a setting",
            ),
            (
                "on = \"note\"\nactions = [\"tasks.tick\"]",
                "not a native action",
            ),
            ("on = \"note\"\nwrite = \"x\"", "write is for inline types"),
            (
                "on = \"inline\"\nmatch = 'x'\nwrite = \"{y}\"",
                "write uses {y}, which is not a field",
            ),
            ("on = \"inline\"", "an inline type needs a match"),
            (
                "on = \"note\"\nstyle.a = { bold = true }",
                "style.a: \"a\" is not a field",
            ),
            (
                "on = \"note\"\nfields.a = \"text\"\nstyle.a = { by-value = { x = { dim = true } } }",
                "a is text, not an enum",
            ),
            (
                "on = \"note\"\nfields.s = { type = \"enum\", values = { \"[ ]\" = \"todo\" } }\n\
                 style.s = { by-value = { done = { dim = true } } }",
                "\"done\" is not one of its values (todo)",
            ),
            (
                "on = \"note\"\nwhere.header = [\"A\"]",
                "where.header doesn't apply on note",
            ),
            (
                "on = \"paragraph\"\nwhere.language = [\"rust\"]",
                "where.language doesn't apply",
            ),
            (
                "on = \"paragraph\"\nfields.a = { pattern = 'x' }",
                "needs a group",
            ),
            (
                "on = \"paragraph\"\nfields.a = { flag = \"⏫\" }",
                "the field's type is bool",
            ),
            ("on = \"note\"\nkey = [\"a\"]", "key \"a\" is not a field"),
            (
                "on = \"paragraph\"\nwhere.inside = [\"fence\"]",
                "where.inside names \"fence\"",
            ),
            (
                "on = \"list-item\"\nfields.a = { children = \"tsk\" }",
                "children names \"tsk\"",
            ),
        ] {
            let t = load([config(&format!("{good}[bad]\n{bad}\n"))]);
            assert_eq!(t.errors.len(), 1, "{bad}: {:?}", t.errors);
            let line = &t.errors[0];
            assert!(line.starts_with("config.toml: [types.bad] "), "{line}");
            assert!(line.contains(needle), "{bad}: {line}");
            assert!(!line.contains('\n'), "{line}");
            assert_eq!(names(&t), ["good"], "{bad}");
        }
    }

    #[test]
    fn a_shape_is_named_in_its_errors_when_there_are_several() {
        let t = load([config(
            "[x]\nfields.a = \"text\"\n[[x.from]]\non = \"note\"\n[[x.from]]\non = \"note\"\n\
             fields.a = { column = \"A\" }\n",
        )]);
        assert_eq!(
            t.errors,
            [
                "config.toml: [types.x] shape 2: fields.a: column can't be read on note (only on table-row)"
            ]
        );
    }

    #[test]
    fn built_in_names_are_extended_not_redefined() {
        let t = load([config(
            "[task]\non = \"list-item\"\n[idea]\nextends = \"task\"\non = \"list-item\"\n\
             where.tag = [\"idea\"]\n",
        )]);
        assert_eq!(names(&t), ["idea"]);
        assert_eq!(t.errors.len(), 1, "{:?}", t.errors);
        assert!(
            t.errors[0].contains("[types.task] task is built in"),
            "{:?}",
            t.errors
        );
        assert_eq!(t.native, NATIVE);
    }

    #[test]
    fn extends_goes_nowhere_circular_or_dropped() {
        let t = load([config(
            "[a]\nextends = \"b\"\non = \"note\"\n[b]\nextends = \"a\"\non = \"note\"\n\
             [c]\nextends = \"d\"\non = \"note\"\n[d]\non = \"nope\"\n\
             [e]\nextends = \"c\"\non = \"note\"\n",
        )]);
        assert!(t.types.is_empty(), "{:?}", names(&t));
        assert!(
            t.errors
                .iter()
                .any(|e| e.contains("[types.a] extends goes round in a circle: a → b → a")),
            "{:?}",
            t.errors
        );
        assert!(
            t.errors
                .iter()
                .any(|e| e.contains("[types.c] extends \"d\", which was dropped for its errors")),
            "{:?}",
            t.errors
        );
        assert!(
            t.errors
                .iter()
                .any(|e| e.contains("[types.e] extends \"c\", which was dropped")),
            "{:?}",
            t.errors
        );
    }

    fn vault(name: &str, text: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("cold-lava-types-{}-{name}", std::process::id()));
        std::fs::create_dir_all(root.join(".cold-lava")).unwrap();
        std::fs::write(root.join(VAULT_FILE), text).unwrap();
        root
    }

    #[test]
    fn load_order_and_replacements() {
        let ext: Definitions = [(
            "count".to_string(),
            definition("on = \"note\"\nfields.words = \"number\""),
        )]
        .into();
        let root = vault(
            "order",
            "version = 1\n[types.book]\non = \"table-row\"\n\
             [types.tally]\nextends = \"word-count.count\"\non = \"note\"\n",
        );
        let t = load(
            [
                Some(Layer::extension("word-count", &ext)),
                Some(config(
                    "[book]\non = \"note\"\n[idea]\nextends = \"task\"\non = \"list-item\"\n",
                )),
                Layer::vault(&root),
            ]
            .into_iter()
            .flatten(),
        );
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        assert_eq!(names(&t), ["word-count.count", "idea", "book", "tally"]);
        let file = root.join(VAULT_FILE);
        assert_eq!(
            t.notes,
            [format!(
                "{}: [types.book] replaces the one from config config.toml",
                file.display()
            )]
        );
        assert_eq!(t.types[2].origin, Origin::Vault { path: file });
        assert_eq!(t.types[2].definition.from[0].on, On::TableRow);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_vault_file_needs_a_version_it_knows() {
        for (name, text, needle) in [
            (
                "none",
                "[types.a]\non = \"note\"\n",
                "no version: start the file with version = 1",
            ),
            (
                "two",
                "version = 2\n[types.a]\non = \"note\"\n",
                "version = 2 is not one this cold-lava reads",
            ),
            ("broken", "version = 1\n[types.a\n", "can't read it"),
        ] {
            let root = vault(name, text);
            let t = load(Layer::vault(&root));
            assert!(t.types.is_empty(), "{name}");
            assert_eq!(t.errors.len(), 1, "{name}: {:?}", t.errors);
            assert!(t.errors[0].contains(needle), "{name}: {:?}", t.errors);
            assert!(!t.errors[0].contains('\n'), "{:?}", t.errors);
            let _ = std::fs::remove_dir_all(root);
        }
        let root = vault(
            "extra",
            "version = 1\ncolour = 1\n[types.a]\non = \"note\"\n",
        );
        let t = load(Layer::vault(&root));
        assert_eq!(names(&t), ["a"]);
        assert_eq!(t.errors.len(), 1, "{:?}", t.errors);
        assert!(
            t.errors[0].contains("\"colour\" is not a setting"),
            "{:?}",
            t.errors
        );
        let _ = std::fs::remove_dir_all(root);
        assert!(Layer::vault(Path::new("/nonexistent/vault")).is_none());
    }

    /// The design's book: what `types/list` sends, and what an extension's manifest can say.
    #[test]
    fn definitions_are_json() {
        let t = load([config(
            r#"
            [book]
            fields = { title = "text", author = "link", rating = "number",
                       status = { type = "enum", values = { "to read" = "todo", reading = "reading", read = "done" } } }
            style.rating = { icon = "★" }
            style.status = { by-value = { done = { colour = "green", strike = true, whole = true } } }

            [[book.from]]
            on = "table-row"
            where.header = ["Title", "Author"]
            key = ["title"]
            fields = { title = { column = "Title" }, author = { column = "Author" } }

            [[book.from]]
            on = "note"
            where.property = { type = "book" }
            fields = { title = { property = "title", note = "name" }, rating = { property = "rating" } }
            "#,
        )]);
        assert!(t.errors.is_empty(), "{:?}", t.errors);
        let expected = json!({
            "name": "book",
            "origin": {"kind": "config", "path": "config.toml"},
            "fields": {
                "title": {"type": "text"},
                "author": {"type": "link"},
                "rating": {"type": "number"},
                "status": {"type": "enum", "values": {"to read": "todo", "reading": "reading", "read": "done"}},
            },
            "from": [
                {
                    "on": "table-row",
                    "where": {"header": ["Title", "Author"]},
                    "key": ["title"],
                    "fields": {"title": [{"column": "Title"}], "author": [{"column": "Author"}]},
                },
                {
                    "on": "note",
                    "where": {"property": {"type": "book"}},
                    "fields": {
                        "title": [{"property": "title"}, {"note": "name"}],
                        "rating": [{"property": "rating"}],
                    },
                },
            ],
            "style": {
                "rating": {"icon": "★"},
                "status": {"by-value": {"done": {"colour": "green", "strike": true, "whole": true}}},
            },
        });
        let book = serde_json::to_value(&t.types[0]).unwrap();
        assert_eq!(book, expected);
        let mut definition = expected;
        definition
            .as_object_mut()
            .unwrap()
            .retain(|k, _| k != "name" && k != "origin");
        assert_eq!(
            serde_json::from_value::<Definition>(definition).unwrap(),
            t.types[0].definition,
            "the JSON reads back as the same definition"
        );
        let list = serde_json::to_value(&t).unwrap();
        assert_eq!(list["native"], json!(["task", "list", "heading", "block"]));
        assert_eq!(list["errors"], json!([]));
    }

    #[test]
    fn placeholders_and_escapes() {
        assert_eq!(
            expand(r"\{date} {date}").unwrap(),
            r"\{date} (?:\d{4}-\d{2}-\d{2})"
        );
        assert_eq!(expand(r"\p{L}+ x{2}").unwrap(), r"\p{L}+ x{2}");
        assert_eq!(expand("${tasks.statuses}").unwrap(), "(?:)");
    }
}
