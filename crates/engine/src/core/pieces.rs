//! Step 3 of the neutral model, end to end (docs/neutral-model.md): an owner of a made-up
//! format that isn't Markdown gives its tasks as pieces, and the workspace has them as `task`s.
//! The owner here is a small real one, played in-process the way an extension answers: it
//! parses its own format, and it never writes or reads a character of Markdown.

use super::*;
use crate::gateway::{Page, Query, Select};
use crate::owner::given::GivenItem;
use serde_json::{Value as Json, json};

const OWNER: &str = "chores";

/// The made-up format, `.chores`: a chore a line, `name | state | due | words`. A line that
/// starts with `;` is a remark. In the words, `@word` is a label. The states are the
/// format's own.
const HOME: &str = "; The house\n\
    plumber | todo  | 2026-11-20 | Call the plumber @home\n\
    rent    | done  |            | Pay #42 for $5 to [[x]] and **x**, not - [ ] this 📅 2026-01-01\n\
    garden  | doing |            | Rake the leaves\n";

const STATES: [(&str, &str); 4] = [
    ("todo", "open"),
    ("done", "done"),
    ("doing", "in-progress"),
    ("dropped", "cancelled"),
];

/// A line's four parts, when it is a chore.
fn chore(line: &str) -> Option<[&str; 4]> {
    let mut parts = line.splitn(4, '|').map(str::trim);
    let chore = [parts.next()?, parts.next()?, parts.next()?, parts.next()?];
    (!line.starts_with(';')).then_some(chore)
}

/// `file/read`, as the owner answers it: each chore a piece, an `item` that is a task, with
/// its state and its date as attributes and its words as plain runs.
pub(super) fn read(text: &str) -> Given {
    let pieces = text
        .lines()
        .filter_map(chore)
        .map(|[name, state, due, words]| {
            let word = STATES.iter().find(|(s, _)| *s == state).map(|(_, w)| *w);
            let mut attributes = json!({"task": true, "checkbox": word});
            if !due.is_empty() {
                attributes["due"] = json!(due);
            }
            let mut runs: Vec<Json> = Vec::new();
            let mut plain = String::new();
            for (i, token) in words.split(' ').enumerate() {
                if i > 0 {
                    plain.push(' ');
                }
                match token.strip_prefix('@').filter(|label| !label.is_empty()) {
                    Some(label) => {
                        runs.extend((!plain.is_empty()).then(|| json!(plain)));
                        runs.push(json!({"label": label}));
                        plain.clear();
                    }
                    None => plain.push_str(token),
                }
            }
            runs.extend((!plain.is_empty()).then(|| json!(plain)));
            json!({"place": name, "role": "item", "attributes": attributes, "text": runs})
        });
    let items: Vec<Json> = pieces.collect();
    serde_json::from_value(json!({"items": items})).unwrap()
}

/// A workspace with `Home.chores` in it, read by its owner.
pub(super) fn open(name: &str) -> Core {
    let dir = std::env::temp_dir().join(format!("cold-lava-pieces-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Home.chores"), HOME).unwrap();
    let mut core = Core::open(&dir).unwrap();
    let kind = OwnedKind {
        extension: "chores".into(),
        owner: OWNER.into(),
        version: "1".into(),
    };
    core.set_owners(vec![kind], None);
    let (owner, asked) = core.take_reads().pop().unwrap();
    assert_eq!(
        (owner.as_str(), asked.path.as_str()),
        (OWNER, "Home.chores")
    );
    core.file_given(OWNER, "Home.chores", &asked.rev, Ok(read(&asked.text)));
    core.poll();
    core
}

pub(super) fn tasks(core: &Core) -> Vec<Item> {
    let select = Select {
        kind: "task".into(),
        ..Select::default()
    };
    let page = Page::default();
    core.query_items(&Query { select, page }).unwrap().items
}

pub(super) fn field(item: &Item, name: &str) -> Json {
    serde_json::to_value(item).unwrap()[name].clone()
}

#[test]
fn a_made_up_formats_tasks_join_the_workspace_as_pieces() {
    let mut core = open("read");
    let all = tasks(&core);
    let words: Vec<Json> = all.iter().map(|t| field(t, "text")).collect();
    assert_eq!(
        words,
        [
            json!("Call the plumber #home"),
            json!("Pay #42 for $5 to [[x]] and **x**, not - [ ] this 📅 2026-01-01"),
            json!("Rake the leaves"),
        ]
    );
    let of = |name: &str| -> Vec<Json> { all.iter().map(|t| field(t, name)).collect() };
    assert_eq!(of("kind"), [json!("task"), json!("task"), json!("task")]);
    assert_eq!(of("note"), [json!("Home.chores"); 3]);
    assert_eq!(of("status"), [json!(" "), json!("x"), json!("/")]);
    assert_eq!(
        of("status_type"),
        [json!("todo"), json!("done"), json!("in_progress")]
    );
    assert_eq!(of("due"), [json!("2026-11-20"), Json::Null, Json::Null]);
    assert_eq!(
        of("tags"),
        [json!(["home"]), json!([]), json!([])],
        "a label is a tag, and `#42` in plain text is none"
    );
    assert_eq!(of("links"), [json!([]), json!([]), json!([])]);

    // The agenda lists the dated one beside a note's tasks, and knows nothing of the format.
    let day = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
    let agenda = crate::tasks::agenda(&core.index.workspace, day("2026-11-01"), day("2026-11-30"));
    let listed: Vec<(&str, &str, &str)> = (agenda.tasks.iter())
        .map(|t| (t.text.as_str(), t.status, t.file.as_str()))
        .collect();
    assert_eq!(listed, [("Call the plumber #home", "todo", "Home.chores")]);

    // Nothing the owner gave is Markdown: every item is a piece, its text in runs.
    let file = &core.index.workspace.owned[0];
    assert_eq!(file.given.items.len(), 3);
    assert!((file.given.items.iter()).all(|g| matches!(g, GivenItem::Piece(_))));
    assert_eq!(file.places[&all[0].id], "plumber");
    assert!(file.links.is_empty(), "`[[x]]` in plain text is no link");

    // A piece is read and not changed yet: an edit says so, and nothing is asked of the owner.
    let tick = Edit {
        id: all[0].id.clone(),
        rev: all[0].rev.clone(),
        op: Op::Toggle,
    };
    let refused = core.edit_item("agenda", &tick).unwrap_err();
    assert_eq!(refused.reason, gateway::Reason::Refused);
    assert!(
        refused.message.contains("can't be changed yet"),
        "{refused}"
    );
    assert!(core.take_file_edits().is_empty());
    let _ = std::fs::remove_dir_all(core.root());
}
