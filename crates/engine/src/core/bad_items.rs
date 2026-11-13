//! An edit in a file that holds a bad item (#257, track A step A4c): an edit to a good item
//! goes ahead when what is left out is the same before and after the owner's change, and
//! anything else is refused, naming the bad item.
//!
//! The owner here is a probe whose format is its answer: the file is the JSON of
//! `{items, links}`, so what it reads is what it holds, a bad item included as written. Its
//! `file/edit` writes the expected answer's changed items into the entries they came from
//! and leaves every other byte; a tamper then does something else to a bad item.

use super::*;
use crate::gateway::{Op, Page, Query, Reason, Select};
use crate::owner::given::{self, GivenItem};
use crate::problem::cut;
use serde_json::{Value as Json, json};

const OWNER: &str = "probe";
const FILE: &str = "A.probe";
const TYPES: &str = "version = 1\n[types.eruption]\non = \"row\"\nwhere.header = [\"VEI\"]\n\
                     fields.vei = { column = \"VEI\", type = \"number\" }\n";

fn chore(place: &str, words: &str) -> Json {
    json!({"place": place, "role": "item", "text": [words],
           "attributes": {"task": true, "checkbox": "open", "due": "2026-11-20"}})
}

fn row(place: &str) -> Json {
    json!({"place": place, "role": "row", "cells": [
        {"name": "Volcano", "text": ["Krakatoa"]}, {"name": "VEI", "text": ["6"]}]})
}

fn typed(place: &str) -> Json {
    json!({"place": place, "type": "task", "fields": {"status": " ", "text": "Typed"}})
}

fn markdown(place: &str) -> Json {
    json!({"place": place, "text": "- [ ] Markdown task"})
}

/// The good items, one of each way to give one: a piece that is a task, a row a type reads
/// a cell of, a typed item and a Markdown piece.
fn good() -> Vec<Json> {
    vec![
        chore("chore", "Water plants"),
        row("row"),
        typed("ty"),
        markdown("md"),
    ]
}

/// A bad item of each kind the engine leaves out, where it goes in the answer, and the words
/// it is reported in (its place cut to 80 characters, then why), which a refusal names.
struct Bad {
    name: &'static str,
    /// `items` or `links`.
    list: &'static str,
    entry: Json,
    /// The same entry changed, still bad.
    altered: Json,
    /// The same entry made good.
    fixed: Json,
    /// Another bad entry of its kind.
    other: Json,
    says: String,
}

fn long() -> String {
    "x".repeat(1025)
}

fn bads() -> Vec<Bad> {
    let far = "L".repeat(4097);
    vec![
        Bad {
            name: "malformed as it arrives",
            list: "items",
            entry: json!({"place": "bad", "role": "item", "text": ["Bad"], "colour": "red"}),
            altered: json!({"place": "bad", "role": "item", "text": ["Bad"], "colour": "blue"}),
            fixed: json!({"place": "bad", "role": "item", "text": ["Bad"]}),
            other: json!({"place": "bad", "role": "item", "text": ["Bad"], "size": 1}),
            says: "bad: unknown field `colour`".into(),
        },
        Bad {
            name: "past a bound as it arrives",
            list: "items",
            entry: json!({"place": long(), "text": "- [ ] long place"}),
            altered: json!({"place": long(), "text": "- [ ] long place!"}),
            fixed: json!({"place": "short", "text": "- [ ] long place"}),
            other: json!({"place": "y".repeat(1025), "text": "- [ ] long place"}),
            says: format!(
                "{}: its place is empty or longer than 1024 bytes",
                cut(&long(), 80)
            ),
        },
        Bad {
            name: "past a bound as it is read",
            list: "links",
            entry: json!({"target": far, "place": "far"}),
            altered: json!({"target": format!("{far}!"), "place": "far"}),
            fixed: json!({"target": "Lava", "place": "far"}),
            other: json!({"target": far, "place": "far2"}),
            says: "far: a link to a file: a link's target is empty or longer than 4096 bytes"
                .into(),
        },
        Bad {
            name: "a second item at a taken place",
            list: "items",
            entry: chore("chore", "Twin"),
            altered: chore("chore", "Twin!"),
            fixed: chore("twin", "Twin"),
            other: chore("chore", "Other twin"),
            says: "chore: another piece is at this place".into(),
        },
        Bad {
            name: "a bad link to a file",
            list: "links",
            entry: json!({"target": "", "place": "f"}),
            altered: json!({"target": "", "place": "f", "text": "around"}),
            fixed: json!({"target": "Lava", "place": "f"}),
            other: json!({"target": "", "place": "g"}),
            says: "f: a link to a file: a link's target is empty or longer than 4096 bytes".into(),
        },
        Bad {
            name: "a bad link between pieces",
            list: "links",
            entry: json!({"from": "chore", "to": "nowhere"}),
            altered: json!({"from": "chore", "to": "nowhere", "label": "needs"}),
            fixed: json!({"from": "chore", "to": "md"}),
            other: json!({"from": "row", "to": "nowhere"}),
            says: "a link from chore to nowhere: no piece is at nowhere".into(),
        },
        Bad {
            name: "a typed item of a type that isn't loaded",
            list: "items",
            entry: json!({"place": "ty2", "type": "volcano", "fields": {"vei": 6}}),
            altered: json!({"place": "ty2", "type": "volcano", "fields": {"vei": 7}}),
            fixed: json!({"place": "ty2", "type": "task", "fields": {"text": "Fixed"}}),
            other: json!({"place": "ty2", "type": "crater", "fields": {"vei": 6}}),
            says: "ty2: no type called volcano".into(),
        },
        Bad {
            name: "a piece in no piece",
            list: "items",
            entry: json!({"place": "orphan", "role": "item", "in": "ghost", "text": ["Orphan"]}),
            altered: json!({"place": "orphan", "role": "item", "in": "ghost", "text": ["Orphan!"]}),
            fixed: json!({"place": "orphan", "role": "item", "text": ["Orphan"]}),
            other: json!({"place": "orphan", "role": "item", "in": "ghost2", "text": ["Orphan"]}),
            says: "orphan: it is in ghost, and no piece before it is at that place".into(),
        },
    ]
}

/// The probe's file: the good items, a link to a file and a link between two pieces, and
/// each of `bad` right after the good entry of its kind.
fn file_with(bad: &[&Bad]) -> Json {
    let mut items = good();
    let mut links = vec![
        json!({"target": "Lava", "place": "chore"}),
        json!({"from": "chore", "to": "row"}),
    ];
    for b in bad {
        match (b.list, b.entry.get("from")) {
            ("items", _) => items.insert(1, b.entry.clone()),
            (_, None) => links.insert(1, b.entry.clone()),
            (_, Some(_)) => links.push(b.entry.clone()),
        }
    }
    json!({"items": items, "links": links})
}

/// `file/read`, as the probe answers it.
fn read(text: &str) -> Result<Given, String> {
    serde_json::from_str(text).map_err(|e| e.to_string())
}

/// `file/edit`, as an honest probe answers it: each item the changes reach is written into
/// the entry it was read from, and nothing else of the file changes.
fn honest(text: &str, changes: &[Change]) -> Json {
    let given = read(text).unwrap();
    let expect = given::apply(&given, changes);
    let mut raw: Json = serde_json::from_str(text).unwrap();
    let mut k = 0;
    for entry in raw["items"].as_array_mut().unwrap() {
        if GivenItem::read(entry.clone()).is_err() {
            continue;
        }
        if expect.items[k] != given.items[k] {
            *entry = serde_json::to_value(&expect.items[k]).unwrap();
        }
        k += 1;
    }
    raw
}

/// The index in `items` of every entry that reads, in order: `given.items[k]` came from
/// entry `entries(raw)[k]`.
fn entries(raw: &Json) -> Vec<usize> {
    let all = raw["items"].as_array().unwrap().iter().enumerate();
    all.filter(|(_, e)| GivenItem::read((*e).clone()).is_ok())
        .map(|(i, _)| i)
        .collect()
}

fn open(name: &str, file: &Json, cache: Option<&Path>) -> Core {
    let dir = std::env::temp_dir().join(format!("cold-lava-bad-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
    std::fs::write(dir.join(".cold-lava/types.toml"), TYPES).unwrap();
    std::fs::write(dir.join(FILE), file.to_string()).unwrap();
    reopen(&dir, cache)
}

fn reopen(dir: &Path, cache: Option<&Path>) -> Core {
    let mut core = Core::open(dir).unwrap();
    let kind = OwnedKind {
        extension: "probe".into(),
        owner: OWNER.into(),
        version: "1".into(),
    };
    core.set_owners(vec![kind], cache);
    if let Some((_, asked)) = core.take_reads().pop() {
        core.file_given(OWNER, FILE, &asked.rev, read(&asked.text));
    }
    core.poll();
    core
}

fn now(core: &Core) -> String {
    std::fs::read_to_string(core.root().join(FILE)).unwrap()
}

/// The good item of `kind` (`task` at `place`, or `eruption`).
fn item(core: &Core, kind: &str, place: &str) -> Item {
    let select = Select {
        kind: kind.into(),
        ..Select::default()
    };
    let page = Page::default();
    let items = core.query_items(&Query { select, page }).unwrap().items;
    let file = &core.index.workspace.owned[0];
    let found = items
        .into_iter()
        .find(|i| file.places.get(&i.id).is_some_and(|p| p == place));
    found.unwrap_or_else(|| panic!("no {kind} at {place}"))
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

/// Each kind of edit, on the good item it is for: a piece ticked, re-dated and reworded
/// (`{place, attribute, value}`, `{place, text}`), a row's cell set (`{place, cell, text}`),
/// a typed item ticked (`{place, field, value}`) and a Markdown piece ticked
/// (`{place, text}`). Made again before each edit, since an edit changes its item's rev.
fn edits(core: &Core) -> Vec<(&'static str, Edit)> {
    let chore = item(core, "task", "chore");
    vec![
        ("tick a piece", tick(&chore)),
        (
            "set a piece's date",
            set(&chore, "due", json!("2026-12-01")),
        ),
        (
            "reword a piece",
            set(&chore, "text", json!("Water the ferns")),
        ),
        (
            "set a row's cell",
            set(&item(core, "eruption", "row"), "vei", json!(7)),
        ),
        ("tick a typed item", tick(&item(core, "task", "ty"))),
        ("tick a Markdown piece", tick(&item(core, "task", "md"))),
    ]
}

/// Make `edit`, have the probe answer `file/edit` with what `answer` makes of the file and
/// its changes, read that back, and hand back the answer to the edit. An edit refused before
/// the probe is asked is `Err` with nothing asked.
fn through_the_probe(
    core: &mut Core,
    edit: &Edit,
    answer: impl FnOnce(&str, &[Change]) -> Json,
) -> Result<Edited, gateway::Error> {
    match core.edit_item("agenda", edit)? {
        Answer::Later(_) => {}
        Answer::Now(edited) => return Ok(*edited),
    }
    let (owner, ask) = core.take_file_edits().pop().expect("the probe is asked");
    assert_eq!(owner, OWNER);
    let before = now(core);
    let new = answer(&ask.text, &ask.changes).to_string();
    core.file_edited(OWNER, &ask.path, &ask.rev, Ok(new.clone()));
    assert_eq!(now(core), before, "nothing is written before it reads back");
    if let Some((_, back)) = core.take_reads().pop() {
        core.file_given(OWNER, &back.path, &back.rev, read(&back.text));
    }
    let (_, _, answer) = core.take_edited().pop().expect("answered");
    core.poll();
    answer
}

/// Every entry of the file, as written, but those at `except` (`items` indices).
fn untouched(file: &Json, except: &[usize]) -> Vec<String> {
    let items = file["items"].as_array().unwrap().iter().enumerate();
    let items = items
        .filter(|(i, _)| !except.contains(i))
        .map(|(_, e)| e.to_string());
    let links = file["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(Json::to_string);
    items.chain(links).collect()
}

/// The items of the file whose entries the expected answer of `changes` changes.
fn changed_entries(text: &str, changes: &[Change]) -> Vec<usize> {
    let before: Json = serde_json::from_str(text).unwrap();
    let after = honest(text, changes);
    let (b, a) = (
        before["items"].as_array().unwrap(),
        after["items"].as_array().unwrap(),
    );
    (0..b.len()).filter(|&i| b[i] != a[i]).collect()
}

/// On main, any item left out refused every edit in its file. Now an edit to a good item goes
/// ahead beside a bad item of every kind, each kind of edit, and only the entry it names
/// changes: every other item, link and bad item keeps its bytes.
#[test]
fn an_edit_beside_every_kind_of_bad_item_goes_ahead() {
    let bads = bads();
    let all: Vec<&Bad> = bads.iter().collect();
    let mut core = open("beside", &file_with(&all), None);
    let left = |core: &Core| core.index.workspace.owned[0].given.left_out.len();
    assert_eq!(left(&core), 2, "two of them are left out as they arrive");
    for k in 0..edits(&core).len() {
        let (what, edit) = edits(&core).remove(k);
        let text = now(&core);
        let file: Json = serde_json::from_str(&text).unwrap();
        let mut named = Vec::new();
        let answer = through_the_probe(&mut core, &edit, |text, changes| {
            named = changed_entries(text, changes);
            honest(text, changes)
        });
        let edited = answer.unwrap_or_else(|e| panic!("{what}: {e}"));
        assert!(edited.changed, "{what}");
        assert_eq!(named.len(), 1, "{what}: one entry changes");
        let after: Json = serde_json::from_str(&now(&core)).unwrap();
        assert_eq!(
            untouched(&after, &named),
            untouched(&file, &named),
            "{what}"
        );
        assert_ne!(after, file, "{what}: written");
        assert_eq!(left(&core), 2, "{what}: still left out, and kept");
    }
    let _ = std::fs::remove_dir_all(core.root());
}

/// What an owner might do to a bad item while it makes the change: everything but keeping it
/// as it was is refused, the refusal names the bad item, and nothing is written. `reorder`
/// reverses the order of its keys: an item left out as it arrived is kept as it was sent, so
/// that is a change; one that read is compared as it reads, and reads the same.
#[test]
fn an_owner_that_touches_a_bad_item_is_refused_naming_it() {
    type Tamper = fn(&mut Vec<Json>, usize, &Bad);
    let tampers: [(&str, Tamper); 8] = [
        ("drop", |list, at, _| {
            list.remove(at);
        }),
        ("alter", |list, at, b| list[at] = b.altered.clone()),
        ("fix", |list, at, b| list[at] = b.fixed.clone()),
        ("replace", |list, at, b| list[at] = b.other.clone()),
        ("move", |list, at, _| {
            let entry = list.remove(at);
            list.insert(0, entry);
        }),
        ("duplicate", |list, at, _| list.insert(at, list[at].clone())),
        ("swap", |list, at, _| list.swap(at, at - 1)),
        ("reorder", |list, at, _| {
            let entry = list[at].as_object().unwrap();
            let reversed = entry.iter().rev().map(|(k, v)| (k.clone(), v.clone()));
            list[at] = Json::Object(reversed.collect());
        }),
    ];
    for (n, bad) in bads().iter().enumerate() {
        let file = file_with(&[bad]);
        let mut core = open(&format!("tamper-{n}"), &file, None);
        let original = now(&core);
        let list = file[bad.list].as_array().unwrap();
        let at = list.iter().position(|e| *e == bad.entry).unwrap();
        let read_alike = !matches!(
            bad.name,
            "malformed as it arrives" | "past a bound as it arrives"
        );
        for (how, tamper) in tampers {
            let edit = tick(&item(&core, "task", "chore"));
            let answer = through_the_probe(&mut core, &edit, |text, changes| {
                let mut new = honest(text, changes);
                tamper(new[bad.list].as_array_mut().unwrap(), at, bad);
                new
            });
            if how == "reorder" && read_alike {
                assert!(answer.is_ok(), "{}, {how}: {:?}", bad.name, answer.err());
                continue;
            }
            let error = answer
                .err()
                .unwrap_or_else(|| panic!("{}, {how}: written", bad.name));
            assert_eq!(
                error.reason,
                Reason::ReadBack,
                "{}, {how}: {error}",
                bad.name
            );
            assert!(
                error.message.contains(&bad.says),
                "{}, {how}: names it: {error}",
                bad.name
            );
            assert!(error.message.contains("nothing was written"), "{error}");
            assert_eq!(now(&core), original, "{}, {how}", bad.name);
        }
        // Kept as it was: the edit goes ahead, for one save and the next.
        for _ in 0..3 {
            let edit = tick(&item(&core, "task", "chore"));
            let answer = through_the_probe(&mut core, &edit, honest);
            assert!(answer.is_ok(), "{}, kept: {:?}", bad.name, answer.err());
        }
        let _ = std::fs::remove_dir_all(core.root());
    }
}

/// Where an item left out shares a place with the good item an edit names (the first at
/// the place left out as it arrives, or the second at a taken place), the edit lands on the
/// good one, and a probe that writes it into the other is refused, naming it.
#[test]
fn an_edit_lands_only_on_the_item_it_names_where_a_bad_item_shares_its_place() {
    let stray = |mut entry: Json| {
        entry["colour"] = json!("red");
        entry
    };
    let mut items = Vec::new();
    for g in good() {
        items.push(stray(g.clone()));
        items.push(g.clone());
        items.push(g);
    }
    let file = json!({"items": items, "links": []});
    let mut core = open("shared", &file, None);
    assert_eq!(core.index.workspace.owned[0].given.left_out.len(), 4);
    for k in 0..edits(&core).len() {
        let (what, edit) = edits(&core).remove(k);
        let text = now(&core);
        let raw: Json = serde_json::from_str(&text).unwrap();
        let entries = entries(&raw);
        // Into the stray entry before it, as written with its stray key, and into the second.
        for into in ["first", "second"] {
            let answer = through_the_probe(&mut core, &edit, |text, changes| {
                let mut new: Json = serde_json::from_str(text).unwrap();
                let honest = honest(text, changes);
                let edited = changed_entries(text, changes)[0];
                let written = honest["items"][edited].clone();
                match into {
                    "first" => new["items"][edited - 1] = stray(written),
                    _ => new["items"][edited + 1] = written,
                }
                new
            });
            let error = answer
                .err()
                .unwrap_or_else(|| panic!("{what}, {into}: written"));
            assert_eq!(error.reason, Reason::ReadBack, "{what}, {into}: {error}");
            let says = match into {
                "first" => ": unknown field `colour`",
                _ => ": another ",
            };
            assert!(error.message.contains(says), "{what}, {into}: {error}");
            assert_eq!(now(&core), text, "{what}, {into}");
        }
        let mut named = Vec::new();
        let answer = through_the_probe(&mut core, &edit, |text, changes| {
            named = changed_entries(text, changes);
            honest(text, changes)
        });
        assert!(answer.is_ok(), "{what}: {:?}", answer.err());
        let after: Json = serde_json::from_str(&now(&core)).unwrap();
        assert_eq!(named.len(), 1, "{what}");
        assert!(entries.contains(&named[0]), "{what}: an entry that reads");
        assert_eq!(untouched(&after, &named), untouched(&raw, &named), "{what}");
    }
    let _ = std::fs::remove_dir_all(core.root());
}

/// One rule for a change to an owned file, an item edit's as a relink's: refused before the
/// owner is asked when it would leave out something new, in the rule's words, which name the
/// item.
#[test]
fn an_edit_that_would_leave_out_its_item_is_refused_in_the_rules_words() {
    let bads = bads();
    let mut core = open("new", &file_with(&[&bads[0]]), None);
    let original = now(&core);
    let long = "w".repeat((1 << 20) + 1);
    let edit = set(&item(&core, "task", "chore"), "text", json!(long));
    let error = core.edit_item("agenda", &edit).unwrap_err();
    assert_eq!(error.reason, Reason::Refused, "{error}");
    assert_eq!(
        error.message,
        "A.probe: the change would leave out chore: its text is longer than 1 MiB"
    );
    assert!(core.take_file_edits().is_empty(), "the probe isn't asked");
    assert_eq!(now(&core), original);
    let _ = std::fs::remove_dir_all(core.root());
}

/// After an edit beside a bad item, the owners' cache holds what was left out, through a
/// second and a third save, and a core that starts from the cache alone edits as well.
#[test]
fn the_cache_keeps_what_was_left_out_through_every_edit() {
    let bads = bads();
    let all: Vec<&Bad> = bads.iter().collect();
    let base = std::env::temp_dir().join(format!("cold-lava-bad-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let cache = base.join("owners");
    std::fs::create_dir_all(&cache).unwrap();
    let mut core = open("cached", &file_with(&all), Some(&cache));
    let dir = core.root().to_path_buf();
    let saved = || -> usize {
        let files = std::fs::read_dir(&cache)
            .unwrap()
            .map(|e| e.unwrap().path());
        let files: Vec<_> = files.collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let json: Json = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
        json["files"][FILE]["left_out"]
            .as_array()
            .map_or(0, Vec::len)
    };
    assert_eq!(saved(), 2);
    for save in 1..=3 {
        let edit = tick(&item(&core, "task", "chore"));
        through_the_probe(&mut core, &edit, honest).unwrap();
        assert_eq!(saved(), 2, "save {save}");
        // From the cache alone: the probe isn't asked, and an edit goes ahead.
        drop(core);
        core = Core::open(&dir).unwrap();
        let kind = OwnedKind {
            extension: "probe".into(),
            owner: OWNER.into(),
            version: "1".into(),
        };
        core.set_owners(vec![kind], Some(&cache));
        assert!(core.take_reads().is_empty(), "save {save}: from the cache");
        core.poll();
        assert_eq!(core.index.workspace.owned[0].given.left_out.len(), 2);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&base);
}

/// A board whose cards are left out (an empty id, an id over 1024 bytes, a second node at a
/// taken place) still has its other cards' tasks ticked, and a tick writes only the node
/// whose text lists the task: every other node keeps its bytes. Boards of two nodes each, in
/// every pairing of those ids and of a task, an empty text, a text with no task and a file,
/// beside a good card (the shape of #300's two hundred boards).
#[test]
fn a_tick_on_a_board_with_cards_left_out_writes_only_its_own_card() {
    let long = "x".repeat(1025);
    let ids = ["t", "t\u{1f}1", "", long.as_str()];
    let card = |n: usize, id: &str, what: usize| {
        let mut node = json!({"id": id, "type": "text", "x": n, "y": 0, "width": 1, "height": 1});
        match what {
            0 => node["text"] = json!(format!("- [ ] n{n}")),
            1 => node["text"] = json!(""),
            2 => node["text"] = json!(format!("plain n{n}")),
            _ => {
                node["type"] = json!("file");
                node["file"] = json!("Lava.md");
            }
        }
        node
    };
    let words = |item: &Item| serde_json::to_value(item).unwrap()["text"].clone();
    let dir = std::env::temp_dir().join(format!("cold-lava-bad-boards-{}", std::process::id()));
    let board_now = || -> Json {
        serde_json::from_slice(&std::fs::read(dir.join("Board.canvas")).unwrap()).unwrap()
    };
    let mut boards = 0;
    for (a, b) in (0..16).flat_map(|a| (0..16).map(move |b| (a, b))) {
        let good = card(9, "g", 0);
        let nodes = vec![card(0, ids[a / 4], a % 4), card(1, ids[b / 4], b % 4), good];
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Lava.md"), "# Lava\n").unwrap();
        let board = json!({"nodes": nodes, "edges": []});
        std::fs::write(dir.join("Board.canvas"), board.to_string()).unwrap();
        let mut core = Core::open(&dir).unwrap();
        let listed: Vec<Item> = core.index.workspace.owned[0].items.clone();
        let tasks: Vec<&Item> = listed.iter().filter(|i| i.data.kind() == "task").collect();
        assert!(
            tasks.iter().any(|t| words(*t) == "n9"),
            "board {a}/{b}: the good card is listed"
        );
        for task in tasks {
            let said = words(task);
            let file = &core.index.workspace.owned[0];
            let task = (file.items.iter())
                .find(|i| words(*i) == said)
                .unwrap()
                .clone();
            let before = board_now();
            let asked = core.edit_item("agenda", &tick(&task));
            assert!(matches!(asked, Ok(Answer::Later(_))), "{a}/{b}: {asked:?}");
            let (_, _, answer) = core.take_edited().pop().expect("answered at once");
            assert!(answer.unwrap().changed, "board {a}/{b}, {said}");
            let after = board_now();
            let (was, now) = (
                before["nodes"].as_array().unwrap(),
                after["nodes"].as_array().unwrap(),
            );
            let changed: Vec<usize> = (0..was.len()).filter(|&i| was[i] != now[i]).collect();
            assert_eq!(changed.len(), 1, "board {a}/{b}, {said}: {changed:?}");
            let node = changed[0];
            let text = |done: &str| format!("- [{done}] {}", said.as_str().unwrap());
            assert_eq!(was[node]["text"], json!(text(" ")), "board {a}/{b}: {said}");
            let ticked = now[node]["text"].as_str().unwrap_or_default();
            assert!(ticked.starts_with(&text("x")), "board {a}/{b}: {ticked}");
            core.poll();
        }
        boards += 1;
    }
    assert_eq!(boards, 256);
    let _ = std::fs::remove_dir_all(&dir);
}
