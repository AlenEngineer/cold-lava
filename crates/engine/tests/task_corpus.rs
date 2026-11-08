//! Every way the Tasks plugin writes a task, and the places a task can and can't be
//! (`tests/tasks/corpus.md`), read by the built-in `task` definition. The expected items in
//! `tests/tasks/*.jsonl` are what the native parser read before the switch-over, ids and revs
//! included, but for the lines in `known.md`, where the definition follows the Tasks plugin
//! (docs/extensions.md, "The task as a definition, as built").

use cold_lava_engine::items::{self, Settings};
use cold_lava_engine::markdown::Flavour;
use cold_lava_engine::tasks::TasksSettings;
use cold_lava_engine::{daily, templates, types};
use serde_json::{Value, json};

const CORPUS: &str = include_str!("tasks/corpus.md");

fn settings(tasks: Value) -> Settings {
    let types = types::load([types::Layer::built_in()]);
    assert_eq!(types.errors, Vec::<String>::new());
    Settings::new(
        Flavour::built_in("obsidian").unwrap(),
        TasksSettings::from_json(&tasks),
        daily::Settings {
            folder: "Daily".into(),
            format: "YYYY-MM-DD".into(),
            template: None,
        },
        templates::Settings::default(),
        types,
    )
}

/// The notes' tasks, as `cold-lava items WORKSPACE task --json` writes them, against `expected`.
fn reads(s: &Settings, notes: &[(&str, &str)], expected: &str) {
    let expected: Vec<Value> = expected
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let mut found = Vec::new();
    for (note, text) in notes {
        let all = items::extract(note, text, s);
        found.extend(
            all.iter()
                .filter(|i| i.data.answers_to("task"))
                .map(|i| serde_json::to_value(i).unwrap()),
        );
    }
    let key = |v: &Value| (v["note"].as_str().unwrap().to_string(), v["line"].as_u64());
    let mut expected = expected;
    expected.sort_by_key(key);
    found.sort_by_key(key);
    assert_eq!(found.len(), expected.len());
    for (f, e) in found.iter().zip(&expected) {
        assert_eq!(f, e);
    }
}

#[test]
fn the_definition_reads_every_format() {
    reads(
        &settings(json!({})),
        &[
            ("Plans.md", CORPUS),
            ("Daily/2026-11-05.md", CORPUS),
            ("Known.md", include_str!("tasks/known.md")),
        ],
        include_str!("tasks/plain.jsonl"),
    );
}

#[test]
fn with_a_global_filter_and_custom_statuses() {
    let settings = settings(json!({
        "globalFilter": "#task",
        "statusSettings": {
            "coreStatuses": [{"symbol": " ", "type": "TODO"}, {"symbol": "x", "type": "DONE"}],
            "customStatuses": [
                {"symbol": ">", "type": "IN_PROGRESS"},
                {"symbol": "?", "type": "NON_TASK"},
                {"symbol": "!", "type": "TODO"},
            ],
        },
    }));
    let note = format!("{CORPUS}{}", include_str!("tasks/filtered.md"));
    reads(
        &settings,
        &[("Plans.md", &note)],
        include_str!("tasks/filter.jsonl"),
    );
}

#[test]
fn the_dataview_setting_reads_the_same() {
    reads(
        &settings(json!({"taskFormat": "dataview"})),
        &[("Plans.md", CORPUS)],
        include_str!("tasks/dataview.jsonl"),
    );
}

/// Where the definition reads a line differently from the native parser it replaced, each as
/// the Tasks plugin reads it, or as the workspace reads tags.
#[test]
fn known_differences_follow_the_tasks_plugin() {
    let s = settings(json!({}));
    let text = include_str!("tasks/known.md");
    let all = items::extract("Known.md", text, &s);
    let field = |line: usize, name: &str| {
        let item = all.iter().find(|i| i.line == line).unwrap();
        serde_json::to_value(item).unwrap()[name].clone()
    };
    assert_eq!(field(1, "tags"), json!(["paren"]), "#2026 is no tag");
    assert_eq!(field(2, "due"), Value::Null, "nothing is read in code");
    assert_eq!(field(2, "text"), json!("Code `📅 2026-01-01` in it"));
    assert_eq!(field(3, "due"), json!("2026-11-12"), "the first of two");
    assert_eq!(field(3, "text"), json!("Twice"), "both are taken out");
    assert_eq!(
        field(4, "due"),
        json!("2026-11-12"),
        "Dataview's over the emoji"
    );
    assert_eq!(field(4, "text"), json!("Both"));
    assert_eq!(
        field(5, "due"),
        Value::Null,
        "Dataview reads due:: on a line's start"
    );
    assert_eq!(field(5, "text"), json!("Unbracketed due:: 2026-11-12"));
    assert_eq!(field(6, "priority"), Value::Null, "urgent is no priority");
    assert_eq!(field(6, "text"), json!("Odd priority"));
    assert_eq!(field(7, "recurrence"), json!("every day"));
}
