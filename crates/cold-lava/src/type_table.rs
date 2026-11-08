//! A type's items as a table (docs/extensions.md, "Types"): what the built-in table extension
//! will send. Columns come from the type's schema in its order, then the note; rows are the
//! items of the type across the workspace, by item id. In the menu the table reads them through
//! the workspace gateway (`types/list`, `items/query`, `items/subscribe`) and keeps them current
//! from `items/changed`.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::ext::Host;
use crate::gateway::{self, News, Page, Query, Select, SubId};
use crate::items::{Data, Item};
use crate::table::{Column, Kind, Row, Table};
use crate::types::{Definition, Source, Type, Value};

/// The column after the schema's: the note an item is in.
pub const NOTE: &str = "note";

/// How many items one `items/query` page brings.
const PAGE: usize = 500;

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
pub fn types(host: &mut Host<'_>) -> Result<Vec<(String, usize)>, gateway::Error> {
    let types = host.types()?;
    (types.into_iter())
        .map(|t| Ok((t.name.clone(), all(host, &select(&t.name))?.len())))
        .collect()
}

pub fn select(ty: &str) -> Select {
    Select {
        kind: ty.to_string(),
        ..Select::default()
    }
}

/// Every item `select` names, a page at a time.
fn all(host: &mut Host<'_>, select: &Select) -> Result<Vec<Item>, gateway::Error> {
    let mut items = Vec::new();
    let mut after = None;
    loop {
        let page = Page {
            after,
            limit: Some(PAGE),
        };
        let select = select.clone();
        let found = host.query_items(&Query { select, page })?;
        items.extend(found.items);
        match found.next {
            Some(next) => after = Some(next),
            None => return Ok(items),
        }
    }
}

/// The table extension's view of one type: its definition and its items, kept current by a
/// subscription.
pub struct Feed {
    ty: Type,
    sub: SubId,
    /// By note and line, as `items/query` orders them.
    items: Vec<Item>,
}

impl Feed {
    /// Follow type `ty`, or `None` when there is no such type.
    pub fn open(host: &mut Host<'_>, ty: &str) -> Result<Option<Feed>, gateway::Error> {
        let Some(ty) = host.types()?.into_iter().find(|t| t.name == ty) else {
            return Ok(None);
        };
        let sub = host.subscribe_items(select(&ty.name))?;
        let items = all(host, &select(&ty.name))?;
        Ok(Some(Feed { ty, sub, items }))
    }

    pub fn name(&self) -> &str {
        &self.ty.name
    }

    pub fn subscription(&self) -> SubId {
        self.sub
    }

    /// By note and line.
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn table(&self) -> TypeTable {
        build(&self.ty, &self.items)
    }

    /// Apply news of its subscription.
    pub fn heard(&mut self, host: &mut Host<'_>, news: News) -> Result<(), gateway::Error> {
        let (added, updated, removed) = match news {
            News::Reset { .. } => {
                self.items = all(host, &select(&self.ty.name))?;
                return Ok(());
            }
            News::Changed {
                added,
                updated,
                removed,
                ..
            } => (added, updated, removed),
        };
        let mut updated: HashMap<String, Item> =
            (updated.into_iter()).map(|i| (i.id.clone(), i)).collect();
        self.items.retain(|i| !removed.contains(&i.id));
        for item in &mut self.items {
            if let Some(new) = updated.remove(&item.id) {
                *item = new;
            }
        }
        self.items.extend(added);
        let note = |i: &Item| i.note.strip_suffix(".md").unwrap_or(&i.note).to_string();
        self.items.sort_by_cached_key(|i| (note(i), i.line));
        Ok(())
    }

    /// Stop following the type.
    pub fn close(self, host: &mut Host<'_>) {
        let _ = host.unsubscribe_items(self.sub);
    }
}

/// The table of type `ty` with these of its items.
pub fn build(ty: &Type, items: &[Item]) -> TypeTable {
    let def = &ty.definition;
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
    for item in items {
        let Data::Typed(t) = &item.data else { continue };
        let mut cells: Vec<Json> = (def.fields.keys())
            .map(|f| t.fields.get(f.as_str()).cloned().unwrap_or(Json::Null))
            .collect();
        let name = Path::new(&item.note).file_stem().unwrap_or_default();
        cells.push(Json::String(name.to_string_lossy().into_owned()));
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
    let table = Table {
        title: ty.name.clone(),
        columns,
        rows,
    };
    TypeTable { table, at }
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
    use crate::core::Core;
    use crate::ext::{Registry, TABLE};

    /// A copy of the example workspace, which the test may write to.
    fn example(name: &str) -> std::path::PathBuf {
        let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
        let to =
            std::env::temp_dir().join(format!("cold-lava-table-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&to);
        for e in walkdir::WalkDir::new(&from) {
            let e = e.unwrap();
            let dest = to.join(e.path().strip_prefix(&from).unwrap());
            match e.file_type().is_dir() {
                true => std::fs::create_dir_all(&dest).unwrap(),
                false => drop(std::fs::copy(e.path(), &dest).unwrap()),
            }
        }
        to
    }

    #[test]
    fn a_types_items_are_rows_and_its_schema_the_columns() {
        let dir = example("rows");
        let mut core = Core::open(&dir).unwrap();
        let mut registry = Registry::empty();
        let mut host = registry.screen(TABLE, &mut core);
        let counts = types(&mut host).unwrap();
        let count = |n: &str| counts.iter().find(|(t, _)| t == n).unwrap().1;
        assert_eq!((count("volcano"), count("eruption")), (9, 4));
        let t = Feed::open(&mut host, "eruption").unwrap().unwrap().table();
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
        let task = Feed::open(&mut host, "task").unwrap().unwrap().table();
        let status = &task.table.columns[1];
        assert_eq!(status.name, "status_type");
        assert!(
            matches!(&status.kind, Kind::Enum { values } if values[..3] == ["todo", "done", "in_progress"]),
            "{:?}",
            status.kind
        );
        assert!(Feed::open(&mut host, "nope").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The table follows the workspace through its subscription: an edit through the gateway
    /// shows in it at once, as does a note written by someone else once the core reads it.
    #[test]
    fn the_table_follows_its_subscription() {
        let dir = example("follow");
        let mut core = Core::open(&dir).unwrap();
        let mut registry = Registry::empty();
        let mut feed = {
            let mut host = registry.screen(TABLE, &mut core);
            Feed::open(&mut host, "eruption").unwrap().unwrap()
        };
        let row = feed.table().table.rows[0].clone();
        let rev = feed.table().at[&row.id].rev.clone();
        let edit = gateway::Edit {
            id: row.id.clone(),
            rev,
            op: gateway::Op::SetField {
                field: "example".into(),
                value: "[[Eyjafjallajökull]] 2010, Iceland".into(),
            },
        };
        registry.screen(TABLE, &mut core).edit_item(&edit).unwrap();
        let news = core.take_item_news();
        assert_eq!(news.len(), 1, "{news:?}");
        assert_eq!(news[0].0, TABLE);
        for (_, n) in news {
            feed.heard(&mut registry.screen(TABLE, &mut core), n)
                .unwrap();
        }
        let t = feed.table();
        assert_eq!(t.table.rows.len(), 4);
        assert_eq!(
            t.table.rows[0].cells[1],
            "[[Eyjafjallajökull]] 2010, Iceland"
        );

        let note = dir.join("Concepts/Volcanic Explosivity Index.md");
        let text = std::fs::read_to_string(&note).unwrap();
        let cut = text
            .lines()
            .filter(|l| !l.contains("Iceland"))
            .collect::<Vec<_>>();
        std::fs::write(&note, cut.join("\n")).unwrap();
        core.note_changed_now(&note);
        for (_, n) in core.take_item_news() {
            feed.heard(&mut registry.screen(TABLE, &mut core), n)
                .unwrap();
        }
        assert_eq!(
            feed.table().table.rows.len(),
            3,
            "the row written away is gone"
        );
        core.rescan_now();
        let news = core.take_item_news();
        assert!(matches!(news[..], [(_, News::Reset { .. })]), "{news:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
