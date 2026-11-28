//! Notes as data: the core reads a note into typed items (tasks, list items, headings and
//! blocks with an id), so features ask for them instead of parsing notes themselves. See
//! "Notes as data" in docs/extensions.md.
//!
//! The wire model (`Item`, `Data`, a typed item's fields), the settings a note is read with,
//! and the ids: Markdown reads a note's items and makes their keys (`owner::markdown::items`),
//! and `identify` gives each item its id and rev.

use crate::configured::Configured;
use crate::daily;
use crate::identity;
use crate::markdown::{self, Flavour};
use crate::matcher::{Matcher, Typed};
use crate::owner::markdown::attributes::{self, Readings};
use crate::sources;
use crate::tasks_settings::{StatusType, TasksSettings};
use crate::templates;
use crate::types::{self, Types};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::Path;

/// Everything about a workspace that changes how its notes read as items.
pub struct Settings {
    pub flavour: Flavour,
    pub tasks: TasksSettings,
    pub daily: daily::Settings,
    /// Notes in the templates folders aren't read: a template's tasks and types aren't real ones.
    pub templates: templates::Settings,
    /// The types its notes are matched against, and what was wrong with the rest.
    pub types: Types,
    /// How Markdown writes the attributes those types read (`owner::markdown::attributes`).
    pub readings: Readings,
    pub matcher: Matcher,
}

impl Settings {
    /// The workspace's settings, with what the frontend `configured`.
    pub fn load(root: &Path, configured: &Configured) -> Settings {
        let types = types::for_workspace(root, configured);
        let (flavour, readings) = (configured.flavour.as_ref(), &configured.attributes);
        Settings::load_with(root, flavour, readings, types)
    }

    /// The workspace's settings, with these types, the configured flavour (`None`: the
    /// workspace's own) and the configured attribute readings before the workspace's.
    pub fn load_with(
        root: &Path,
        flavour: Option<&Flavour>,
        readings: &[attributes::Layer],
        types: Types,
    ) -> Settings {
        Settings::with(
            markdown::flavour_for(root, flavour),
            TasksSettings::load(root),
            daily::Settings::load(root),
            templates::Settings::load(root),
            types,
            attributes::for_workspace(root, readings),
        )
    }

    /// Settings with the attribute readings built into the engine.
    pub fn new(
        flavour: Flavour,
        tasks: TasksSettings,
        daily: daily::Settings,
        templates: templates::Settings,
        types: Types,
    ) -> Settings {
        let built_in = Readings::built_in();
        Settings::with(flavour, tasks, daily, templates, types, built_in)
    }

    pub fn with(
        flavour: Flavour,
        tasks: TasksSettings,
        daily: daily::Settings,
        templates: templates::Settings,
        mut types: Types,
        mut readings: Readings,
    ) -> Settings {
        // The plugin's core symbols first, so a status set by its type is written as they are.
        let mut statuses: Vec<(&char, &StatusType)> = tasks.statuses.iter().collect();
        statuses.sort_by_key(|(c, _)| {
            [' ', 'x', '/', '-']
                .iter()
                .position(|k| k == *c)
                .unwrap_or(usize::MAX)
        });
        let provided = sources::Provided {
            statuses: statuses
                .into_iter()
                .map(|(c, t)| (c.to_string(), t.name().to_string()))
                .collect(),
            global_filter: tasks.global_filter.clone(),
            tasks_format: if tasks.dataview { "dataview" } else { "emoji" }.into(),
            daily_format: daily.format.clone(),
        };
        types.fill(&provided);
        readings.fill(&provided);
        let matcher = Matcher::new(&types, &readings, &flavour, &daily);
        Settings {
            flavour,
            tasks,
            daily,
            templates,
            types,
            readings,
            matcher,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    /// Stable across edits that keep the item's anchor, restarts and processes.
    pub id: String,
    /// Workspace-relative path, with `.md`.
    pub note: String,
    /// The item's own lines (1-based, inclusive); a list item's children are items of their own.
    pub line: usize,
    pub end_line: usize,
    /// The headings it sits under, outermost first.
    pub section: Vec<String>,
    /// The id of the list item it is nested in.
    pub parent: Option<String>,
    /// The author's `^id`.
    pub block_id: Option<String>,
    /// Fingerprint of the item's own lines: which version of it this is. For a twin whose lines
    /// another twin has, it also holds how many twins there are and the rev of the twin it is
    /// under (`items/twins.rs`, `identity::twin_rev`), so a twin that moves changes its rev.
    pub rev: String,
    #[serde(flatten)]
    pub data: Data,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Data {
    List {
        text: String,
    },
    Heading {
        level: u8,
        text: String,
    },
    /// A paragraph, or the block before a `^id` line, named by its block id.
    Block,
    /// A block or note a type matched: `kind` is the type's name.
    #[serde(untagged)]
    Typed(Box<Typed>),
}

/// The native kinds. They are older than the alphabet's roles (`alphabet::Role`) and are not
/// roles: an item's id hashes its kind, so the kinds keep their names. Where the two meet:
///
/// - `list`, and a `task` (a type on a list item): an `item`.
/// - `heading`: a `section`. The item is the section's title; its body is every item whose
///   `section` path runs through that title.
/// - `block`: whatever a `^id` names, most often a `text`, also a table or an `aside`. It
///   says that a block has a name, not what the block is.
/// - a typed item: the role its shape is `on` (`sources::On::role`); an inline type's item is a
///   reading of a piece's text and has no role of its own.
pub const KINDS: &[&str] = &["list", "heading", "block"];

impl Data {
    pub fn kind(&self) -> &str {
        match self {
            Data::List { .. } => "list",
            Data::Heading { .. } => "heading",
            Data::Block => "block",
            Data::Typed(t) => &t.kind,
        }
    }

    /// Whether `items KIND` lists it: its own kind, or a base it extends.
    pub fn answers_to(&self, kind: &str) -> bool {
        self.kind() == kind || matches!(self, Data::Typed(t) if t.is.iter().any(|b| b == kind))
    }
}

/// Flat, like every item: `kind`, then `is` when it extends a type, then each schema field.
impl Serialize for Typed {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        map.serialize_entry("kind", &self.kind)?;
        if !self.is.is_empty() {
            map.serialize_entry("is", &self.is)?;
        }
        for (name, value) in &self.fields {
            map.serialize_entry(&**name, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Typed {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Typed, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Typed;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a typed item's fields")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Typed, A::Error> {
                let mut t = Typed {
                    kind: String::new(),
                    is: Vec::new(),
                    fields: Default::default(),
                    spans: Default::default(),
                    unparsed: 0,
                    place: Default::default(),
                };
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "kind" => t.kind = map.next_value()?,
                        "is" => t.is = map.next_value()?,
                        _ => {
                            let value = map.next_value()?;
                            t.fields.insert(&key, value);
                        }
                    }
                }
                if t.kind.is_empty() {
                    return Err(serde::de::Error::missing_field("kind"));
                }
                Ok(t)
            }
        }
        d.deserialize_map(V)
    }
}

/// An item before its id: an id needs its parent's, so ids are given once the note is read.
pub(crate) struct Raw {
    pub(crate) line: usize,
    pub(crate) end_line: usize,
    pub(crate) section: Vec<String>,
    pub(crate) parent: Option<usize>,
    pub(crate) block_id: Option<String>,
    pub(crate) data: Data,
    /// A typed item's key: its shape's `key` fields, or what tells its primitive apart.
    pub(crate) key: String,
}

/// Each item's key, as its owner made it, in the order of its items: the key, what its anchor
/// is made of (the item it is keyed under, whose key the anchor starts with, and what the anchor
/// has after that key) and whether an anchor was counted twice, so that two items may be twins.
pub(crate) struct Keys {
    pub(crate) keys: Vec<String>,
    pub(crate) anchors: Vec<(Option<usize>, String)>,
    pub(crate) repeated: bool,
}

/// The items of the note at workspace-relative `note` (with `.md`) whose text is `text`, as its
/// owner, Markdown, reads them (`owner::markdown::items`).
pub fn extract(note: &str, text: &str, s: &Settings) -> Vec<Item> {
    crate::owner::markdown::items(note, text, s)
}

/// Gives each item its id and rev, in document order (a parent before its children): the id of
/// its key (`keys`, as its owner made it), and the rev of its lines, or a twin's (`twins`).
pub(crate) fn identify(note: &str, text: &str, raws: Vec<Raw>, keys: Keys) -> Vec<Item> {
    let lines: Vec<&str> = text.lines().collect();
    let mut items: Vec<Item> = Vec::with_capacity(raws.len());
    // An item's own lines, joined, reused from item to item.
    let mut joined = String::new();
    for (raw, key) in raws.into_iter().zip(&keys.keys) {
        let id = identity::id(&[note, raw.data.kind(), key]);
        joined.clear();
        for (i, line) in (lines.get(raw.line - 1..raw.end_line.min(lines.len())))
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            if i > 0 {
                joined.push('\n');
            }
            joined.push_str(line);
        }
        items.push(Item {
            id,
            note: note.to_string(),
            line: raw.line,
            end_line: raw.end_line,
            section: raw.section,
            parent: raw.parent.map(|p| items[p].id.clone()),
            block_id: raw.block_id,
            rev: identity::line_id(&joined),
            data: raw.data,
        });
    }
    // Skipped for speed (#327): it cost every note about 40 µs, though with no anchor counted
    // twice every key is its anchor and no two of a kind share one, so no item has a twin.
    // A debug build runs it anyway and checks that it changes nothing, on every note a test reads.
    if keys.repeated {
        twin_revs(&mut items, &keys.anchors);
    } else if cfg!(debug_assertions) {
        let mut passed = items.clone();
        twin_revs(&mut passed, &keys.anchors);
        assert_eq!(passed, items, "the skipped twin pass would change {note}");
    }
    items.sort_by_key(|i| i.line);
    items
}

/// Gives the items one of whose twins has the same lines a twin's rev (`twins`): `items` as
/// `identify` made them, a parent before its children, and what their anchors are made of.
fn twin_revs(items: &mut [Item], anchors: &[(Option<usize>, String)]) {
    let revs = {
        let keyed: Vec<twins::Keyed> = (items.iter().zip(anchors))
            .map(|(item, (under, own))| twins::Keyed {
                kind: item.data.kind(),
                under: *under,
                own,
                rev: &item.rev,
            })
            .collect();
        // A twin's rev for each twin. The item it is under comes before it, so that one's rev
        // is here when it is a twin's, and else is the rev of its lines.
        let mut revs: Vec<Option<String>> = Vec::with_capacity(keyed.len());
        for (item, twin) in keyed.iter().zip(twins::of(&keyed)) {
            let rev = twin.map(|twin| {
                let under = (twin.under).map(|p| revs[p].as_deref().unwrap_or(keyed[p].rev));
                identity::twin_rev(item.rev, twin.twins, under)
            });
            revs.push(rev);
        }
        revs
    };
    for (item, rev) in items.iter_mut().zip(revs) {
        if let Some(rev) = rev {
            item.rev = rev;
        }
    }
}

pub mod anchors;
mod twins;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::cst;
    use crate::owner::markdown::walk::pulldown;
    use serde_json::json;

    pub(crate) fn settings(flavour: &str) -> Settings {
        let nowhere = Path::new("/nonexistent-cold-lava-workspace");
        Settings::new(
            Flavour::built_in(flavour).unwrap(),
            TasksSettings::load(nowhere),
            daily::Settings::load(nowhere),
            templates::Settings::default(),
            types::load([types::Layer::built_in()]),
        )
    }

    /// A typed item's text field.
    fn field<'a>(i: &'a Item, name: &str) -> &'a str {
        match &i.data {
            Data::Typed(t) => t.fields.get(name).and_then(|v| v.as_str()).unwrap_or(""),
            _ => "",
        }
    }

    fn items(text: &str) -> Vec<Item> {
        extract("Note.md", text, &settings("obsidian"))
    }

    /// (kind, line, end_line, short text)
    fn summary(items: &[Item]) -> Vec<(&'static str, usize, usize, String)> {
        items
            .iter()
            .map(|i| {
                let text = match &i.data {
                    Data::Typed(t) if t.kind == "task" => {
                        format!("[{}] {}", field(i, "status"), field(i, "text"))
                    }
                    Data::List { text } => text.clone(),
                    Data::Heading { text, .. } => text.clone(),
                    Data::Block => format!("^{}", i.block_id.as_deref().unwrap_or("")),
                    Data::Typed(t) => t.kind.clone(),
                };
                let kind = match &i.data {
                    Data::Typed(t) if t.kind == "task" => "task",
                    Data::Typed(_) => "typed",
                    d => KINDS.iter().find(|k| **k == d.kind()).unwrap(),
                };
                (kind, i.line, i.end_line, text)
            })
            .collect()
    }

    fn s(
        kind: &'static str,
        line: usize,
        end: usize,
        text: &str,
    ) -> (&'static str, usize, usize, String) {
        (kind, line, end, text.to_string())
    }

    fn by_text<'a>(items: &'a [Item], text: &str) -> &'a Item {
        items
            .iter()
            .find(|i| match &i.data {
                Data::List { text: t } | Data::Heading { text: t, .. } => t == text,
                Data::Typed(_) => field(i, "text") == text,
                Data::Block => false,
            })
            .unwrap_or_else(|| panic!("no item {text:?} in {:?}", summary(items)))
    }

    #[test]
    fn reads_headings_lists_tasks_and_blocks() {
        let note = "\
# Trip
Intro paragraph ^intro

## Packing
- [ ] Boots 📅 2026-11-12
  - socks
  - [x] laces
    continued
- Map ^map

| a | b |
|---|---|
| 1 | 2 |

^table

Setext
------
";
        let all = items(note);
        assert_eq!(
            summary(&all),
            [
                s("heading", 1, 1, "Trip"),
                s("block", 2, 2, "^intro"),
                s("heading", 4, 4, "Packing"),
                s("task", 5, 5, "[ ] Boots"),
                s("list", 6, 6, "socks"),
                s("task", 7, 8, "[x] laces"),
                s("list", 9, 9, "Map"),
                s("block", 11, 15, "^table"),
                s("heading", 17, 18, "Setext"),
            ]
        );
        let boots = by_text(&all, "Boots");
        assert_eq!(boots.section, ["Trip", "Packing"]);
        assert_eq!(boots.parent, None);
        assert_eq!(by_text(&all, "socks").parent.as_ref(), Some(&boots.id));
        assert_eq!(by_text(&all, "laces").parent.as_ref(), Some(&boots.id));
        assert_eq!(by_text(&all, "Map").block_id.as_deref(), Some("map"));
        assert_eq!(by_text(&all, "Packing").section, ["Trip"]);
        assert_eq!(
            by_text(&all, "Setext").section,
            ["Trip"],
            "a level 2 heading ends Packing"
        );
        let ids: std::collections::HashSet<&str> = all.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids.len(), all.len(), "ids are unique");
    }

    /// A section is a title with its body (docs/extensions.md, "The alphabet, as built"): a
    /// heading closes every open section of its level or deeper and opens its own, and
    /// `section` is the titles still open, outermost first.
    #[test]
    fn a_section_is_a_title_and_its_body_by_heading_levels() {
        let note = "\
- before any heading
# One
- under one
### Three
- under three
> #### Quoted
> - in the quote
## Two
- under two
### Three again
# Other
- under other
- holds a heading
  ## Listed
- after the list item
";
        let all = items(note);
        let section = |text: &str| by_text(&all, text).section.clone();
        let none: [&str; 0] = [];
        assert_eq!(section("before any heading"), none);
        assert_eq!(section("One"), none, "a title is not in its own section");
        assert_eq!(section("under one"), ["One"]);
        assert_eq!(
            section("Three"),
            ["One"],
            "a skipped level nests all the same"
        );
        assert_eq!(section("under three"), ["One", "Three"]);
        assert_eq!(
            section("in the quote"),
            ["One", "Three", "Quoted"],
            "a heading in a quote opens a section like any other"
        );
        assert_eq!(
            section("Two"),
            ["One"],
            "a shallower heading closes the deeper ones"
        );
        assert_eq!(section("under two"), ["One", "Two"]);
        assert_eq!(section("Three again"), ["One", "Two"]);
        assert_eq!(section("Other"), none, "the same level closes its sibling");
        assert_eq!(section("under other"), ["Other"]);
        assert_eq!(
            section("after the list item"),
            ["Other", "Listed"],
            "a heading in a list item opens a section that outlasts the list item"
        );
    }

    #[test]
    fn code_and_comments_hold_no_items_but_callouts_do() {
        let note = "\
~~~
- [ ] in a tilde fence
~~~
```
- [ ] in a backtick fence
```
%% - [ ] in a comment
- [ ] still in it %%
- [ ] real %%with a comment%%
> [!todo] Callout
> - [ ] in a callout 📅 2026-11-12
> > - [/] in a nested quote
";
        let all = items(note);
        assert_eq!(
            summary(&all),
            [
                s("task", 9, 9, "[ ] real"),
                s("task", 11, 11, "[ ] in a callout"),
                s("task", 12, 12, "[/] in a nested quote"),
            ]
        );
        assert_eq!(
            all[0].rev,
            identity::line_id("- [ ] real %%with a comment%%"),
            "rev is of the line as written"
        );
    }

    #[test]
    fn custom_statuses_are_tasks() {
        let all = items("- [/] going\n- [-] dropped\n- [?] unsure\n- [ ] \n");
        let types: Vec<_> = all
            .iter()
            .map(|i| (i.data.kind() == "task").then(|| field(i, "status_type")))
            .collect();
        assert_eq!(
            types,
            [
                Some("in_progress"),
                Some("cancelled"),
                Some("non_task"),
                None,
            ],
            "an empty checkbox is a list item"
        );
    }

    #[test]
    fn the_flavour_decides_what_is_read() {
        let note = "- [ ] task ^b1\n\nPara ^p1\n";
        let kinds = |flavour: &str| {
            summary(&extract("Note.md", note, &settings(flavour)))
                .into_iter()
                .map(|s| (s.0, s.3))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds("obsidian"),
            [("task", "[ ] task".into()), ("block", "^p1".into())]
        );
        assert_eq!(kinds("github"), [("task", "[ ] task ^b1".into())]);
        assert_eq!(kinds("commonmark"), [("list", "[ ] task ^b1".into())]);
    }

    #[test]
    fn ids_survive_ticking_redating_indenting_and_lines_above() {
        let before =
            "- [ ] Pay rent 📅 2026-11-01\n  - [ ] Transfer\n- [ ] Pay rent 📅 2026-12-01\n";
        let id = |text: &str, which: &str, nth: usize| {
            let all = items(text);
            all.iter()
                .filter(|i| i.data.kind() == "task" && field(i, "text") == which)
                .nth(nth)
                .unwrap()
                .id
                .clone()
        };
        let rent = id(before, "Pay rent", 0);
        let second = id(before, "Pay rent", 1);
        let child = id(before, "Transfer", 0);
        assert_ne!(rent, second, "identical siblings get their own ids");
        for after in [
            "- [x] Pay rent 📅 2026-11-01 ✅ 2026-11-02\n  - [x] Transfer\n- [ ] Pay rent 📅 2026-12-01\n",
            "- [ ] Pay rent 📅 2026-11-08 ⏫\n  - [ ] Transfer 🛫 2026-11-07\n- [ ] Pay rent 📅 2026-12-01\n",
            "  - [ ] Pay rent 📅 2026-11-01\n\t- [ ] Transfer\n  - [ ] Pay rent 📅 2026-12-01\n",
            "# Bills\n\nSome words.\n\n- [ ] Pay rent 📅 2026-11-01\n  - [ ] Transfer\n- [ ] Pay rent 📅 2026-12-01\n",
        ] {
            assert_eq!(id(after, "Pay rent", 0), rent, "{after}");
            assert_eq!(id(after, "Pay rent", 1), second, "{after}");
            assert_eq!(id(after, "Transfer", 0), child, "{after}");
        }
        let edited =
            "- [ ] Pay the rent 📅 2026-11-01\n  - [ ] Transfer\n- [ ] Pay rent 📅 2026-12-01\n";
        assert_ne!(id(edited, "Pay the rent", 0), rent, "new words, new id");
        assert_ne!(
            id(edited, "Transfer", 0),
            child,
            "its parent's words changed"
        );
        let anchored = "- [ ] Pay rent ^rent\n";
        let renamed = "- [x] Pay the landlord ^rent\n";
        assert_eq!(
            items(anchored)[0].id,
            items(renamed)[0].id,
            "a block id anchors it"
        );
        assert_eq!(
            items(anchored)[0].id,
            extract("Note.md", anchored, &settings("obsidian"))[0].id,
            "ids don't depend on the process"
        );
        assert_ne!(
            items(anchored)[0].id,
            extract("Other.md", anchored, &settings("obsidian"))[0].id,
            "ids include the note"
        );
    }

    #[test]
    fn rev_is_the_line_id_of_a_one_line_task() {
        let line = "- [ ] Pay rent 📅 2026-11-01";
        let all = items(&format!("intro\n\n{line}\n"));
        assert_eq!(all[0].rev, identity::line_id(line));
        let two = items("- [ ] Pay rent\n  by transfer\n");
        assert_eq!(
            two[0].rev,
            identity::line_id("- [ ] Pay rent\n  by transfer")
        );
    }

    /// Twins (#327, `twins`): where a twin has an item's lines, the item's rev says how many
    /// there are of them, so it changes when one comes or goes, and, under twins, which one it
    /// is under. Twins told apart by their lines keep the revs of their lines.
    #[test]
    fn a_twins_rev_says_how_many_twins_there_are_and_which_twin_it_is_under() {
        let line = "- [ ] Pay rent";
        let lines = identity::line_id(line);
        let read = |n: usize| items(&format!("{line}\n").repeat(n));
        let (one, two, three) = (read(1), read(2), read(3));
        assert_eq!(one[0].rev, lines, "no twin");
        for (n, all) in [(2, &two), (3, &three)] {
            let revs: Vec<&str> = all.iter().map(|i| i.rev.as_str()).collect();
            assert_eq!(revs, vec![identity::twin_rev(&lines, n, None); n]);
        }
        assert_eq!((&two[0].id, &two[1].id), (&three[0].id, &three[1].id));
        let dated = items("- [ ] Pay rent 📅 2026-11-12\n- [ ] Pay rent 📅 2026-12-12\n");
        let want = identity::line_id("- [ ] Pay rent 📅 2026-12-12");
        assert_eq!(dated[1].rev, want, "told apart by its lines");

        let block = |date: &str| format!("- [ ] Rent 📅 {date}\n  - [ ] Transfer\n");
        let (nov, dec) = (block("2026-11-01"), block("2026-12-01"));
        let transfer = |text: &str| -> Item {
            let mut all = items(text).into_iter();
            all.find(|i| field(i, "text") == "Transfer").unwrap()
        };
        let first = transfer(&format!("{nov}{dec}"));
        let under = identity::line_id("- [ ] Rent 📅 2026-11-01");
        let own = identity::line_id("  - [ ] Transfer");
        assert_eq!(first.rev, identity::twin_rev(&own, 2, Some(&under)));
        // The first block gone, or the two swapped: another item holds the first's address.
        for after in [format!("{dec}{nov}"), dec] {
            let now = transfer(&after);
            assert_eq!(now.id, first.id, "{after}");
            assert_ne!(now.rev, first.rev, "{after}");
        }
        // Under an item with no twins, twins are told apart by their place under it alone: a
        // tick of it leaves their revs.
        let milk = |status: &str| items(&format!("- [{status}] Shop\n  - milk\n  - milk\n"));
        let (open, done) = (milk(" "), milk("x"));
        let own = identity::line_id("  - milk");
        assert_eq!(open[1].rev, identity::twin_rev(&own, 2, None));
        assert_eq!((&open[1].rev, &open[2].rev), (&done[1].rev, &done[2].rev));
    }

    /// A note in which no anchor is counted twice skips the twin pass (`identify`, for
    /// speed), and a debug build checks there that the pass would change nothing. This reads
    /// every note of the example workspace, the corpus and the tricky cases, and a note with
    /// no twins at all, so that check runs on each.
    #[test]
    fn a_note_without_twins_gets_the_same_ids_and_revs_without_the_twin_pass() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let mut notes: Vec<(String, String)> = Vec::new();
        for e in walkdir::WalkDir::new(&workspace).into_iter().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md") {
                let name = p.strip_prefix(&workspace).unwrap().to_string_lossy();
                notes.push((name.into_owned(), std::fs::read_to_string(p).unwrap()));
            }
        }
        notes.extend(cst::corpus());
        for (k, text) in TRICKY.iter().enumerate() {
            notes.push((format!("Tricky {k}.md"), text.to_string()));
        }
        let alone = "# Bills\n- [ ] Pay rent 📅 2026-11-12\n  - [ ] Transfer\n- milk\n- eggs\n";
        notes.push(("Alone.md".into(), alone.into()));
        let s = every_type("obsidian");
        for (name, text) in &notes {
            extract(name, text, &s);
        }
    }

    #[test]
    fn daily_notes_date_their_tasks() {
        let s = settings("obsidian");
        let all = extract("2026-11-05.md", "- [ ] Water plants\n", &s);
        assert_eq!(field(&all[0], "daily_note"), "2026-11-05");
    }

    #[test]
    fn item_json_is_pinned() {
        let all = items(
            "# Plan\n- [/] Call [[Ana]] 📅 2026-11-12 ^call\n  - bring notes\n\nAside ^aside\n",
        );
        let id = |n: usize| all[n].id.clone();
        let rev = |n: usize| all[n].rev.clone();
        let wire = [
            json!({
                "id": id(0), "note": "Note.md", "line": 1, "end_line": 1, "section": [],
                "parent": null, "block_id": null, "rev": rev(0),
                "kind": "heading", "level": 1, "text": "Plan"
            }),
            json!({
                "id": id(1), "note": "Note.md", "line": 2, "end_line": 2, "section": ["Plan"],
                "parent": null, "block_id": "call", "rev": rev(1),
                "kind": "task", "status": "/", "status_type": "in_progress",
                "text": "Call [[Ana]]", "due": "2026-11-12", "scheduled": null, "start": null,
                "created": null, "done": null, "cancelled": null, "recurrence": null,
                "priority": null, "tags": [], "links": ["Ana"], "daily_note": null
            }),
            json!({
                "id": id(2), "note": "Note.md", "line": 3, "end_line": 3, "section": ["Plan"],
                "parent": id(1), "block_id": null, "rev": rev(2),
                "kind": "list", "text": "bring notes"
            }),
            json!({
                "id": id(3), "note": "Note.md", "line": 5, "end_line": 5, "section": ["Plan"],
                "parent": null, "block_id": "aside", "rev": rev(3),
                "kind": "block"
            }),
        ];
        assert_eq!(all.len(), wire.len());
        for (item, json) in all.iter().zip(wire) {
            assert_eq!(serde_json::to_value(item).unwrap(), json);
            let back = serde_json::from_value::<Item>(json.clone()).unwrap();
            assert_eq!(serde_json::to_value(back).unwrap(), json, "it reads back");
        }
        assert!(all.iter().all(|i| i.id.len() == 16 && i.rev.len() == 12));
    }

    /// Types that read every kind of block, the note's tags and links, and what a block is in.
    const EVERY_TYPE: &str = r#"
[page]
on = "note"
where.file-type = ["md"]
fields.tags = { inlines = "tags", type = "tag" }
fields.links = { inlines = "links", type = "link" }

[para]
on = "paragraph"
fields.text = { rest = true }

[head]
on = "heading"
fields.title = { rest = true }

[line]
on = "list-item"
fields.what = { rest = true }

[row]
on = "table-row"
fields.links = { inlines = "links", type = "link" }

[word]
on = "inline"
match = '\bthe (?<w>\w+)'
fields.w = { capture = "w" }

[quoted]
on = "inline"
match = '\ba (?<w>\w+)'
where.inside = ["quote"]
fields.w = { capture = "w" }
"#;

    fn every_type(flavour: &str) -> Settings {
        with_types(flavour, EVERY_TYPE)
    }

    /// Settings in `flavour` with the built-in types and those of `toml`, as `[types]` in
    /// config.toml gives them.
    pub(crate) fn with_types(flavour: &str, toml: &str) -> Settings {
        let table: toml::Table = toml::from_str(toml).unwrap();
        let found = types::load([
            types::Layer::built_in(),
            types::Layer::config(Path::new("config.toml"), table),
        ]);
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        let nowhere = Path::new("/nonexistent-cold-lava-workspace");
        Settings::new(
            Flavour::built_in(flavour).unwrap(),
            TasksSettings::load(nowhere),
            daily::Settings::load(nowhere),
            templates::Settings::default(),
            found,
        )
    }

    /// Markdown whose blocks are easy to read differently from pulldown-cmark.
    pub(crate) const TRICKY: &[&str] = &[
        // Lazy lines.
        "- [ ] task\nlazy line\n- [ ] next\n\n> quote\nlazy in quote ^q1\n\nPara\nlazy\n    still para ^p1\n",
        // Loose and tight lists.
        "- a\n\n- b\n  \n  more b\n- c\n\n\n- d after two blank lines\n\n1. one\n2. two\n\n   para in two ^two\n3) other delimiter\n",
        // Indented code.
        "Para\n\n    code ^not\n    - [ ] not a task\n\n- item\n\n      code in item\n- [ ] after code\n\n\tTabbed code\n",
        // HTML blocks.
        "<div>\n- [ ] in html\n\n- [ ] after html\n<!-- comment\n- [ ] in comment\n-->\n<details><summary>x</summary>\n\nInside details ^d1\n\n</details>\n<span>inline</span> text ^s1\n<custom-tag>\n- [ ] after a lone tag\n",
        // Block ids.
        "First para\n\n^alone\n\n- item ^i1\n  continued ^i2\n- item two\n  ^i3\n\n# Heading ^h1\n\n> quoted ^qq\n> ^quote-alone\n\n| a | b |\n|---|---|\n| 1 | 2 |\n^table-id\n\nText\n^glued\n\n***\n\n^after-rule\n```\ncode\n```\n^after-code\n",
        // Nested lists under headings.
        "# One\n## Two\n- a\n  - b\n    - [ ] c\n      - d\n  - e\n### Three\n- [x] f\n  1. g\n  2. h\n# Four\n- i\n- i\n",
        // Headings.
        "#NoSpace\n# Closed #\n## Trailing ##   \n#\n   ### Indented\nSetext one\nline two\n===\n> Quoted setext\n> ---\n- # In an item\n  Setext in an item\n  ---\n",
        // Tables.
        "| Name | VEI | Note |\n|---|:-:|--:|\n| [[Krakatoa]] the big | 6 | big |\n| a \\| b | 2 |\n| x | y | z | extra |\nno pipes row\n\n> | q | r |\n> |---|---|\n> | 1 | 2 |\n\n| [[A]] | c |\n| --- | --- |\n| d | [[E]] |\n|| empty |\n  | indented | row |\n",
        // Fences.
        "~~~md\n- [ ] in tilde\n~~~\n````\n```\n- [ ] in nested\n```\n````\n- item\n  ```\n  - [ ] in item fence\n  ```\n- [ ] after\n```\nunclosed\n- [ ] in unclosed\n",
        // Callouts.
        "> [!note] Title\n> body text ^c1\n\n> [!todo]- Folded\n> - [ ] in callout\n\n> [!info] Alone ^c2\n\n> [!NOTE]\n> GitHub alert body ^c3\n\n> [!tip]\n>\n> after blank ^c4\n\n> [!quote] the title\n> the body\n> > [!note] nested\n> > nested body ^c5\n",
        // Comments.
        "Text %%hidden%% more ^k1\n%%\n- [ ] commented\n%%\n- [ ] visible %%note%%\nPara\n%%inline only%%\nmore ^k2\n``%%code%%`` stays\n",
        // Math.
        "$$\nE = mc^2\n$$\n^eq1\n\nInline $x$ and $$y$$ VEI 4 the end\n\n$$\na\n$$ ^eq2\n",
        // Frontmatter, and the body's tags and links.
        "---\ntags: [a, b]\nlinks: \"[[X]]\"\n---\n# Body #tag\nSee [[Note]], [[Note#H|alias]], ![[Embed]], [md](Other%20Note.md), [title](Some.md \"t\"), [web](https://x.y), ![img](pic.md), [anchor](#local), `[[code]]` and #tag, #nested/tag, (#paren), #123, a#not ^b1\n",
        // Line breaks, tabs, empty items, markers.
        "# Title\r\n- [ ] crlf task\r\n  more\r\n\r\nPara ^crlf\r\n",
        "- [ ] last line without a break",
        "-\ttab item\n\t- nested by a tab\n*\tstar\n",
        "-\n  content below\n- \n-   \n\n1.\n",
        "- a\n+ b\n* c\n1. d\n2) e\n",
        "Para\n- [ ] item interrupts\nPara2\n2. no interruption\nPara3\n# heading interrupts\n",
        "> a\n> > b ^n1\n> c\n\n> - [ ] task in a quote\n>   - nested\n> lazy\n",
        "- a\n---\n* * *\n- b\n",
        "- [ ] \n- [x]\n- [ ]no space\n- [/] in progress\n  - [-] cancelled child\n- [  ] two spaces\n",
        // A table that starts with `|` ends the paragraph above it, not a lazy one after a list
        // item; one that doesn't start with `|` doesn't.
        "Caption line\n| a | b |\n|---|---|\n| 1 | the 2 |\n\n> quoted caption\n> | q | r |\n> |---|---|\n> | 3 | 4 |\n\n- item\n| x | y |\n|---|---|\n| 5 | 6 |\n\nNot heavy\nc | d\n---|---\n7 | 8\n\n> [!info] Stats\n> | a | b |\n> |---|---|\n> | 1 | 2 |\n",
        // Reference links: full, collapsed and shortcut, an image, a label in another case, a
        // label nothing defines, a label defined twice.
        "See [the note][n], [N][], [n] and ![pic][n]; [web][w], [missing][y] and [y].\n\n[n]: Note.md\n[w]: https://example.com \"Title\"\n[N]: Other.md\n",
        // Definitions in a quote and in a list item, and before a paragraph's text.
        "> [q]: Quoted.md\n> the text with [q]\n\n- [i]: Item.md\n- see [i] and [Q]\n\n[t]: <Top Note.md> 'title'\nSetext after a definition, [t]\n===\n",
        // Markdown links whose destination has a title, is in `<>` or is none: what follows
        // `B.md` is no title, so the link names no note.
        "[a](B.md and more) [b](C.md \"title\") [c](<D E.md>) [d]( F.md ) [e](G.md 'x' y) [f](<H.md>z)\n",
        // Footnote definitions hold blocks: a second paragraph indented under a blank line,
        // a list, a task, a fenced block, and the paragraph after one, which isn't its.
        "Text[^1] here.\n\n[^1]: First paragraph. ^f1\n\n    Second paragraph the same note. ^f2\n\nAfter the note.\n",
        "[^1]: Note with a list:\n\n    - a\n    - b ^li\n\n[^t]: Tasks:\n\n    - [ ] a task in a footnote\n    - [x] done\n",
        "[^c]: Code:\n\n    ```\n    - [ ] not a task\n    ```\n\n    after the code ^ac\n",
        "[^1]: The note.\nlazy line of it ^lz\n\n[^2]: Another.\n\nUnindented paragraph the end. ^un\n",
        // Two definitions in a row, right under a paragraph, and definitions in a quote.
        "Para the first ^p\n[^1]: One. ^one\n[^2]: Two. ^two\n",
        "> [^q]: In a quote the note. ^q1\n>\n>     Second in the quote. ^q2\n\n> text\n> [^r]: after quoted text ^r1\n",
        // A definition right under a list item ends the list, and what is indented under it
        // is its, not the item's. A `===` or `---` under a definition's line is no setext
        // underline: the heading the tree read there before #212 never was one.
        "- item one\n- item two\n[^1]: right after a list item\n    - [ ] continuation task [[Tgt]]\n",
        "[^1]: def line\n===\n\n[^2]: def two\n---\n\nParagraph above. ^above\n[^n]: Note text ^on\n\n^standalone\n",
        // Two `$$` in two paragraphs close nothing: what is between them is read.
        "$$100 is what it cost ^p0\n\n- [ ] a task the first ^t1\n\n## A heading\n\nPara the middle ^p1\n\n$$ at the end again ^p2\n",
        // A fence stays code after an open `$$`.
        "$$ open before fence\n\n```\n$$\n```\n\n- [ ] a task the second ^t2\n\nPara the third ^p3\n",
        // A price pairs with no later `$`.
        "cost $12 for [[Note]] #pricetag and $x$ the end ^p4\n\n- [ ] task $5 and $6 [[Other]] #t\n",
        // `$$` over a blank line, and after spaces.
        "$$\nx = 1\n\ny = 2\n$$ ^p5\n\n   $$ spaced ^p6\n",
        // Math over the lines of an item.
        "- [ ] task with $a\n  b$ math over lines [[L]]\n- $$\n  c\n  $$ ^m1\n",
        // `$$` that nothing closes is text, and the tasks after it are tasks.
        "$$ is slang for money\n\n- [ ] a task the first ^t1\n\nPara the last ^p1\n",
        // An escape stops a wikilink and a link; a link's text holds no link; a lazy line
        // under a callout's title is the callout's.
        "\\[[Note]] and \\[text](Other.md) and [[Real]] the end ^e1\n",
        "[a [b](Inner.md) d](Outer.md) and [![pic](p.png) e](Third.md)\n",
        "> [!note] Title the first\nlazy line the second ^lz\n\n> [!todo] T\n- [ ] not lazy\n",
        // Tags right after the markers that open emphasis and a strikethrough.
        "Read **#bold** and _#slanted_, *#star*, __#under__, ***#both*** and ~~#gone~~, but a#b and http://x.y/#frag are no tags.\n",
    ];

    /// Markdown pulldown-cmark read otherwise than Obsidian shows it, where the items differ
    /// from the ones it gave.
    const DIFFERENT: &[&str] = &[
        // A frontmatter whose first line is blank, as the Kanban plugin writes it, was a rule
        // and Markdown.
        "---\n\nkanban-plugin: basic\n- [ ] YAML\n\n---\n\n- [ ] Pack\n",
        // A `---` line after a blank line, with a `---` or `...` line below it, opened YAML in
        // the middle of a note, hiding what was between.
        "Intro\n\n---\n- [ ] between the rules\n---\n\n- [ ] after\n",
        // A byte order mark made the first line text.
        "\u{feff}- [ ] first\n- [ ] second\n",
        // `[[#Heading]]` held the tag `Heading`.
        "See [[#Heading]].\n",
        // A highlight was text to pulldown-cmark, so the tag right after its `==` was none. It
        // is one in Obsidian.
        "Read ==#marked==.\n",
        // `[[A\|B]]` in a table cell, where the pipe before the alias is escaped so that it
        // doesn't end the cell, linked to `A\`. The note is `A`.
        "| [[A\\|B]] | c |\n| --- | --- |\n| d | [[E\\|F]] |\n",
    ];

    /// How the tree's items and pulldown-cmark's differ for one note, if they do.
    fn difference(name: &str, text: &str, s: &Settings) -> Option<String> {
        let (tree, old) = (extract(name, text, s), pulldown::extract(name, text, s));
        if tree == old {
            return None;
        }
        let short = |i: Option<&Item>| format!("{i:?}").chars().take(400).collect::<String>();
        let mut out = format!(
            "{name} ({}): {} items, pulldown-cmark {}",
            s.flavour.name,
            tree.len(),
            old.len()
        );
        let n = tree.len().max(old.len());
        for k in (0..n).filter(|&k| tree.get(k) != old.get(k)).take(3) {
            out += &format!(
                "\n  tree: {}\n  then: {}",
                short(tree.get(k)),
                short(old.get(k))
            );
        }
        Some(out)
    }

    /// The walk through the tree gives every note the items pulldown-cmark gave it: the example
    /// workspace as `cold-lava items` reads it, then every Markdown file of the repository and
    /// the tricky cases in each flavour, with types that read every kind of block.
    #[test]
    fn the_tree_reads_items_as_pulldown_cmark_did() {
        let mut differ = Vec::new();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let s = Settings::load(&workspace, &Configured::default());
        for e in walkdir::WalkDir::new(&workspace).into_iter().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "md") {
                let name = p.strip_prefix(&workspace).unwrap().to_string_lossy();
                let text = std::fs::read_to_string(p).unwrap();
                differ.extend(difference(&name, &text, &s));
            }
        }
        // The one way they differ: pulldown-cmark took a frontmatter whose first line is blank,
        // as the Kanban plugin writes it, for a rule and Markdown. The tree reads frontmatter,
        // as Obsidian does (`a_frontmatter_with_a_blank_first_line_is_frontmatter`).
        let corpus: Vec<(String, String)> = cst::corpus()
            .into_iter()
            .filter(|(_, text)| !text.starts_with("---\n\n"))
            .collect();
        for flavour in ["obsidian", "github", "commonmark"] {
            let s = every_type(flavour);
            for (name, text) in &corpus {
                differ.extend(difference(name, text, &s));
            }
            for (k, text) in TRICKY.iter().enumerate() {
                differ.extend(difference(&format!("Tricky {k}.md"), text, &s));
            }
        }
        let s = every_type("obsidian");
        for text in DIFFERENT {
            if difference("Different.md", text, &s).is_none() {
                differ.push(format!("reads alike now, so it isn't DIFFERENT: {text:?}"));
            }
        }
        assert!(
            differ.is_empty(),
            "{} read differently:\n{}",
            differ.len(),
            differ.join("\n")
        );
    }

    /// Tasks that are easy to read differently once a task is found by its attributes and not
    /// by its characters.
    const TRICKY_TASKS: &[&str] = &[
        // A marker and a Dataview field for one date: the field counts, both leave the words.
        "- [ ] both [due:: 2026-11-20] 📅 2026-11-21 📆 2026-11-22 #task\n- [ ] (due:: 2026-11-23) round 📅 2026-11-24\n",
        // Several priority glyphs, with a Dataview priority, and one in code.
        "- [ ] many 🔺 ⏬ 🔼 #task\n- [ ] field wins? ⏫ [priority:: low]\n- [ ] in code `⏫` 🔽\n- [ ] no such [priority:: urgent]\n",
        // The global filter: before the words, after them, twice, missing, and glued.
        "- [ ] #task first\n- [ ] last #task\n- [ ] #task twice #task\n- [ ] none\n- [ ] glued#task\n- [ ] #tasks longer\n- [#] task #task\n- [x] x marks x\n- [ ] ] bracket [ ] again\n",
        // A filter that is one letter, the status itself; the filter before the checkbox, on
        // the line above, and in a quote's marker line; the filter and no other words.
        "- [x] buy\n- [x] buy x\n- [ ] x\n- x [ ] before the box\n- #task [ ] before the box\n#task\n- [ ] under a filter line\n> #task\n> - [ ] quoted under it\n- [ ] #task\n- [ ] #task #task\n- [ ] #task 📅 2026-11-12\n",
        // Custom statuses, known and unknown, and odd characters between the brackets.
        "- [?] question\n- [!] urgent\n- [/] going #task\n- [-] dropped\n- [X] capital\n- [é] accent\n- [📅] emoji\n- [ ]\ttab after\n- [ ]\u{a0}nbsp after\n- [ ]\u{3000}wide space after\n",
        // No words: a bare checkbox, only a date, only the filter, only a tag, only a priority.
        "- [ ] \n- [x]\n- [ ] 📅 2026-11-12\n- [ ] #task\n- [ ] ⏫\n- [ ] [due:: 2026-11-12]\n- [ ] ^id\n",
        // Not a checkbox: no space after, two characters, none, a link, an escaped bracket.
        "- [x]no space\n- [  ] two\n- [] none\n- [x](link.md) a link\n- \\[ ] escaped\n- [ ]x\n- [ ]] bracket\n",
        // In a quote, a callout, a nested list, an ordered list, a footnote.
        "> - [ ] quoted 📅 2026-11-18 #task\n> > - [x] deeper ✅ 2026-11-01\n\n> [!todo]- Folded\n> - [ ] in a callout ⏳ 2026-11-19 #task\n\n- parent\n  - [ ] nested 🛫 2026-11-20 #task\n    - [/] deeper [scheduled:: 2026-11-22] #task\n1. [ ] numbered ➕ 2026-11-01\n2) [x] other delimiter ❌ 2026-11-02\n\n[^1]: Note:\n\n    - [ ] in a footnote 📅 2026-11-23 #task\n",
        // Recurrence with odd characters, a variation selector, a Dataview repeat, both.
        "- [ ] odd 🔁 every week, on Monday! 📅 2026-11-16\n- [ ] selector 🔁\u{fe0f} every 2 days when done ⏳\u{fe0f} 2026-11-25\n- [ ] dv [repeat:: every month on the 1st]\n- [ ] both 🔁 every day [repeat:: every week]\n- [ ] stops 🔁 every day; not this\n- [ ] bare 🔁\n",
        // Dates that aren't dates, and dates glued to words.
        "- [ ] someday 📅 tomorrow\n- [ ] short 📅 2026-1-2\n- [ ] impossible 📅 2026-02-30\n- [ ] glued 📅2026-11-12\n- [ ] empty [due:: ]\n- [ ] late 📅 later 📆 2026-11-12\n- [ ] twice 📅 2026-11-12 📅 2026-11-13\n- [ ] ✅ 2026-11-01 done first ❌ nope\n",
        // Code, comments and math around the markers; a second line; a block id.
        "- [ ] code `📅 2026-01-01` real 📅 2026-11-12\n- [ ] comment %%📅 2026-01-02%% after\n- [ ] math $📅$ ⏫\n- [ ] first line 📅 2026-11-12\n  second line ⏳ 2026-11-13\n- [ ] anchored 📅 2026-11-12 ^t1\n- [ ] links [[Ana]] and #tag ⏬\n",
        // A daily note's task, a line field, a task under a heading with the same words twice.
        "# Day\n- [ ] same words\n- [ ] same words\n  - [ ] same words\ndue:: 2026-11-12\n- [ ] after a field\n",
        // Line breaks and tabs.
        "- [ ] crlf 📅 2026-11-12 #task\r\n- [x] crlf done ✅ 2026-11-11\r\n",
        "-\t[ ] tab marker 📅 2026-11-12\n*   [ ] wide marker\n+ [ ] plus\n10. [ ] ten\n",
    ];

    /// User types beside the task: one that extends it, and one on a list item that reads the
    /// checkbox and a marker itself, as types written before attributes do.
    const TYPES_ON_TASKS: &str = r#"
[idea]
extends = "task"
on = "list-item"
where.tag = ["task"]
match = '^\[[^\n]\]\s'
fields.status = { checkbox = true }
fields.text = { rest = true }
fields.scheduled = { marker = "⏳", type = "date" }

[chore]
on = "list-item"
priority = 2
where.tag = ["tag"]
fields.state = { checkbox = true }
fields.when = { marker = "📅", type = "date" }
fields.what = { rest = true }
"#;

    /// Settings with the task as it is defined now, or as it was before it read attributes.
    fn with_task(before: bool, flavour: &str, plugin: &serde_json::Value, more: &str) -> Settings {
        let mut layers = vec![match before {
            true => types::Layer::built_in_before_attributes(),
            false => types::Layer::built_in(),
        }];
        if !more.is_empty() {
            let table: toml::Table = toml::from_str(more).unwrap();
            layers.push(types::Layer::config(Path::new("config.toml"), table));
        }
        let found = types::load(layers);
        assert!(found.errors.is_empty(), "{:?}", found.errors);
        Settings::new(
            Flavour::built_in(flavour).unwrap(),
            TasksSettings::from_json(plugin),
            daily::Settings::load(Path::new("/nonexistent-cold-lava-workspace")),
            templates::Settings::default(),
            found,
        )
    }

    /// The first items that differ between two readings of one note.
    fn unlike(what: &str, now: &[Item], then: &[Item]) -> Option<String> {
        if now == then {
            return None;
        }
        let short = |i: Option<&Item>| format!("{i:?}").chars().take(600).collect::<String>();
        let mut out = format!("{what}: {} items now, {} before", now.len(), then.len());
        let n = now.len().max(then.len());
        for k in (0..n).filter(|&k| now.get(k) != then.get(k)).take(2) {
            out += &format!(
                "\n  now:  {}\n  then: {}",
                short(now.get(k)),
                short(then.get(k))
            );
        }
        Some(out)
    }

    /// The task found by its attributes is the task found by Markdown's characters (#242):
    /// every item of every note is equal, whole (id, rev, lines, fields, spans, and where each
    /// field was read), with the definition as it is and as it was. Over the example workspace
    /// with its own types, every Markdown file of the repository, the tricky notes and the
    /// tricky tasks; in each flavour; with the Tasks plugin's settings unset, with a global
    /// filter, with a filter that is also a status character, and with custom statuses in the
    /// Dataview format; with the built-in types alone, with types on every kind of block, and
    /// with user types that extend the task or read a checkbox themselves. And on a canvas
    /// card, whose text goes through the same reading.
    #[test]
    fn the_task_reads_from_attributes_as_it_did_from_characters() {
        let mut differ = Vec::new();
        let mut compared = 0;
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let of_workspace = |before: bool| {
            let built_in = match before {
                true => types::Layer::built_in_before_attributes(),
                false => types::Layer::built_in(),
            };
            let layers = std::iter::once(built_in).chain(types::Layer::workspace(&workspace));
            Settings::load_with(&workspace, None, &[], types::load(layers))
        };
        let (now, then) = (of_workspace(false), of_workspace(true));
        assert!(now.matcher.names().any(|n| n == "volcano"));
        for e in walkdir::WalkDir::new(&workspace).into_iter().flatten() {
            let p = e.path();
            let name = p.strip_prefix(&workspace).unwrap().to_string_lossy();
            if p.extension().is_some_and(|x| x == "md") {
                let text = std::fs::read_to_string(p).unwrap();
                let (a, b) = (extract(&name, &text, &now), extract(&name, &text, &then));
                differ.extend(unlike(&name, &a, &b));
                compared += a.len();
            } else if p.extension().is_some_and(|x| x == "canvas") {
                let text = std::fs::read_to_string(p).unwrap();
                let given = crate::owner::canvas::read(&text).unwrap();
                let read = |s: &Settings| crate::owner::given::read(&name, &given, s).items;
                let (a, b) = (read(&now), read(&then));
                differ.extend(unlike(&name, &a, &b));
                compared += a.len();
            }
        }
        assert!(compared > 30, "the example workspace was read: {compared}");

        let corpus = cst::corpus();
        let plugins = [
            json!({}),
            json!({"globalFilter": "#task"}),
            json!({"globalFilter": "x"}),
            json!({"globalFilter": "] "}),
            json!({"taskFormat": "dataview", "statusSettings": {"customStatuses": [
                {"symbol": "?", "name": "Question", "nextStatusSymbol": "!", "type": "TODO"},
                {"symbol": "!", "name": "Urgent", "nextStatusSymbol": "x", "type": "IN_PROGRESS"},
                {"symbol": "é", "name": "Accent", "nextStatusSymbol": " ", "type": "NON_TASK"},
            ]}}),
        ];
        let mut tasks = 0;
        for flavour in ["obsidian", "github", "commonmark"] {
            for plugin in &plugins {
                for more in ["", EVERY_TYPE, TYPES_ON_TASKS] {
                    let now = with_task(false, flavour, plugin, more);
                    let then = with_task(true, flavour, plugin, more);
                    let mut read = |name: &str, text: &str| {
                        let (a, b) = (extract(name, text, &now), extract(name, text, &then));
                        let what = format!("{name} ({flavour}, {plugin}, {} types)", more.len());
                        differ.extend(unlike(&what, &a, &b));
                        tasks += a.iter().filter(|i| i.data.answers_to("task")).count();
                    };
                    for (k, text) in TRICKY_TASKS.iter().enumerate() {
                        read(&format!("2026-11-{:02}.md", k + 1), text);
                    }
                    for (k, text) in TRICKY.iter().enumerate() {
                        read(&format!("Tricky {k}.md"), text);
                    }
                    // The whole repository once per flavour and plugin setting is enough.
                    if more.is_empty() {
                        for (name, text) in &corpus {
                            read(name, text);
                        }
                    }
                    let card = TRICKY_TASKS.concat();
                    let canvas = json!({"nodes": [
                        {"id": "a", "type": "text", "text": card, "x": 0, "y": 0, "width": 9, "height": 9},
                        {"id": "a", "type": "text", "text": TRICKY_TASKS[0], "x": 0, "y": 9, "width": 9, "height": 9},
                    ], "edges": []});
                    let given = crate::owner::canvas::read(&canvas.to_string()).unwrap();
                    let cards = |s: &Settings| crate::owner::given::read("B.canvas", &given, s);
                    let (a, b) = (cards(&now).items, cards(&then).items);
                    differ.extend(unlike(&format!("a card ({flavour}, {plugin})"), &a, &b));
                    tasks += a.iter().filter(|i| i.data.answers_to("task")).count();
                }
            }
        }
        assert!(tasks > 500, "the tricky notes hold tasks: {tasks}");
        assert!(
            differ.is_empty(),
            "{} read differently:\n{}",
            differ.len(),
            differ.join("\n")
        );
    }

    /// What the tricky tasks are, pinned for a few of them, so the comparison above is not two
    /// readings that agree on nothing being a task.
    #[test]
    fn the_tricky_tasks_read_as_the_tasks_plugin_reads_them() {
        let read = |plugin: serde_json::Value, text: &str| {
            let s = with_task(false, "obsidian", &plugin, "");
            extract("Note.md", text, &s)
                .iter()
                .map(|i| match &i.data {
                    Data::Typed(t) if t.kind == "task" => format!(
                        "[{}] {} due={} priority={}",
                        field(i, "status"),
                        field(i, "text"),
                        field(i, "due"),
                        field(i, "priority")
                    ),
                    d => d.kind().to_string(),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            read(json!({}), TRICKY_TASKS[0]),
            [
                "[ ] both #task due=2026-11-20 priority=",
                "[ ] round due=2026-11-23 priority="
            ],
            "the Dataview field wins, and every form leaves the words"
        );
        assert_eq!(
            read(json!({}), TRICKY_TASKS[1]),
            [
                "[ ] many #task due= priority=lowest",
                "[ ] field wins? due= priority=high",
                "[ ] in code `⏫` due= priority=low",
                "[ ] no such due= priority="
            ],
            "of several glyphs the lowest counts, a glyph before a field, never in code"
        );
        assert_eq!(
            read(json!({"globalFilter": "#task"}), TRICKY_TASKS[2]),
            [
                "[ ] first due= priority=",
                "[ ] last due= priority=",
                "[ ] twice due= priority=",
                "list",
                "[ ] glued due= priority=",
                "[ ] s longer due= priority=",
                "[#] task due= priority=",
                "list",
                "list"
            ],
            "with a global filter, only the items that carry it, without it in their words"
        );
        assert_eq!(
            read(
                json!({"globalFilter": "x"}),
                "- [x] buy\n- [x] buy x\n- [ ] x\n- x [ ] before\n"
            ),
            ["list", "[x] buy due= priority=", "list", "list"],
            "the filter counts after the checkbox, never in it or before it, and is no word"
        );
        assert_eq!(
            read(
                json!({"globalFilter": "#task"}),
                "- [ ] #task\n- [ ] #task 📅 2026-11-12\n- #task [ ] before\n"
            ),
            ["list", "list", "list"],
            "the filter and no other words is no task"
        );
        assert_eq!(
            read(json!({}), TRICKY_TASKS[5]),
            [
                "list",
                "list",
                "list",
                "[ ] #task due= priority=",
                "list",
                "list",
                "list"
            ],
            "a checkbox with no words is no task"
        );
    }

    /// A Markdown link names a note where the tree reads a destination, as the index counts
    /// it: `[a](B.md and more)` was a link to `B` here and none there.
    #[test]
    fn a_link_names_a_note_where_the_tree_reads_its_destination() {
        let text = "[a](B.md and more) [b](C.md \"title\") [c](<D E.md>) [d]( F.md ) [e](G.md#h)\n";
        let all = extract("Note.md", text, &every_type("obsidian"));
        let page = all.iter().find(|i| i.data.kind() == "page").unwrap();
        let Data::Typed(page) = &page.data else {
            panic!("{page:?}")
        };
        assert_eq!(
            page.fields.get("links"),
            Some(&json!(["C", "D E", "F", "G"]))
        );
    }

    /// pulldown-cmark read it as a rule, then Markdown, so its YAML could hold items.
    #[test]
    fn a_frontmatter_with_a_blank_first_line_is_frontmatter() {
        let text = "---\n\nkanban-plugin: basic\n- [ ] YAML\n\n---\n\n## To do\n\n- [ ] Pack\n";
        assert_eq!(
            summary(&items(text)),
            [s("heading", 8, 8, "To do"), s("task", 10, 10, "[ ] Pack")]
        );
    }

    /// What reading every note's items costs, pulldown-cmark's pass against the tree's walk,
    /// with the workspace's types and with none (the pass and the ids alone). Run on
    /// `scripts/bench-items.py`'s workspace: `COLD_LAVA_BENCH_WS=DIR cargo test --release -p
    /// cold-lava-engine items_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn items_cost() {
        use std::time::{Duration, Instant};
        let dir = std::env::var("COLD_LAVA_BENCH_WS").expect("COLD_LAVA_BENCH_WS");
        let root = Path::new(&dir);
        let none = Configured::default();
        let typed = Settings::load(root, &none);
        let plain = Settings::load_with(root, None, &[], types::load(Vec::<types::Layer>::new()));
        let notes: Vec<(String, String)> = walkdir::WalkDir::new(root)
            .into_iter()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
            .filter_map(|e| {
                let name = e.path().strip_prefix(root).ok()?;
                let name = name.to_string_lossy().to_string();
                Some((name, std::fs::read_to_string(e.path()).ok()?))
            })
            .collect();
        let bytes: usize = notes.iter().map(|n| n.1.len()).sum();
        type Read = fn(&str, &str, &Settings) -> Vec<Item>;
        let sides: [(&str, Read, &Settings); 4] = [
            ("pulldown-cmark, with the types", pulldown::extract, &typed),
            ("the tree, with the types", extract, &typed),
            ("pulldown-cmark, no types", pulldown::extract, &plain),
            ("the tree, no types", extract, &plain),
        ];
        let mut runs: Vec<Vec<Duration>> = vec![Vec::new(); sides.len()];
        let mut items = vec![0usize; sides.len()];
        // The sides take turns, so drift and caches favour none.
        for _ in 0..9 {
            for (k, (_, read, s)) in sides.iter().enumerate() {
                let t = Instant::now();
                items[k] = notes
                    .iter()
                    .map(|(name, text)| read(name, text, s).len())
                    .sum();
                runs[k].push(t.elapsed());
            }
        }
        println!(
            "items_cost: {} notes, {} KiB; median (min-max) of 9:",
            notes.len(),
            bytes / 1024
        );
        for (k, (what, ..)) in sides.iter().enumerate() {
            runs[k].sort();
            println!(
                "  {what}: {:?} ({:?}-{:?}), {} items",
                runs[k][4], runs[k][0], runs[k][8], items[k]
            );
        }
    }
}
