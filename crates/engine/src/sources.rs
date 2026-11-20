//! The words a type definition and an attribute reading are both written in: what a shape is
//! found on (`On`), where a field is read (`Source`, `Sources`), where a definition came from
//! (`Origin`), the workspace's settings `${…}` stands for (`Provided`, `fill`), and patterns with
//! their placeholders (`compile`). `types.rs` reads type definitions in them and re-exports them;
//! the Markdown owner's attribute readings (`owner/markdown/attributes.rs`) are written in them
//! too. A leaf, so neither of those two depends on the other for them.

use std::fmt;
use std::path::PathBuf;

use indexmap::IndexMap;
use regex::{Regex, RegexBuilder};
use serde::de::{self, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::alphabet::Role;

/// `${…}` settings a definition can follow, read from the workspace's own settings.
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

/// What one pattern may compile to, so no definition can make matching slow.
const REGEX_LIMIT: usize = 256 * 1024;

/// What a shape is found on: a piece, by its role in the alphabet (docs/extensions.md, "The
/// alphabet, as built"), or `inline`. The variants are the roles some reading exists for; `embed`
/// and `opaque` are refused by name until something is read as one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum On {
    /// A whole note, read from its frontmatter, name and tags.
    Document,
    Section,
    Text,
    Item,
    Row,
    Aside,
    Verbatim,
    /// Not a role: a pattern found in the text of a piece, whatever the piece's role.
    Inline,
}

use On as O;

const TEXT: &[On] = &[O::Section, O::Text, O::Item, O::Aside];
const BLOCKS: &[On] = &[O::Section, O::Text, O::Item, O::Row, O::Aside, O::Verbatim];
const ALL: &[On] = &[
    O::Document,
    O::Section,
    O::Text,
    O::Item,
    O::Row,
    O::Aside,
    O::Verbatim,
    O::Inline,
];

impl On {
    pub fn role(self) -> Option<Role> {
        match self {
            O::Document => Some(Role::Document),
            O::Section => Some(Role::Section),
            O::Text => Some(Role::Text),
            O::Item => Some(Role::Item),
            O::Row => Some(Role::Row),
            O::Verbatim => Some(Role::Verbatim),
            O::Aside => Some(Role::Aside),
            O::Inline => None,
        }
    }

    /// Its name: the role's, or `inline`.
    pub fn name(self) -> &'static str {
        self.role().map_or("inline", Role::name)
    }

    /// The name it had before the roles were named (`list-item` for `item`). It is still read,
    /// it is what a definition's JSON says as `on`, and it is what a block's `inside` holds,
    /// where it shares a list with kinds and type names.
    pub fn alias(self) -> &'static str {
        self.role().and_then(Role::alias).unwrap_or("inline")
    }

    /// A role's name or its older one.
    pub(crate) fn from_name(name: &str) -> Option<On> {
        (ALL.iter().copied()).find(|o| o.name() == name || o.alias() == name)
    }

    /// The role with the older name beside it, for a message: whoever reads it wrote one of
    /// the two.
    pub(crate) fn both(self) -> String {
        match self.role() {
            Some(_) => format!("{} ({})", self.name(), self.alias()),
            None => self.name().to_string(),
        }
    }

    /// Why `name` can't be an `on`. The TOML reader adds "in `on`" after it.
    pub(crate) fn unknown(name: &str) -> String {
        let names = |of: fn(On) -> &'static str| {
            let roles = ALL.iter().filter(|o| o.role().is_some());
            roles.map(|o| of(*o)).collect::<Vec<_>>().join(", ")
        };
        match Role::from_name(name) {
            Some(_) => format!(
                "nothing is read as {name:?} yet, so no type can be on it: use {} or inline",
                names(On::name)
            ),
            None => format!(
                "{name:?} is not a role ({}), inline, or an older name ({})",
                names(On::name),
                names(On::alias)
            ),
        }
    }

    /// Why `where.inside` can't name this piece, when it can't: nothing a block's `inside`
    /// holds says it. The older `heading` there is a pattern in a heading's own words, which is
    /// not what being inside a section means, so `section` is not read as it.
    pub(crate) fn not_inside(name: &str) -> Option<&'static str> {
        match name {
            "section" => Some(
                "which where.inside can't test yet: for a block under a heading use \
                 where.section = [\"Its title\"], and for a pattern in a heading's own words \
                 \"heading\"",
            ),
            "document" => Some(
                "which where.inside can't test yet: every block is inside its document, so \
                 name the note's type instead",
            ),
            _ => None,
        }
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
    Rest(Rest),
    /// The character between a task's brackets: `x` in `[x] Pay rent`, kept as written, a
    /// space included.
    Checkbox(Yes),
    /// An attribute the piece's owner gives it, by name: `checkbox`, `level`, `callout`. The
    /// type doesn't say how the file writes it; the owner does.
    Attribute(String),
}

/// `rest = true`, or `rest = { without = ["#task"] }`: the leftover text, with every
/// occurrence of these strings taken out as well.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rest {
    pub without: Vec<String>,
}

impl Serialize for Rest {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.without.is_empty() {
            return s.serialize_bool(true);
        }
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(1))?;
        map.serialize_entry("without", &self.without)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Rest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Rest, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Bool(bool),
            Table { without: OneOrMany },
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }
        match Written::deserialize(d) {
            Ok(Written::Bool(true)) => Ok(Rest::default()),
            Ok(Written::Bool(false)) => {
                Err(de::Error::custom("rest = false: leave it out instead"))
            }
            Ok(Written::Table { without }) => Ok(Rest {
                without: match without {
                    OneOrMany::One(s) => vec![s],
                    OneOrMany::Many(v) => v,
                },
            }),
            Err(_) => Err(de::Error::custom(
                "rest is true, or { without = [\"text\", …] }",
            )),
        }
    }
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
    /// The date of the daily note it is in, by the workspace's Daily notes folder and format.
    Date,
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
            Source::Checkbox(_) => "checkbox",
            Source::Attribute(_) => "attribute",
        }
    }

    /// Whether a value set on the field can be put back through this source: only its span
    /// changes. The others are derived from text the source doesn't own, or, for a heading's
    /// `level` and a callout's `fold`, from the note's structure.
    pub fn writable(&self) -> bool {
        match self {
            Source::Inlines(_)
            | Source::Children(_)
            | Source::Note(_)
            | Source::Section(_)
            | Source::Rest(_) => false,
            Source::Attribute(name) => !matches!(name.as_str(), "level" | "fold"),
            _ => true,
        }
    }

    /// What it can be read on.
    pub fn on(&self) -> &'static [On] {
        match self {
            Source::Capture(_) | Source::Note(_) => ALL,
            Source::Pattern(_) | Source::Attribute(_) => BLOCKS,
            Source::Marker(_) | Source::Flag(_) | Source::Flags(_) | Source::Rest(_) => TEXT,
            Source::Inline(_) => &[O::Document, O::Section, O::Text, O::Item, O::Aside],
            Source::Column(_) => &[O::Row],
            Source::Property(_) => &[O::Document],
            Source::Info(_) => &[O::Verbatim],
            Source::Checkbox(_) => &[O::Item],
            Source::Inlines(_) => &[O::Document, O::Section, O::Text, O::Item, O::Row, O::Aside],
            Source::Children(_) => &[O::Document, O::Section, O::Item],
            Source::Section(_) => &[
                O::Section,
                O::Text,
                O::Item,
                O::Row,
                O::Verbatim,
                O::Aside,
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
pub(crate) fn source<'de, A: MapAccess<'de>>(key: String, map: &mut A) -> Result<Source, A::Error> {
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

/// Where a type was defined.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Origin {
    /// `types/*.toml` in the engine: the task.
    BuiltIn,
    /// An extension's manifest; its types are named `<extension>.<name>`.
    Extension { name: String },
    /// `[types.*]` in config.toml.
    Config { path: PathBuf },
    /// The workspace's `.cold-lava/types.toml`.
    Workspace { path: PathBuf },
}

impl Origin {
    /// What an error line starts with: the file, or the extension.
    pub(crate) fn place(&self) -> String {
        match self {
            Origin::BuiltIn => "built in".into(),
            Origin::Extension { name } => format!("extension {name}"),
            Origin::Config { path } | Origin::Workspace { path } => path.display().to_string(),
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Origin::BuiltIn => f.write_str("built in"),
            Origin::Extension { name } => write!(f, "extension {name}"),
            Origin::Config { path } => write!(f, "config {}", path.display()),
            Origin::Workspace { path } => write!(f, "workspace {}", path.display()),
        }
    }
}

/// The workspace's own settings that `${…}` in a definition stands for.
#[derive(Clone, Debug, Default)]
pub struct Provided {
    /// The Tasks plugin's status symbols and the names of their types (`x` = `done`).
    pub statuses: IndexMap<String, String>,
    pub global_filter: String,
    /// `emoji` or `dataview`.
    pub tasks_format: String,
    pub daily_format: String,
}

impl Provided {
    /// What `${name}` is in a pattern: the setting as literal text; the statuses as any one of
    /// their symbols.
    pub(crate) fn in_pattern(&self, name: &str) -> String {
        match name {
            "tasks.statuses" => {
                let symbols: Vec<String> = self.statuses.keys().map(|s| regex::escape(s)).collect();
                format!("(?:{})", symbols.join("|"))
            }
            "tasks.global-filter" => regex::escape(&self.global_filter),
            "tasks.format" => regex::escape(&self.tasks_format),
            "daily.format" => regex::escape(&self.daily_format),
            _ => String::new(),
        }
    }

    /// What `${name}` is in plain text (`rest`'s `without`).
    pub(crate) fn in_text(&self, name: &str) -> String {
        match name {
            "tasks.statuses" => String::new(),
            "tasks.global-filter" => self.global_filter.clone(),
            "tasks.format" => self.tasks_format.clone(),
            "daily.format" => self.daily_format.clone(),
            _ => String::new(),
        }
    }
}

/// `text` with each `${name}` replaced; in a pattern, escapes are kept whole.
pub(crate) fn fill(text: &str, pattern: bool, value: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if pattern && c == '\\' {
            let end = 1 + rest[1..].chars().next().map_or(0, char::len_utf8);
            out.push_str(&rest[..end]);
            rest = &rest[end..];
        } else if let Some(r) = rest.strip_prefix("${")
            && let Some(end) = r.find('}')
            && PROVIDERS.contains(&&r[..end])
        {
            out.push_str(&value(&r[..end]));
            rest = &r[end + 1..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

pub(crate) fn is_name(s: &str, dots: bool) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || (dots && c == '.'))
}

/// A `match` or `pattern` with its placeholders and settings filled in, compiled under the size
/// limit. Settings stand for an empty group here: their values come from the workspace.
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
            // An escape, with `\p{…}` and `\x{…}` kept whole so their braces aren't read as a
            // placeholder.
            let mut end = 1 + rest[1..].chars().next().map_or(0, char::len_utf8);
            if matches!(&rest[1..end], "p" | "P" | "x" | "u" | "U") && rest[end..].starts_with('{')
            {
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

pub(crate) fn unknown_provider(name: &str) -> String {
    let known: Vec<String> = PROVIDERS.iter().map(|p| format!("${{{p}}}")).collect();
    format!("${{{name}}} is not a setting ({})", known.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_and_escapes() {
        assert_eq!(
            expand(r"\{date} {date}").unwrap(),
            r"\{date} (?:\d{4}-\d{2}-\d{2})"
        );
        assert_eq!(expand(r"\p{L}+ x{2}").unwrap(), r"\p{L}+ x{2}");
        assert_eq!(expand(r"\x{FE0F}?").unwrap(), r"\x{FE0F}?");
        assert_eq!(expand("${tasks.statuses}").unwrap(), "(?:)");
    }
}
