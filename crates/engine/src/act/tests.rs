use super::*;
use crate::core::Event;
use crate::shapes::{self, Layer, Shapes, load};
use serde_json::json;
use std::path::PathBuf;

/// A workspace of volcanoes (their tags read from the text, as the example workspace's are; a
/// `visited` property `set-field` can write), a note of tasks, new notes going to `Inbox`, and
/// `shapes` as its own shapes file.
fn workspace(name: &str, shapes: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cold-lava-act-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
    std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
    std::fs::create_dir_all(dir.join("Inbox")).unwrap();
    std::fs::write(
        dir.join(".obsidian/app.json"),
        r#"{"newFileLocation": "folder", "newFileFolderPath": "Inbox"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join(".cold-lava/types.toml"),
        "version = 1\n[types.volcano]\non = \"note\"\nwhere.tag = [\"volcano\"]\n\
         fields.name = { note = \"name\" }\nfields.tags = { inlines = \"tags\", type = \"tag\" }\n\
         fields.visited = { property = \"visited\" }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(".cold-lava/shapes.toml"),
        format!("version = 1\n{shapes}"),
    )
    .unwrap();
    for (name, tags) in [
        ("Krakatoa", "volcano, ring-of-fire"),
        ("Mount Fuji", "volcano, ring-of-fire"),
        ("Vesuvius", "volcano"),
    ] {
        let text = format!("---\ntags: [{tags}]\n---\n# {name}\n");
        std::fs::write(dir.join(format!("{name}.md")), text).unwrap();
    }
    std::fs::write(
        dir.join("Trip.md"),
        "- [ ] Pack ash masks\n- [ ] Book the ferry\n",
    )
    .unwrap();
    dir
}

fn bind(core: &mut Core, shapes: &Shapes, shape: &str, pairs: &[(&str, &str)]) -> Binding {
    let sources = (pairs.iter()).map(|(n, s)| (n.to_string(), Source::parse(s).unwrap()));
    Binding::new(core, shapes, shape, sources).unwrap()
}

fn labels(b: &Binding, region: &str) -> Vec<String> {
    let shown = b.shown();
    let r = shown.regions.iter().find(|r| r.id == region).unwrap();
    r.items.iter().map(|e| e.label.clone()).collect()
}

fn ran(a: Result<Acted, Error>) -> Done {
    match a.unwrap() {
        Acted::Ran(done) => done,
        Acted::Ask(ask) => panic!("asked its extension: {ask:?}"),
    }
}

/// Hands the core's news to the binding, as a frontend does each frame.
fn follow(core: &mut Core, b: &mut Binding) {
    for (owner, news) in core.take_item_news() {
        assert_eq!(owner, OWNER);
        b.heard(core, &news);
    }
}

fn edited_by(core: &mut Core) -> Vec<(String, String)> {
    (core.poll().into_iter())
        .filter_map(|e| match e {
            Event::ItemEdited { by, what, .. } => Some((by, what)),
            _ => None,
        })
        .collect()
}

/// Two circles: what is on the list, and what was visited; dropping an item into both marks it
/// visited, with the list's title.
const VISITS: &str = r#"
[[shapes]]
id = "visits"
inputs = ["a", "b"]
key = "name"
parts = [
  { circle = { id = "left", center = [0.38, 0.5], radius = 0.32 } },
  { circle = { id = "right", center = [0.62, 0.5], radius = 0.32 } },
]
regions = [
  { id = "only-a", area = "left - right", shows = "a - b" },
  { id = "both", area = "left & right", shows = "a & b" },
]

[[shapes.on]]
drop = { from = "only-a", to = "both" }
edit = { item = "dropped", set-field = { field = "visited", value = "${a.title}" } }
"#;

#[test]
fn a_drop_edits_the_dropped_item_as_the_canvas() {
    let dir = workspace("drop", VISITS);
    let mut core = Core::open(&dir).unwrap();
    let shapes = shapes::for_workspace(&dir);
    let mut b = bind(
        &mut core,
        &shapes,
        "visits",
        &[
            ("a", "type=volcano tag=ring-of-fire title=\"Ring of Fire\""),
            ("b", "type=volcano where visited"),
        ],
    );
    assert_eq!(labels(&b, "only-a"), ["Krakatoa", "Mount Fuji"]);
    let fuji = b.shown().regions[0].items[1].id.clone();

    let drop = Gesture::Drop {
        from: "only-a".into(),
        to: "both".into(),
        item: fuji.clone(),
    };
    let done = ran(b.act(&mut Direct::user(&mut core), &shapes, &drop, 1));
    assert_eq!(
        done.steps,
        [Did::Edited {
            item: fuji,
            later: false
        }]
    );
    let text = std::fs::read_to_string(dir.join("Mount Fuji.md")).unwrap();
    assert!(text.contains("visited: Ring of Fire"), "{text}");
    assert_eq!(
        edited_by(&mut core),
        [("canvas".to_string(), "set visited".to_string())],
        "attributed to the canvas"
    );
    follow(&mut core, &mut b);
    assert_eq!(labels(&b, "only-a"), ["Krakatoa"]);
    assert_eq!(labels(&b, "both"), ["Mount Fuji"]);

    let other = Gesture::Drop {
        from: "both".into(),
        to: "only-a".into(),
        item: "x".into(),
    };
    let refused = b
        .act(&mut Direct::user(&mut core), &shapes, &other, 1)
        .unwrap_err();
    assert_eq!(refused.reason, Reason::Refused);
    assert_eq!(
        refused.message,
        "visits does nothing on a drop from both to only-a"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_built_in_venns_drop_is_refused_where_tags_are_in_the_text() {
    let dir = workspace("venn-drop", "");
    let mut core = Core::open(&dir).unwrap();
    let shapes = load([Layer::built_in()]);
    let b = bind(
        &mut core,
        &shapes,
        "venn2",
        &[
            ("a", "type=volcano tag=volcano"),
            ("b", "type=volcano tag=ring-of-fire"),
        ],
    );
    assert_eq!(labels(&b, "only-a"), ["Vesuvius"]);
    let drop = Gesture::Drop {
        from: "only-a".into(),
        to: "both".into(),
        item: b.shown().regions[0].items[0].id.clone(),
    };
    let before = std::fs::read_to_string(dir.join("Vesuvius.md")).unwrap();
    let e = b
        .act(&mut Direct::user(&mut core), &shapes, &drop, 1)
        .unwrap_err();
    assert_eq!(e.reason, Reason::Refused);
    assert!(e.message.contains("can't be written"), "{}", e.message);
    assert_eq!(
        std::fs::read_to_string(dir.join("Vesuvius.md")).unwrap(),
        before
    );
    assert!(edited_by(&mut core).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn add_and_remove_change_a_list_once_folded() {
    let mut m = Member::label("Krakatoa");
    m.fields
        .insert("tags".into(), json!(["volcano", "Ring-of-Fire"]));
    m.fields.insert("country".into(), json!("Indonesia"));
    let add = |v: &str| listed(&m, "tags", v, true).unwrap();
    assert_eq!(
        add("caldera"),
        json!(["volcano", "Ring-of-Fire", "caldera"])
    );
    assert_eq!(
        add("#ring-of-fire"),
        json!(["volcano", "Ring-of-Fire"]),
        "already there"
    );
    assert_eq!(
        listed(&m, "tags", "ring-of-fire", false).unwrap(),
        json!(["volcano"])
    );
    assert_eq!(
        listed(&m, "aliases", "Krakatau", true).unwrap(),
        json!(["Krakatau"])
    );
    let e = listed(&m, "country", "Java", true).unwrap_err();
    assert_eq!(
        e.message,
        "country isn't a list, so nothing is added to or taken from it"
    );
}

const CHECKLIST: &str = r#"
[[shapes]]
id = "checklist"
inputs = ["tasks"]
parts = [{ rect = { id = "box", from = [0, 0], to = [1, 1] } }]
regions = [{ id = "open", area = "box", shows = "tasks where status_type != done" }]

[[shapes.on]]
click = "open"
edit = { item = "clicked", toggle = true }

[[shapes.on]]
key = "r"
edit = { recipe = "rename-tag", options = { from = "${tasks.tag}", to = "fire-ring" } }
"#;

#[test]
fn a_click_ticks_the_clicked_task_and_a_key_runs_a_recipe() {
    let dir = workspace("click", CHECKLIST);
    let mut core = Core::open(&dir).unwrap();
    let shapes = shapes::for_workspace(&dir);
    assert!(shapes.errors.is_empty(), "{:?}", shapes.errors);
    let mut b = bind(
        &mut core,
        &shapes,
        "checklist",
        &[("tasks", "type=task note=Trip.md")],
    );
    assert_eq!(labels(&b, "open"), ["Pack ash masks", "Book the ferry"]);
    let pack = b.shown().regions[0].items[0].id.clone();

    let click = Gesture::Click {
        on: "open".into(),
        item: Some(pack),
    };
    ran(b.act(&mut Direct::user(&mut core), &shapes, &click, 1));
    let text = std::fs::read_to_string(dir.join("Trip.md")).unwrap();
    assert!(text.starts_with("- [x] Pack ash masks"), "{text}");
    assert_eq!(
        edited_by(&mut core),
        [("canvas".to_string(), "ticked".to_string())]
    );
    follow(&mut core, &mut b);
    assert_eq!(
        labels(&b, "open"),
        ["Book the ferry"],
        "done: no longer open"
    );

    let mut volcanoes = bind(
        &mut core,
        &shapes,
        "checklist",
        &[("tasks", "type=volcano tag=ring-of-fire")],
    );
    let done = ran(volcanoes.act(
        &mut Direct::user(&mut core),
        &shapes,
        &Gesture::Key { key: "r".into() },
        2,
    ));
    assert_eq!(
        done.steps,
        [Did::Recipe {
            title: "rename-tag from=ring-of-fire to=fire-ring".into()
        }]
    );
    let text = std::fs::read_to_string(dir.join("Krakatoa.md")).unwrap();
    assert!(
        text.contains("fire-ring") && !text.contains("ring-of-fire"),
        "{text}"
    );
    core.poll();
    follow(&mut core, &mut volcanoes);
    assert_eq!(
        labels(&volcanoes, "open"),
        Vec::<String>::new(),
        "no note has the old tag now"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn make_it_a_note_creates_the_note_where_obsidian_would_and_rebinds_as_a_card() {
    let dir = workspace("note", "");
    let mut core = Core::open(&dir).unwrap();
    let shapes = load([Layer::built_in()]);
    let mut sticky = bind(
        &mut core,
        &shapes,
        "sticky",
        &[("text", "text:Ash masks\nTwo per person.")],
    );

    let make = Gesture::Action {
        name: "make-a-note".into(),
    };
    let done = ran(sticky.act(&mut Direct::user(&mut core), &shapes, &make, 3));
    assert_eq!(
        done.steps,
        [Did::Created {
            path: "Inbox/Ash masks.md".into()
        }]
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("Inbox/Ash masks.md")).unwrap(),
        "Ash masks\nTwo per person."
    );
    let rebind = done.rebind.expect("a rebind");
    assert_eq!(
        rebind,
        Rebind {
            shape: "card".into(),
            inputs: IndexMap::from([("note".into(), Source::Note("Inbox/Ash masks.md".into()))]),
        }
    );
    sticky.rebind(&mut core, &shapes, &rebind).unwrap();
    assert_eq!(sticky.shape().id, "card");
    assert_eq!(sticky.value("note.title"), "Ash masks");
    assert_eq!(sticky.value("note.excerpt"), "Two per person.");

    // A second time, the name is taken: Obsidian's numbering.
    let again = bind(&mut core, &shapes, "sticky", &[("text", "text:Ash masks")]);
    let done = ran(again.act(&mut Direct::user(&mut core), &shapes, &make, 4));
    assert_eq!(
        done.steps,
        [Did::Created {
            path: "Inbox/Ash masks 1.md".into()
        }]
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A gateway that may do nothing, as an extension's `Host` without permissions answers.
struct Forbidden;

impl Gateway for Forbidden {
    fn edit_item(&mut self, _: &Edit) -> Result<Answer, Error> {
        Err(Error::new(Reason::Forbidden, "no items.edit"))
    }
    fn create_file(&mut self, _: &Create) -> Result<Created, Error> {
        Err(Error::new(Reason::Forbidden, "no files.write"))
    }
    fn run_recipe(&mut self, _: &str, _: recipe::Options) -> Result<String, Error> {
        Err(Error::new(Reason::Forbidden, "no files.write"))
    }
}

#[test]
fn a_refused_step_stops_the_rest() {
    let dir = workspace("refused", "");
    let mut core = Core::open(&dir).unwrap();
    let shapes = load([Layer::built_in()]);
    let sticky = bind(&mut core, &shapes, "sticky", &[("text", "text:Nothing")]);
    let make = Gesture::Action {
        name: "make-a-note".into(),
    };
    let e = sticky.act(&mut Forbidden, &shapes, &make, 1).unwrap_err();
    assert_eq!(e.reason, Reason::Forbidden);
    assert!(!dir.join("Inbox/Nothing.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}

const SHIPPED: &str = r#"
inputs = ["a"]
parts = [{ rect = { id = "box", from = [0, 0], to = [1, 1] } }]
regions = [{ id = "count", area = "box", extension = true }]

[[on]]
click = "count"
extension = true
"#;

#[test]
fn an_extensions_shape_asks_it_for_a_region_and_a_gesture() {
    let dir = workspace("ext", "");
    let mut core = Core::open(&dir).unwrap();
    let table: toml::Table = toml::from_str(SHIPPED).unwrap();
    let shapes = load([
        Layer::built_in(),
        Layer::extension("tally", [("tally.box".to_string(), table)]),
    ]);
    assert!(shapes.errors.is_empty(), "{:?}", shapes.errors);
    let mut b = bind(
        &mut core,
        &shapes,
        "tally.box",
        &[("a", "type=volcano tag=ring-of-fire")],
    );
    assert_eq!(b.extension(), Some("tally"));

    let asks = b.region_asks(7);
    let [
        Ask::Region {
            placed: 7,
            region,
            inputs,
            ..
        },
    ] = &asks[..]
    else {
        panic!("{asks:?}");
    };
    assert_eq!(region, "count");
    assert_eq!(inputs["a"].title, "volcano #ring-of-fire");
    assert_eq!(inputs["a"].members.len(), 2);
    assert_eq!(
        labels(&b, "count"),
        Vec::<String>::new(),
        "until it answers"
    );
    let reply: Reply =
        serde_json::from_value(json!({"items": [{"label": "2 volcanoes"}]})).unwrap();
    let Ok(Answered::Region { items }) = reply.read(&asks[0], &shapes) else {
        panic!()
    };
    b.answered("count", items);
    assert_eq!(labels(&b, "count"), ["2 volcanoes"]);

    let krakatoa = b.data()["a"].members[0].id.clone();
    let click = Gesture::Click {
        on: "count".into(),
        item: Some(krakatoa),
    };
    let Acted::Ask(ask) = b.act(&mut Forbidden, &shapes, &click, 7).unwrap() else {
        panic!("its extension decides");
    };
    assert_eq!(ask.placed(), 7);
    let reply: Reply = serde_json::from_value(json!({"do": [
        {"edit": {"item": "clicked", "set-field": {"field": "visited", "value": "${clicked.name}"}}}
    ]}))
    .unwrap();
    let Ok(Answered::Gesture { steps }) = reply.read(&ask, &shapes) else {
        panic!()
    };
    let e = b.run(&mut Forbidden, &click, &steps).unwrap_err();
    assert_eq!(
        e.reason,
        Reason::Forbidden,
        "its steps go through its own gateway"
    );
    b.run(
        &mut Direct {
            core: &mut core,
            by: "tally",
        },
        &click,
        &steps,
    )
    .unwrap();
    assert_eq!(
        edited_by(&mut core),
        [("tally".to_string(), "set visited".to_string())]
    );
    let text = std::fs::read_to_string(dir.join("Krakatoa.md")).unwrap();
    assert!(text.contains("visited: Krakatoa"), "{text}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn extension_regions_and_gestures_are_an_extensions_only() {
    let mine = format!(
        "[[shapes]]\nid = \"tally\"\n{}",
        SHIPPED.replace("[[on]]", "[[shapes.on]]")
    );
    let dir = workspace("not-ext", &mine);
    let found = shapes::for_workspace(&dir);
    assert!(found.get("tally").is_none());
    assert!(
        found.errors.iter().any(|e| e.ends_with(
            "shape tally: extension = true hands a region or a gesture to the extension that ships the shape, and this one comes from none"
        )),
        "{:?}",
        found.errors
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_messages_on_the_wire() {
    let wire = |v: serde_json::Value| v;
    assert_eq!(
        serde_json::to_value(Gesture::Drop {
            from: "only-a".into(),
            to: "both".into(),
            item: "i1".into()
        })
        .unwrap(),
        wire(json!({"gesture": "drop", "from": "only-a", "to": "both", "item": "i1"}))
    );
    assert_eq!(
        serde_json::from_value::<Gesture>(json!({"gesture": "click", "on": "count"})).unwrap(),
        Gesture::Click {
            on: "count".into(),
            item: None
        }
    );
    let ask = Ask::Region {
        placed: 7,
        shape: "tally.box".into(),
        region: "count".into(),
        inputs: IndexMap::from([(
            "a".to_string(),
            InputData {
                title: "volcano".into(),
                members: vec![MemberData {
                    id: "i1".into(),
                    rev: Some("r1".into()),
                    fields: json!({"name": "Krakatoa"}).as_object().unwrap().clone(),
                }],
            },
        )]),
    };
    let json = json!({"method": "shape/region", "params": {
        "placed": 7, "shape": "tally.box", "region": "count",
        "inputs": {"a": {"title": "volcano", "members": [
            {"id": "i1", "rev": "r1", "fields": {"name": "Krakatoa"}}
        ]}}
    }});
    assert_eq!(serde_json::to_value(&ask).unwrap(), json);
    assert_eq!(serde_json::from_value::<Ask>(json).unwrap(), ask);
    let gesture = Ask::Gesture {
        placed: 7,
        shape: "tally.box".into(),
        gesture: Gesture::Key { key: "r".into() },
        inputs: IndexMap::new(),
    };
    assert_eq!(
        serde_json::to_value(&gesture).unwrap(),
        json!({"method": "shape/gesture", "params": {
            "placed": 7, "shape": "tally.box", "gesture": {"gesture": "key", "key": "r"}, "inputs": {}
        }})
    );
    assert_eq!(
        serde_json::to_value(Reply::Region {
            items: vec![Entry {
                id: "i1".into(),
                label: "Krakatoa".into()
            }]
        })
        .unwrap(),
        json!({"items": [{"id": "i1", "label": "Krakatoa"}]})
    );
    let done = Done {
        steps: vec![Did::Created {
            path: "Inbox/A.md".into(),
        }],
        rebind: Some(Rebind {
            shape: "card".into(),
            inputs: IndexMap::from([("note".into(), Source::Note("Inbox/A.md".into()))]),
        }),
    };
    assert_eq!(
        serde_json::to_value(&done).unwrap(),
        json!({"steps": [{"step": "created", "path": "Inbox/A.md"}],
               "rebind": {"shape": "card", "inputs": {"note": {"note": "Inbox/A.md"}}}})
    );
}
