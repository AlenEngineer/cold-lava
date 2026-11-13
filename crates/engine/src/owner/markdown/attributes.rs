//! How Markdown writes attributes (docs/extensions.md, "Attributes in Markdown, as built").
//!
//! A type reads `{ attribute = "due" }` and doesn't say how a file writes a due date. For
//! Markdown this table does: for a role and an attribute's name, the readings of the piece's
//! text that give the value, which are also how a value is written back. The readings are data
//! (`markdown/attributes.toml`, then config.toml and the workspace's own file, in the order
//! types load), so the conventions they hold (the Tasks plugin's emoji, Dataview's fields) can
//! be seen, changed, and later moved into a profile without touching a type.
//!
//! What the tree itself says about a piece (`level`, `callout`, `fold`) is not here: it is
//! syntax, read by the walk (`matcher::Given`).

use std::borrow::Cow;
use std::path::Path;
use std::sync::OnceLock;

use serde::Serialize;

use crate::problem::Problem;
use crate::types::{self, On, Origin, Provided, Source, Sources};

/// The only `version` an attributes file can have today.
pub const VERSION: i64 = 1;

/// Where a workspace keeps its own.
pub const WORKSPACE_FILE: &str = ".cold-lava/attributes.toml";

/// The readings built into the engine.
pub const BUILT_IN: &str = include_str!("../../../markdown/attributes.toml");

/// The entry for every attribute that has none of its own. In its readings `*` stands for the
/// attribute's name: `"*" = { inline = "*" }` reads `rating` from `[rating:: 5]`.
pub const ANY: &str = "*";

/// The attributes `tasks::toggle` writes or moves itself, with the Tasks plugin's markers in
/// its own code: a tick's done and cancelled dates, a recurring task's next dates. Reading
/// and `set-field` follow the table; ticking doesn't yet.
pub const TICKED: &[&str] = &["due", "scheduled", "start", "done", "cancelled"];

/// What is said of an entry for one of `TICKED` that isn't the built-in one.
pub const TICKING: &str =
    "ticking a task and its recurrence still write the built-in markers, whatever this entry says";

/// The attributes the tree gives, which no file can say how to read.
pub const STRUCTURAL: &[&str] = &["level", "callout", "fold"];

/// How one attribute of pieces with one role is read from their text, and written back.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reading {
    /// The role, by name.
    #[serde(serialize_with = "role_name")]
    pub on: On,
    pub name: String,
    pub origin: Origin,
    /// Tried in order; the first with a value gives it. Empty for an entry a later file took
    /// out (`due = []`): the attribute is then on no piece, and `*` doesn't stand in for it.
    pub read: Vec<Source>,
    /// Text the piece must also hold, after its checkbox, to have the attribute at all; it is
    /// not part of the piece's words. Empty when there is no such text.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub with: String,
}

fn role_name<S: serde::Serializer>(on: &On, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(on.name())
}

/// Readings from one origin, as written.
#[derive(Clone)]
pub struct Layer {
    origin: Origin,
    /// Role → attribute → readings, or why the file doesn't read.
    roles: Result<toml::Table, String>,
}

impl Layer {
    pub fn built_in() -> Layer {
        Layer::from_file(Origin::BuiltIn, Ok(BUILT_IN.to_string()))
    }

    /// config.toml's `[attributes]` table, which the frontend reads and hands over.
    pub fn config(path: &Path, roles: toml::Table) -> Layer {
        let path = path.to_path_buf();
        Layer {
            origin: Origin::Config { path },
            roles: Ok(roles),
        }
    }

    /// The workspace's `.cold-lava/attributes.toml`; `None` when it has none.
    pub fn workspace(root: &Path) -> Option<Layer> {
        let path = root.join(WORKSPACE_FILE);
        let text = match std::fs::read_to_string(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            r => r.map_err(|e| e.to_string()),
        };
        Some(Layer::from_file(Origin::Workspace { path }, text))
    }

    /// An attributes file: `version = 1`, then a table per role.
    fn from_file(origin: Origin, text: Result<String, String>) -> Layer {
        let roles = text.and_then(|t| {
            let mut file: toml::Table = toml::from_str(&t)
                .map_err(|e| format!("can't read it: {}", Problem::toml(&e).one_line()))?;
            match file.remove("version") {
                Some(toml::Value::Integer(VERSION)) => Ok(file),
                Some(v) => Err(format!(
                    "version = {v} is not one this cold-lava reads (it reads version = \
                     {VERSION}); the file is skipped"
                )),
                None => Err(format!(
                    "no version: start the file with version = {VERSION}; the file is skipped"
                )),
            }
        });
        Layer { origin, roles }
    }
}

/// How Markdown writes each attribute, and what was wrong with the rest.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Readings {
    /// In load order; a reading replaced by a later origin takes the later one's place.
    pub readings: Vec<Reading>,
    /// One line per mistake: where, which attribute, what. The reading is dropped.
    pub errors: Vec<String>,
    /// Things worth knowing that aren't mistakes, such as a reading replacing an earlier one.
    pub notes: Vec<String>,
}

static CONFIGURED: OnceLock<Vec<Layer>> = OnceLock::new();

/// The layers every workspace gets before its own: config.toml's, which the frontend reads and
/// sets once at startup.
pub fn set_configured(layers: Vec<Layer>) {
    let _ = CONFIGURED.set(layers);
}

/// The readings a workspace's notes are read with: the built-in ones, the configured layers,
/// then the workspace's own file.
pub fn for_workspace(root: &Path) -> Readings {
    let configured = CONFIGURED.get().cloned().unwrap_or_default();
    load(
        std::iter::once(Layer::built_in())
            .chain(configured)
            .chain(Layer::workspace(root)),
    )
}

/// Loads layers in order. A later reading of the same attribute on the same role replaces the
/// earlier one whole, and says so in a note. A broken one is dropped, and the earlier one stays.
pub fn load(layers: impl IntoIterator<Item = Layer>) -> Readings {
    let mut found = Readings::default();
    for layer in layers {
        let place = layer.origin.place();
        let roles = match layer.roles {
            Ok(roles) => roles,
            Err(e) => {
                found.errors.push(format!("{place}: {e}"));
                continue;
            }
        };
        for (role, attributes) in roles {
            let at = format!("{place}: [{role}]");
            let on = match On::from_name(&role).filter(|on| on.name() == role) {
                Some(on) if !matches!(on, On::Document | On::Inline) => on,
                _ => {
                    found.errors.push(format!(
                        "{at} is not a role attributes are read on (section, text, item, row, \
                         aside, verbatim)"
                    ));
                    continue;
                }
            };
            let toml::Value::Table(attributes) = attributes else {
                found.errors.push(format!(
                    "{at} is a table of attributes, each with its readings"
                ));
                continue;
            };
            for (name, written) in attributes {
                match reading(on, &name, written) {
                    Ok((read, with)) => {
                        let old =
                            (found.readings.iter()).position(|r| r.on == on && r.name == name);
                        let old = old.map(|old| found.readings.remove(old));
                        // No readings takes the entry out: it stays in the table, empty,
                        // so the attribute is on no piece, whatever `*` says.
                        let does = if read.is_empty() {
                            "removes"
                        } else {
                            "replaces"
                        };
                        if let Some(old) = &old {
                            let from = &old.origin;
                            found
                                .notes
                                .push(format!("{at} {name} {does} the one from {from}"));
                        }
                        let changed = layer.origin != Origin::BuiltIn;
                        if changed && on == On::Item && TICKED.contains(&name.as_str()) {
                            found.notes.push(format!("{at} {name}: {TICKING}"));
                        }
                        found.readings.push(Reading {
                            on,
                            name,
                            origin: layer.origin.clone(),
                            read,
                            with,
                        });
                    }
                    Err(e) => found.errors.push(format!("{at} {name}: {e}")),
                }
            }
        }
    }
    found
}

/// One attribute's readings as written: sources like a type's field has, and `with` beside a
/// checkbox.
fn reading(on: On, name: &str, mut written: toml::Value) -> Result<(Vec<Source>, String), String> {
    if name != ANY && !types::is_name(name, false) {
        return Err(
            "not an attribute's name: a letter, then letters, digits, - or _, or \"*\" for \
             every attribute with no entry of its own"
                .into(),
        );
    }
    if STRUCTURAL.contains(&name) {
        return Err(format!(
            "{name} is read from the note's structure and can't be given readings ({})",
            STRUCTURAL.join(", ")
        ));
    }
    let with = match written.as_table_mut().and_then(|t| t.remove("with")) {
        Some(toml::Value::String(with)) => with,
        Some(other) => return Err(format!("with = {other}: with is text")),
        None => String::new(),
    };
    if written.as_table().is_some_and(|t| t.is_empty()) {
        return Err(
            "no readings: give it at least one, like { marker = \"📅\" }, or [] to take the \
             entry out"
                .into(),
        );
    }
    let Sources(read) = written
        .try_into()
        .map_err(|e: toml::de::Error| Problem::toml(&e).one_line())?;
    for s in &read {
        let text = matches!(
            s,
            Source::Marker(_)
                | Source::Inline(_)
                | Source::Flag(_)
                | Source::Flags(_)
                | Source::Pattern(_)
                | Source::Checkbox(_)
        );
        if !text {
            return Err(format!(
                "{} is not a reading of a piece's text (marker, inline, flag, flags, pattern, \
                 checkbox)",
                s.kind()
            ));
        }
        if !s.on().contains(&on) {
            return Err(format!("{} can't be read on {}", s.kind(), on.name()));
        }
        match s {
            Source::Marker(t) | Source::Inline(t) | Source::Flag(t) if t.trim().is_empty() => {
                return Err(format!("{} is empty", s.kind()));
            }
            Source::Flags(glyphs) if glyphs.is_empty() => {
                return Err("flags needs at least one glyph".into());
            }
            Source::Pattern(re) => match types::compile(re) {
                Err(e) => return Err(format!("pattern '{re}': {e}")),
                Ok(r) if r.captures_len() < 2 => {
                    return Err(format!("pattern '{re}' needs a group (…) around the value"));
                }
                Ok(_) => {}
            },
            _ => {}
        }
    }
    if !with.is_empty() && !matches!(read.as_slice(), [Source::Checkbox(_)]) {
        return Err("with goes beside checkbox = true, and nothing else".into());
    }
    Ok((read, with))
}

/// A reading of the `*` entry, for the attribute `name`: `*` in what it looks for is the name.
fn named(reading: &Source, name: &str) -> Source {
    match reading {
        Source::Inline(key) => Source::Inline(key.replace(ANY, name)),
        Source::Marker(glyph) => Source::Marker(glyph.replace(ANY, name)),
        Source::Flag(glyph) => Source::Flag(glyph.replace(ANY, name)),
        other => other.clone(),
    }
}

/// One source of a field once its attributes are spelled out.
#[derive(Clone, Debug, PartialEq)]
pub struct Flat<'a> {
    pub source: Cow<'a, Source>,
    /// The attribute it reads, when the type wrote `attribute = …`, and that reading's `with`.
    pub attribute: Option<(&'a str, &'a str)>,
}

impl Readings {
    /// The readings built into the engine, with no workspace's settings filled in.
    pub fn built_in() -> Readings {
        load([Layer::built_in()])
    }

    /// Fills in each `${…}` setting with the workspace's value, as `Types::fill` does: in a
    /// pattern as literal text, in `with` as itself.
    pub fn fill(&mut self, p: &Provided) {
        for r in &mut self.readings {
            r.with = types::fill(&r.with, false, |n| p.in_text(n));
            for s in &mut r.read {
                if let Source::Pattern(re) = s {
                    *re = types::fill(re, true, |n| p.in_pattern(n));
                }
            }
        }
    }

    fn reading(&self, on: On, name: &str) -> Option<&Reading> {
        self.readings.iter().find(|r| r.on == on && r.name == name)
    }

    /// A field's sources on a piece with the role `on`, with each `attribute = "name"` spelled
    /// out as Markdown writes it, in place: its readings from the table; itself when the tree
    /// gives it (`level`, `callout`, `fold`); else the readings of the table's `*` entry for
    /// that name (built in: the Dataview field, `[name:: value]`). The
    /// matcher reads through this list and `edit.rs` writes through it, so a typed item's
    /// `Place::sources` index means the same to both.
    pub fn flat<'a>(&'a self, on: On, sources: &'a [Source]) -> Vec<Flat<'a>> {
        let mut flat = Vec::with_capacity(sources.len());
        for source in sources {
            let Source::Attribute(name) = source else {
                flat.push(Flat {
                    source: Cow::Borrowed(source),
                    attribute: None,
                });
                continue;
            };
            let name = name.as_str();
            match self.reading(on, name) {
                Some(r) => flat.extend(r.read.iter().map(|s| Flat {
                    source: Cow::Borrowed(s),
                    attribute: Some((name, r.with.as_str())),
                })),
                // The tree's own, then the entry for every attribute that has none, where
                // `*` is the name; with neither, the source stays and reads nothing.
                None if STRUCTURAL.contains(&name) => flat.push(Flat {
                    source: Cow::Borrowed(source),
                    attribute: Some((name, "")),
                }),
                None => match self.reading(on, ANY) {
                    Some(any) => flat.extend(any.read.iter().map(|s| Flat {
                        source: Cow::Owned(named(s, name)),
                        attribute: Some((name, any.with.as_str())),
                    })),
                    None => flat.push(Flat {
                        source: Cow::Borrowed(source),
                        attribute: Some((name, "")),
                    }),
                },
            }
        }
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Layer {
        Layer::config(Path::new("config.toml"), toml::from_str(text).unwrap())
    }

    fn kinds(flat: &[Flat]) -> Vec<String> {
        flat.iter()
            .map(|f| match f.attribute {
                Some((name, _)) => format!("{}:{name}", f.source.kind()),
                None => f.source.kind().to_string(),
            })
            .collect()
    }

    #[test]
    fn the_built_in_readings_load() {
        let a = Readings::built_in();
        assert_eq!(a.errors, Vec::<String>::new());
        let names: Vec<&str> = a.readings.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "checkbox",
                "task",
                "due",
                "scheduled",
                "start",
                "created",
                "done",
                "cancelled",
                "recurrence",
                "priority",
                "*",
                "*",
                "*",
                "*"
            ]
        );
        let roles: Vec<&str> = a.readings[10..].iter().map(|r| r.on.name()).collect();
        assert_eq!(roles, ["item", "section", "text", "aside"]);
        assert_eq!(
            (a.readings[0].with.as_str(), a.readings[1].with.as_str()),
            ("", "${tasks.global-filter}"),
            "every checkbox is a checkbox; the filter says which are tasks"
        );
        assert_eq!(a.readings[0].read, a.readings[1].read);
        assert_eq!(
            a.readings[2].read,
            [
                Source::Inline("due".into()),
                Source::Marker("📅".into()),
                Source::Marker("📆".into()),
                Source::Marker("🗓".into())
            ]
        );
    }

    #[test]
    fn an_attribute_is_spelled_out_where_it_is_read() {
        let a = Readings::built_in();
        let written = [
            Source::Attribute("due".into()),
            Source::Marker("!".into()),
            Source::Attribute("rating".into()),
            Source::Attribute("level".into()),
        ];
        assert_eq!(
            kinds(&a.flat(On::Item, &written)),
            [
                "inline:due",
                "marker:due",
                "marker:due",
                "marker:due",
                "marker",
                "inline:rating",
                "attribute:level"
            ],
            "the table's readings, a source as written, a Dataview field, the tree's"
        );
        assert_eq!(
            kinds(&a.flat(On::Row, &written[..1])),
            ["attribute:due"],
            "a row has no readings and no entry for the rest"
        );
        let without = load([Layer::built_in(), config("[item]\n\"*\" = []\n")]);
        assert_eq!(
            kinds(&without.flat(On::Item, &written[2..3])),
            Vec::<String>::new(),
            "with the entry for the rest taken out, an attribute nobody declares reads nothing"
        );
        let untasked = load([Layer::built_in(), config("[item]\ntask = []\n")]);
        let task = [Source::Attribute("task".into())];
        assert_eq!(
            kinds(&untasked.flat(On::Item, &task)),
            Vec::<String>::new(),
            "an entry taken out is on no piece: `*` doesn't make it a Dataview field"
        );
        assert_eq!(
            kinds(&untasked.flat(On::Item, &written[2..3])),
            ["inline:rating"],
            "and a name with no entry at all still falls to `*`"
        );
        assert_eq!(
            without.notes,
            ["config.toml: [item] * removes the one from built in"]
        );
        let mine = load([config("[item]\n\"*\" = { marker = \"@*\" }\n")]);
        assert_eq!(
            mine.flat(On::Item, &written[2..3])[0].source.as_ref(),
            &Source::Marker("@rating".into())
        );
    }

    #[test]
    fn a_later_layer_replaces_a_reading_and_a_broken_one_is_dropped() {
        let mine = config(
            "[item]\ndue = { marker = \"@due\" }\nstars = { pattern = 'r(\\d)' }\n\
             level = { marker = \"#\" }\nbroken = { rest = true }\nnothing = {}\n\
             filtered = { marker = \"x\", with = \"y\" }\n\
             [note]\ndue = { marker = \"x\" }\n",
        );
        let a = load([Layer::built_in(), mine]);
        assert_eq!(
            a.reading(On::Item, "due").unwrap().read,
            [Source::Marker("@due".into())]
        );
        assert!(a.reading(On::Item, "stars").is_some());
        assert_eq!(
            a.notes,
            [
                "config.toml: [item] due replaces the one from built in".to_string(),
                format!("config.toml: [item] due: {TICKING}"),
            ]
        );
        let errors = a.errors.join("\n");
        for wanted in [
            "[item] level: level is read from the note's structure",
            "[item] broken: rest is not a reading of a piece's text",
            "[item] filtered: with goes beside checkbox = true",
            "[item] nothing: no readings: give it at least one",
            "[note] is not a role attributes are read on",
        ] {
            assert!(errors.contains(wanted), "{wanted}\nin\n{errors}");
        }
        assert_eq!(a.errors.len(), 5, "{errors}");
    }

    #[test]
    fn settings_are_filled_in() {
        let mut a = Readings::built_in();
        a.fill(&Provided {
            global_filter: "#task".into(),
            ..Provided::default()
        });
        assert_eq!(a.readings[1].with, "#task");
    }

    /// A broken entry in the workspace's own file is one line among the types' mistakes, the
    /// entry is dropped, and the rest load: the built-in reading of that attribute stays.
    #[test]
    fn a_broken_entry_is_told_with_the_types_mistakes() {
        let dir = std::env::temp_dir().join(format!(
            "cold-lava-attributes-broken-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
        std::fs::write(
            dir.join(WORKSPACE_FILE),
            "version = 1\n[item]\ndue = { rest = true }\nstars = { marker = \"★\" }\n",
        )
        .unwrap();
        let found = types::for_workspace(&dir);
        assert_eq!(found.errors.len(), 1, "{:?}", found.errors);
        assert!(
            found.errors[0].ends_with(
                "attributes.toml: [item] due: rest is not a reading of a piece's text (marker, \
                 inline, flag, flags, pattern, checkbox)"
            ),
            "{:?}",
            found.errors
        );
        assert!(found.types.iter().any(|t| t.name == "task"));
        let readings = for_workspace(&dir);
        assert_eq!(
            readings.reading(On::Item, "due").unwrap().origin,
            Origin::BuiltIn,
            "the reading it would have replaced stays"
        );
        assert!(readings.reading(On::Item, "stars").is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
