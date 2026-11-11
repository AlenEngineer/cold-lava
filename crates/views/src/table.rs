//! The table view's words (docs/extensions.md, "Views"): the sender says what the columns are
//! and what each row holds, and hears `Event`s about rows by id. Every type derives serde, so a
//! table crosses the view gateway unchanged; a frontend draws it its own way.

use cold_lava_engine::types::Look;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::list::Filter;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Table {
    pub title: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    #[serde(default, skip_serializing_if = "Filter::is_slash")]
    pub filter: Filter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    /// How its cells sort, align and read.
    #[serde(flatten)]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
    /// Why its cells can't be edited, when they can't.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<String>,
}

/// A column's value type: numbers and durations sort as numbers and align right, dates as
/// ISO text, enums in their declared order, the rest as folded text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Kind {
    Text,
    Number,
    Date,
    Datetime,
    /// Minutes.
    Duration,
    Bool,
    Link,
    Tag,
    Enum {
        values: Vec<String>,
    },
}

/// One row: an id the sender knows it by, and a value per column (`null` for none, a list
/// for several).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub cells: Vec<Json>,
}

/// What the view tells its sender.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// Enter on a row.
    Activated { row: String },
    /// A cell was edited to `value`, as typed.
    Edit {
        row: String,
        column: String,
        value: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_table_json_is_pinned() {
        let col = |name: &str, kind| Column {
            name: name.into(),
            kind,
            look: None,
            read_only: None,
        };
        let mut t = Table {
            title: "volcano".into(),
            columns: vec![
                col("name", Kind::Text),
                col("elevation", Kind::Number),
                col(
                    "status",
                    Kind::Enum {
                        values: vec!["todo".into(), "in_progress".into(), "done".into()],
                    },
                ),
            ],
            rows: vec![Row {
                id: "a".into(),
                cells: vec![json!("Kīlauea"), json!(1247), json!("done")],
            }],
            filter: Default::default(),
        };
        assert_eq!(
            serde_json::to_value(&t).unwrap(),
            json!({
                "title": "volcano",
                "columns": [
                    {"name": "name", "type": "text"},
                    {"name": "elevation", "type": "number"},
                    {"name": "status", "type": "enum", "values": ["todo", "in_progress", "done"]},
                ],
                "rows": [{"id": "a", "cells": ["Kīlauea", 1247, "done"]}],
            })
        );
        t.filter = Filter::Typing;
        let typing = serde_json::to_value(&t).unwrap();
        assert_eq!(
            typing["filter"], "typing",
            "the default, slash, is left out"
        );
        let e = Event::Edit {
            row: "a".into(),
            column: "due".into(),
            value: "2026-12-01".into(),
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"event": "edit", "row": "a", "column": "due", "value": "2026-12-01"})
        );
    }
}
