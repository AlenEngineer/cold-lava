//! Shadow mode for the task as a definition (docs/extensions.md, "Order of work"): every note is
//! read twice, once with tasks read by `tasks::parse_line` (what the engine uses) and once with
//! the built-in `task` definition (`types/task.toml`), and the two are compared field by field.
//! `cold-lava types VAULT --shadow` prints it. Nothing here writes. This module goes when the
//! definition takes over.

use crate::items::{self, Data, Item, Settings};
use crate::tasks::Task;
use crate::types;
use crate::vault::Vault;
use serde::Serialize;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;
use std::path::Path;

/// What the two readings of a vault found.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub notes: usize,
    /// Tasks `tasks::parse_line` found.
    pub tasks: usize,
    /// Of those, the ones the definition read as a subtype (`extends = "task"`), which are a
    /// kind of their own and so have ids of their own.
    pub subtyped: usize,
    pub differences: Vec<Difference>,
    /// What was wrong loading the types with the definition, the built-in one included.
    pub errors: Vec<String>,
}

/// One field that reads differently.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Difference {
    pub note: String,
    pub line: usize,
    /// A task's field, `id`, `rev`, … or `kind` when only one side reads a task there, `item`
    /// when an item that is no task differs, `missing` / `extra` when only one side has an
    /// item on that line.
    pub field: String,
    pub native: Json,
    pub definition: Json,
}

impl Report {
    /// How many differences each field has.
    pub fn by_field(&self) -> BTreeMap<&str, usize> {
        let mut out = BTreeMap::new();
        for d in &self.differences {
            *out.entry(d.field.as_str()).or_default() += 1;
        }
        out
    }
}

/// The vault's settings read both ways: native tasks, and the built-in definition.
pub fn settings(root: &Path) -> (Settings, Settings) {
    (
        Settings::load(root),
        Settings::load_with(root, types::with_built_in(root)),
    )
}

/// Reads every note of the vault both ways.
pub fn compare(vault: &Vault) -> Report {
    let (native, defined) = settings(&vault.root);
    let mut report = Report {
        errors: defined.types.errors.clone(),
        ..Report::default()
    };
    for note in &vault.notes {
        compare_note(
            &format!("{}.md", note.rel),
            &note.text,
            &native,
            &defined,
            &mut report,
        );
    }
    report
}

/// Reads one note both ways and adds what differs to `report`.
pub fn compare_note(
    note: &str,
    text: &str,
    native: &Settings,
    defined: &Settings,
    report: &mut Report,
) {
    report.notes += 1;
    let a = items::extract(note, text, native);
    let b = items::extract(note, text, defined);
    let diff = |report: &mut Report, line: usize, field: &str, native: Json, definition: Json| {
        report.differences.push(Difference {
            note: note.to_string(),
            line,
            field: field.to_string(),
            native,
            definition,
        })
    };
    // Items come in the order of their lines, and on one line in the same order both ways.
    let mut b_left: Vec<Option<&Item>> = b.iter().map(Some).collect();
    for x in &a {
        let pair = b_left
            .iter_mut()
            .find(|y| y.is_some_and(|y| y.line == x.line))
            .and_then(Option::take);
        let is_task = matches!(x.data, Data::Task(_));
        report.tasks += is_task as usize;
        let Some(y) = pair else {
            diff(report, x.line, "missing", json!(x.data.kind()), Json::Null);
            continue;
        };
        match (&x.data, &y.data) {
            (Data::Task(t), Data::Typed(typed)) if typed.kind == "task" => {
                for (field, n, d) in item_fields(x, y) {
                    diff(report, x.line, field, n, d);
                }
                let expected = task_fields(t);
                for (field, n) in &expected {
                    let d = typed.fields.get(*field).cloned().unwrap_or(Json::Null);
                    if *n != d {
                        diff(report, x.line, field, n.clone(), d);
                    }
                }
            }
            (Data::Task(_), d) if d.answers_to("task") => report.subtyped += 1,
            (Data::Task(_), d) => diff(report, x.line, "kind", json!("task"), json!(d.kind())),
            (n, Data::Typed(t)) if t.kind == "task" => {
                diff(report, x.line, "kind", json!(n.kind()), json!("task"))
            }
            _ if x != y => diff(
                report,
                x.line,
                "item",
                serde_json::to_value(x).unwrap_or_default(),
                serde_json::to_value(y).unwrap_or_default(),
            ),
            _ => {}
        }
    }
    for y in b_left.into_iter().flatten() {
        diff(report, y.line, "extra", Json::Null, json!(y.data.kind()));
    }
}

/// What every item has, where the two differ.
fn item_fields(x: &Item, y: &Item) -> Vec<(&'static str, Json, Json)> {
    let pairs = [
        ("id", json!(x.id), json!(y.id)),
        ("rev", json!(x.rev), json!(y.rev)),
        ("end_line", json!(x.end_line), json!(y.end_line)),
        ("section", json!(x.section), json!(y.section)),
        ("parent", json!(x.parent), json!(y.parent)),
        ("block_id", json!(x.block_id), json!(y.block_id)),
    ];
    pairs.into_iter().filter(|(_, a, b)| a != b).collect()
}

/// A natively read task's fields as the definition's schema writes them: the status type by the
/// name `agenda --json` uses, tags without their `#`.
pub fn task_fields(t: &Task) -> Vec<(&'static str, Json)> {
    let date = |d: Option<chrono::NaiveDate>| json!(d.map(|d| d.to_string()));
    vec![
        ("status", json!(t.status.to_string())),
        ("status_type", json!(t.status_type.agenda_name())),
        ("text", json!(t.text)),
        ("due", date(t.due)),
        ("scheduled", date(t.scheduled)),
        ("start", date(t.start)),
        ("created", date(t.created)),
        ("done", date(t.done)),
        ("cancelled", date(t.cancelled)),
        ("recurrence", json!(t.recurrence)),
        (
            "priority",
            json!(
                t.priority
                    .map(|p| serde_json::to_value(p).unwrap_or_default())
            ),
        ),
        (
            "tags",
            json!(
                t.tags
                    .iter()
                    .map(|t| t.trim_start_matches('#'))
                    .collect::<Vec<_>>()
            ),
        ),
        ("links", json!(t.links)),
        ("daily_note", date(t.daily_note)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Flavour;
    use crate::tasks::TasksSettings;
    use crate::types::Layer;
    use crate::{daily, templates};

    /// Every way the Tasks plugin writes a task, and the places a task can and can't be.
    pub const CORPUS: &str = r#"---
tags: [plan]
---
# Emoji format
- [ ] Pay rent 📅 2026-11-12
- [ ] All dates 🛫 2026-11-01 ⏳ 2026-11-05 📅 2026-11-12 ➕ 2026-10-30
- [x] Done one 📅 2026-11-01 ✅ 2026-11-02
- [-] Dropped one ❌ 2026-11-03
- [/] Going 📆 2026-11-20 and 🗓 later
- [ ] Scheduled the other way ⌛ 2026-11-06
- [ ] No space📅2026-11-07
- [ ] Variation selectors ⏳️ 2026-11-05 ✅️ 2026-11-06
- [ ] Highest 🔺
- [ ] High ⏫ 📅 2026-11-12
- [ ] Medium 🔼
- [ ] Low 🔽
- [ ] Lowest ⏬
- [ ] Two priorities ⏫ 🔽
- [ ] Weekly 🔁 every week 📅 2026-11-12
- [ ] Monthly 🔁 every month on the 1st when done
- [ ] Comma 🔁 every day, 📅 2026-11-12
- [ ] Recurring then a tag 🔁 every 2 weeks #home
* [ ] Star marker 📅 2026-11-12
+ [ ] Plus marker
1. [ ] Numbered 📅 2026-11-12
2) [x] Numbered with a paren
- [ ] Tags #home #work/deep and #under_score
- [ ] Links [[Lava]] and [[Ash|grey ash]] and [md](Rock.md)
- [ ] Block id 📅 2026-11-12 ^rent
- [ ] Code in it `x = 1` stays
- [ ] 📅 2026-11-12
- [ ] ⏫ 🔁 every day
- [ ] Invalid date 📅 2026-13-45
- [ ] Ids and dependencies 🆔 abc123 ⛔ def456 🏁 delete
- [ ]
- [ ]
- [?] Unknown status
- [>] Forwarded

## Dataview format
- [ ] Pay rent [due:: 2026-11-12]
- [ ] All [start:: 2026-11-01] [scheduled:: 2026-11-05] [created:: 2026-10-30]
- [x] Done [completion:: 2026-11-02]
- [-] Cancelled (cancelled:: 2026-11-03)
- [ ] Priority [priority:: high]
- [ ] Repeats [repeat:: every week] [due:: 2026-11-12]
- [ ] Other fields [id:: abc] [estimate:: 2h]
- [ ] Mixed [due:: 2026-11-12] 🔼

## Nested
- [ ] Parent 📅 2026-11-12
  - [ ] Child ⏳ 2026-11-10
    - [x] Grandchild ✅ 2026-11-09
  - a plain list item
- plain parent
  - [ ] Task under a list item
- [ ] Pay rent 📅 2026-12-12
- [ ] Pay rent 📅 2027-01-12
- [ ] Two lines
  with a continuation

## Quotes and callouts
> - [ ] In a quote 📅 2026-11-12
> [!todo] Callout
> - [ ] In a callout ⏫
> > - [/] In a nested quote

## Not tasks
```
- [ ] In a fence 📅 2026-11-12
```
~~~md
- [ ] In a tilde fence
~~~
    - [ ] Indented code
%% - [ ] In a comment %%
- [ ] Commented %%📅 2026-11-12%% words
Not a list [ ] item
- [ ]No space after the checkbox
"#;

    fn both(json: &Json) -> (Settings, Settings) {
        let make = |types| {
            Settings::new(
                Flavour::built_in("obsidian").unwrap(),
                TasksSettings::from_json(json),
                daily::Settings {
                    folder: "Daily".into(),
                    format: "YYYY-MM-DD".into(),
                    template: None,
                },
                templates::Settings::default(),
                types,
            )
        };
        (
            make(types::load([])),
            make(types::load([Layer::built_in()])),
        )
    }

    fn compared(json: Json, notes: &[(&str, &str)]) -> Report {
        let (native, defined) = both(&json);
        assert_eq!(defined.types.errors, Vec::<String>::new());
        assert!(native.native_tasks && !defined.native_tasks);
        let mut report = Report::default();
        for (note, text) in notes {
            compare_note(note, text, &native, &defined, &mut report);
        }
        report
    }

    fn no_differences(report: &Report) {
        let lines: Vec<String> = report
            .differences
            .iter()
            .map(|d| {
                format!(
                    "{}:{} {}: native {} · definition {}",
                    d.note, d.line, d.field, d.native, d.definition
                )
            })
            .collect();
        assert!(lines.is_empty(), "{}", lines.join("\n"));
    }

    #[test]
    fn the_definition_reads_every_format_as_the_native_parser_does() {
        let report = compared(
            json!({}),
            &[("Plans.md", CORPUS), ("Daily/2026-11-05.md", CORPUS)],
        );
        no_differences(&report);
        assert_eq!(report.tasks, 2 * 49, "the corpus's tasks, in both notes");
    }

    #[test]
    fn with_a_global_filter_and_custom_statuses() {
        let settings = json!({
            "globalFilter": "#task",
            "statusSettings": {
                "coreStatuses": [{"symbol": " ", "type": "TODO"}, {"symbol": "x", "type": "DONE"}],
                "customStatuses": [
                    {"symbol": ">", "type": "IN_PROGRESS"},
                    {"symbol": "?", "type": "NON_TASK"},
                    {"symbol": "!", "type": "TODO"},
                ],
            },
        });
        let note = format!(
            "{CORPUS}\n- [ ] #task Filtered 📅 2026-11-12\n- [!] Urgent #task ⏫\n\
             - [x] Filter at the end ✅ 2026-11-02 #task\n- [ ] #task\n- [>] Forwarded #task\n"
        );
        let report = compared(settings, &[("Plans.md", &note)]);
        no_differences(&report);
        assert_eq!(report.tasks, 4, "only tasks with the filter and words");
    }

    #[test]
    fn the_dataview_setting_reads_the_same() {
        let report = compared(json!({"taskFormat": "dataview"}), &[("Plans.md", CORPUS)]);
        no_differences(&report);
    }

    /// Lines the two read differently, each for a reason docs/extensions.md gives.
    #[test]
    fn known_differences() {
        let lines = [
            "- [ ] Tags #2026 and (#paren)",
            "- [ ] Code `📅 2026-01-01` in it",
            "- [ ] Twice 📅 2026-11-12 📅 2026-11-13",
            "- [ ] Both [due:: 2026-11-12] 📅 2026-11-13",
            "- [ ] Unbracketed due:: 2026-11-12",
            "- [ ] Odd priority [priority:: urgent]",
            "- [ ] Recurs oddly 🔁 every day (or so)",
        ];
        let report = compared(json!({}), &[("Plans.md", &lines.join("\n"))]);
        let found: Vec<(usize, &str)> = report
            .differences
            .iter()
            .filter(|d| d.field != "id")
            .map(|d| (d.line, d.field.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (1, "tags"),
                (2, "text"),
                (2, "due"),
                (3, "text"),
                (3, "due"),
                (4, "text"),
                (5, "text"),
                (5, "due"),
                (6, "priority"),
                (7, "recurrence"),
            ]
        );
    }

    #[test]
    fn a_subtype_of_task_takes_its_tasks_and_still_answers_to_task() {
        let idea: toml::Table = toml::from_str(
            r#"
            [idea]
            extends = "task"
            on = "list-item"
            where.tag = ["idea"]
            match = '^\[[^\n]\]\s'
            fields.text = { rest = true }
            "#,
        )
        .unwrap();
        let (native, _) = both(&json!({}));
        let types = types::load([
            Layer::built_in(),
            Layer::config(Path::new("config.toml"), idea),
        ]);
        assert_eq!(types.errors, Vec::<String>::new());
        let defined = Settings::new(
            Flavour::built_in("obsidian").unwrap(),
            TasksSettings::from_json(&json!({})),
            daily::Settings::default(),
            templates::Settings::default(),
            types,
        );
        let note = "- [ ] Glass from sand #idea\n- [ ] Pay rent 📅 2026-11-12\n";
        let mut report = Report::default();
        compare_note("Ideas.md", note, &native, &defined, &mut report);
        no_differences(&report);
        assert_eq!((report.tasks, report.subtyped), (2, 1));
        let all = items::extract("Ideas.md", note, &defined);
        assert_eq!(all[0].data.kind(), "idea");
        assert!(all[0].data.answers_to("task"));
        assert_eq!(all[1].data.kind(), "task");
    }
}
