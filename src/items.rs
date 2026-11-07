//! Notes as data: the core reads a note into typed items (tasks, list items, headings and
//! blocks with an id), so features ask for them instead of parsing notes themselves. See
//! "Notes as data" in docs/extensions.md.
//!
//! One pass of pulldown-cmark per note, in the vault's flavour, over the same text the reader
//! parses. Tasks are list items whose first line `tasks::parse_line` reads as one.

use crate::daily;
use crate::markdown::{self, Flavour, Syntax};
use crate::tasks::{self, Task, TasksSettings, fnv};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Everything about a vault that changes how its notes read as items.
pub struct Settings {
    pub flavour: Flavour,
    pub tasks: TasksSettings,
    pub daily: daily::Settings,
}

impl Settings {
    pub fn load(root: &Path) -> Settings {
        Settings {
            flavour: markdown::flavour_for(root),
            tasks: TasksSettings::load(root),
            daily: daily::Settings::load(root),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    /// Stable across edits that keep the item's anchor, restarts and processes.
    pub id: String,
    /// Vault-relative path, with `.md`.
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
    /// Fingerprint of the item's own lines: which version of it this is.
    pub rev: String,
    #[serde(flatten)]
    pub data: Data,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Data {
    Task(Task),
    List {
        text: String,
    },
    Heading {
        level: u8,
        text: String,
    },
    /// A paragraph, or the block before a `^id` line, named by its block id.
    Block,
}

pub const KINDS: &[&str] = &["task", "list", "heading", "block"];

impl Data {
    pub fn kind(&self) -> &'static str {
        match self {
            Data::Task(_) => "task",
            Data::List { .. } => "list",
            Data::Heading { .. } => "heading",
            Data::Block => "block",
        }
    }
}

/// An item before its id: an id needs its parent's, so ids are given once the note is read.
struct Raw {
    line: usize,
    end_line: usize,
    section: Vec<String>,
    parent: Option<usize>,
    block_id: Option<String>,
    data: Data,
}

/// A container pulldown-cmark has opened and not yet closed.
struct Open {
    start: usize,
    line: usize,
    /// The `Raw` this container is, for a list item.
    item: Option<usize>,
    /// For a list item: the last line before its first nested list.
    own_end: Option<usize>,
    /// The first line of the last child block that ended.
    prev_child: Option<usize>,
}

/// The items of the note at vault-relative `note` (with `.md`) whose text is `text`.
pub fn extract(note: &str, text: &str, s: &Settings) -> Vec<Item> {
    let flavour = &s.flavour;
    let src = flavour.source(text);
    let newlines: Vec<usize> = src.match_indices('\n').map(|(i, _)| i).collect();
    let line_of = |offset: usize| newlines.partition_point(|&n| n < offset) + 1;
    let src_lines: Vec<&str> = src.lines().collect();
    let src_line = |n: usize| src_lines.get(n - 1).copied().unwrap_or_default();
    let is_blank = |n: usize| src_line(n).trim().trim_start_matches('>').trim().is_empty();
    let daily_note = s.daily.date_of(note);

    let mut raws: Vec<Raw> = Vec::new();
    let mut headings: Vec<(u8, String)> = Vec::new();
    let names = |h: &[(u8, String)]| h.iter().map(|h| h.1.clone()).collect::<Vec<_>>();
    let mut open = vec![Open {
        start: 0,
        line: 1,
        item: None,
        own_end: None,
        prev_child: None,
    }];
    let enclosing = |open: &[Open]| open.iter().rev().find_map(|o| o.item);

    for (event, range) in Parser::new_ext(&src, flavour.options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                let line = line_of(range.start);
                let mut item = None;
                match tag {
                    Tag::Heading { level, .. } => {
                        let level = level as u8;
                        while headings.last().is_some_and(|h| h.0 >= level) {
                            headings.pop();
                        }
                        let text = heading_text(&src[range.clone()]);
                        raws.push(Raw {
                            line,
                            end_line: line_of(range.end.saturating_sub(1)).max(line),
                            section: names(&headings),
                            parent: enclosing(&open),
                            block_id: None,
                            data: Data::Heading {
                                level,
                                text: text.clone(),
                            },
                        });
                        headings.push((level, text));
                    }
                    Tag::Item => {
                        item = Some(raws.len());
                        raws.push(Raw {
                            line,
                            end_line: line,
                            section: names(&headings),
                            parent: enclosing(&open),
                            block_id: None,
                            data: Data::Block,
                        });
                    }
                    Tag::List(_) => {
                        if let Some(top) = open.last_mut()
                            && top.item.is_some()
                            && top.own_end.is_none()
                        {
                            top.own_end = Some(line.saturating_sub(1).max(top.line));
                        }
                    }
                    _ => {}
                }
                open.push(Open {
                    start: range.start,
                    line,
                    item,
                    own_end: None,
                    prev_child: None,
                });
            }
            Event::End(end) => {
                let Some(frame) = open.pop() else { break };
                let Some(parent) = open.last() else { break };
                let (parent_item, parent_prev) = (parent.item, parent.prev_child);
                let mut last = line_of(range.end.saturating_sub(1)).max(frame.line);
                match (end, frame.item) {
                    (TagEnd::Item, Some(at)) => {
                        last = frame.own_end.unwrap_or(last);
                        while last > frame.line && is_blank(last) {
                            last -= 1;
                        }
                        let first = &src[frame.start..];
                        let first = first.lines().next().unwrap_or_default();
                        let (first, block_id) = if flavour.has(Syntax::BlockIds) {
                            let own_last = if last == frame.line {
                                first
                            } else {
                                src_line(last)
                            };
                            match markdown::block_id_pattern().captures(own_last) {
                                Some(c) if last == frame.line => {
                                    (&first[..c.get(0).unwrap().start()], Some(c[1].to_string()))
                                }
                                Some(c) => (first, Some(c[1].to_string())),
                                None => (first, None),
                            }
                        } else {
                            (first, None)
                        };
                        let task = flavour
                            .has(Syntax::TaskLists)
                            .then(|| tasks::parse_line(first, &s.tasks))
                            .flatten();
                        let raw = &mut raws[at];
                        raw.end_line = last;
                        raw.block_id = block_id;
                        raw.data = match task {
                            Some(t) => Data::Task(Task { daily_note, ..t }),
                            None => Data::List {
                                text: list_text(first),
                            },
                        };
                    }
                    (TagEnd::Paragraph, _)
                        if flavour.has(Syntax::BlockIds) && parent_item.is_none() =>
                    {
                        while last > frame.line && is_blank(last) {
                            last -= 1;
                        }
                        if let Some(c) = markdown::block_id_pattern().captures(src_line(last)) {
                            let alone = last == frame.line && c.get(0).unwrap().start() == 0;
                            let line = match parent_prev {
                                Some(prev) if alone => prev,
                                _ => frame.line,
                            };
                            raws.push(Raw {
                                line,
                                end_line: last,
                                section: names(&headings),
                                parent: enclosing(&open),
                                block_id: Some(c[1].to_string()),
                                data: Data::Block,
                            });
                        }
                    }
                    _ => {}
                }
                if let Some(parent) = open.last_mut() {
                    parent.prev_child = Some(frame.line);
                }
            }
            _ => {}
        }
    }
    identify(note, text, raws)
}

/// Gives each item its id and rev, in document order (a parent before its children).
fn identify(note: &str, text: &str, raws: Vec<Raw>) -> Vec<Item> {
    let lines: Vec<&str> = text.lines().collect();
    let mut keys: Vec<String> = Vec::with_capacity(raws.len());
    let mut seen: HashMap<(&'static str, String), usize> = HashMap::new();
    let mut ids: Vec<String> = Vec::with_capacity(raws.len());
    let mut items = Vec::with_capacity(raws.len());
    for raw in raws {
        let kind = raw.data.kind();
        let parent_key = raw.parent.map_or("", |p| keys[p].as_str());
        let anchor = match (&raw.block_id, &raw.data) {
            (Some(id), _) => format!("^{id}"),
            (None, Data::Heading { text, .. }) => {
                let mut path = raw.section.clone();
                path.push(text.clone());
                path.join("#")
            }
            (None, Data::Task(t)) => format!("{parent_key}/{:016x}", fnv(t.text.as_bytes())),
            (None, Data::List { text }) => format!("{parent_key}/{:016x}", fnv(text.as_bytes())),
            (None, Data::Block) => String::new(),
        };
        let n = seen.entry((kind, anchor.clone())).or_default();
        let key = match *n {
            0 => anchor,
            n => format!("{anchor}.{n}"),
        };
        *n += 1;
        let id = format!("{:016x}", fnv(format!("{note}\0{kind}\0{key}").as_bytes()));
        let own = lines
            .get(raw.line - 1..raw.end_line.min(lines.len()))
            .unwrap_or_default();
        items.push(Item {
            id: id.clone(),
            note: note.to_string(),
            line: raw.line,
            end_line: raw.end_line,
            section: raw.section,
            parent: raw.parent.map(|p| ids[p].clone()),
            block_id: raw.block_id,
            rev: tasks::line_id(&own.join("\n")),
            data: raw.data,
        });
        keys.push(key);
        ids.push(id);
    }
    items.sort_by_key(|i| i.line);
    items
}

/// A heading's text as written: ATX without its `#`s, setext without its underline.
fn heading_text(raw: &str) -> String {
    let raw = raw.trim();
    let text = match raw.strip_prefix('#') {
        Some(_) => {
            let t = raw
                .lines()
                .next()
                .unwrap_or_default()
                .trim_start_matches('#');
            let closed = t.trim_end().trim_end_matches('#');
            if closed.is_empty() || closed.ends_with([' ', '\t']) {
                closed.to_string()
            } else {
                t.to_string()
            }
        }
        None => {
            let lines: Vec<&str> = raw.lines().collect();
            lines[..lines.len().saturating_sub(1)].join(" ")
        }
    };
    text.trim().to_string()
}

/// A list item's first line without its marker.
fn list_text(first: &str) -> String {
    let t = first.trim_start();
    let t = t
        .strip_prefix(['-', '*', '+'])
        .unwrap_or_else(|| t.trim_start_matches(|c: char| c.is_ascii_digit()))
        .trim_start_matches(['.', ')']);
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::StatusType;
    use serde_json::json;

    fn settings(flavour: &str) -> Settings {
        let nowhere = Path::new("/nonexistent-cold-lava-vault");
        Settings {
            flavour: Flavour::built_in(flavour).unwrap(),
            tasks: TasksSettings::load(nowhere),
            daily: daily::Settings::load(nowhere),
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
                    Data::Task(t) => format!("[{}] {}", t.status, t.text),
                    Data::List { text } => text.clone(),
                    Data::Heading { text, .. } => text.clone(),
                    Data::Block => format!("^{}", i.block_id.as_deref().unwrap_or("")),
                };
                (i.data.kind(), i.line, i.end_line, text)
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
                Data::Task(t) => t.text == text,
                Data::List { text: t } | Data::Heading { text: t, .. } => t == text,
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
            tasks::line_id("- [ ] real %%with a comment%%"),
            "rev is of the line as written"
        );
    }

    #[test]
    fn custom_statuses_are_tasks() {
        let all = items("- [/] going\n- [-] dropped\n- [?] unsure\n- [ ] \n");
        let types: Vec<_> = all
            .iter()
            .map(|i| match &i.data {
                Data::Task(t) => Some(t.status_type),
                _ => None,
            })
            .collect();
        assert_eq!(
            types,
            [
                Some(StatusType::InProgress),
                Some(StatusType::Cancelled),
                Some(StatusType::NonTask),
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
                .filter(|i| matches!(&i.data, Data::Task(t) if t.text == which))
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
        assert_eq!(all[0].rev, tasks::line_id(line));
        let two = items("- [ ] Pay rent\n  by transfer\n");
        assert_eq!(two[0].rev, tasks::line_id("- [ ] Pay rent\n  by transfer"));
    }

    #[test]
    fn daily_notes_date_their_tasks() {
        let s = settings("obsidian");
        let all = extract("2026-11-05.md", "- [ ] Water plants\n", &s);
        let Data::Task(t) = &all[0].data else {
            panic!()
        };
        assert_eq!(t.daily_note, chrono::NaiveDate::from_ymd_opt(2026, 11, 5));
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
            assert_eq!(&serde_json::from_value::<Item>(json).unwrap(), item);
        }
        assert!(all.iter().all(|i| i.id.len() == 16 && i.rev.len() == 12));
    }
}
