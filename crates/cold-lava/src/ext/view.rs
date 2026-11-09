//! The view gateway (docs/extensions.md, "The view gateway, as built"): an extension describes
//! a declarative view, a list or a table, and the frontend draws it, owns its keys and tells
//! the extension what the user did with its rows. What crosses derives serde and tests pin its
//! JSON. The frontend's side, the views it has open, is `crate::views`.

use serde::{Deserialize, Serialize};

use crate::{list, table};

/// A view, named by the host when it opens one: unique among every extension's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewId(pub u32);

/// What a view shows: `{"kind": "list", ...}` with a `list::List`, or `{"kind": "table", ...}`
/// with a `table::Table`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Content {
    List(list::List),
    Table(table::Table),
    Image(crate::image::Image),
}

/// The most rows (a list's groups count too) and columns one view may have, and the most keys
/// a list may bind. A bigger view is refused, so one extension can't make the menu slow.
pub const MAX_ROWS: usize = 50_000;
pub const MAX_COLUMNS: usize = 64;
pub const MAX_KEYS: usize = 32;

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
        };
        match rows > super::PREVIEW_ROWS {
            true => Err(format!(
                "a preview has at most {} rows and groups; this one has {rows}",
                super::PREVIEW_ROWS
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
    /// The user closed it (Esc, `q`), or the host did to make room: it is gone, and an update
    /// to it does nothing.
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
}
