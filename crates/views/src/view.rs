//! The view gateway (docs/extensions.md, "The view gateway, as built"): an extension describes
//! a declarative view, a list or a table, and the frontend draws it, owns its keys and tells
//! the extension what the user did with its rows. What crosses derives serde and tests pin its
//! JSON. A frontend's side, the views it has open, is its own (the terminal's is `views.rs`).

use serde::{Deserialize, Serialize};

use crate::{list, table};

/// A view, named by the host when it opens one: unique among every extension's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewId(pub u32);

/// What a view shows: `{"kind": "list", ...}` with a `list::List`, `{"kind": "table", ...}`
/// with a `table::Table`, an image, or `{"kind": "scene", ...}` with a `scene::Scene`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    List(list::List),
    Table(table::Table),
    Image(crate::image::Image),
    Scene(crate::scene::Scene),
}

/// The most rows (a list's groups count too) and columns one view may have, and the most keys
/// a list may bind. A bigger view is refused, so one extension can't make the menu slow.
pub const MAX_ROWS: usize = 50_000;
pub const MAX_COLUMNS: usize = 64;
pub const MAX_KEYS: usize = 32;
/// A scene's nodes count as rows; it may have this many links per row allowed, colour groups
/// and flags.
pub const MAX_EDGES_PER_ROW: usize = 4;
pub const MAX_GROUPS: usize = 64;
pub const MAX_FLAGS: usize = 8;
/// The most rows and groups a preview may have: a preview is small, so more is refused.
pub const PREVIEW_ROWS: usize = 200;

impl Content {
    /// Refused (with why) when it is over the limits.
    pub fn check(&self) -> Result<(), String> {
        let rows = match self {
            Content::List(l) => {
                if l.keys.len() > MAX_KEYS {
                    return Err(format!("a list binds at most {MAX_KEYS} keys"));
                }
                if l.filter == list::Filter::Typing && !l.keys.is_empty() {
                    let why = "a list that filters as you type can't bind keys: typing takes them";
                    return Err(why.into());
                }
                l.groups.len() + l.groups.iter().map(|g| g.rows.len()).sum::<usize>()
            }
            Content::Table(t) => {
                if t.columns.len() > MAX_COLUMNS {
                    return Err(format!("a table has at most {MAX_COLUMNS} columns"));
                }
                t.rows.len()
            }
            Content::Image(i) => return i.check(),
            Content::Scene(s) => {
                if s.edges.len() > MAX_EDGES_PER_ROW * MAX_ROWS {
                    let most = MAX_EDGES_PER_ROW * MAX_ROWS;
                    return Err(format!("a scene has at most {most} edges"));
                }
                if s.groups.len() > MAX_GROUPS || s.flags.len() > MAX_FLAGS {
                    let why = format!(
                        "a scene has at most {MAX_GROUPS} colour groups and {MAX_FLAGS} flags"
                    );
                    return Err(why);
                }
                s.nodes.len()
            }
        };
        match rows > MAX_ROWS {
            true => Err(format!(
                "a view has at most {MAX_ROWS} rows and groups; this one has {rows}"
            )),
            false => Ok(()),
        }
    }
}

impl Content {
    /// Refused (with why) when it has more rows and groups than a preview may.
    pub fn small(&self) -> Result<(), String> {
        let rows = match self {
            Content::List(l) => {
                l.groups.len() + l.groups.iter().map(|g| g.rows.len()).sum::<usize>()
            }
            Content::Table(t) => t.rows.len(),
            Content::Image(_) => 0,
            Content::Scene(s) => s.nodes.len(),
        };
        match rows > PREVIEW_ROWS {
            true => Err(format!(
                "a preview has at most {PREVIEW_ROWS} rows and groups; this one has {rows}"
            )),
            false => Ok(()),
        }
    }
}

/// What the user did in a view: the list's events and the table's, by row id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// Enter on a row.
    Activated { row: String },
    /// `x` on a list row with a checkbox.
    Toggled { row: String },
    /// One of the keys the list binds.
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        row: Option<String>,
    },
    /// A table's cell was edited to `value`, as typed.
    Edit {
        row: String,
        column: String,
        value: String,
    },
}

impl From<list::Event> for Event {
    fn from(e: list::Event) -> Event {
        match e {
            list::Event::Activated { row } => Event::Activated { row },
            list::Event::Toggled { row } => Event::Toggled { row },
            list::Event::Command { command, row } => Event::Command { command, row },
        }
    }
}

impl From<table::Event> for Event {
    fn from(e: table::Event) -> Event {
        match e {
            table::Event::Activated { row } => Event::Activated { row },
            table::Event::Edit { row, column, value } => Event::Edit { row, column, value },
        }
    }
}

/// An event in a view: `{"view": 3, "event": "activated", "row": "…"}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Happened {
    pub view: ViewId,
    #[serde(flatten)]
    pub event: Event,
}

/// News of a view the extension opened (host → extension).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum ViewNews {
    #[serde(rename = "view/event")]
    Event(Happened),
    /// The user closed it (Esc, `q`), the host did to make room, or the extension's own
    /// `view/close` was done after waiting for the user to finish typing in it: it is gone,
    /// and an update to it does nothing. A view the extension asked to close while the user
    /// was typing in it sends no more events, and is closed when this names it.
    #[serde(rename = "view/closed")]
    Closed { view: ViewId },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list::{Check, Group, KeyCommand, List, Piece, Row};
    use serde_json::{Value, json};

    fn wire<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(
        value: T,
        json: Value,
    ) {
        assert_eq!(serde_json::to_value(&value).unwrap(), json);
        assert_eq!(serde_json::from_value::<T>(json).unwrap(), value);
    }

    fn list(rows: usize) -> List {
        List {
            title: "Open tasks".into(),
            groups: vec![Group {
                id: "Plan.md".into(),
                title: "Plan".into(),
                collapsed: false,
                rows: (0..rows)
                    .map(|i| Row {
                        id: format!("r{i}"),
                        text: vec![Piece::plain("Pay rent")],
                        check: Some(Check::Unchecked),
                        right: Vec::new(),
                        depth: 0,
                        look: None,
                    })
                    .collect(),
            }],
            keys: vec![KeyCommand {
                key: 't',
                command: "today".into(),
                title: "due today".into(),
            }],
            empty: None,
            filter: Default::default(),
        }
    }

    #[test]
    fn content_is_the_list_or_table_json_with_its_kind() {
        wire(
            Content::List(list(1)),
            json!({
                "kind": "list",
                "title": "Open tasks",
                "groups": [{"id": "Plan.md", "title": "Plan", "rows": [
                    {"id": "r0", "text": [{"text": "Pay rent"}], "check": "unchecked"},
                ]}],
                "keys": [{"key": "t", "command": "today", "title": "due today"}],
            }),
        );
        let table = table::Table {
            title: "volcano".into(),
            columns: vec![table::Column {
                name: "elevation".into(),
                kind: table::Kind::Number,
                look: None,
                read_only: None,
            }],
            rows: vec![table::Row {
                id: "v1".into(),
                cells: vec![json!(4169)],
            }],
            filter: Default::default(),
        };
        wire(
            Content::Table(table),
            json!({
                "kind": "table",
                "title": "volcano",
                "columns": [{"name": "elevation", "type": "number"}],
                "rows": [{"id": "v1", "cells": [4169]}],
            }),
        );
        let image = crate::image::Image {
            title: "Lava lake".into(),
            source: crate::image::Source::Path("Volcanoes/lava-lake.png".into()),
        };
        wire(
            Content::Image(image),
            json!({"kind": "image", "title": "Lava lake", "path": "Volcanoes/lava-lake.png"}),
        );
    }

    #[test]
    fn news_is_json_rpc_notifications() {
        let view = ViewId(3);
        let event = |event| ViewNews::Event(Happened { view, event });
        wire(
            event(Event::Activated { row: "r1".into() }),
            json!({"method": "view/event", "params": {"view": 3, "event": "activated", "row": "r1"}}),
        );
        wire(
            event(Event::Toggled { row: "r1".into() }),
            json!({"method": "view/event", "params": {"view": 3, "event": "toggled", "row": "r1"}}),
        );
        wire(
            event(Event::Command {
                command: "today".into(),
                row: Some("r1".into()),
            }),
            json!({"method": "view/event", "params": {
                "view": 3, "event": "command", "command": "today", "row": "r1",
            }}),
        );
        wire(
            event(Event::Edit {
                row: "r1".into(),
                column: "due".into(),
                value: "2026-11-12".into(),
            }),
            json!({"method": "view/event", "params": {
                "view": 3, "event": "edit", "row": "r1", "column": "due", "value": "2026-11-12",
            }}),
        );
        wire(
            ViewNews::Closed { view },
            json!({"method": "view/closed", "params": {"view": 3}}),
        );
    }

    #[test]
    fn a_view_over_the_limits_is_refused() {
        assert_eq!(Content::List(list(MAX_ROWS - 1)).check(), Ok(()));
        let big = Content::List(list(MAX_ROWS)).check().unwrap_err();
        assert!(big.contains("50000"), "{big}");
        let mut keys = list(1);
        keys.keys = vec![keys.keys[0].clone(); MAX_KEYS + 1];
        assert!(Content::List(keys).check().is_err());
        let mut typing = list(1);
        typing.filter = list::Filter::Typing;
        let why = Content::List(typing.clone()).check().unwrap_err();
        assert!(why.contains("filters as you type"), "{why}");
        typing.keys.clear();
        assert_eq!(Content::List(typing).check(), Ok(()));
        let wide = table::Table {
            title: "wide".into(),
            columns: vec![
                table::Column {
                    name: "c".into(),
                    kind: table::Kind::Text,
                    look: None,
                    read_only: None,
                };
                MAX_COLUMNS + 1
            ],
            rows: Vec::new(),
            filter: Default::default(),
        };
        assert!(Content::Table(wide).check().is_err());
    }

    #[test]
    fn a_scene_is_its_nodes_edges_and_groups_and_is_bounded() {
        use crate::scene::{Edge, Group, Look, Node, Scene};
        let node = |id: &str, look| Node {
            id: id.into(),
            label: id.rsplit([':', '/']).next().unwrap().into(),
            look,
            group: None,
            file: None,
            detail: None,
        };
        let mut lava = node("n:concepts/lava", Look::Note);
        lava.group = Some(0);
        lava.file = Some("Concepts/Lava.md".into());
        lava.detail = Some("Concepts/Lava".into());
        let scene = Scene {
            title: "example-workspace".into(),
            nodes: vec![
                lava,
                node("u:pele", Look::Ghost),
                node("t:volcano", Look::Tag),
            ],
            edges: vec![Edge {
                from: "n:concepts/lava".into(),
                to: "u:pele".into(),
            }],
            groups: vec![Group {
                label: "path:Concepts".into(),
                color: "#fad65a".into(),
            }],
            flags: vec!["obsidian groups".into()],
        };
        wire(
            Content::Scene(scene.clone()),
            json!({
                "kind": "scene",
                "title": "example-workspace",
                "nodes": [
                    {"id": "n:concepts/lava", "label": "lava", "group": 0,
                     "file": "Concepts/Lava.md", "detail": "Concepts/Lava"},
                    {"id": "u:pele", "label": "pele", "look": "ghost"},
                    {"id": "t:volcano", "label": "volcano", "look": "tag"},
                ],
                "edges": [{"from": "n:concepts/lava", "to": "u:pele"}],
                "groups": [{"label": "path:Concepts", "color": "#fad65a"}],
                "flags": ["obsidian groups"],
            }),
        );
        assert_eq!(Content::Scene(scene.clone()).check(), Ok(()));
        let mut big = scene.clone();
        big.nodes = vec![node("x", Look::Note); MAX_ROWS + 1];
        assert!(Content::Scene(big).check().is_err());
        let mut links = scene.clone();
        links.edges = vec![links.edges[0].clone(); MAX_EDGES_PER_ROW * MAX_ROWS + 1];
        assert!(Content::Scene(links).check().is_err());
        let mut groups = scene;
        groups.groups = vec![groups.groups[0].clone(); MAX_GROUPS + 1];
        assert!(Content::Scene(groups).check().is_err());
    }
}
