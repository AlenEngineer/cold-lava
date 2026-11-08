//! A type's items as a table (docs/extensions.md, "Types"): what the built-in table extension
//! will send. Columns come from the type's schema in its order, then the note; rows are the
//! items of the type across the workspace, from the core's cached items, by item id.

use std::collections::HashMap;

use serde_json::Value as Json;

use crate::items::Data;
use crate::table::{Column, Kind, Row, Table};
use crate::types::{Definition, Source, Value};
use crate::workspace::Workspace;

/// The column after the schema's: the note an item is in.
pub const NOTE: &str = "note";

/// Where a row's item is, and which version of it the table shows.
#[derive(Clone, Debug, PartialEq)]
pub struct At {
    /// Workspace-relative, with `.md`.
    pub note: String,
    pub line: usize,
    pub rev: String,
}

pub struct TypeTable {
    pub table: Table,
    pub at: HashMap<String, At>,
}

/// Every type, with how many items answer to it, in the order types load.
pub fn types(workspace: &Workspace) -> Vec<(String, usize)> {
    let names: Vec<String> = (workspace.item_settings().types.types.iter())
        .map(|t| t.name.clone())
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for note in &workspace.notes {
        for item in workspace.items(note) {
            for n in &names {
                if item.data.answers_to(n) {
                    *counts.entry(n).or_default() += 1;
                }
            }
        }
    }
    (names.iter())
        .map(|n| (n.clone(), counts.get(n.as_str()).copied().unwrap_or(0)))
        .collect()
}

/// The table of type `ty`, or `None` when there is no such type.
pub fn build(workspace: &Workspace, ty: &str) -> Option<TypeTable> {
    let def = &(workspace.item_settings().types.types.iter())
        .find(|t| t.name == ty)?
        .definition;
    let mut columns: Vec<Column> = (def.fields.iter())
        .map(|(name, value)| Column {
            name: name.clone(),
            kind: kind(value),
            look: def.style.get(name).cloned(),
            read_only: read_only(def, name),
        })
        .collect();
    columns.push(Column {
        name: NOTE.into(),
        kind: Kind::Link,
        look: None,
        read_only: Some("the note is where the item is: rename or move the note instead".into()),
    });
    let mut rows = Vec::new();
    let mut at = HashMap::new();
    for note in &workspace.notes {
        for item in workspace.items(note) {
            let Data::Typed(t) = &item.data else { continue };
            if !item.data.answers_to(ty) {
                continue;
            }
            let mut cells: Vec<Json> = (def.fields.keys())
                .map(|f| t.fields.get(f.as_str()).cloned().unwrap_or(Json::Null))
                .collect();
            cells.push(Json::String(note.name.clone()));
            rows.push(Row {
                id: item.id.clone(),
                cells,
            });
            at.insert(
                item.id.clone(),
                At {
                    note: item.note.clone(),
                    line: item.line,
                    rev: item.rev.clone(),
                },
            );
        }
    }
    let table = Table {
        title: ty.to_string(),
        columns,
        rows,
    };
    Some(TypeTable { table, at })
}

fn kind(v: &Value) -> Kind {
    match v {
        Value::Text => Kind::Text,
        Value::Number => Kind::Number,
        Value::Date { .. } => Kind::Date,
        Value::Datetime { .. } => Kind::Datetime,
        Value::Duration => Kind::Duration,
        Value::Bool => Kind::Bool,
        Value::Link => Kind::Link,
        Value::Tag => Kind::Tag,
        Value::Enum { values } => {
            let mut names: Vec<String> = Vec::new();
            for n in values.map().values() {
                if !names.contains(n) {
                    names.push(n.clone());
                }
            }
            names.extend(values.other().map(str::to_string));
            Kind::Enum { values: names }
        }
    }
}

/// Why a field can't be set, when no shape of the type has a source that writes it.
fn read_only(def: &Definition, field: &str) -> Option<String> {
    let sources: Vec<&Source> = (def.from.iter())
        .filter_map(|s| s.fields.get(field))
        .flat_map(|s| &s.0)
        .collect();
    if (sources.iter()).any(|s| s.writable() && !matches!(s, Source::Info(_))) {
        return None;
    }
    let from = match sources.first() {
        Some(Source::Note(_)) => "the note's name or path",
        Some(Source::Inlines(_)) => "the tags or links in its text",
        Some(Source::Rest(_)) => "the rest of its text",
        Some(Source::Section(_)) => "the headings above it",
        Some(Source::Children(_)) => "the items under it",
        Some(_) => "a source that can't be written",
        None => "nowhere",
    };
    Some(format!("{field} is read-only: it is read from {from}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn example() -> Workspace {
        crate::workspace::scan(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace"),
        )
        .unwrap()
    }

    #[test]
    fn a_types_items_are_rows_and_its_schema_the_columns() {
        let v = example();
        let counts = types(&v);
        let count = |n: &str| counts.iter().find(|(t, _)| t == n).unwrap().1;
        assert_eq!((count("volcano"), count("eruption")), (9, 4));
        let t = build(&v, "eruption").unwrap();
        let names: Vec<&str> = t.table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["vei", "example", "volcanoes", "note"]);
        assert_eq!(t.table.rows.len(), 4);
        assert_eq!(t.table.rows[0].cells[0], 4);
        assert_eq!(t.table.columns[0].kind, Kind::Number);
        assert!(t.table.columns[0].look.as_ref().unwrap().badge);
        let ro: Vec<bool> = (t.table.columns.iter())
            .map(|c| c.read_only.is_some())
            .collect();
        assert_eq!(ro, [false, false, true, true]);
        let at = &t.at[&t.table.rows[0].id];
        assert_eq!(
            (at.note.as_str(), at.line),
            ("Concepts/Volcanic Explosivity Index.md", 11)
        );
        let task = build(&v, "task").unwrap();
        let status = &task.table.columns[1];
        assert_eq!(status.name, "status_type");
        assert!(
            matches!(&status.kind, Kind::Enum { values } if values[..3] == ["todo", "done", "in_progress"]),
            "{:?}",
            status.kind
        );
        assert!(build(&v, "nope").is_none());
    }
}
