//! The agenda as a list (docs/extensions.md, "The agenda, as built"): what the built-in agenda
//! extension will send. Every item that answers to `task` (subtypes too), filed by the date
//! `cold-lava agenda` files it under (due, else scheduled, else start, else its daily note's):
//! overdue, today, each of the next seven days, later, and undated last.

use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::{Datelike, Days, NaiveDate};
use regex::Regex;
use serde_json::Value as Json;

use crate::items::{Data, Item};
use crate::list::{Check, Group, KeyCommand, List, Piece, Row};
use crate::tasks::{StatusType, Task};
use crate::type_table::At;
use crate::types::{Colour, Definition, Look};
use crate::vault::Vault;

/// The command `d` sends: show or hide done and cancelled tasks.
pub const SHOW_DONE: &str = "show-done";

/// Days after today that get a group of their own.
const WEEK: u64 = 7;

pub struct Agenda {
    pub list: List,
    pub at: HashMap<String, At>,
}

/// Which group a task is in; groups show in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
    Overdue,
    Day(NaiveDate),
    Later,
    Undated,
}

impl Bucket {
    fn of(date: Option<NaiveDate>, today: NaiveDate) -> Bucket {
        match date {
            None => Bucket::Undated,
            Some(d) if d < today => Bucket::Overdue,
            Some(d) if d <= today + Days::new(WEEK) => Bucket::Day(d),
            Some(_) => Bucket::Later,
        }
    }

    fn id(self) -> String {
        match self {
            Bucket::Overdue => "overdue".into(),
            Bucket::Day(d) => d.to_string(),
            Bucket::Later => "later".into(),
            Bucket::Undated => "undated".into(),
        }
    }

    fn title(self, today: NaiveDate, show_done: bool) -> String {
        match self {
            // Done tasks from before today are no longer overdue.
            Bucket::Overdue if show_done => "Earlier".into(),
            Bucket::Overdue => "Overdue".into(),
            Bucket::Day(d) if d == today => format!("Today · {}", day(d, today)),
            Bucket::Day(d) if d == today + Days::new(1) => format!("Tomorrow · {}", day(d, today)),
            Bucket::Day(d) => day(d, today),
            Bucket::Later => "Later".into(),
            Bucket::Undated => "Undated".into(),
        }
    }

    /// Whether its rows have different dates, so each shows its own.
    fn dated_rows(self) -> bool {
        matches!(self, Bucket::Overdue | Bucket::Later)
    }
}

/// `Sat 7 Nov`, with the year when it isn't this one.
fn day(d: NaiveDate, today: NaiveDate) -> String {
    match d.year() == today.year() {
        true => d.format("%a %-d %b").to_string(),
        false => d.format("%a %-d %b %Y").to_string(),
    }
}

/// The agenda on `today`: open tasks, tasks closed today (so a tick doesn't make one vanish),
/// and with `show_done` every closed task.
pub fn build(vault: &Vault, today: NaiveDate, show_done: bool) -> Agenda {
    let settings = vault.item_settings();
    let defs: HashMap<&str, &Definition> = (settings.types.types.iter())
        .map(|t| (t.name.as_str(), &t.definition))
        .collect();
    let mut found: Vec<(Bucket, Option<NaiveDate>, &str, usize, Row)> = Vec::new();
    let mut at = HashMap::new();
    for note in &vault.notes {
        let lines: Vec<&str> = note.text.lines().collect();
        for item in vault.items(note) {
            let raw = lines.get(item.line - 1).copied().unwrap_or_default();
            let Some(task) = Task::of(item, raw, &settings.tasks) else {
                continue;
            };
            let check = match task.status_type {
                StatusType::Todo => Check::Unchecked,
                StatusType::InProgress => Check::Partial,
                StatusType::Done => Check::Checked,
                StatusType::Cancelled => Check::Crossed,
                StatusType::NonTask => continue,
            };
            let closed_today = task.done == Some(today) || task.cancelled == Some(today);
            let open = matches!(check, Check::Unchecked | Check::Partial);
            if !open && !closed_today && !show_done {
                continue;
            }
            let date = task.agenda_date().map(|d| d.0);
            let bucket = Bucket::of(date, today);
            let mut right = Vec::new();
            if let (true, Some(d)) = (bucket.dated_rows(), date) {
                let late = Look {
                    colour: Some(Colour::Red),
                    ..Look::default()
                };
                right.push(Piece {
                    text: day(d, today),
                    look: (open && d < today).then_some(late),
                });
            }
            right.push(Piece::plain(note.name.clone()));
            let row = Row {
                id: item.id.clone(),
                text: text(item, &task, &defs),
                check: Some(check),
                right,
                depth: 0,
                look: style(item, &defs, "status_type").filter(|l| l.whole),
            };
            at.insert(
                item.id.clone(),
                At {
                    note: item.note.clone(),
                    line: item.line,
                    rev: item.rev.clone(),
                },
            );
            found.push((bucket, date, item.note.as_str(), item.line, row));
        }
    }
    found.sort_by(|a, b| (a.0, a.1, a.2, a.3).cmp(&(b.0, b.1, b.2, b.3)));
    let mut groups: Vec<Group> = Vec::new();
    let mut last = None;
    for (bucket, _, _, _, row) in found {
        if last != Some(bucket) {
            groups.push(Group {
                id: bucket.id(),
                title: bucket.title(today, show_done),
                collapsed: bucket == Bucket::Undated,
                rows: Vec::new(),
            });
            last = Some(bucket);
        }
        groups.last_mut().expect("pushed above").rows.push(row);
    }
    let title = match show_done {
        true => "Agenda · with done tasks",
        false => "Agenda",
    };
    let list = List {
        title: title.into(),
        groups,
        keys: vec![KeyCommand {
            key: 'd',
            command: SHOW_DONE.into(),
            title: match show_done {
                true => "hide done".into(),
                false => "show done".into(),
            },
        }],
        empty: Some("no tasks: write one as - [ ] in any note".into()),
    };
    Agenda { list, at }
}

/// The task's words with links as their text, then its priority on a chip and 🔁 when it
/// recurs.
fn text(item: &Item, task: &Task, defs: &HashMap<&str, &Definition>) -> Vec<Piece> {
    let mut out = vec![Piece::plain(links_as_text(&task.text))];
    if task.recurrence.is_some() {
        out.push(Piece {
            text: "🔁".into(),
            look: Some(Look {
                colour: Some(Colour::Muted),
                ..Look::default()
            }),
        });
    }
    if let Some(p) = &task.priority {
        let glyph = match p.as_str() {
            "highest" => "🔺",
            "high" => "⏫",
            "medium" => "🔼",
            "low" => "🔽",
            "lowest" => "⏬",
            other => other,
        };
        out.push(Piece {
            text: glyph.into(),
            look: style(item, defs, "priority"),
        });
    }
    out
}

/// The look the item's type (or the type it extends) gives `field`, for its value.
fn style(item: &Item, defs: &HashMap<&str, &Definition>, field: &str) -> Option<Look> {
    let Data::Typed(t) = &item.data else {
        return None;
    };
    let look = (std::iter::once(&t.kind).chain(&t.is))
        .filter_map(|name| defs.get(name.as_str()))
        .find_map(|d| d.style.get(field))?;
    crate::looks::for_value(look, t.fields.get(field).unwrap_or(&Json::Null))
}

/// `[[Note|alias]]` as `alias`, `[[Note#Heading]]` as `Note#Heading`, `[text](url)` as `text`.
pub fn links_as_text(s: &str) -> String {
    static RE: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (wiki, md) = RE.get_or_init(|| {
        (
            Regex::new(r"!?\[\[([^\]|]*)(?:\|([^\]]*))?\]\]").unwrap(),
            Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").unwrap(),
        )
    });
    let s = wiki.replace_all(s, |c: &regex::Captures| {
        c.get(2)
            .unwrap_or_else(|| c.get(1).unwrap())
            .as_str()
            .to_string()
    });
    md.replace_all(&s, "$1").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn vault(name: &str, notes: &[(&str, &str)]) -> (PathBuf, Vault) {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-agenda-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(
            dir.join(".obsidian/daily-notes.json"),
            r#"{"folder":"Journal","format":"YYYY-MM-DD"}"#,
        )
        .unwrap();
        for (path, text) in notes {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let v = crate::vault::scan(&dir).unwrap();
        (dir, v)
    }

    /// Each group's title and its rows' text and right side.
    fn shown(a: &Agenda) -> Vec<(String, Vec<String>)> {
        (a.list.groups.iter())
            .map(|g| {
                let rows = (g.rows.iter())
                    .map(|r| {
                        let t = |p: &[Piece]| {
                            p.iter()
                                .map(|p| p.text.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        format!("{} | {}", t(&r.text), t(&r.right))
                    })
                    .collect();
                (g.title.clone(), rows)
            })
            .collect()
    }

    #[test]
    fn files_tasks_by_their_agenda_date_around_a_fixed_today() {
        let (dir, v) = vault(
            "group",
            &[
                (
                    "Plan.md",
                    "- [ ] Pay rent 📅 2026-11-07\n\
                     - [ ] Ring [[Kīlauea|the observatory]] ⏫ ⏳ 2026-11-08\n\
                     - [/] Paint 🛫 2026-11-14\n\
                     - [ ] Climb Fuji 📅 2026-11-15\n\
                     - [ ] Sort photos\n\
                     - [ ] Late one 📅 2026-11-01\n\
                     - [x] Done before ✅ 2026-11-02 📅 2026-11-01\n\
                     - [x] Done today ✅ 2026-11-07 📅 2026-11-03\n\
                     - [-] Dropped today ❌ 2026-11-07\n\
                     - [ ] Next year 📅 2027-01-04\n",
                ),
                ("Journal/2026-11-05.md", "- [ ] From the journal\n"),
            ],
        );
        let a = build(&v, d("2026-11-07"), false);
        assert_eq!(
            shown(&a),
            [
                (
                    "Overdue".into(),
                    vec![
                        "Late one | Sun 1 Nov Plan".into(),
                        "Done today | Tue 3 Nov Plan".into(),
                        "From the journal | Thu 5 Nov 2026-11-05".into(),
                    ]
                ),
                ("Today · Sat 7 Nov".into(), vec!["Pay rent | Plan".into()]),
                (
                    "Tomorrow · Sun 8 Nov".into(),
                    vec!["Ring the observatory ⏫ | Plan".into()]
                ),
                ("Sat 14 Nov".into(), vec!["Paint | Plan".into()]),
                (
                    "Later".into(),
                    vec![
                        "Climb Fuji | Sun 15 Nov Plan".into(),
                        "Next year | Mon 4 Jan 2027 Plan".into(),
                    ]
                ),
                (
                    "Undated".into(),
                    vec!["Sort photos | Plan".into(), "Dropped today | Plan".into()]
                ),
            ]
        );
        let groups = &a.list.groups;
        assert!(groups.last().unwrap().collapsed, "Undated starts folded");
        let late = &groups[0].rows[0];
        assert_eq!(late.check, Some(Check::Unchecked));
        assert_eq!(
            late.right[0].look.as_ref().unwrap().colour,
            Some(Colour::Red)
        );
        let done = &groups[0].rows[1];
        assert_eq!(done.check, Some(Check::Checked));
        assert!(
            done.look.as_ref().is_some_and(|l| l.strike),
            "drawn as done"
        );
        assert!(done.right[0].look.is_none(), "a done task isn't late");
        let chip = &groups[2].rows[0].text[1];
        assert!(
            chip.look
                .as_ref()
                .is_some_and(|l| l.badge && l.colour == Some(Colour::Orange))
        );
        assert_eq!(groups[3].rows[0].check, Some(Check::Partial));
        let at = &a.at[&groups[1].rows[0].id];
        assert_eq!((at.note.as_str(), at.line), ("Plan.md", 1));

        let all = build(&v, d("2026-11-07"), true);
        assert_eq!(all.list.groups[0].title, "Earlier");
        assert_eq!(
            all.list.groups[0].rows.len(),
            4,
            "the one done before shows too"
        );
        assert_eq!(all.list.keys[0].title, "hide done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_subtype_of_task_is_in_the_agenda() {
        let (dir, v) = vault(
            "subtype",
            &[
                (
                    ".cold-lava/types.toml",
                    r#"version = 1
                    [types.idea]
                    extends = "task"
                    on = "list-item"
                    where.tag = ["idea"]
                    match = '^\[[^\n]\]\s'
                    fields.status = { checkbox = true }
                    fields.text = { rest = true }
                    fields.due = { marker = "📅", type = "date" }
                    "#,
                ),
                ("Ideas.md", "- [ ] Glass from sand #idea 📅 2026-11-07\n"),
            ],
        );
        assert_eq!(v.item_settings().types.errors, Vec::<String>::new());
        let a = build(&v, d("2026-11-07"), false);
        assert_eq!(
            shown(&a),
            [(
                "Today · Sat 7 Nov".into(),
                vec!["Glass from sand #idea | Ideas".into()]
            )]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_read_as_their_text() {
        assert_eq!(
            links_as_text("See [[Mauna Loa]], [[Lava|flows]] and [docs](https://x.org)"),
            "See Mauna Loa, flows and docs"
        );
    }
}
