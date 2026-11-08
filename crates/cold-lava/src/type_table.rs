//! A type's items as a table (docs/extensions.md, "The table view, as built"): the built-in
//! `table` extension, on the same path as one in a process. `i` (`table.open`) sends the types
//! to pick from as a table through the view gateway; Enter on one sends that type's table.
//! Columns come from the type's schema in its order, then the note; rows are the items of the
//! type across the workspace, by item id, read through the workspace gateway (`types/list`,
//! `items/query`, `items/subscribe`) and kept current from `items/changed`. An edited cell is
//! `items/edit set-field`, and Enter opens the row's note with `workspace/open`.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::ext::view::{self, Content, Happened, ViewId, ViewNews};
use crate::ext::{
    CommandRequest, EditorRequest, Extension, Host, Manifest, PaneNews, Permission, TABLE,
};
use crate::gateway::{
    self, Answer, Edit, Edited, News, Op, Page, Query, Reason, Select, SubId, Ticket,
};
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

/// The command that opens it: a core command (`i`) the menu hands to it.
pub const OPEN: &str = "table.open";

/// The table extension: the type picker and the tables it has open.
#[derive(Default)]
pub struct Tables {
    picker: Option<ViewId>,
    open: HashMap<ViewId, Shown>,
    /// Edits answered later (of items in files other owners read), with their table.
    waiting: HashMap<Ticket, (ViewId, Setting)>,
}

/// One cell being set: its column, the value, and the item's file.
struct Setting {
    column: String,
    value: String,
    name: String,
}

/// One type's table as shown, and where each row's item is.
struct Shown {
    feed: Feed,
    at: HashMap<String, At>,
    /// The row to select with the next update: an edited item, under its id now.
    select: Option<String>,
}

impl Tables {
    /// The types to pick from, with how many items each has, in place of the picker it had.
    fn pick(&mut self, host: &mut Host<'_>) -> Result<(), String> {
        if let Some(old) = self.picker.take() {
            host.close_view(old);
        }
        let types = types(host).map_err(|e| e.to_string())?;
        let column = |name: &str, kind| Column {
            name: name.into(),
            kind,
            look: None,
            read_only: Some("pick a type with Enter".into()),
        };
        let table = Table {
            title: "Table of…".into(),
            columns: vec![column("type", Kind::Text), column("items", Kind::Number)],
            rows: (types.into_iter())
                .map(|(name, n)| Row {
                    id: name.clone(),
                    cells: vec![name.into(), n.into()],
                })
                .collect(),
        };
        let view = host
            .open_view(Content::Table(table), None)
            .map_err(|e| e.to_string())?;
        self.picker = Some(view);
        Ok(())
    }

    /// The table of type `ty`, in place of the picker.
    fn show(&mut self, ty: &str, host: &mut Host<'_>) -> Result<(), String> {
        if let Some(picker) = self.picker.take() {
            host.close_view(picker);
        }
        let feed = match Feed::open(host, ty) {
            Ok(Some(feed)) => feed,
            Ok(None) => return Err(format!("no type called {ty}")),
            Err(e) => return Err(e.to_string()),
        };
        let built = feed.table();
        match host.open_view(Content::Table(built.table), None) {
            Ok(view) => {
                let shown = Shown {
                    feed,
                    at: built.at,
                    select: None,
                };
                self.open.insert(view, shown);
                Ok(())
            }
            Err(e) => {
                feed.close(host);
                Err(e.to_string())
            }
        }
    }
}

impl Shown {
    fn send(&mut self, view: ViewId, host: &mut Host<'_>) {
        let built = self.feed.table();
        self.at = built.at;
        let select = self.select.take();
        if let Err(e) = host.update_view(view, Content::Table(built.table), select) {
            host.notify(format!("the table couldn't be shown: {e}"));
        }
    }

    /// `items/edit set-field` on the row's item, as the version the table shows. Returns the
    /// ticket when the answer comes later.
    fn edit(
        &mut self,
        row: &str,
        column: &str,
        value: String,
        host: &mut Host<'_>,
    ) -> Option<(Ticket, Setting)> {
        let Some(at) = self.at.get(row) else {
            host.notify(format!("{row} is gone"));
            return None;
        };
        let edit = Edit {
            id: row.to_string(),
            rev: at.rev.clone(),
            op: Op::SetField {
                field: column.to_string(),
                value: Json::String(value.clone()),
            },
        };
        let setting = Setting {
            column: column.to_string(),
            value,
            name: at.note.trim_end_matches(".md").to_string(),
        };
        let answer = match host.edit_item(&edit) {
            Ok(Answer::Later(ticket)) => return Some((ticket, setting)),
            Ok(Answer::Now(set)) => Ok(*set),
            Err(e) => Err(e),
        };
        if let Ok(set) = &answer {
            self.select = Some(set.item.id.clone());
        }
        host.notify(set(&setting, answer));
        None
    }
}

/// What the status line says after setting a cell.
fn set(s: &Setting, answer: Result<Edited, gateway::Error>) -> String {
    let (column, name) = (&s.column, &s.name);
    match answer {
        Ok(set) if !set.changed => format!("{column} is already {}", s.value),
        Ok(_) => match s.value.trim() {
            "" => format!("{name}: {column} removed"),
            v => format!("{name}: {column} set to {v}"),
        },
        Err(e) if e.reason == Reason::Changed => {
            format!("not saved, {} · the table is read again", e.message)
        }
        Err(e) => format!("not saved: {e}"),
    }
}

impl Extension for Tables {
    fn initialize(&mut self) -> Manifest {
        Manifest {
            name: TABLE.into(),
            title: "Table".into(),
            version: String::new(),
            description: "A type's items as a table".into(),
            roles: Vec::new(),
            hints: Vec::new(),
            commands: Vec::new(),
            types: Default::default(),
            permissions: vec![Permission::ItemsRead, Permission::ItemsEdit],
            file_types: Vec::new(),
        }
    }

    fn pane(&mut self, _: PaneNews, _: &mut Host<'_>) {}

    fn editor(&mut self, _: EditorRequest, _: &mut Host<'_>) {}

    fn command(&mut self, request: CommandRequest, host: &mut Host<'_>) {
        let CommandRequest::Run { id, .. } = request;
        if id != OPEN {
            return host.notify(format!("the table has no command {id}"));
        }
        if let Err(e) = self.pick(host) {
            host.notify(format!("no types: {e}"));
        }
    }

    fn items(&mut self, news: News, host: &mut Host<'_>) {
        let sub = news.subscription();
        let Some((&view, shown)) = (self.open.iter_mut()).find(|(_, s)| s.feed.sub == sub) else {
            return;
        };
        match shown.feed.heard(host, news) {
            Ok(()) => shown.send(view, host),
            Err(e) => host.notify(format!("the table couldn't be read again: {e}")),
        }
    }

    fn view(&mut self, news: ViewNews, host: &mut Host<'_>) {
        match news {
            ViewNews::Closed { view } if self.picker == Some(view) => self.picker = None,
            ViewNews::Closed { view } => {
                if let Some(shown) = self.open.remove(&view) {
                    shown.feed.close(host);
                }
            }
            ViewNews::Event(Happened {
                view,
                event: view::Event::Activated { row },
            }) if self.picker == Some(view) => {
                if let Err(e) = self.show(&row, host) {
                    host.notify(format!("no table of {row}: {e}"));
                }
            }
            ViewNews::Event(Happened { view, event }) => {
                let Some(shown) = self.open.get_mut(&view) else {
                    return;
                };
                match event {
                    view::Event::Activated { row } => {
                        if let Some(at) = shown.at.get(&row) {
                            host.open_note(at.note.clone().into(), Some(at.line), Some(row));
                        }
                    }
                    view::Event::Edit { row, column, value } => {
                        if let Some((ticket, setting)) = shown.edit(&row, &column, value, host) {
                            self.waiting.insert(ticket, (view, setting));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn edited(
        &mut self,
        ticket: Ticket,
        answer: Result<Edited, gateway::Error>,
        host: &mut Host<'_>,
    ) {
        let Some((view, setting)) = self.waiting.remove(&ticket) else {
            return;
        };
        if let (Ok(set), Some(shown)) = (&answer, self.open.get_mut(&view)) {
            shown.select = Some(set.item.id.clone());
        }
        host.notify(set(&setting, answer));
    }

    /// Another workspace: its views and subscriptions went with the old one.
    fn workspace(&mut self, _: Option<&Path>, _: &mut Host<'_>) {
        self.picker = None;
        self.open.clear();
        self.waiting.clear();
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

    /// The extension's whole path: the picker, a type's table in its place, an edit through
    /// `items/edit` that comes back as an update selecting the row, Enter opening the note,
    /// and closing it ending the subscription.
    #[test]
    fn the_table_is_an_extension_of_the_gateways() {
        use crate::ext::{Effect, ExtId};
        let dir = example("extension");
        let mut core = Core::open(&dir).unwrap();
        let mut r = Registry::new(vec![Box::new(Tables::default())]);
        let ext = ExtId(0);
        let run = CommandRequest::Run {
            id: OPEN.into(),
            args: Json::Null,
        };
        let effects = r.call(ext, Some(&mut core), |e, h| e.command(run, h));
        let [
            Effect::OpenView {
                view: picker,
                content: Content::Table(types),
                ..
            },
        ] = &effects[..]
        else {
            panic!("{effects:?}")
        };
        assert_eq!(types.title, "Table of…");
        let eruption = (types.rows.iter()).find(|r| r.id == "eruption").unwrap();
        assert_eq!(eruption.cells, [Json::from("eruption"), Json::from(4)]);
        let happened = |view, event| ViewNews::Event(Happened { view, event });

        let pick = happened(
            *picker,
            view::Event::Activated {
                row: "eruption".into(),
            },
        );
        let effects = r.call(ext, Some(&mut core), |e, h| e.view(pick, h));
        let [
            Effect::CloseView { view: closed },
            Effect::OpenView {
                view,
                content: Content::Table(table),
                ..
            },
        ] = &effects[..]
        else {
            panic!("{effects:?}")
        };
        assert_eq!(closed, picker, "the table shows in the picker's place");
        assert_eq!((table.title.as_str(), table.rows.len()), ("eruption", 4));
        let row = table.rows[0].id.clone();

        let set = happened(
            *view,
            view::Event::Edit {
                row: row.clone(),
                column: "example".into(),
                value: "[[Eyjafjallajökull]] 2010, Iceland".into(),
            },
        );
        let effects = r.call(ext, Some(&mut core), |e, h| e.view(set, h));
        let said = Effect::Notify {
            message: "Concepts/Volcanic Explosivity Index: example set to [[Eyjafjallajökull]] 2010, Iceland".into(),
        };
        assert_eq!(effects, [said]);
        let mut updates = Vec::new();
        for (by, news) in core.take_item_news() {
            assert_eq!(by, TABLE, "the edit is the table's");
            updates.extend(r.call(ext, Some(&mut core), |e, h| e.items(news, h)));
        }
        let [
            Effect::UpdateView {
                content: Content::Table(table),
                select,
                ..
            },
        ] = &updates[..]
        else {
            panic!("{updates:?}")
        };
        let edited = select.as_deref().expect("the edited row stays selected");
        let cells = &table.rows.iter().find(|r| r.id == edited).unwrap().cells;
        assert_eq!(cells[1], "[[Eyjafjallajökull]] 2010, Iceland");

        let open = happened(*view, view::Event::Activated { row: edited.into() });
        let effects = r.call(ext, Some(&mut core), |e, h| e.view(open, h));
        let [
            Effect::OpenNote {
                note,
                line: Some(_),
                ..
            },
        ] = &effects[..]
        else {
            panic!("{effects:?}")
        };
        assert_eq!(note, Path::new("Concepts/Volcanic Explosivity Index.md"));

        let closed = ViewNews::Closed { view: *view };
        r.call(ext, Some(&mut core), |e, h| e.view(closed, h));
        let note = dir.join("Concepts/Volcanic Explosivity Index.md");
        let text = std::fs::read_to_string(&note).unwrap();
        std::fs::write(&note, text.replace("Krakatoa", "Krakatau")).unwrap();
        core.note_changed_now(&note);
        assert_eq!(core.take_item_news(), [], "it stopped following the type");
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
