//! Step 3 of the neutral model, end to end (docs/neutral-model.md): an owner of a made-up
//! format that isn't Markdown gives its tasks as pieces, the workspace has them as `task`s, and
//! they are ticked, re-dated and reworded through the gateway, as changes in the model's terms.
//! The owner here is a small real one, played in-process the way an extension answers: it
//! parses and writes its own format, and it never writes or reads a character of Markdown.

use super::*;
use crate::gateway::{Op, Page, Query, Select};
use crate::owner::given::{GivenItem, Run};
use serde_json::{Value as Json, json};

const OWNER: &str = "chores";

/// The made-up format, `.chores`: a chore a line, `name | state | due | words`. A line that
/// starts with `;` is a remark. In the words, `@word` is a label and `>Word` a link to the
/// file `Word`. The states are the format's own.
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
                let named = |sign: char| token.strip_prefix(sign).filter(|name| !name.is_empty());
                let run = match (named('@'), named('>')) {
                    (Some(label), _) => json!({"label": label}),
                    (None, Some(target)) => json!({"link": target}),
                    (None, None) => {
                        plain.push_str(token);
                        continue;
                    }
                };
                runs.extend((!plain.is_empty()).then(|| json!(plain)));
                runs.push(run);
                plain.clear();
            }
            runs.extend((!plain.is_empty()).then(|| json!(plain)));
            json!({"place": name, "role": "item", "attributes": attributes, "text": runs})
        });
    let items: Vec<Json> = pieces.collect();
    serde_json::from_value(json!({"items": items})).unwrap()
}

/// One part of a chore's line replaced, the space around it kept, so that no other byte of
/// the file moves.
fn with_part(line: &str, n: usize, new: &str) -> String {
    let mut parts: Vec<String> = line.splitn(4, '|').map(String::from).collect();
    let old = parts[n].clone();
    let (lead, trail) = (
        old.len() - old.trim_start().len(),
        old.len() - old.trim_end().len(),
    );
    parts[n] = match old.trim().is_empty() {
        true if new.is_empty() => old,
        true => format!(" {new} "),
        false => format!("{}{new}{}", &old[..lead], &old[old.len() - trail..]),
    };
    parts.join("|")
}

/// `file/edit`, as the owner answers it: each change written the way the format writes it.
/// An attribute is a state or a date, and a piece's runs are its words, a label as `@word` and
/// a link as `>Word`. It refuses what a chore can't hold.
pub(super) fn edit(text: &str, changes: &[Change]) -> Result<String, String> {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    for change in changes {
        let place = match change {
            Change::Attribute { place, .. }
            | Change::Runs { place, .. }
            | Change::Link { place, .. } => place,
            other => return Err(format!("a chore takes no {other:?}")),
        };
        let is_it = |line: &String| chore(line).is_some_and(|[name, ..]| name == place);
        let line = lines.iter_mut().find(|line| is_it(line));
        let line = line.ok_or_else(|| format!("no chore called {place}"))?;
        *line = match change {
            Change::Attribute {
                attribute, value, ..
            } => match (attribute.as_str(), value.as_str()) {
                ("checkbox", Some(word)) => {
                    let state = STATES.iter().find(|(_, w)| *w == word);
                    let (state, _) = state.ok_or_else(|| format!("no state for {word}"))?;
                    with_part(line, 1, state)
                }
                ("due", due) => with_part(line, 2, due.unwrap_or_default()),
                (other, _) => return Err(format!("a chore has no {other}")),
            },
            Change::Runs {
                text, cell: None, ..
            } => {
                let mut words = String::new();
                for run in text {
                    match run {
                        Run::Plain(text) => words.push_str(text),
                        Run::Label(l) => words.push_str(&format!("@{}", l.label)),
                        Run::Link(l) => words.push_str(&format!(">{}", l.link)),
                        other => return Err(format!("a chore's words can't hold {other:?}")),
                    }
                }
                with_part(line, 3, &words)
            }
            Change::Link { link, to, .. } => line.replace(&format!(">{link}"), &format!(">{to}")),
            other => return Err(format!("a chore takes no {other:?}")),
        };
    }
    Ok(lines.join("\n") + "\n")
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
    let core = open("read");
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
    assert_eq!(of("note"), vec![json!("Home.chores"); 3]);
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

    let _ = std::fs::remove_dir_all(core.root());
}

fn tick(item: &Item) -> Edit {
    Edit {
        id: item.id.clone(),
        rev: item.rev.clone(),
        op: Op::Toggle,
    }
}

fn set(item: &Item, field: &str, value: Json) -> Edit {
    let field = field.to_string();
    Edit {
        id: item.id.clone(),
        rev: item.rev.clone(),
        op: Op::SetField { field, value },
    }
}

fn home(core: &Core) -> String {
    std::fs::read_to_string(core.root().join("Home.chores")).unwrap()
}

/// Make `edit` as `agenda`, play the owner for it, and hand back what the owner was asked
/// (as the JSON of `file/edit`'s `changes`) and the answer to the edit.
fn through_the_owner(edit_: &Edit, core: &mut Core) -> (Json, Result<Edited, gateway::Error>) {
    let ticket = match core.edit_item("agenda", edit_) {
        Ok(Answer::Later(ticket)) => ticket,
        Ok(Answer::Now(edited)) => return (json!([]), Ok(*edited)),
        Err(e) => return (json!([]), Err(e)),
    };
    let (owner, ask) = core.take_file_edits().pop().unwrap();
    assert_eq!(owner, OWNER);
    let before = home(core);
    assert_eq!(ask.text, before, "the owner gets the file as it is");
    core.file_edited(OWNER, &ask.path, &ask.rev, edit(&ask.text, &ask.changes));
    assert_eq!(
        home(core),
        before,
        "nothing is written before it reads back"
    );
    if let Some((_, back)) = core.take_reads().pop() {
        core.file_given(OWNER, &back.path, &back.rev, Ok(read(&back.text)));
    }
    let mut done = core.take_edited();
    let (by, t, answer) = done.pop().unwrap();
    assert_eq!((by.as_str(), t), ("agenda", ticket));
    (serde_json::to_value(&ask.changes).unwrap(), answer)
}

/// The test of step 3: tasks an owner gave as pieces are ticked, re-dated and reworded
/// through the gateway. Each edit reaches the owner as a change in the model's terms, the
/// owner writes it its own way, and only the bytes it names change in the file.
#[test]
fn a_made_up_formats_tasks_are_ticked_re_dated_and_reworded() {
    let mut core = open("edit");
    let plumber = |core: &Core| tasks(core).remove(0);
    let id = plumber(&core).id;

    // Ticked: `checkbox = done` at its place, never a line of Markdown.
    let (asked, answer) = through_the_owner(&tick(&plumber(&core)), &mut core);
    assert_eq!(
        asked,
        json!([{"place": "plumber", "attribute": "checkbox", "value": "done"}])
    );
    let ticked = answer.unwrap();
    assert!(ticked.changed);
    assert_eq!(
        (
            field(&ticked.item, "status"),
            field(&ticked.item, "status_type")
        ),
        (json!("x"), json!("done"))
    );
    assert_eq!(ticked.item.id, id, "a piece keeps its id through an edit");
    let now = HOME.replace("plumber | todo ", "plumber | done ");
    assert_eq!(home(&core), now, "one word changed, and no other byte");
    let events = core.poll();
    let named = |e: &Event| {
        matches!(e, Event::ItemEdited { note, by, what, .. }
            if note == "Home.chores" && by == "agenda" && what == "ticked")
    };
    assert!(events.iter().any(named), "{events:?}");
    assert!(core.take_reads().is_empty(), "what it read back is kept");

    // Unticked: back to `open`. A status that is neither goes on to done.
    let (asked, answer) = through_the_owner(&tick(&plumber(&core)), &mut core);
    assert_eq!(asked[0]["value"], "open");
    assert_eq!(field(&answer.unwrap().item, "status_type"), "todo");
    assert_eq!(home(&core), HOME);
    let garden = tasks(&core).remove(2);
    let (asked, _) = through_the_owner(&tick(&garden), &mut core);
    assert_eq!(
        asked,
        json!([{"place": "garden", "attribute": "checkbox", "value": "done"}])
    );
    let now = HOME.replace("| doing |", "| done |");
    assert_eq!(home(&core), now);

    // Re-dated: the attribute, as the field's type carries it.
    let redate = set(&plumber(&core), "due", json!("2026-11-21"));
    let (asked, answer) = through_the_owner(&redate, &mut core);
    assert_eq!(
        asked,
        json!([{"place": "plumber", "attribute": "due", "value": "2026-11-21"}])
    );
    assert_eq!(field(&answer.unwrap().item, "due"), "2026-11-21");
    let now = now.replace("2026-11-20", "2026-11-21");
    assert_eq!(home(&core), now);
    let same = set(&plumber(&core), "due", json!("2026-11-21"));
    let (asked, answer) = through_the_owner(&same, &mut core);
    assert_eq!((asked, answer.unwrap().changed), (json!([]), false));
    let rent = tasks(&core).remove(1);
    let (asked, _) = through_the_owner(&set(&rent, "due", json!("2026-12-01")), &mut core);
    assert_eq!(asked[0]["place"], "rent");
    let now = now.replace("done  |            |", "done  | 2026-12-01 |");
    assert_eq!(home(&core), now, "a date where there was none");
    let rent = tasks(&core).remove(1);
    let (asked, answer) = through_the_owner(&set(&rent, "due", Json::Null), &mut core);
    assert_eq!(
        asked,
        json!([{"place": "rent", "attribute": "due", "value": null}])
    );
    assert_eq!(field(&answer.unwrap().item, "due"), Json::Null);
    let now = now.replace("done  | 2026-12-01 |", "done  |  |");
    assert_eq!(home(&core), now);
    let status = set(&plumber(&core), "status_type", json!("cancelled"));
    let (asked, answer) = through_the_owner(&status, &mut core);
    assert_eq!(asked[0]["value"], "cancelled");
    assert_eq!(field(&answer.unwrap().item, "status"), "-");
    let now = now.replace("| todo  |", "| dropped  |");
    assert_eq!(home(&core), now);

    // Reworded: its text as runs. The label that is still among the words stays a label, and
    // what is typed is characters.
    let reword = set(
        &plumber(&core),
        "text",
        json!("Call the electrician #home on [[Monday]]"),
    );
    let (asked, answer) = through_the_owner(&reword, &mut core);
    assert_eq!(
        asked,
        json!([{"place": "plumber",
                "text": ["Call the electrician ", {"label": "home"}, " on [[Monday]]"]}])
    );
    let reworded = answer.unwrap().item;
    assert_eq!(
        (
            field(&reworded, "text"),
            field(&reworded, "tags"),
            field(&reworded, "links")
        ),
        (
            json!("Call the electrician #home on [[Monday]]"),
            json!(["home"]),
            json!([])
        )
    );
    assert_eq!(reworded.id, id);
    let now = now.replace(
        "Call the plumber @home",
        "Call the electrician @home on [[Monday]]",
    );
    assert_eq!(home(&core), now);
    let plain = set(&plumber(&core), "text", json!("Call nobody about #42"));
    let (asked, answer) = through_the_owner(&plain, &mut core);
    assert_eq!(
        asked,
        json!([{"place": "plumber", "text": ["Call nobody about #42"]}])
    );
    assert_eq!(
        field(&answer.unwrap().item, "tags"),
        json!([]),
        "the label went with its word"
    );
    let now = now.replace(
        "Call the electrician @home on [[Monday]]",
        "Call nobody about #42",
    );
    assert_eq!(home(&core), now);
    let _ = std::fs::remove_dir_all(core.root());
}

/// What a piece can't hold, or an owner doesn't write as asked, is refused with the reason,
/// and the file is as it was.
#[test]
fn an_edit_a_piece_cant_hold_is_refused_and_nothing_is_written() {
    let mut core = open("refused");
    let plumber = |core: &Core| tasks(core).remove(0);
    let refused = |edit_: &Edit, core: &mut Core| {
        let (asked, answer) = through_the_owner(edit_, core);
        let error = answer.unwrap_err();
        assert_eq!(home(core), HOME, "{error}");
        assert!(core.take_file_edits().is_empty() && core.take_reads().is_empty());
        (asked, error)
    };

    // Derived from its text, or not a value the field's type takes: the owner isn't asked.
    for (name, value, says) in [
        ("tags", json!("garden"), "can't be set"),
        ("links", json!("Krakatoa"), "can't be set"),
        ("due", json!("next week"), "is not a date"),
        ("status", json!("?"), "is not a status"),
        ("colour", json!("red"), "has no field colour"),
    ] {
        let (asked, error) = refused(&set(&plumber(&core), name, value), &mut core);
        assert_eq!(
            (asked, error.reason),
            (json!([]), Reason::Refused),
            "{name}"
        );
        assert!(error.message.contains(says), "{name}: {error}");
    }
    let stale = Edit {
        rev: "000000000000".into(),
        ..tick(&plumber(&core))
    };
    assert_eq!(refused(&stale, &mut core).1.reason, Reason::Changed);

    // The owner is asked, and its format can't say it: `@word` typed into the words would
    // read back as a label, which was not asked. Nothing is written.
    let at = set(&plumber(&core), "text", json!("Call @nobody"));
    let (asked, error) = refused(&at, &mut core);
    assert_eq!(
        asked,
        json!([{"place": "plumber", "text": ["Call @nobody"]}])
    );
    assert_eq!(error.reason, Reason::ReadBack, "{error}");
    assert!(error.message.contains("nothing was written"), "{error}");
    // Words taken away leave no task, so it wouldn't read back.
    let empty = set(&plumber(&core), "text", json!(""));
    assert_eq!(refused(&empty, &mut core).1.reason, Reason::ReadBack);

    // An owner that ticks the chore and quietly changes another piece: caught piece by piece.
    let Ok(Answer::Later(_)) = core.edit_item("agenda", &tick(&plumber(&core))) else {
        panic!("answered later");
    };
    let (_, ask) = core.take_file_edits().pop().unwrap();
    let more = edit(&ask.text, &ask.changes).unwrap();
    let more = more.replace("Rake the leaves", "Rake the leaves @quietly");
    core.file_edited(OWNER, &ask.path, &ask.rev, Ok(more.clone()));
    let (_, back) = core.take_reads().pop().unwrap();
    core.file_given(OWNER, &back.path, &back.rev, Ok(read(&more)));
    let (_, _, answer) = core.take_edited().pop().unwrap();
    let error = answer.unwrap_err();
    assert_eq!(error.reason, Reason::ReadBack, "{error}");
    assert_eq!(home(&core), HOME);
    let _ = std::fs::remove_dir_all(core.root());
}

/// A link a piece says is repointed when its file is renamed, as `{place, link, to}`.
#[test]
fn a_rename_repoints_the_links_a_piece_says() {
    use crate::rename;
    let dir = std::env::temp_dir().join(format!("cold-lava-pieces-mv-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Krakatoa.md"), "# Krakatoa\n").unwrap();
    let chores = "trip | todo | | Pack for >Krakatoa and [[Krakatoa]]\n";
    std::fs::write(dir.join("Home.chores"), chores).unwrap();
    let mut core = Core::open(&dir).unwrap();
    let kind = OwnedKind {
        extension: "chores".into(),
        owner: OWNER.into(),
        version: "1".into(),
    };
    core.set_owners(vec![kind], None);
    let (_, asked) = core.take_reads().pop().unwrap();
    core.file_given(OWNER, "Home.chores", &asked.rev, Ok(read(&asked.text)));
    core.poll();
    let krakatoa = core.index.by_rel("Krakatoa").unwrap();
    assert_eq!(
        core.index.backlinks(krakatoa).len(),
        1,
        "the link run, not the characters"
    );
    assert_eq!(field(&tasks(&core)[0], "links"), json!(["Krakatoa"]));

    let mut plan = rename::plan(&core.index, krakatoa, "Krakatau").unwrap();
    let ticket = core
        .relink(&mut plan.edits)
        .unwrap()
        .expect("waits for the owner");
    let (_, ask) = core.take_file_edits().pop().unwrap();
    assert_eq!(
        serde_json::to_value(&ask.changes).unwrap(),
        json!([{"place": "trip", "link": "Krakatoa", "to": "Krakatau"}])
    );
    let new = edit(&ask.text, &ask.changes).unwrap();
    core.file_edited(OWNER, &ask.path, &ask.rev, Ok(new.clone()));
    let (_, back) = core.take_reads().pop().unwrap();
    core.file_given(OWNER, &back.path, &back.rev, Ok(read(&new)));
    let relinked = core.take_relinked();
    let [(t, Ok(answers))] = &relinked[..] else {
        panic!("{relinked:?}");
    };
    assert_eq!(*t, ticket);
    rename::answered(&mut plan.edits, answers.clone());
    rename::apply(&plan).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("Home.chores")).unwrap(),
        "trip | todo | | Pack for >Krakatau and [[Krakatoa]]\n",
        "the link moved, and the characters that only look like one stayed"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A typed item an owner gave is ticked too: its status goes to the next symbol, as a field.
/// Before, `toggle` on one was refused, and a status of one space was read as none.
#[test]
fn a_typed_task_an_owner_gave_is_ticked_as_a_field() {
    let dir = std::env::temp_dir().join(format!("cold-lava-typed-tick-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The owner's format: `status;words`, one line, its place `1`.
    let file = |status: &str| format!("{status};Water the plants\n");
    let given = |status: &str| -> Given {
        let fields = json!({"status": status, "text": "Water the plants"});
        serde_json::from_value(json!({"items": [{"place": "1", "type": "task", "fields": fields}]}))
            .unwrap()
    };
    std::fs::write(dir.join("Plants.list"), file(" ")).unwrap();
    let mut core = Core::open(&dir).unwrap();
    let kind = OwnedKind {
        extension: "list".into(),
        owner: "lists".into(),
        version: "1".into(),
    };
    core.set_owners(vec![kind], None);
    let (_, asked) = core.take_reads().pop().unwrap();
    core.file_given("lists", "Plants.list", &asked.rev, Ok(given(" ")));
    core.poll();
    let now = |core: &Core| std::fs::read_to_string(core.root().join("Plants.list")).unwrap();
    for (from, to) in [(" ", "x"), ("x", " ")] {
        let task = tasks(&core).remove(0);
        assert_eq!(field(&task, "status"), from);
        let Ok(Answer::Later(_)) = core.edit_item("agenda", &tick(&task)) else {
            panic!("answered later");
        };
        let (_, ask) = core.take_file_edits().pop().unwrap();
        assert_eq!(
            serde_json::to_value(&ask.changes).unwrap(),
            json!([{"place": "1", "field": "status", "value": to}])
        );
        core.file_edited("lists", "Plants.list", &ask.rev, Ok(file(to)));
        let (_, back) = core.take_reads().pop().unwrap();
        core.file_given("lists", "Plants.list", &back.rev, Ok(given(to)));
        let (_, _, answer) = core.take_edited().pop().unwrap();
        assert_eq!(field(&answer.unwrap().item, "status"), to);
        assert_eq!(now(&core), file(to));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
