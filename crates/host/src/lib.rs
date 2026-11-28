//! The extension host: what runs cold-lava's extensions and guards the workspace gateway, the
//! same for every frontend (the terminal UI today, any other tomorrow). It has no user
//! interface and must not depend on ratatui, crossterm, portable-pty or vt100; frontends depend
//! on it, and it depends on the engine, never the other way.
//!
//! This is the JSON-RPC protocol of docs/extensions.md, called in-process: each `Extension`
//! method is one host→extension method, each `Host` method one extension→host method, and every
//! type that crosses derives serde, so extensions in other processes share this code path.
//! Extensions never see keystrokes and never draw: the frontend owns the panes they open. What an
//! extension asks for is queued as `Effect`s, which the frontend applies after the call returns.
//! Notes as data are the exception: `items/*`, `types/list` and the file messages
//! (`workspace/read`, …) are answered during the call by the core (the engine's `gateway` and
//! `files`), within the permissions the extension's manifest asks for. An extension can
//! own a kind of file (`FileType`): the frontend hands it those files (`file/open`) and their
//! outside changes (`file/changed`), and asks it what each says as items and links (`file/read`).
//! Extensions installed on disk (`installed`) run in processes of their own (`process`), over a
//! `transport` that frames JSON-RPC messages (`frame`, `rpc`); they reach the same `Host`.
//!
//! What a frontend keeps for itself: its built-in extensions (the terminal's Neovim pane, its
//! agenda, table and graph), applying `Effect`s, its command table and keys, and drawing views
//! (the view gateway's messages are `cold-lava-views`' `view`, named here as `view`).

pub mod frame;
pub mod installed;
pub mod process;
pub mod rpc;
pub mod transport;

mod deliver;
mod extension;
mod host;
mod manifest;
mod protocol;
mod registry;

pub use cold_lava_views::view;

pub use deliver::{Deliveries, Delivery, Next, Step, after_sync, frame};
pub use extension::Extension;
pub use host::{ExtId, Host};
pub use manifest::{CommandSpec, FileType, Manifest, Permission, Role, Shows, Spec, namespaced};
pub use protocol::{
    CommandRequest, EditorRequest, Effect, FileNews, FilePreview, PaneId, PaneNews, PaneSpec,
};
pub use registry::{AGENDA, GRAPH, KEPT, MENU, NVIM, READER, Registry, TABLE};

#[cfg(test)]
use cold_lava_engine::{act, bind, shapes, shows};
use cold_lava_engine::{
    files, gateway, links, matcher, owner, paths, recipe, shape_ask, style, types,
};
use manifest::read_only;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape_ask::Ask;
    use cold_lava_engine::core::{Core, Jump};
    use cold_lava_engine::files::Create;
    use cold_lava_engine::gateway::{Answer, Edit, News, Query, Reason, Select, SubId};
    use cold_lava_engine::problem::Problem;
    use serde::Serialize;
    use serde_json::Value;
    use std::path::PathBuf;
    use view::{Content, ViewId};

    /// Each problem as it is said on one line.
    fn said(problems: &[Problem]) -> Vec<String> {
        problems.iter().map(Problem::one_line).collect()
    }
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use std::fmt::Debug;

    /// `value` is exactly `json` on the wire, and reads back from it.
    fn wire<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: T, json: Value) {
        assert_eq!(serde_json::to_value(&value).unwrap(), json);
        assert_eq!(serde_json::from_value::<T>(json).unwrap(), value);
    }

    #[test]
    fn effects_are_json_rpc_messages() {
        let pane = PaneId(3);
        wire(
            Effect::Notify {
                message: "Neovim closed".into(),
            },
            json!({"method": "ui/notify", "params": {"message": "Neovim closed"}}),
        );
        wire(
            Effect::EditorOpened {
                note: "/v/Lava.md".into(),
            },
            json!({"method": "editor/opened", "params": {"note": "/v/Lava.md"}}),
        );
        wire(
            Effect::RunCommand {
                id: "search.open".into(),
                args: json!({"query": "ash"}),
            },
            json!({"method": "command/run", "params": {"id": "search.open", "args": {"query": "ash"}}}),
        );
        wire(
            Effect::Saved {
                note: "/v/Lava.md".into(),
            },
            json!({"method": "workspace/saved", "params": {"note": "/v/Lava.md"}}),
        );
        wire(
            Effect::OpenPane {
                pane,
                spec: PaneSpec {
                    program: "nvim".into(),
                    args: vec!["/v/Lava.md".into()],
                },
            },
            json!({"method": "pane/open", "params": {
                "pane": 3,
                "spec": {"program": "nvim", "args": ["/v/Lava.md"]},
            }}),
        );
        wire(
            Effect::PaneInput {
                pane,
                text: ":w\r".into(),
            },
            json!({"method": "pane/input", "params": {"pane": 3, "text": ":w\r"}}),
        );
        let view = ViewId(4);
        let content = || {
            Content::List(cold_lava_views::list::List {
                title: "Open".into(),
                groups: Vec::new(),
                keys: Vec::new(),
                empty: None,
                filter: Default::default(),
            })
        };
        let list = json!({"kind": "list", "title": "Open", "groups": []});
        wire(
            Effect::OpenView {
                view,
                content: content(),
                select: None,
                file: None,
            },
            json!({"method": "view/open", "params": {"view": 4, "content": list}}),
        );
        wire(
            Effect::OpenView {
                view,
                content: content(),
                select: None,
                file: Some("Volcanoes/Eruptions.csv".into()),
            },
            json!({"method": "view/open", "params": {"view": 4, "content": list, "file": "Volcanoes/Eruptions.csv"}}),
        );
        wire(
            Effect::UpdateView {
                view,
                content: content(),
                select: Some("r1".into()),
            },
            json!({"method": "view/update", "params": {"view": 4, "content": list, "select": "r1"}}),
        );
        wire(
            Effect::CloseView { view },
            json!({"method": "view/close", "params": {"view": 4}}),
        );
        wire(
            Effect::ShowView { view },
            json!({"method": "view/show", "params": {"view": 4}}),
        );
        wire(
            Effect::OpenNote {
                note: "Plan.md".into(),
                line: Some(3),
                item: None,
            },
            json!({"method": "workspace/open", "params": {"note": "Plan.md", "line": 3}}),
        );
        wire(
            Effect::OpenNote {
                note: "Volcanoes/Eruptions.csv".into(),
                line: None,
                item: Some("6f3e".into()),
            },
            json!({"method": "workspace/open",
                   "params": {"note": "Volcanoes/Eruptions.csv", "item": "6f3e"}}),
        );
    }

    #[test]
    fn host_calls_are_json_rpc_messages() {
        let pane = PaneId(3);
        let open = |jump| EditorRequest::Open {
            note: "/v/Lava.md".into(),
            jump,
        };
        wire(
            open(Some(Jump::Heading("Flows".into()))),
            json!({"method": "editor/open", "params": {"note": "/v/Lava.md", "jump": {"heading": "Flows"}}}),
        );
        wire(
            open(Some(Jump::Line(12))),
            json!({"method": "editor/open", "params": {"note": "/v/Lava.md", "jump": {"line": 12}}}),
        );
        wire(
            open(Some(Jump::Block("ab12".into()))),
            json!({"method": "editor/open", "params": {"note": "/v/Lava.md", "jump": {"block": "ab12"}}}),
        );
        wire(
            open(None),
            json!({"method": "editor/open", "params": {"note": "/v/Lava.md", "jump": null}}),
        );
        wire(EditorRequest::Save, json!({"method": "editor/save"}));
        wire(
            EditorRequest::Renamed {
                from: "/v/Lava.md".into(),
                to: "/v/Rock/Lava.md".into(),
            },
            json!({"method": "editor/renamed", "params": {"from": "/v/Lava.md", "to": "/v/Rock/Lava.md"}}),
        );
        wire(
            EditorRequest::Deleted {
                note: "/v/Lava.md".into(),
            },
            json!({"method": "editor/deleted", "params": {"note": "/v/Lava.md"}}),
        );
        wire(
            PaneNews::Message {
                pane,
                kind: "open".into(),
                payload: "/v/Lava.md".into(),
            },
            json!({"method": "pane/message", "params": {"pane": 3, "kind": "open", "payload": "/v/Lava.md"}}),
        );
        wire(
            PaneNews::Exited {
                pane,
                error: Some("not found".into()),
            },
            json!({"method": "pane/exited", "params": {"pane": 3, "error": "not found"}}),
        );
    }

    #[test]
    fn command_run_is_a_json_rpc_message() {
        wire(
            CommandRequest::Run {
                id: "nvim.restart".into(),
                args: Value::Null,
            },
            json!({"method": "command/run", "params": {"id": "nvim.restart", "args": null}}),
        );
    }

    fn command(id: &str, keys: &[&str]) -> CommandSpec {
        CommandSpec {
            id: id.into(),
            title: format!("does {id}"),
            keys: keys.iter().map(|k| k.to_string()).collect(),
        }
    }

    #[test]
    fn manifest_is_json() {
        wire(
            Manifest {
                name: "nvim".into(),
                title: "Neovim".into(),
                version: String::new(),
                description: String::new(),
                roles: vec![Role::Editor],
                hints: vec![("Ctrl-o".into(), "back".into())],
                commands: vec![command("nvim.restart", &["ctrl+r"])],
                types: Vec::new(),
                shapes: Vec::new(),
                permissions: vec![Permission::ItemsRead, Permission::ItemsEdit],
                file_types: Vec::new(),
                shows: Vec::new(),
            },
            json!({
                "name": "nvim",
                "title": "Neovim",
                "roles": ["editor"],
                "hints": [["Ctrl-o", "back"]],
                "commands": [{"id": "nvim.restart", "title": "does nvim.restart", "keys": ["ctrl+r"]}],
                "permissions": ["items.read", "items.edit"],
            }),
        );
        let without =
            json!({"name": "wc", "title": "wc", "roles": [], "hints": [], "commands": []});
        let read = serde_json::from_value::<Manifest>(without).unwrap();
        assert_eq!(
            read.permissions,
            [Permission::ItemsRead],
            "read only by default"
        );
    }

    /// An extension that only registers commands.
    struct Commands {
        name: &'static str,
        commands: Vec<CommandSpec>,
    }

    impl Extension for Commands {
        fn initialize(&mut self) -> Manifest {
            Manifest {
                name: self.name.into(),
                title: self.name.into(),
                version: String::new(),
                description: String::new(),
                roles: Vec::new(),
                hints: Vec::new(),
                commands: self.commands.clone(),
                types: Vec::new(),
                shapes: Vec::new(),
                permissions: Vec::new(),
                file_types: Vec::new(),
                shows: Vec::new(),
            }
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
    }

    /// What a `Ticker` did and heard.
    #[derive(Default)]
    struct Log {
        subscribed: Option<Result<SubId, Reason>>,
        ticked: Option<Result<bool, Reason>>,
        heard: Vec<News>,
    }

    /// An extension that, on its command, follows the workspace's tasks and ticks the first.
    struct Ticker {
        name: &'static str,
        permissions: Vec<Permission>,
        log: std::rc::Rc<std::cell::RefCell<Log>>,
    }

    impl Extension for Ticker {
        fn initialize(&mut self) -> Manifest {
            let mut manifest = (Commands {
                name: self.name,
                commands: Vec::new(),
            })
            .initialize();
            manifest.permissions = self.permissions.clone();
            manifest
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
        fn command(&mut self, _: CommandRequest, host: &mut Host<'_>) {
            let mut log = self.log.borrow_mut();
            let tasks = Select {
                kind: "task".into(),
                ..Select::default()
            };
            log.subscribed = Some(host.subscribe_items(tasks.clone()).map_err(|e| e.reason));
            let query = Query {
                select: tasks,
                ..Query::default()
            };
            let Ok(found) = host.query_items(&query) else {
                return;
            };
            let first = &found.items[0];
            let tick = Edit {
                id: first.id.clone(),
                rev: first.rev.clone(),
                op: gateway::Op::Toggle,
            };
            log.ticked = Some(
                host.edit_item(&tick)
                    .map(|e| matches!(e, Answer::Now(e) if e.changed))
                    .map_err(|e| e.reason),
            );
        }
        fn items(&mut self, news: News, _: &mut Host<'_>) {
            self.log.borrow_mut().heard.push(news);
        }
    }

    #[test]
    fn the_gateway_answers_within_the_manifests_permissions() {
        use Permission::{ItemsEdit, ItemsRead};
        let dir = std::env::temp_dir().join(format!("cold-lava-ext-items-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(
            dir.join("Plan.md"),
            "- [ ] Pay rent\n- [ ] Call the plumber\n",
        )
        .unwrap();
        let mut core = Core::open(&dir, Default::default()).unwrap();
        let tickers = [
            ("blind", vec![]),
            ("look", vec![ItemsRead]),
            ("tick", vec![ItemsRead, ItemsEdit]),
        ];
        let logs: Vec<_> = tickers.iter().map(|_| Default::default()).collect();
        let loaded = (tickers.iter().zip(&logs)).map(|((name, permissions), log)| {
            let (name, permissions, log) = (*name, permissions.clone(), std::rc::Rc::clone(log));
            Box::new(Ticker {
                name,
                permissions,
                log,
            }) as Box<dyn Extension>
        });
        let mut r = Registry::new(loaded.collect());
        let run = |e: &mut dyn Extension, host: &mut Host<'_>| {
            e.command(
                CommandRequest::Run {
                    id: String::new(),
                    args: Value::Null,
                },
                host,
            )
        };
        for i in 0..3 {
            r.call(ExtId(i), Some(&mut core), run);
        }
        let log = |i: usize| logs[i].borrow();
        assert_eq!(log(0).subscribed, Some(Err(Reason::Forbidden)));
        assert_eq!(log(0).ticked, None, "it couldn't query either");
        assert!(matches!(log(1).subscribed, Some(Ok(_))));
        assert_eq!(log(1).ticked, Some(Err(Reason::Forbidden)));
        assert_eq!(log(2).ticked, Some(Ok(true)));
        let text = std::fs::read_to_string(dir.join("Plan.md")).unwrap();
        assert!(text.starts_with("- [x] Pay rent ✅ "), "{text}");
        let edited = core.poll().into_iter().find_map(|e| match e {
            cold_lava_engine::core::Event::ItemEdited { by, what, .. } => Some((by, what)),
            _ => None,
        });
        assert_eq!(edited, Some(("tick".into(), "ticked".into())), "attributed");

        // The menu hands each subscription's news to the extension that made it.
        r.frame(&mut core, &[]);
        for i in [1, 2] {
            let heard = &log(i).heard;
            assert!(
                matches!(&heard[..], [News::Changed { updated, .. }] if updated.len() == 1),
                "{heard:?}"
            );
        }
        assert!(log(0).heard.is_empty());

        let no_workspace = r.call(ExtId(1), None, run);
        assert!(no_workspace.is_empty());
        assert_eq!(log(1).subscribed, Some(Err(Reason::NoWorkspace)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An extension that opens files of some kinds and does nothing itself: its host is used
    /// directly, through `Registry::call`.
    struct Owner {
        name: &'static str,
        opens: &'static [&'static str],
        permissions: Vec<Permission>,
    }

    impl Extension for Owner {
        fn initialize(&mut self) -> Manifest {
            let mut manifest = (Commands {
                name: self.name,
                commands: Vec::new(),
            })
            .initialize();
            manifest.permissions = self.permissions.clone();
            manifest.file_types = (self.opens.iter())
                .map(|x| FileType {
                    extension: x.to_string(),
                    title: format!("{x} file"),
                    new: None,
                })
                .collect();
            manifest
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
    }

    /// An extension that ships the shape `tally.list` and answers for it: its region says how
    /// many volcanoes it is bound to, and a click it answers with `steps`.
    struct Tally {
        permissions: Vec<Permission>,
        steps: serde_json::Value,
    }

    const TALLY: &str = r#"
        inputs = ["a"]
        parts = [{ rect = { id = "box", from = [0, 0], to = [1, 1] } }]
        regions = [{ id = "count", area = "box", extension = true }]
        [[on]]
        click = "count"
        extension = true
    "#;

    impl Extension for Tally {
        fn initialize(&mut self) -> Manifest {
            let mut manifest = (Commands {
                name: "tally",
                commands: Vec::new(),
            })
            .initialize();
            manifest.permissions = self.permissions.clone();
            manifest
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
        fn shape(&mut self, ask: Ask, host: &mut Host<'_>) {
            let answer = match &ask {
                Ask::Region { inputs, .. } => json!({"items": [
                    {"label": format!("{} volcanoes", inputs["a"].members.len())}
                ]}),
                Ask::Gesture { .. } => json!({"do": self.steps}),
            };
            host.shape_answered(&ask, Ok(serde_json::from_value(answer).unwrap()));
        }
    }

    /// A workspace of two volcanoes whose `visited` an edit can set, and Tally's shape loaded.
    fn tally_workspace(name: &str) -> (PathBuf, crate::shapes::Shapes) {
        use crate::shapes::{Layer, load};
        let dir = std::env::temp_dir().join(format!("cold-lava-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".cold-lava")).unwrap();
        std::fs::write(
            dir.join(".cold-lava/types.toml"),
            "version = 1\n[types.volcano]\non = \"note\"\nwhere.tag = [\"volcano\"]\n\
             fields.name = { note = \"name\" }\nfields.visited = { property = \"visited\" }\n",
        )
        .unwrap();
        for name in ["Krakatoa", "Vesuvius"] {
            std::fs::write(
                dir.join(format!("{name}.md")),
                "---\ntags: [volcano]\n---\n",
            )
            .unwrap();
        }
        let table: toml::Table = toml::from_str(TALLY).unwrap();
        let shapes = load([
            Layer::built_in(),
            Layer::extension("tally", [("tally.list".to_string(), table)]),
        ]);
        assert!(shapes.errors.is_empty(), "{:?}", shapes.errors);
        (dir, shapes)
    }

    /// A click on the first volcano, in the region Tally answers for.
    fn click_on_a_volcano(core: &mut Core) -> crate::shape_ask::Gesture {
        let q = crate::gateway::Query {
            select: Select {
                kind: "volcano".into(),
                ..Default::default()
            },
            page: Default::default(),
        };
        crate::shape_ask::Gesture::Click {
            on: "count".into(),
            item: Some(core.query_items(&q).unwrap().items.remove(0).id),
        }
    }

    #[test]
    fn an_extension_answers_for_its_shape_within_its_permissions() {
        use crate::act::Acted;
        use crate::bind::{Binding, Source};
        use crate::shape_ask::Answered;
        use ItemsEdit as E;
        use ItemsRead as R;
        use Permission::{ItemsEdit, ItemsRead};
        let (dir, shapes) = tally_workspace("ext-shape");

        for (permissions, region, clicked) in [
            (
                vec![],
                Err(
                    "tally may not be told a shape's inputs: its manifest's permissions don't include items.read",
                ),
                None,
            ),
            (vec![R], Ok("2 volcanoes"), Some(Err(Reason::Forbidden))),
            (vec![R, E], Ok("2 volcanoes"), Some(Ok(()))),
        ] {
            let mut core = Core::open(&dir, Default::default()).unwrap();
            let mut r = Registry::new(vec![Box::new(Tally {
                permissions: permissions.clone(),
                steps: json!([{"edit": {"item": "clicked",
                    "set-field": {"field": "visited", "value": "yes"}}}]),
            })]);
            let source = Source::parse("type=volcano").unwrap();
            let mut b =
                Binding::new(&mut core, &shapes, "tally.list", [("a".into(), source)]).unwrap();
            assert_eq!(b.extension(), Some("tally"));
            for ask in b.region_asks(1) {
                r.ask_shape(ExtId(0), Some(&mut core), ask);
            }
            let answers = core.take_shape_answers(&shapes);
            let got = match &answers[..] {
                [a] => a.answer.clone(),
                _ => panic!("{answers:?}"),
            };
            match (got, region) {
                (Ok(Answered::Region { items }), Ok(label)) => {
                    b.answered("count", items);
                    assert_eq!(b.shown().regions[0].items[0].label, label);
                }
                (Err(why), Err(expected)) => assert_eq!(why, expected),
                (got, expected) => panic!("{permissions:?}: {got:?}, not {expected:?}"),
            }
            let Some(clicked) = clicked else {
                continue;
            };
            let click = click_on_a_volcano(&mut core);
            let mut acted = None;
            r.call(ExtId(0), Some(&mut core), |_, host| {
                acted = Some(b.act(host, &shapes, &click, 1))
            });
            let Some(Ok(Acted::Ask(ask))) = acted else {
                panic!("its extension decides: {acted:?}");
            };
            r.ask_shape(ExtId(0), Some(&mut core), ask);
            let steps = match core.take_shape_answers(&shapes).remove(0).answer {
                Ok(Answered::Gesture { steps }) => steps,
                a => panic!("{a:?}"),
            };
            let mut ran = None;
            r.call(ExtId(0), Some(&mut core), |_, host| {
                ran = Some(b.run(host, &click, &steps))
            });
            let text = std::fs::read_to_string(dir.join("Krakatoa.md")).unwrap();
            match (ran.unwrap(), clicked) {
                (Err(e), Err(reason)) => {
                    assert_eq!(e.reason, reason);
                    assert!(!text.contains("visited"), "nothing written: {text}");
                }
                (Ok(_), Ok(())) => {
                    assert!(text.contains("visited: yes"), "{text}");
                    let by = core.poll().into_iter().find_map(|e| match e {
                        cold_lava_engine::core::Event::ItemEdited { by, .. } => Some(by),
                        _ => None,
                    });
                    assert_eq!(by.as_deref(), Some("tally"), "attributed to the extension");
                }
                (got, expected) => panic!("{got:?}, not {expected:?}"),
            }
            b.close(&mut core);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The steps of an answer pass the rules a loaded shape's steps pass (#268), or the
    /// frontend is handed the reason and never steps to run: the answer is refused whole, so
    /// a step that could run before the bad one doesn't.
    #[test]
    fn steps_an_extension_answers_that_cant_run_are_refused() {
        use crate::act::Acted;
        use crate::bind::{Binding, Source};
        use Permission::{FilesWrite, ItemsEdit, ItemsRead};
        let cases = [
            (
                json!([{"edit": {"item": "clicked", "set-field": {"field": "visited"}}}]),
                "set-field takes exactly one of value, add and remove",
            ),
            (
                json!([{"create": {"text": "# Made"}}, {"rebind": {"shape": "nope"}}]),
                "its rebind names shape nope, which isn't loaded",
            ),
        ];
        for (steps, problem) in cases {
            let (dir, shapes) = tally_workspace("ext-shape-unchecked");
            let mut core = Core::open(&dir, Default::default()).unwrap();
            let mut r = Registry::new(vec![Box::new(Tally {
                permissions: vec![ItemsRead, ItemsEdit, FilesWrite],
                steps,
            })]);
            let source = Source::parse("type=volcano").unwrap();
            let mut b =
                Binding::new(&mut core, &shapes, "tally.list", [("a".into(), source)]).unwrap();
            let click = click_on_a_volcano(&mut core);
            let mut acted = None;
            r.call(ExtId(0), Some(&mut core), |_, host| {
                acted = Some(b.act(host, &shapes, &click, 1))
            });
            let Some(Ok(Acted::Ask(ask))) = acted else {
                panic!("its extension decides: {acted:?}");
            };
            r.ask_shape(ExtId(0), Some(&mut core), ask);
            let answers = core.take_shape_answers(&shapes);
            let [answer] = &answers[..] else {
                panic!("{answers:?}");
            };
            assert_eq!(answer.by, "tally");
            let refused = format!("it answered shape/gesture with steps that can't run: {problem}");
            assert_eq!(answer.answer, Err(refused));
            let mut files: Vec<String> = (std::fs::read_dir(&dir).unwrap())
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| !name.starts_with('.'))
                .collect();
            files.sort();
            assert_eq!(files, ["Krakatoa.md", "Vesuvius.md"], "nothing created");
            for name in files {
                let text = std::fs::read_to_string(dir.join(name)).unwrap();
                assert!(!text.contains("visited"), "nothing written: {text}");
            }
            b.close(&mut core);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    fn owners(extensions: Vec<Owner>) -> Registry {
        Registry::new(
            (extensions.into_iter())
                .map(|o| Box::new(o) as Box<dyn Extension>)
                .collect(),
        )
    }

    #[test]
    fn each_kind_of_file_has_one_owner_and_notes_have_none() {
        let owner = |name, opens| Owner {
            name,
            opens,
            permissions: Vec::new(),
        };
        let r = owners(vec![
            owner("sheets", &["csv", "tsv"]),
            owner("other", &["csv", "md", "kanban"]),
        ]);
        let ext = |path: &str| r.owner_of(path).map(|(e, t)| (e, t.title.clone()));
        assert_eq!(
            ext("Data/Eruptions.CSV"),
            Some((ExtId(0), "csv file".into()))
        );
        assert_eq!(ext("Board.kanban"), Some((ExtId(1), "kanban file".into())));
        assert_eq!(ext("Plan.md"), None);
        assert_eq!(ext("csv"), None, "a name without an extension");
        assert_eq!((r.owner("tsv"), r.owner("md")), (Some(ExtId(0)), None));
        assert_eq!(
            said(r.refused()),
            [
                "other and sheets both open .csv files: sheets does",
                "other can't open .md files: cold-lava does",
            ]
        );
    }

    struct Shower {
        name: &'static str,
        shows: &'static [&'static str],
    }

    impl Extension for Shower {
        fn initialize(&mut self) -> Manifest {
            let mut manifest = (Commands {
                name: self.name,
                commands: Vec::new(),
            })
            .initialize();
            manifest.shows = (self.shows.iter())
                .map(|w| Shows {
                    title: format!("{} view", self.name),
                    conditions: toml::from_str(w).unwrap(),
                })
                .collect();
            manifest
        }
        fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}
        fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}
    }

    #[test]
    fn the_most_specific_claim_shows_a_note_and_equal_claims_conflict() {
        let r = Registry::new(vec![
            Box::new(Shower {
                name: "kanban",
                shows: &[r#"property = { kanban-plugin = "basic" }"#],
            }),
            Box::new(Shower {
                name: "boards",
                shows: &[
                    r#"property = { kanban-plugin = "basic" }"#,
                    r#"property = { kanban-plugin = "basic" }
                       folder = ["Work"]"#,
                    r#"tag = ["cards"]"#,
                    r#"header = ["Front"]"#,
                    "",
                ],
            }),
        ]);
        let board = "---\nkanban-plugin: basic\n---\n## Doing\n";
        let shower = |path: &str, text: &str, tags: &[&str]| {
            let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
            (r.shower_of(path, text, &tags)).map(|(e, s)| (e, s.describe()))
        };
        assert_eq!(
            shower("Board.md", board, &[]),
            Some((ExtId(0), "kanban-plugin: basic".into()))
        );
        assert_eq!(
            shower("Work/Board.md", board, &["cards"]),
            Some((ExtId(1), "kanban-plugin: basic, in Work".into())),
            "more conditions win"
        );
        assert_eq!(
            shower("Deck.md", "# Deck\n", &["cards/spanish"]),
            Some((ExtId(1), "#cards".into())),
            "a nested tag counts"
        );
        assert_eq!(shower("Plan.md", "# Plan\n", &[]), None);
        assert_eq!(
            said(r.refused()),
            [
                "boards and kanban both show notes with kanban-plugin: basic: kanban does",
                "boards: shows \"\" was left out: its where may only name property, tag and folder",
                "boards: shows \"\" was left out: its where must name a property, a tag or a \
                 folder: it can't show every note",
            ]
        );
        assert!(r.shows(ExtId(1), 1) && !r.shows(ExtId(1), 0));
    }

    /// A kept text's hidden name ends in its file's suffix, or a staged file's `tmp`: neither
    /// is a kind its owner may read it by. Reading or stating one, or listing them, needs
    /// `files.read`, and putting one away `files.delete`, whatever kinds the asker owns.
    #[test]
    fn kept_texts_need_files_read_whatever_kinds_are_owned() {
        use Permission::{FilesDelete, FilesRead};
        let dir = std::env::temp_dir().join(format!("cold-lava-ext-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(dir.join("Eruptions.csv"), "name\n").unwrap();
        let kept = ".Eruptions.cold-lava-kept-5-0.csv";
        std::fs::write(dir.join(kept), "theirs").unwrap();
        let mut core = Core::open(&dir, Default::default()).unwrap();
        core.set_system_trash(None);
        let rev = files::read(core.root(), kept).unwrap().rev;
        let owner = |name, opens, permissions| Owner {
            name,
            opens,
            permissions,
        };
        let mut r = owners(vec![
            owner("sheets", &["csv", "tmp"], Vec::new()),
            owner("look", &[], vec![FilesRead]),
            owner("tidy", &[], vec![FilesRead, FilesDelete]),
        ]);
        let mut asked = |ext: usize, f: &dyn Fn(&mut Host<'_>) -> Result<(), gateway::Error>| {
            let mut answer = None;
            r.call(ExtId(ext), Some(&mut core), |_, host| {
                answer = Some(f(host))
            });
            answer.unwrap().map_err(|e| e.reason)
        };
        let forbidden = Err(Reason::Forbidden);
        let read = |path: &'static str| move |h: &mut Host<'_>| h.read_file(path).map(drop);
        let stat = |path: &'static str| move |h: &mut Host<'_>| h.stat_file(path).map(drop);
        let listed = |h: &mut Host<'_>| h.kept_texts().map(drop);
        let staged = ".Eruptions.csv.cold-lava-2147483647-0.tmp";
        for path in [kept, staged] {
            assert_eq!(asked(0, &read(path)), forbidden, "{path}");
            assert_eq!(asked(0, &stat(path)), forbidden, "{path}");
        }
        assert_eq!(asked(0, &listed), forbidden);
        assert_eq!(asked(1, &read(kept)), Ok(()));
        assert_eq!(asked(1, &stat(kept)), Ok(()));
        assert_eq!(asked(1, &listed), Ok(()));
        let put_away = move |h: &mut Host<'_>| h.delete_file(kept, &rev).map(drop);
        assert_eq!(asked(0, &put_away), forbidden);
        assert_eq!(asked(1, &put_away), forbidden);
        assert_eq!(asked(2, &put_away), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn owners_read_and_write_their_own_kinds_and_others_need_files_permissions() {
        use Permission::{FilesRead, FilesWrite};
        let dir = std::env::temp_dir().join(format!("cold-lava-ext-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(dir.join("Plan.md"), "- [ ] Pay rent\n").unwrap();
        std::fs::write(dir.join("Eruptions.csv"), "name\nKrakatoa\n").unwrap();
        let mut core = Core::open(&dir, Default::default()).unwrap();
        let owner = |name, opens, permissions| Owner {
            name,
            opens,
            permissions,
        };
        let mut r = owners(vec![
            owner("sheets", &["csv"], Vec::new()),
            owner("blind", &[], Vec::new()),
            owner("look", &[], vec![FilesRead]),
            owner("write", &[], vec![FilesRead, FilesWrite]),
        ]);
        let mut asked = |ext: usize, f: &dyn Fn(&mut Host<'_>) -> Result<(), gateway::Error>| {
            let mut answer = None;
            r.call(ExtId(ext), Some(&mut core), |_, host| {
                answer = Some(f(host))
            });
            answer.unwrap().map_err(|e| e.reason)
        };
        let read = |path: &'static str| move |h: &mut Host<'_>| h.read_file(path).map(drop);
        let write = |path: &'static str| {
            move |h: &mut Host<'_>| {
                let now = h.read_file(path)?;
                h.write_file(path, &format!("{}x\n", now.text), &now.rev)
                    .map(drop)
            }
        };
        let list = |x: &'static str| move |h: &mut Host<'_>| h.list_files(x).map(drop);
        let create = |path: &'static str| {
            move |h: &mut Host<'_>| {
                let at = files::NewFile::At { path: path.into() };
                let text = String::new();
                h.create_file(&Create { at, text }).map(drop)
            }
        };
        let forbidden = Err(Reason::Forbidden);
        assert_eq!(asked(0, &read("Eruptions.csv")), Ok(()), "its own kind");
        assert_eq!(asked(0, &write("Eruptions.csv")), Ok(()));
        assert_eq!(asked(0, &list("csv")), Ok(()));
        assert_eq!(asked(0, &create("New.csv")), Ok(()));
        assert_eq!(
            asked(0, &read("Plan.md")),
            forbidden,
            "a note isn't its kind"
        );
        assert_eq!(asked(0, &list("md")), forbidden);
        assert_eq!(
            asked(1, &read("Eruptions.csv")),
            forbidden,
            "another's kind"
        );
        assert_eq!(asked(2, &read("Eruptions.csv")), Ok(()));
        assert_eq!(asked(2, &list("csv")), Ok(()));
        assert_eq!(asked(2, &write("Plan.md")), forbidden);
        assert_eq!(asked(2, &create("Other.md")), forbidden);
        assert_eq!(asked(3, &write("Plan.md")), Ok(()));
        assert_eq!(asked(3, &create("Other.md")), Ok(()));
        assert_eq!(asked(3, &create("Other.md")), Err(Reason::Exists));
        let text = std::fs::read_to_string(dir.join("Eruptions.csv")).unwrap();
        assert_eq!(text, "name\nKrakatoa\nx\n");
        assert!(dir.join("New.csv").exists() && dir.join("Other.md").exists());
        let mut screen = r.screen(READER, &mut core);
        let screens = screen.read_file("Eruptions.csv").map_err(|e| e.reason);
        assert_eq!(
            screens.map(drop),
            forbidden,
            "screens read notes as items only"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_news_and_file_types_are_json() {
        wire(
            FileNews::Open {
                path: "Data/Eruptions.csv".into(),
                place: None,
            },
            json!({"method": "file/open", "params": {"path": "Data/Eruptions.csv"}}),
        );
        wire(
            FileNews::Open {
                path: "Data/Eruptions.csv".into(),
                place: Some("Krakatoa".into()),
            },
            json!({"method": "file/open",
                   "params": {"path": "Data/Eruptions.csv", "place": "Krakatoa"}}),
        );
        wire(
            FileNews::Changed {
                path: "Data/Eruptions.csv".into(),
                rev: None,
            },
            json!({"method": "file/changed", "params": {"path": "Data/Eruptions.csv", "rev": null}}),
        );
        wire(
            FileType {
                extension: "csv".into(),
                title: "CSV table".into(),
                new: Some("csv-table.new".into()),
            },
            json!({"extension": "csv", "title": "CSV table", "new": "csv-table.new"}),
        );
        wire(
            FilePreview {
                path: "Data/Eruptions.csv".into(),
                rev: "00ff00ff00ff00ff".into(),
                width: 60,
                height: 8,
            },
            json!({"path": "Data/Eruptions.csv", "rev": "00ff00ff00ff00ff",
                   "width": 60, "height": 8}),
        );
    }
}
