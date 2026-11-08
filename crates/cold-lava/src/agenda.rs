//! The agenda (docs/extensions.md, "The list view and the agenda, as built"): a built-in
//! extension on the same path as one in a process. It follows every item that answers to
//! `task` (subtypes too) through the workspace gateway (`items/query`, `items/subscribe`),
//! sends them as a list through the view gateway (`view/open`, `view/update`), filed by the
//! date `cold-lava agenda` files them under (due, else scheduled, else start, else its daily
//! note's): overdue, today, each of the next seven days, later, and undated last. It ticks
//! through `items/edit` and opens a task's note with `workspace/open`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use chrono::{Datelike, Days, NaiveDate};
use regex::Regex;
use serde_json::Value as Json;

use crate::ext::view::{self, Content, Happened, ViewId, ViewNews};
use crate::ext::{
    AGENDA, CommandRequest, EditorRequest, Extension, Host, Manifest, PaneNews, Permission,
};
use crate::gateway::{self, Answer, Edit, Edited, News, Op, Reason, Ticket};
use crate::items::{Data, Item};
use crate::list::{Check, Group, KeyCommand, List, Piece, Row};
use crate::type_table::{At, Feed};
use crate::types::{Colour, Definition, Look, Type};

/// The command that opens it: a core command (`A`) the menu hands to it.
pub const OPEN: &str = "agenda.open";

/// The command `d` sends: show or hide done and cancelled tasks.
pub const SHOW_DONE: &str = "show-done";

/// Days after today that get a group of their own.
const WEEK: u64 = 7;

/// The agenda's list, and where each row's task is.
pub struct Built {
    pub list: List,
    pub at: HashMap<String, At>,
}

/// Which group a task is in; groups show in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
    Overdue,
    Day(NaiveDate),
    Later,
    Undated,
}

impl Bucket {
    fn of(date: Option<NaiveDate>, today: NaiveDate) -> Bucket {
        match date {
            None => Bucket::Undated,
            Some(d) if d < today => Bucket::Overdue,
            Some(d) if d <= today + Days::new(WEEK) => Bucket::Day(d),
            Some(_) => Bucket::Later,
        }
    }

    fn id(self) -> String {
        match self {
            Bucket::Overdue => "overdue".into(),
            Bucket::Day(d) => d.to_string(),
            Bucket::Later => "later".into(),
            Bucket::Undated => "undated".into(),
        }
    }

    fn title(self, today: NaiveDate, show_done: bool) -> String {
        match self {
            // Done tasks from before today are no longer overdue.
            Bucket::Overdue if show_done => "Earlier".into(),
            Bucket::Overdue => "Overdue".into(),
            Bucket::Day(d) if d == today => format!("Today · {}", day(d, today)),
            Bucket::Day(d) if d == today + Days::new(1) => format!("Tomorrow · {}", day(d, today)),
            Bucket::Day(d) => day(d, today),
            Bucket::Later => "Later".into(),
            Bucket::Undated => "Undated".into(),
        }
    }

    /// Whether its rows have different dates, so each shows its own.
    fn dated_rows(self) -> bool {
        matches!(self, Bucket::Overdue | Bucket::Later)
    }
}

/// `Sat 7 Nov`, with the year when it isn't this one.
fn day(d: NaiveDate, today: NaiveDate) -> String {
    match d.year() == today.year() {
        true => d.format("%a %-d %b").to_string(),
        false => d.format("%a %-d %b %Y").to_string(),
    }
}

/// A task's field as text.
fn field<'a>(item: &'a Item, name: &str) -> Option<&'a str> {
    match &item.data {
        Data::Typed(t) => t.fields.get(name).and_then(Json::as_str),
        _ => None,
    }
}

fn date(item: &Item, name: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(field(item, name)?, "%Y-%m-%d").ok()
}

/// The agenda on `today` of these task items (as the gateway gives them, each with its
/// `status_type`), with the looks of `types`: open tasks, tasks closed today (so a tick doesn't
/// make one vanish), and with `show_done` every closed task.
pub fn build(types: &[Type], tasks: &[Item], today: NaiveDate, show_done: bool) -> Built {
    let defs: HashMap<&str, &Definition> = (types.iter())
        .map(|t| (t.name.as_str(), &t.definition))
        .collect();
    let mut found: Vec<(Bucket, Option<NaiveDate>, &str, usize, Row)> = Vec::new();
    let mut at = HashMap::new();
    for item in tasks {
        let check = match field(item, "status_type") {
            Some("todo") => Check::Unchecked,
            Some("in_progress") => Check::Partial,
            Some("done") => Check::Checked,
            Some("cancelled") => Check::Crossed,
            _ => continue,
        };
        let closed_today = [date(item, "done"), date(item, "cancelled")].contains(&Some(today));
        let open = matches!(check, Check::Unchecked | Check::Partial);
        if !open && !closed_today && !show_done {
            continue;
        }
        let filed =
            (["due", "scheduled", "start", "daily_note"].iter()).find_map(|f| date(item, f));
        let bucket = Bucket::of(filed, today);
        let mut right = Vec::new();
        if let (true, Some(d)) = (bucket.dated_rows(), filed) {
            let late = Look {
                colour: Some(Colour::Red),
                ..Look::default()
            };
            right.push(Piece {
                text: day(d, today),
                look: (open && d < today).then_some(late),
            });
        }
        let name = Path::new(&item.note).file_stem().unwrap_or_default();
        right.push(Piece::plain(name.to_string_lossy()));
        let row = Row {
            id: item.id.clone(),
            text: text(item, &defs),
            check: Some(check),
            right,
            depth: 0,
            look: style(item, &defs, "status_type").filter(|l| l.whole),
        };
        at.insert(
            item.id.clone(),
            At {
                note: item.note.clone(),
                line: item.line,
                rev: item.rev.clone(),
            },
        );
        found.push((bucket, filed, item.note.as_str(), item.line, row));
    }
    found.sort_by(|a, b| (a.0, a.1, a.2, a.3).cmp(&(b.0, b.1, b.2, b.3)));
    let mut groups: Vec<Group> = Vec::new();
    let mut last = None;
    for (bucket, _, _, _, row) in found {
        if last != Some(bucket) {
            groups.push(Group {
                id: bucket.id(),
                title: bucket.title(today, show_done),
                collapsed: bucket == Bucket::Undated,
                rows: Vec::new(),
            });
            last = Some(bucket);
        }
        groups.last_mut().expect("pushed above").rows.push(row);
    }
    let title = match show_done {
        true => "Agenda · with done tasks",
        false => "Agenda",
    };
    let list = List {
        title: title.into(),
        groups,
        keys: vec![KeyCommand {
            key: 'd',
            command: SHOW_DONE.into(),
            title: match show_done {
                true => "hide done".into(),
                false => "show done".into(),
            },
        }],
        empty: Some("no tasks: write one as - [ ] in any note".into()),
    };
    Built { list, at }
}

/// The task's words with links as their text, then its priority on a chip and 🔁 when it
/// recurs.
fn text(item: &Item, defs: &HashMap<&str, &Definition>) -> Vec<Piece> {
    let words = field(item, "text").unwrap_or_default();
    let mut out = vec![Piece::plain(links_as_text(words))];
    if field(item, "recurrence").is_some() {
        out.push(Piece {
            text: "🔁".into(),
            look: Some(Look {
                colour: Some(Colour::Muted),
                ..Look::default()
            }),
        });
    }
    if let Some(p) = field(item, "priority") {
        let glyph = match p {
            "highest" => "🔺",
            "high" => "⏫",
            "medium" => "🔼",
            "low" => "🔽",
            "lowest" => "⏬",
            other => other,
        };
        out.push(Piece {
            text: glyph.into(),
            look: style(item, defs, "priority"),
        });
    }
    out
}

/// The look the item's type (or the type it extends) gives `field`, for its value.
fn style(item: &Item, defs: &HashMap<&str, &Definition>, field: &str) -> Option<Look> {
    let Data::Typed(t) = &item.data else {
        return None;
    };
    let look = (std::iter::once(&t.kind).chain(&t.is))
        .filter_map(|name| defs.get(name.as_str()))
        .find_map(|d| d.style.get(field))?;
    crate::looks::for_value(look, t.fields.get(field).unwrap_or(&Json::Null))
}

/// The agenda extension, and while its view is open, what it shows.
pub struct Agenda {
    /// What day it is: `tasks::today`, or a fixed day in tests.
    today: Box<dyn Fn() -> NaiveDate>,
    shown: Option<Shown>,
    /// Ticks answered later (of tasks in files other owners read): the task's words and file.
    waiting: HashMap<Ticket, (String, String)>,
}

impl Default for Agenda {
    fn default() -> Agenda {
        Agenda::on(crate::tasks::today)
    }
}

struct Shown {
    view: ViewId,
    /// The day the list was filed by.
    day: NaiveDate,
    feed: Feed,
    types: Vec<Type>,
    at: HashMap<String, At>,
    show_done: bool,
    /// The row to select with the next update: a ticked task, under its id now.
    select: Option<String>,
}

impl Shown {
    fn send(&mut self, today: NaiveDate, host: &mut Host<'_>) {
        self.day = today;
        let built = build(&self.types, self.feed.items(), today, self.show_done);
        self.at = built.at;
        let select = self.select.take();
        if let Err(e) = host.update_view(self.view, Content::List(built.list), select) {
            host.notify(format!("the agenda couldn't be shown: {e}"));
        }
    }

    /// `items/edit toggle` on the row's task, as the version the agenda shows. Returns the
    /// ticket when the answer comes later, with the task's words and file.
    fn tick(&mut self, row: &str, host: &mut Host<'_>) -> Option<(Ticket, (String, String))> {
        let at = self.at.get(row).cloned()?;
        let words = (self.feed.items().iter())
            .find(|i| i.id == row)
            .and_then(|i| field(i, "text"))
            .map(links_as_text)
            .unwrap_or_default();
        let edit = Edit {
            id: row.to_string(),
            rev: at.rev,
            op: Op::Toggle,
        };
        let note = at.note.trim_end_matches(".md").to_string();
        let answer = match host.edit_item(&edit) {
            Ok(Answer::Later(ticket)) => return Some((ticket, (words, note))),
            Ok(Answer::Now(e)) => Ok(*e),
            Err(e) => Err(e),
        };
        if let Ok(e) = &answer {
            self.select = Some(e.item.id.clone());
        }
        host.notify(ticked(&words, &note, answer));
        None
    }
}

/// What the status line says after a tick of the task with these words in `note`.
fn ticked(words: &str, note: &str, answer: Result<Edited, gateway::Error>) -> String {
    match answer {
        Ok(e) => {
            let what = match field(&e.item, "status_type") {
                Some("done") => "done",
                Some("cancelled") => "cancelled",
                Some("in_progress") => "in progress",
                Some("todo") => "open again",
                _ => "ticked",
            };
            let mut message = format!("{what}: {words}");
            if e.next.is_some() {
                message.push_str(" · next occurrence added");
            }
            if let Some(w) = &e.warning {
                message = format!("{message} · {w}");
            }
            message
        }
        Err(e) if e.reason == Reason::Changed => {
            format!("not ticked: {note} changed since it was read · read again")
        }
        Err(e) => format!("not ticked: {e}"),
    }
}

impl Agenda {
    /// The agenda with its own clock.
    pub fn on(today: impl Fn() -> NaiveDate + 'static) -> Agenda {
        Agenda {
            today: Box::new(today),
            shown: None,
            waiting: HashMap::new(),
        }
    }

    /// A new view of the tasks, in place of the one it had open.
    fn open(&mut self, host: &mut Host<'_>) -> Result<(), String> {
        if let Some(old) = self.shown.take() {
            host.close_view(old.view);
            old.feed.close(host);
        }
        let types = host.types().map_err(|e| e.to_string())?;
        let feed = match Feed::open(host, "task") {
            Ok(Some(feed)) => feed,
            Ok(None) => return Err("the workspace has no task type".into()),
            Err(e) => return Err(e.to_string()),
        };
        let day = (self.today)();
        let built = build(&types, feed.items(), day, false);
        match host.open_view(Content::List(built.list), None) {
            Ok(view) => {
                self.shown = Some(Shown {
                    view,
                    day,
                    feed,
                    types,
                    at: built.at,
                    show_done: false,
                    select: None,
                });
                Ok(())
            }
            Err(e) => {
                feed.close(host);
                Err(e.to_string())
            }
        }
    }
}

impl Extension for Agenda {
    fn initialize(&mut self) -> Manifest {
        Manifest {
            name: AGENDA.into(),
            title: "Agenda".into(),
            version: String::new(),
            description: "The workspace's tasks by day".into(),
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
            return host.notify(format!("the agenda has no command {id}"));
        }
        if let Err(e) = self.open(host) {
            host.notify(format!("no agenda: {e}"));
        }
    }

    fn items(&mut self, news: News, host: &mut Host<'_>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        if shown.feed.subscription() != news.subscription() {
            return;
        }
        match shown.feed.heard(host, news) {
            Ok(()) => shown.send((self.today)(), host),
            Err(e) => host.notify(format!("the agenda couldn't be read again: {e}")),
        }
    }

    fn view(&mut self, news: ViewNews, host: &mut Host<'_>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        match news {
            ViewNews::Closed { view } if view == shown.view => {
                if let Some(shown) = self.shown.take() {
                    shown.feed.close(host);
                }
            }
            ViewNews::Event(Happened { view, event }) if view == shown.view => match event {
                view::Event::Activated { row } => {
                    if let Some(at) = shown.at.get(&row) {
                        host.open_note(at.note.clone().into(), Some(at.line), Some(row));
                    }
                }
                view::Event::Toggled { row } => {
                    if let Some((ticket, task)) = shown.tick(&row, host) {
                        self.waiting.insert(ticket, task);
                    }
                }
                view::Event::Command { command, .. } if command == SHOW_DONE => {
                    shown.show_done = !shown.show_done;
                    shown.send((self.today)(), host);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn edited(
        &mut self,
        ticket: Ticket,
        answer: Result<Edited, gateway::Error>,
        host: &mut Host<'_>,
    ) {
        let Some((words, note)) = self.waiting.remove(&ticket) else {
            return;
        };
        if let (Ok(e), Some(shown)) = (&answer, self.shown.as_mut()) {
            shown.select = Some(e.item.id.clone());
        }
        host.notify(ticked(&words, &note, answer));
    }

    /// Past midnight the tasks are filed by the new day.
    fn poll(&mut self, host: &mut Host<'_>) {
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        let today = (self.today)();
        if today != shown.day {
            shown.send(today, host);
        }
    }

    /// Another workspace: its view and subscription went with the old one.
    fn workspace(&mut self, _: Option<&Path>, _: &mut Host<'_>) {
        self.shown = None;
    }
}

/// `[[Note|alias]]` as `alias`, `[[Note#Heading]]` as `Note#Heading`, `[text](url)` as `text`.
pub fn links_as_text(s: &str) -> String {
    static RE: OnceLock<(Regex, Regex)> = OnceLock::new();
    let (wiki, md) = RE.get_or_init(|| {
        (
            Regex::new(r"!?\[\[([^\]|]*)(?:\|([^\]]*))?\]\]").unwrap(),
            Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").unwrap(),
        )
    });
    let s = wiki.replace_all(s, |c: &regex::Captures| {
        c.get(2)
            .unwrap_or_else(|| c.get(1).unwrap())
            .as_str()
            .to_string()
    });
    md.replace_all(&s, "$1").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use std::path::PathBuf;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// The agenda of the workspace's tasks, as the gateway gives them.
    fn built(v: &Workspace, today: NaiveDate, show_done: bool) -> Built {
        let query = crate::gateway::Query {
            select: crate::type_table::select("task"),
            ..Default::default()
        };
        let tasks = crate::gateway::query(v, &query).unwrap().items;
        build(&v.item_settings().types.types, &tasks, today, show_done)
    }

    fn workspace(name: &str, notes: &[(&str, &str)]) -> (PathBuf, Workspace) {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-agenda-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        std::fs::write(
            dir.join(".obsidian/daily-notes.json"),
            r#"{"folder":"Journal","format":"YYYY-MM-DD"}"#,
        )
        .unwrap();
        for (path, text) in notes {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let v = crate::workspace::scan(&dir).unwrap();
        (dir, v)
    }

    /// Each group's title and its rows' text and right side.
    fn shown(a: &Built) -> Vec<(String, Vec<String>)> {
        (a.list.groups.iter())
            .map(|g| {
                let rows = (g.rows.iter())
                    .map(|r| {
                        let t = |p: &[Piece]| {
                            p.iter()
                                .map(|p| p.text.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        format!("{} | {}", t(&r.text), t(&r.right))
                    })
                    .collect();
                (g.title.clone(), rows)
            })
            .collect()
    }

    #[test]
    fn files_tasks_by_their_agenda_date_around_a_fixed_today() {
        let (dir, v) = workspace(
            "group",
            &[
                (
                    "Plan.md",
                    "- [ ] Pay rent 📅 2026-11-07\n\
                     - [ ] Ring [[Kīlauea|the observatory]] ⏫ ⏳ 2026-11-08\n\
                     - [/] Paint 🛫 2026-11-14\n\
                     - [ ] Climb Fuji 📅 2026-11-15\n\
                     - [ ] Sort photos\n\
                     - [ ] Late one 📅 2026-11-01\n\
                     - [x] Done before ✅ 2026-11-02 📅 2026-11-01\n\
                     - [x] Done today ✅ 2026-11-07 📅 2026-11-03\n\
                     - [-] Dropped today ❌ 2026-11-07\n\
                     - [ ] Next year 📅 2027-01-04\n",
                ),
                ("Journal/2026-11-05.md", "- [ ] From the journal\n"),
            ],
        );
        let a = built(&v, d("2026-11-07"), false);
        assert_eq!(
            shown(&a),
            [
                (
                    "Overdue".into(),
                    vec![
                        "Late one | Sun 1 Nov Plan".into(),
                        "Done today | Tue 3 Nov Plan".into(),
                        "From the journal | Thu 5 Nov 2026-11-05".into(),
                    ]
                ),
                ("Today · Sat 7 Nov".into(), vec!["Pay rent | Plan".into()]),
                (
                    "Tomorrow · Sun 8 Nov".into(),
                    vec!["Ring the observatory ⏫ | Plan".into()]
                ),
                ("Sat 14 Nov".into(), vec!["Paint | Plan".into()]),
                (
                    "Later".into(),
                    vec![
                        "Climb Fuji | Sun 15 Nov Plan".into(),
                        "Next year | Mon 4 Jan 2027 Plan".into(),
                    ]
                ),
                (
                    "Undated".into(),
                    vec!["Sort photos | Plan".into(), "Dropped today | Plan".into()]
                ),
            ]
        );
        let groups = &a.list.groups;
        assert!(groups.last().unwrap().collapsed, "Undated starts folded");
        let late = &groups[0].rows[0];
        assert_eq!(late.check, Some(Check::Unchecked));
        assert_eq!(
            late.right[0].look.as_ref().unwrap().colour,
            Some(Colour::Red)
        );
        let done = &groups[0].rows[1];
        assert_eq!(done.check, Some(Check::Checked));
        assert!(
            done.look.as_ref().is_some_and(|l| l.strike),
            "drawn as done"
        );
        assert!(done.right[0].look.is_none(), "a done task isn't late");
        let chip = &groups[2].rows[0].text[1];
        assert!(
            chip.look
                .as_ref()
                .is_some_and(|l| l.badge && l.colour == Some(Colour::Orange))
        );
        assert_eq!(groups[3].rows[0].check, Some(Check::Partial));
        let at = &a.at[&groups[1].rows[0].id];
        assert_eq!((at.note.as_str(), at.line), ("Plan.md", 1));

        let all = built(&v, d("2026-11-07"), true);
        assert_eq!(all.list.groups[0].title, "Earlier");
        assert_eq!(
            all.list.groups[0].rows.len(),
            4,
            "the one done before shows too"
        );
        assert_eq!(all.list.keys[0].title, "hide done");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_subtype_of_task_is_in_the_agenda() {
        let (dir, v) = workspace(
            "subtype",
            &[
                (
                    ".cold-lava/types.toml",
                    r#"version = 1
                    [types.idea]
                    extends = "task"
                    on = "list-item"
                    where.tag = ["idea"]
                    match = '^\[[^\n]\]\s'
                    fields.status = { checkbox = true }
                    fields.text = { rest = true }
                    fields.due = { marker = "📅", type = "date" }
                    "#,
                ),
                ("Ideas.md", "- [ ] Glass from sand #idea 📅 2026-11-07\n"),
            ],
        );
        assert_eq!(v.item_settings().types.errors, Vec::<String>::new());
        let a = built(&v, d("2026-11-07"), false);
        assert_eq!(
            shown(&a),
            [(
                "Today · Sat 7 Nov".into(),
                vec!["Glass from sand #idea | Ideas".into()]
            )]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_agenda_is_an_extension_of_the_gateways() {
        use crate::ext::{Effect, ExtId, Registry};
        let (dir, _) = workspace(
            "extension",
            &[(
                "Plan.md",
                "- [ ] Pay rent 📅 2026-11-07\n- [ ] Call the plumber\n",
            )],
        );
        let mut core = crate::core::Core::open(&dir).unwrap();
        let mut r = Registry::new(vec![Box::new(Agenda::default())]);
        let ext = ExtId(0);
        let run = CommandRequest::Run {
            id: OPEN.into(),
            args: Json::Null,
        };
        let effects = r.call(ext, Some(&mut core), |e, h| e.command(run, h));
        let [Effect::OpenView { view, content, .. }] = &effects[..] else {
            panic!("{effects:?}")
        };
        let Content::List(list) = content else {
            panic!()
        };
        let rent = list.groups[0].rows[0].id.clone();
        let happened = |event| ViewNews::Event(Happened { view: *view, event });

        let tick = happened(view::Event::Toggled { row: rent.clone() });
        let effects = r.call(ext, Some(&mut core), |e, h| e.view(tick, h));
        let done = Effect::Notify {
            message: "done: Pay rent".into(),
        };
        assert_eq!(effects, [done]);
        let text = std::fs::read_to_string(dir.join("Plan.md")).unwrap();
        assert!(
            text.starts_with("- [x] Pay rent 📅 2026-11-07 ✅ "),
            "{text}"
        );
        let mut updates = Vec::new();
        for (_, news) in core.take_item_news() {
            updates.extend(r.call(ext, Some(&mut core), |e, h| e.items(news, h)));
        }
        let [Effect::UpdateView { select, .. }] = &updates[..] else {
            panic!("{updates:?}")
        };
        assert_eq!(
            select.as_deref(),
            Some(rent.as_str()),
            "the ticked row stays"
        );

        let open = happened(view::Event::Activated { row: rent.clone() });
        let effects = r.call(ext, Some(&mut core), |e, h| e.view(open, h));
        let note = Effect::OpenNote {
            note: "Plan.md".into(),
            line: Some(1),
            item: Some(rent),
        };
        assert_eq!(effects, [note]);

        let closed = ViewNews::Closed { view: *view };
        r.call(ext, Some(&mut core), |e, h| e.view(closed, h));
        std::fs::write(dir.join("Plan.md"), "- [ ] Water the fern\n").unwrap();
        core.note_changed_now(&dir.join("Plan.md"));
        assert_eq!(core.take_item_news(), [], "it stopped following the tasks");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn past_midnight_the_open_agenda_files_by_the_new_day() {
        use crate::ext::{Effect, ExtId, Registry};
        use std::cell::Cell;
        use std::rc::Rc;
        let (dir, _) = workspace("midnight", &[("Plan.md", "- [ ] Pay rent 📅 2026-11-08\n")]);
        let mut core = crate::core::Core::open(&dir).unwrap();
        let clock = Rc::new(Cell::new(d("2026-11-07")));
        let today = Rc::clone(&clock);
        let agenda = Agenda::on(move || today.get());
        let mut r = Registry::new(vec![Box::new(agenda)]);
        let ext = ExtId(0);
        let run = CommandRequest::Run {
            id: OPEN.into(),
            args: Json::Null,
        };
        let effects = r.call(ext, Some(&mut core), |e, h| e.command(run, h));
        let [Effect::OpenView { content, .. }] = &effects[..] else {
            panic!("{effects:?}")
        };
        let Content::List(list) = content else {
            panic!()
        };
        assert_eq!(list.groups[0].title, "Tomorrow · Sun 8 Nov");

        let effects = r.call(ext, Some(&mut core), |e, h| e.poll(h));
        assert_eq!(effects, [], "the same day sends nothing");

        clock.set(d("2026-11-08"));
        let effects = r.call(ext, Some(&mut core), |e, h| e.poll(h));
        let [Effect::UpdateView { content, .. }] = &effects[..] else {
            panic!("{effects:?}")
        };
        let Content::List(list) = content else {
            panic!()
        };
        assert_eq!(list.groups[0].title, "Today · Sun 8 Nov");
        let effects = r.call(ext, Some(&mut core), |e, h| e.poll(h));
        assert_eq!(effects, [], "once a day");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_read_as_their_text() {
        assert_eq!(
            links_as_text("See [[Mauna Loa]], [[Lava|flows]] and [docs](https://x.org)"),
            "See Mauna Loa, flows and docs"
        );
    }
}
