//! The item messages of the workspace gateway (docs/extensions.md, "The workspace gateway, as
//! built"): `items/query` reads notes as data, `items/subscribe` follows a query as notes
//! change, `items/edit` changes one item through the safe edits, and `types/list` names the
//! types. Every type here derives serde and tests pin its JSON, so a transport carries them as
//! they are. The core answers them (`Core::query_items`, `Core::subscribe_items`,
//! `Core::edit_item`); the frontend's `Host` checks an extension's permissions first.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::edit::EditError;
use crate::items::{Data, Item, KINDS, Settings};
use crate::owner::markdown::{self, After};
use crate::workspace::{Note, Owned, Workspace, search_key};
use crate::{tasks, write};

/// What a query or a subscription is about: items of a kind or type (a subtype answers to its
/// base, so `task` brings ideas too), narrowed by a filter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Select {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Filter::is_empty")]
    pub filter: Filter,
}

/// A fixed filter, no query language: every condition given must hold.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Filter {
    /// Items of this one note or file: workspace-relative, with its extension (`Board.md`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Notes in this folder or under it, workspace-relative.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Notes with this tag (with or without `#`); a nested tag counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Items whose `status_type` is one of these (`todo`, `in_progress`, `done`, …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<DateRange>,
    /// Every folded word is in one of the item's text values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Items with a value for this field (a note's property, any typed item's field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
}

/// Items whose date `field` is between `from` and `to`, both included; an open end is no
/// bound. An item without that date doesn't match.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DateRange {
    pub field: DateField,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<NaiveDate>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateField {
    Due,
    Scheduled,
    Start,
    Done,
    /// The date `cold-lava agenda` files a task under: due, else scheduled, else start, else
    /// its daily note's.
    Agenda,
}

/// `items/query`: the items `select` names, ordered by note and line, a page at a time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Query {
    #[serde(flatten)]
    pub select: Select,
    #[serde(default, skip_serializing_if = "Page::is_all")]
    pub page: Page,
}

/// Which page: the items after the one with id `after` (the last of the previous page), at
/// most `limit` of them. Without a limit, every item that is left.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Page {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// The answer to `items/query`. `next` is the `after` of the next page, when there is one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub items: Vec<Item>,
    pub next: Option<String>,
}

/// A subscription, numbered by the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubId(pub u64);

/// The answer to `items/subscribe`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subscribed {
    pub subscription: SubId,
}

/// `items/edit`: change the item `id`, refused unless it is still the version `rev`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    pub id: String,
    pub rev: String,
    #[serde(flatten)]
    pub op: Op,
}

/// What to do to the item. Each op is one of the engine's safe edits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Op {
    /// One field of a typed item, through `edit::plan`: the value in the form items carry
    /// it, or as text the field's type reads; `null` or `""` removes it.
    SetField { field: String, value: Json },
    /// Tick a task the way the Tasks plugin does, through `tasks::toggle_text`: done dates,
    /// recurrence, custom statuses.
    Toggle,
}

/// The answer to `items/edit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edited {
    /// The item as it reads now. Its id is new when the edit changed its key.
    pub item: Item,
    /// False when the note already said this, and nothing was written.
    pub changed: bool,
    /// A recurring task's next occurrence, added by ticking it.
    pub next: Option<Item>,
    /// Something worth saying that didn't stop the edit.
    pub warning: Option<String>,
}

/// The answer to `items/edit` as the core gives it: now, or, for an item in a file another
/// owner reads, once that owner has written it (`Core::take_edited`, under this ticket). Over
/// the wire both are the same answer, the second one later.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Now(Box<Edited>),
    Later(Ticket),
}

/// An `items/edit` that is answered later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ticket(pub u64);

/// Why a request was refused. On the wire it is the JSON-RPC error's `data`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Error {
    pub reason: Reason,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// The item is gone or not the version named: read it again.
    Changed,
    /// The request can't be done this way: an unknown kind or field, a read-only source, a
    /// value that doesn't fit, a toggle on something that is no task.
    Refused,
    /// The note wouldn't read back as asked, so nothing was written.
    ReadBack,
    Io,
    /// The extension's manifest doesn't ask for the permission this needs.
    Forbidden,
    /// No workspace is open.
    NoWorkspace,
    /// `workspace/create` at a path where a file is already.
    Exists,
}

impl Error {
    pub fn new(reason: Reason, message: impl Into<String>) -> Error {
        let message = message.into();
        Error { reason, message }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<EditError> for Error {
    fn from(e: EditError) -> Error {
        let reason = match e {
            EditError::Changed(_) => Reason::Changed,
            EditError::Refused(_) => Reason::Refused,
            EditError::ReadBack(_) => Reason::ReadBack,
            EditError::Io(_) => Reason::Io,
        };
        Error::new(reason, e.to_string())
    }
}

/// What an extension asks of the workspace gateway about items (extension → host). Each has an
/// answer: `Found`, `Subscribed`, nothing, `Edited`, or the types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Request {
    #[serde(rename = "items/query")]
    Query(Query),
    #[serde(rename = "items/subscribe")]
    Subscribe(Select),
    #[serde(rename = "items/unsubscribe")]
    Unsubscribe { subscription: SubId },
    #[serde(rename = "items/edit")]
    Edit(Edit),
    /// Every loaded type with its definition, as `cold-lava types --json` lists them.
    #[serde(rename = "types/list")]
    TypesList,
}

/// What a subscriber hears (host → extension), once per batch of changes the core reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum News {
    /// Items that now match, items that match and changed, and ids that no longer match.
    #[serde(rename = "items/changed")]
    Changed {
        subscription: SubId,
        added: Vec<Item>,
        updated: Vec<Item>,
        removed: Vec<String>,
    },
    /// The workspace was read again from scratch: query again.
    #[serde(rename = "items/reset")]
    Reset { subscription: SubId },
}

impl News {
    pub fn subscription(&self) -> SubId {
        match self {
            News::Changed { subscription, .. } | News::Reset { subscription } => *subscription,
        }
    }
}

/// A file items come from: a note, or a file another owner reads.
#[derive(Clone, Copy)]
enum File<'a> {
    Note(&'a Note),
    Owned(&'a Owned),
}

impl<'a> File<'a> {
    /// Workspace-relative: a note without `.md`, another file with its extension.
    fn rel(self) -> &'a str {
        match self {
            File::Note(n) => &n.rel,
            File::Owned(f) => &f.rel,
        }
    }

    fn tags(self) -> &'a [String] {
        match self {
            File::Note(n) => &n.tags,
            File::Owned(_) => &[],
        }
    }

    /// How subscriptions know it: its path with its extension, as items name it.
    fn key(self) -> String {
        match self {
            File::Note(n) => format!("{}.md", n.rel),
            File::Owned(f) => f.rel.clone(),
        }
    }
}

/// Every file items come from: the notes, then the files other owners read.
fn files(workspace: &Workspace) -> impl Iterator<Item = File<'_>> {
    let notes = workspace.notes.iter().map(File::Note);
    notes.chain(workspace.owned.iter().map(File::Owned))
}

/// The file at `key` (`File::key`).
fn file<'a>(workspace: &'a Workspace, key: &str) -> Option<File<'a>> {
    if let Some(rel) = key.strip_suffix(".md")
        && let Ok(i) = (workspace.notes).binary_search_by(|n| n.rel.as_str().cmp(rel))
    {
        return Some(File::Note(&workspace.notes[i]));
    }
    let i = (workspace.owned)
        .binary_search_by(|f| f.rel.as_str().cmp(key))
        .ok()?;
    Some(File::Owned(&workspace.owned[i]))
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    fn matches(&self, file: File<'_>, item: &Item) -> bool {
        let fields = match &item.data {
            Data::Typed(t) => Some(&t.fields),
            _ => None,
        };
        let field = |name: &str| fields.and_then(|f| f.get(name)).filter(|v| has_value(v));
        if self.note.as_ref().is_some_and(|n| *n != file.key()) {
            return false;
        }
        if let Some(folder) = &self.folder {
            let folder = folder.trim_matches('/');
            let inside = file.rel().strip_prefix(folder);
            if !folder.is_empty() && !inside.is_some_and(|r| r.starts_with('/')) {
                return false;
            }
        }
        if let Some(tag) = &self.tag {
            let tag = tag.trim_start_matches('#').to_lowercase();
            let nested = format!("{tag}/");
            if !(file.tags().iter()).any(|t| *t == tag || t.starts_with(&nested)) {
                return false;
            }
        }
        if !self.status.is_empty() {
            let status = field("status_type").and_then(Json::as_str);
            if !status.is_some_and(|s| self.status.iter().any(|w| w == s)) {
                return false;
            }
        }
        if let Some(range) = &self.date {
            let names: &[&str] = match range.field {
                DateField::Due => &["due"],
                DateField::Scheduled => &["scheduled"],
                DateField::Start => &["start"],
                DateField::Done => &["done"],
                DateField::Agenda => &["due", "scheduled", "start", "daily_note"],
            };
            let date = (names.iter()).find_map(|n| date(field(n)?));
            let inside = date.is_some_and(|d| {
                range.from.is_none_or(|f| f <= d) && range.to.is_none_or(|t| d <= t)
            });
            if !inside {
                return false;
            }
        }
        if let Some(text) = &self.text {
            let mut hay = String::new();
            let mut add = |s: &str| {
                hay.push_str(&search_key(s));
                hay.push('\n');
            };
            match &item.data {
                Data::List { text } | Data::Heading { text, .. } => add(text),
                Data::Block => {}
                Data::Typed(t) => {
                    for v in t.fields.values() {
                        match v {
                            Json::String(s) => add(s),
                            Json::Array(a) => a.iter().filter_map(Json::as_str).for_each(&mut add),
                            _ => {}
                        }
                    }
                }
            }
            if !search_key(text).split_whitespace().all(|w| hay.contains(w)) {
                return false;
            }
        }
        if let Some(p) = &self.property
            && field(p).is_none()
        {
            return false;
        }
        true
    }
}

fn has_value(v: &Json) -> bool {
    !(v.is_null() || v.as_array().is_some_and(Vec::is_empty))
}

/// A date value: `2026-11-12`, or the day of a datetime.
fn date(v: &Json) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(v.as_str()?.get(..10)?, "%Y-%m-%d").ok()
}

impl Select {
    /// Refused unless its kind is a native kind or a loaded type.
    fn check(&self, workspace: &Workspace) -> Result<(), Error> {
        let kind = self.kind.as_str();
        let typed = || workspace.item_settings().matcher.names().any(|n| n == kind);
        match KINDS.contains(&kind) || typed() {
            true => Ok(()),
            false => Err(Error::new(
                Reason::Refused,
                format!("no item kind or type called {kind}"),
            )),
        }
    }

    /// The file's items it names, as the gateway hands them out (`answer`).
    fn in_file<'a>(&self, workspace: &'a Workspace, file: File<'a>) -> Vec<Cow<'a, Item>> {
        let items: &[Item] = match file {
            File::Note(note) => workspace.items(note),
            File::Owned(f) => &f.items,
        };
        (items.iter())
            .filter(|i| i.data.answers_to(&self.kind))
            .map(|i| match file {
                File::Note(note) => answer(workspace, note, i),
                File::Owned(_) => Cow::Borrowed(i),
            })
            .filter(|i| self.filter.matches(file, i))
            .collect()
    }
}

/// An item as the gateway hands it out. One that answers to `task` always has a `status` and
/// a `status_type`, even when its type reads neither (a subtype that names its checkbox
/// otherwise): the status from its line, its type from the workspace's Tasks settings, as
/// `tasks::Task` reads them. So a client never needs the settings or the line.
fn answer<'a>(workspace: &Workspace, note: &Note, item: &'a Item) -> Cow<'a, Item> {
    let line = || note.text.lines().nth(item.line - 1).unwrap_or_default();
    match needs_status(item) {
        true => answer_line(workspace.item_settings(), line(), item),
        false => Cow::Borrowed(item),
    }
}

/// Whether a task's `status` or `status_type` isn't read by its type, so `answer` adds them.
fn needs_status(item: &Item) -> bool {
    let Data::Typed(t) = &item.data else {
        return false;
    };
    let has = |f: &str| t.fields.get(f).is_some_and(|v| !v.is_null());
    item.data.answers_to("task") && !(has("status") && has("status_type"))
}

/// `answer`, for an item whose first line is `line`.
pub(crate) fn answer_line<'a>(s: &Settings, line: &str, item: &'a Item) -> Cow<'a, Item> {
    if !needs_status(item) {
        return Cow::Borrowed(item);
    }
    let Some(task) = tasks::Task::of(item, line, &s.tasks) else {
        return Cow::Borrowed(item);
    };
    let mut item = item.clone();
    if let Data::Typed(t) = &mut item.data {
        for (field, value) in [
            ("status", task.status.to_string()),
            ("status_type", task.status_type.name().to_string()),
        ] {
            if t.fields.get(field).is_none_or(Json::is_null) {
                t.fields.insert(field, Json::String(value));
            }
        }
    }
    Cow::Owned(item)
}

/// `answer`, for an item of the note at `rel` (workspace-relative, with `.md`).
fn answered(workspace: &Workspace, rel: &str, item: Item) -> Item {
    let rel = rel.strip_suffix(".md").unwrap_or(rel);
    match workspace
        .notes
        .binary_search_by(|n| n.rel.as_str().cmp(rel))
    {
        Ok(i) => answer(workspace, &workspace.notes[i], &item).into_owned(),
        Err(_) => item,
    }
}

impl Page {
    pub fn is_all(&self) -> bool {
        *self == Page::default()
    }
}

/// `items/query` on the workspace as the core has it now.
pub fn query(workspace: &Workspace, q: &Query) -> Result<Found, Error> {
    q.select.check(workspace)?;
    let mut all = files(workspace).flat_map(|f| q.select.in_file(workspace, f));
    if let Some(after) = &q.page.after
        && !all.by_ref().any(|i| i.id == *after)
    {
        let message = format!("the page ended at {after}, which no longer matches: query again");
        return Err(Error::new(Reason::Changed, message));
    }
    let limit = q.page.limit.unwrap_or(usize::MAX).max(1);
    let items: Vec<Item> = all.by_ref().take(limit).map(Cow::into_owned).collect();
    let next = match items.len() == limit && all.next().is_some() {
        true => items.last().map(|i| i.id.clone()),
        false => None,
    };
    Ok(Found { items, next })
}

/// Who follows which query, and the version of each item they have heard about.
#[derive(Default)]
pub struct Subscriptions {
    next: u64,
    subs: Vec<Sub>,
}

struct Sub {
    id: SubId,
    /// The extension that asked: the name in its manifest.
    owner: String,
    select: Select,
    /// By file (`File::key`): the matching items' ids and revs.
    seen: HashMap<String, Vec<(String, String)>>,
}

impl Sub {
    fn snapshot(&mut self, workspace: &Workspace) {
        self.seen = files(workspace)
            .map(|f| (f.key(), revs(&self.select.in_file(workspace, f))))
            .filter(|(_, items)| !items.is_empty())
            .collect();
    }
}

fn revs(items: &[Cow<'_, Item>]) -> Vec<(String, String)> {
    (items.iter())
        .map(|i| (i.id.clone(), i.rev.clone()))
        .collect()
}

impl Subscriptions {
    pub fn add(
        &mut self,
        workspace: &Workspace,
        owner: &str,
        select: Select,
    ) -> Result<SubId, Error> {
        select.check(workspace)?;
        self.next += 1;
        let id = SubId(self.next);
        let mut sub = Sub {
            id,
            owner: owner.to_string(),
            select,
            seen: HashMap::new(),
        };
        sub.snapshot(workspace);
        self.subs.push(sub);
        Ok(id)
    }

    /// Whether `owner` had such a subscription.
    pub fn remove(&mut self, owner: &str, id: SubId) -> bool {
        let before = self.subs.len();
        self.subs.retain(|s| s.id != id || s.owner != owner);
        self.subs.len() != before
    }

    /// What each subscriber hears about these files having been read again (workspace-relative
    /// paths with their extension, as items name them): one `items/changed` per subscription
    /// that any of them touched, with its owner's name.
    pub fn changed(&mut self, workspace: &Workspace, files: &[String]) -> Vec<(String, News)> {
        let mut out = Vec::new();
        for sub in &mut self.subs {
            let (mut added, mut updated, mut removed) = (Vec::new(), Vec::new(), Vec::new());
            for rel in files {
                let now = match file(workspace, rel) {
                    Some(f) => sub.select.in_file(workspace, f),
                    None => Vec::new(),
                };
                let old: HashMap<String, String> = sub
                    .seen
                    .remove(rel)
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                let ids: HashSet<&str> = now.iter().map(|i| i.id.as_str()).collect();
                removed.extend(
                    (old.keys())
                        .filter(|id| !ids.contains(id.as_str()))
                        .cloned(),
                );
                for item in &now {
                    match old.get(&item.id) {
                        None => added.push(item.as_ref().clone()),
                        Some(rev) if *rev != item.rev => updated.push(item.as_ref().clone()),
                        Some(_) => {}
                    }
                }
                if !now.is_empty() {
                    sub.seen.insert(rel.to_string(), revs(&now));
                }
            }
            if added.is_empty() && updated.is_empty() && removed.is_empty() {
                continue;
            }
            removed.sort();
            let news = News::Changed {
                subscription: sub.id,
                added,
                updated,
                removed,
            };
            out.push((sub.owner.clone(), news));
        }
        out
    }

    /// The workspace was read from scratch: every subscriber hears `items/reset`.
    pub fn reset(&mut self, workspace: &Workspace) -> Vec<(String, News)> {
        (self.subs.iter_mut())
            .map(|sub| {
                sub.snapshot(workspace);
                (
                    sub.owner.clone(),
                    News::Reset {
                        subscription: sub.id,
                    },
                )
            })
            .collect()
    }
}

/// An edit written to its note, before the core reads the note again.
pub(crate) struct Written {
    /// Workspace-relative, with `.md`.
    pub note: String,
    pub changed: bool,
    /// What it did, for the status line: "set due", "ticked".
    pub what: String,
    after: After,
    warning: Option<String>,
}

/// Make the edit on disk: the note's owner makes its new text, which is written with
/// `write::replace`, refused if the note changed since it was read.
pub(crate) fn write(root: &Path, workspace: &Workspace, req: &Edit) -> Result<Written, Error> {
    let item = (workspace.notes.iter())
        .flat_map(|n| workspace.items(n))
        .find(|i| i.id == req.id)
        .ok_or_else(|| {
            let message = format!(
                "no item {} now: it changed or is gone, read it again",
                req.id
            );
            Error::new(Reason::Changed, message)
        })?;
    let note = item.note.clone();
    let path = root.join(&note);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| Error::new(Reason::Io, format!("{}: {e}", path.display())))?;
    let made = markdown::edit(
        &note,
        &text,
        &Settings::load(root),
        item,
        req,
        tasks::today(),
    )?;
    if made.changed {
        write::replace(&path, text.as_bytes(), made.text.as_bytes()).map_err(|e| {
            let reason = match e {
                write::WriteError::Changed(_) => Reason::Changed,
                _ => Reason::Io,
            };
            Error::new(reason, e.to_string())
        })?;
    }
    Ok(Written {
        note,
        changed: made.changed,
        what: made.what,
        after: made.after,
        warning: made.warning,
    })
}

/// The answer to an edit, from the note as the core has read it since.
pub(crate) fn read_back(workspace: &Workspace, w: Written) -> Result<Edited, Error> {
    let task_at = |line: usize| {
        let rel = w.note.strip_suffix(".md").unwrap_or(&w.note);
        let i = workspace
            .notes
            .binary_search_by(|n| n.rel.as_str().cmp(rel))
            .ok()?;
        let items = workspace.items(&workspace.notes[i]);
        (items.iter())
            .find(|i| i.line == line && i.data.answers_to("task"))
            .cloned()
    };
    let (item, next) = match w.after {
        After::Set(item) => (answered(workspace, &w.note, item), None),
        After::Toggled { line, next } => {
            let item = task_at(line).ok_or_else(|| {
                let message = format!("ticked, but line {line} of {} reads as no task", w.note);
                Error::new(Reason::ReadBack, message)
            })?;
            let next = next.and_then(task_at);
            let fill = |i: Item| answered(workspace, &w.note, i);
            (fill(item), next.map(fill))
        }
    };
    Ok(Edited {
        item,
        changed: w.changed,
        next,
        warning: w.warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Core, Event};
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use std::fmt::Debug;
    use std::path::PathBuf;
    use std::time::Duration;

    /// An edit of a note, which is answered at once.
    fn now(answer: Result<Answer, Error>) -> Result<Edited, Error> {
        answer.map(|a| match a {
            Answer::Now(edited) => *edited,
            Answer::Later(_) => panic!("an edit of a note is answered at once"),
        })
    }

    /// `value` is exactly `json` on the wire, and reads back from it.
    fn wire<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: T, json: Json) {
        assert_eq!(serde_json::to_value(&value).unwrap(), json);
        assert_eq!(serde_json::from_value::<T>(json).unwrap(), value);
    }

    const IDEA: &str = r#"version = 1
[types.idea]
extends = "task"
on = "list-item"
where.tag = ["idea"]
match = '^\[[^\n]\]\s'
fields.status = { checkbox = true }
fields.text = { rest = true }
fields.scheduled = { marker = "⏳", type = "date" }
"#;

    fn workspace(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-gateway-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let notes = [
            (".cold-lava/types.toml", IDEA),
            (
                "Plan.md",
                "---\ntags: [home]\n---\n# Plan\n\
                 - [ ] Pay rent 📅 2026-11-07\n\
                 - [x] Done thing ✅ 2026-11-02 📅 2026-11-01\n\
                 - [ ] Water plants 🔁 every week 📅 2026-11-10\n\
                 - plain item\n",
            ),
            (
                "Work/Ideas.md",
                "- [ ] Glass from sand #idea ⏳ 2026-11-20\n",
            ),
            (
                "Work/Deep/Notes.md",
                "#home/garden\n\n- [/] Repot the fern\n",
            ),
        ];
        for (path, text) in notes {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        dir
    }

    fn select(kind: &str, filter: Filter) -> Select {
        let kind = kind.into();
        Select { kind, filter }
    }

    fn all(select: Select) -> Query {
        let page = Page::default();
        Query { select, page }
    }

    /// The words of the items a query finds.
    fn words(core: &Core, kind: &str, filter: Filter) -> Vec<String> {
        let found = core.query_items(&all(select(kind, filter))).unwrap();
        assert_eq!(found.next, None);
        (found.items.iter())
            .map(|i| match &i.data {
                Data::Typed(t) => t.fields["text"].as_str().unwrap().to_string(),
                Data::List { text } | Data::Heading { text, .. } => text.clone(),
                Data::Block => i.id.clone(),
            })
            .collect()
    }

    fn day(s: &str) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
    }

    fn range(field: DateField, from: &str, to: &str) -> Filter {
        let date = Some(DateRange {
            field,
            from: day(from),
            to: day(to),
        });
        Filter {
            date,
            ..Filter::default()
        }
    }

    #[test]
    fn queries_filter_by_folder_tag_status_date_text_and_property() {
        let dir = workspace("filter");
        let core = Core::open(&dir).unwrap();
        let f = |f: Filter| words(&core, "task", f);
        let none = Vec::<String>::new();
        assert_eq!(
            f(Filter::default()),
            [
                "Pay rent",
                "Done thing",
                "Water plants",
                "Repot the fern",
                "Glass from sand #idea"
            ],
            "by note, then line; a subtype answers to its base"
        );
        assert_eq!(
            words(&core, "idea", Filter::default()),
            ["Glass from sand #idea"]
        );
        assert_eq!(words(&core, "list", Filter::default()), ["plain item"]);
        let folder = |s: &str| Filter {
            folder: Some(s.into()),
            ..Filter::default()
        };
        assert_eq!(
            f(folder("Work")),
            ["Repot the fern", "Glass from sand #idea"]
        );
        assert_eq!(f(folder("Work/Deep/")), ["Repot the fern"]);
        assert_eq!(f(folder("Wor")), none, "a folder, not a prefix");
        let note = |s: &str| Filter {
            note: Some(s.into()),
            ..Filter::default()
        };
        assert_eq!(f(note("Work/Deep/Notes.md")), ["Repot the fern"]);
        assert_eq!(f(note("Work/Deep/Notes")), none, "with its extension");
        let tag = Filter {
            tag: Some("#home".into()),
            ..Filter::default()
        };
        assert_eq!(
            f(tag),
            ["Pay rent", "Done thing", "Water plants", "Repot the fern"],
            "a nested tag counts"
        );
        let status = Filter {
            status: vec!["done".into(), "in_progress".into()],
            ..Filter::default()
        };
        assert_eq!(f(status), ["Done thing", "Repot the fern"]);
        let todo = Filter {
            status: vec!["todo".into()],
            ..Filter::default()
        };
        assert_eq!(
            f(todo),
            ["Pay rent", "Water plants", "Glass from sand #idea"],
            "a subtype that reads no status_type has one"
        );
        let due = range(DateField::Due, "2026-11-05", "");
        assert_eq!(f(due), ["Pay rent", "Water plants"]);
        let agenda = range(DateField::Agenda, "2026-11-08", "2026-11-30");
        assert_eq!(
            f(agenda),
            ["Water plants", "Glass from sand #idea"],
            "scheduled counts"
        );
        assert_eq!(f(range(DateField::Done, "", "2026-11-02")), ["Done thing"]);
        let text = Filter {
            text: Some("PLANTS water".into()),
            ..Filter::default()
        };
        assert_eq!(f(text), ["Water plants"]);
        let recurring = Filter {
            property: Some("recurrence".into()),
            ..Filter::default()
        };
        assert_eq!(f(recurring), ["Water plants"]);
        let e = core.query_items(&all(select("book", Filter::default())));
        let e = e.unwrap_err();
        assert_eq!(e.reason, Reason::Refused);
        assert!(e.message.contains("book"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_always_says_its_status_and_status_type() {
        let dir = workspace("status");
        let chore = r#"version = 1
[types.chore]
extends = "task"
on = "list-item"
where.tag = ["chore"]
match = '^\[[^\n]\]\s'
fields.text = { rest = true }
"#;
        std::fs::write(dir.join(".cold-lava/types.toml"), chore).unwrap();
        std::fs::write(dir.join("Chores.md"), "- [/] Sweep #chore\n").unwrap();
        let mut core = Core::open(&dir).unwrap();
        let errors = &core.index.workspace.item_settings().types.errors;
        assert!(errors.is_empty(), "{errors:?}");
        let chore = core.query_items(&all(select("chore", Filter::default())));
        let chore = chore.unwrap().items.remove(0);
        let field = |item: &Item, f: &str| match &item.data {
            Data::Typed(t) => t.fields[f].clone(),
            _ => panic!("{item:?}"),
        };
        assert_eq!(field(&chore, "status"), "/", "read from its line");
        assert_eq!(field(&chore, "status_type"), "in_progress");
        let sub = core.subscribe_items("t", select("task", Filter::default()));
        let edited = now(core.edit_item("t", &tick(&chore))).unwrap();
        assert_eq!(
            field(&edited.item, "status_type"),
            "done",
            "an edit's answer"
        );
        let news = core.take_item_news();
        let [
            (
                _,
                News::Changed {
                    subscription,
                    added,
                    ..
                },
            ),
        ] = &news[..]
        else {
            panic!("{news:?}")
        };
        assert_eq!(Ok(*subscription), sub);
        // Its words are its key, and they read the checkbox too, so it has a new id.
        assert_eq!(field(&added[0], "status_type"), "done", "and news");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queries_page_after_the_last_item() {
        let dir = workspace("page");
        let core = Core::open(&dir).unwrap();
        let page = |after: Option<String>| {
            let page = Page {
                after,
                limit: Some(2),
            };
            let select = select("task", Filter::default());
            core.query_items(&Query { select, page })
        };
        let mut seen = Vec::new();
        let (mut after, mut pages) = (None, 0);
        loop {
            let found = page(after).unwrap();
            pages += 1;
            seen.extend(found.items.iter().map(|i| i.id.clone()));
            match found.next {
                Some(next) => after = Some(next),
                None => break,
            }
        }
        assert_eq!((pages, seen.len()), (3, 5));
        assert_eq!(page(None).unwrap().next.as_deref(), Some(seen[1].as_str()));
        let gone = page(Some("no-such-item".into())).unwrap_err();
        assert_eq!(gone.reason, Reason::Changed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn task(core: &Core, text: &str) -> Item {
        let filter = Filter {
            text: Some(text.into()),
            ..Filter::default()
        };
        let found = core.query_items(&all(select("task", filter))).unwrap();
        found.items.into_iter().next().unwrap()
    }

    fn set(item: &Item, field: &str, value: Json) -> Edit {
        let field = field.into();
        Edit {
            id: item.id.clone(),
            rev: item.rev.clone(),
            op: Op::SetField { field, value },
        }
    }

    fn tick(item: &Item) -> Edit {
        Edit {
            id: item.id.clone(),
            rev: item.rev.clone(),
            op: Op::Toggle,
        }
    }

    /// How many items each piece of news adds, updates and removes.
    fn counts(news: &[(String, News)]) -> Vec<(usize, usize, usize)> {
        (news.iter())
            .map(|(_, n)| match n {
                News::Changed {
                    added,
                    updated,
                    removed,
                    ..
                } => (added.len(), updated.len(), removed.len()),
                News::Reset { .. } => (0, 0, 0),
            })
            .collect()
    }

    #[test]
    fn subscribers_hear_what_was_added_updated_and_removed() {
        let dir = workspace("subscribe");
        let mut core = Core::open(&dir).unwrap();
        let open = Filter {
            status: vec!["todo".into()],
            ..Filter::default()
        };
        let todo = core.subscribe_items("todo", select("task", open)).unwrap();
        let ideas = (core.subscribe_items("ideas", select("idea", Filter::default()))).unwrap();
        assert_ne!(todo, ideas);

        // Ticking "Pay rent" takes it out of the open tasks; ideas hear nothing.
        let rent = task(&core, "Pay rent");
        now(core.edit_item("test", &tick(&rent))).unwrap();
        let news = core.take_item_news();
        let removed = News::Changed {
            subscription: todo,
            added: vec![],
            updated: vec![],
            removed: vec![rent.id.clone()],
        };
        assert_eq!(news, [("todo".to_string(), removed)]);

        // Written elsewhere: a new open task and one re-dated, and the ideas' note deleted.
        let plan = dir.join("Plan.md");
        let text = std::fs::read_to_string(&plan).unwrap();
        let text = text.replace("2026-11-10", "2026-11-11") + "- [ ] Call the plumber\n";
        std::fs::write(&plan, text).unwrap();
        std::fs::remove_file(dir.join("Work/Ideas.md")).unwrap();
        core.note_changed(&plan);
        core.note_changed(&dir.join("Work/Ideas.md"));
        let mut news = Vec::new();
        for _ in 0..100 {
            core.poll();
            news.extend(core.take_item_news());
            if !news.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        // The watcher may report a folder too, and then the core reads everything again. The
        // idea reads no `status_type`, but has one, so it was an open task too.
        if !(news.iter()).any(|(_, n)| matches!(n, News::Reset { .. })) {
            assert_eq!(counts(&news), [(1, 1, 1), (0, 0, 1)], "one batch each");
            let News::Changed { added, .. } = &news[0].1 else {
                unreachable!()
            };
            assert_eq!(added[0].note, "Plan.md");
        }

        assert!(core.unsubscribe_items("todo", todo));
        assert!(!core.unsubscribe_items("todo", ideas), "only its own");
        core.rescan_now();
        let reset = News::Reset {
            subscription: ideas,
        };
        assert_eq!(core.take_item_news(), [("ideas".to_string(), reset)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edits_go_through_the_safe_path_and_name_who_made_them() {
        let dir = workspace("edit");
        let mut core = Core::open(&dir).unwrap();
        let rent = task(&core, "Pay rent");
        let done = now(core.edit_item("table", &set(&rent, "due", json!("2026-12-01"))));
        let done = done.unwrap();
        assert!(done.changed);
        assert_eq!(done.item.id, rent.id);
        let text = std::fs::read_to_string(dir.join("Plan.md")).unwrap();
        assert!(text.contains("- [ ] Pay rent 📅 2026-12-01\n"), "{text}");
        assert_eq!(
            task(&core, "Pay rent"),
            done.item,
            "the core read it at once"
        );
        let edited = Event::ItemEdited {
            note: "Plan".into(),
            item: rent.id.clone(),
            by: "table".into(),
            what: "set due".into(),
        };
        assert_eq!(core.poll(), [Event::NoteChanged("Plan".into()), edited]);

        let again = now(core.edit_item("table", &set(&done.item, "due", json!("2026-12-01"))));
        assert!(!again.unwrap().changed);
        assert_eq!(core.poll(), [], "nothing written, nothing to hear");
        let removed = now(core.edit_item("table", &set(&done.item, "due", Json::Null)));
        assert!(removed.unwrap().changed);
        assert!(matches!(&core.poll()[1], Event::ItemEdited { what, .. } if what == "removed due"));

        let refused =
            |core: &mut Core, edit: &Edit| now(core.edit_item("table", edit)).unwrap_err();
        let stale = refused(&mut core, &set(&rent, "due", json!("2026-12-02")));
        assert_eq!(stale.reason, Reason::Changed, "{stale}");
        let rent = task(&core, "Pay rent");
        let read_only = refused(&mut core, &set(&rent, "text", json!("x")));
        assert_eq!(read_only.reason, Reason::Refused, "{read_only}");
        let unfit = refused(&mut core, &set(&rent, "due", json!("soon")));
        assert_eq!(unfit.reason, Reason::Refused, "{unfit}");
        let workspace = &core.index.workspace;
        let list = (workspace.notes.iter())
            .flat_map(|n| workspace.items(n))
            .find(|i| i.data.kind() == "list")
            .unwrap()
            .clone();
        let no_task = refused(&mut core, &tick(&list));
        assert_eq!(no_task.reason, Reason::Refused);
        assert!(no_task.message.contains("not a task"), "{no_task}");
        let gone = Edit {
            id: "nope".into(),
            ..tick(&list)
        };
        assert_eq!(refused(&mut core, &gone).reason, Reason::Changed);

        // Ticking a recurring task: done, with its next occurrence.
        let water = task(&core, "Water plants");
        let ticked = now(core.edit_item("agenda", &tick(&water))).unwrap();
        let field = |i: &Item, f: &str| match &i.data {
            Data::Typed(t) => t.fields[f].as_str().map(String::from),
            _ => None,
        };
        assert_eq!(field(&ticked.item, "status_type").as_deref(), Some("done"));
        let next = ticked.next.expect("the next occurrence");
        assert_eq!(field(&next, "status_type").as_deref(), Some("todo"));
        assert_eq!(field(&next, "due").as_deref(), Some("2026-11-17"));
        let stale = refused(&mut core, &tick(&water));
        assert_eq!(stale.reason, Reason::Changed, "{stale}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn item() -> Item {
        serde_json::from_value(json!({
            "id": "81e7d3e0c9328c61", "note": "Plan.md", "line": 5, "end_line": 5,
            "section": ["Plan"], "parent": null, "block_id": null, "rev": "c1e0d4d17c74",
            "kind": "task", "status": " ", "text": "Pay rent", "due": "2026-11-07",
        }))
        .unwrap()
    }

    #[test]
    fn requests_are_json_rpc_messages() {
        let filter = Filter {
            note: Some("Journal/2026-11-05.md".into()),
            folder: Some("Journal".into()),
            tag: Some("home".into()),
            status: vec!["todo".into()],
            text: Some("rent".into()),
            property: Some("due".into()),
            ..range(DateField::Agenda, "2026-11-01", "2026-11-30")
        };
        let page = Page {
            after: Some("81e7d3e0c9328c61".into()),
            limit: Some(50),
        };
        wire(
            Request::Query(Query {
                select: select("task", filter),
                page,
            }),
            json!({"method": "items/query", "params": {
                "kind": "task",
                "filter": {
                    "note": "Journal/2026-11-05.md", "folder": "Journal", "tag": "home",
                    "status": ["todo"],
                    "date": {"field": "agenda", "from": "2026-11-01", "to": "2026-11-30"},
                    "text": "rent", "property": "due",
                },
                "page": {"after": "81e7d3e0c9328c61", "limit": 50},
            }}),
        );
        wire(
            Request::Query(all(select("book", Filter::default()))),
            json!({"method": "items/query", "params": {"kind": "book"}}),
        );
        wire(
            Request::Subscribe(select("task", range(DateField::Due, "", "2026-11-07"))),
            json!({"method": "items/subscribe", "params": {
                "kind": "task", "filter": {"date": {"field": "due", "to": "2026-11-07"}},
            }}),
        );
        wire(
            Request::Unsubscribe {
                subscription: SubId(3),
            },
            json!({"method": "items/unsubscribe", "params": {"subscription": 3}}),
        );
        let item = item();
        wire(
            Request::Edit(set(&item, "due", json!("2026-12-01"))),
            json!({"method": "items/edit", "params": {
                "id": "81e7d3e0c9328c61", "rev": "c1e0d4d17c74",
                "op": "set-field", "field": "due", "value": "2026-12-01",
            }}),
        );
        wire(
            Request::Edit(tick(&item)),
            json!({"method": "items/edit", "params": {
                "id": "81e7d3e0c9328c61", "rev": "c1e0d4d17c74", "op": "toggle",
            }}),
        );
        wire(Request::TypesList, json!({"method": "types/list"}));
    }

    #[test]
    fn unknown_ops_and_filters_are_refused_when_read() {
        let op = json!({"id": "a", "rev": "b", "op": "set-status", "status": "done"});
        let e = serde_json::from_value::<Edit>(op).unwrap_err().to_string();
        assert!(e.contains("set-status") && e.contains("set-field"), "{e}");
        let filter = json!({"kind": "task", "filter": {"folders": "Work"}});
        let e = serde_json::from_value::<Select>(filter)
            .unwrap_err()
            .to_string();
        assert!(e.contains("folders"), "{e}");
    }

    #[test]
    fn answers_and_news_are_json() {
        let item = item();
        let item_json = serde_json::to_value(&item).unwrap();
        wire(
            Found {
                items: vec![item.clone()],
                next: Some(item.id.clone()),
            },
            json!({"items": [item_json], "next": "81e7d3e0c9328c61"}),
        );
        wire(
            Subscribed {
                subscription: SubId(1),
            },
            json!({"subscription": 1}),
        );
        wire(
            Edited {
                item: item.clone(),
                changed: true,
                next: None,
                warning: None,
            },
            json!({"item": item_json, "changed": true, "next": null, "warning": null}),
        );
        let reasons = [
            (Reason::Changed, "changed"),
            (Reason::Refused, "refused"),
            (Reason::ReadBack, "read-back"),
            (Reason::Io, "io"),
            (Reason::Forbidden, "forbidden"),
            (Reason::NoWorkspace, "no-workspace"),
        ];
        for (reason, name) in reasons {
            wire(
                Error::new(reason, "why"),
                json!({"reason": name, "message": "why"}),
            );
        }
        let read_back = Error::from(EditError::ReadBack("due".into()));
        assert_eq!(read_back.reason, Reason::ReadBack);
        assert_eq!(
            read_back.message,
            "refused, the note would not read back: due"
        );
        wire(
            News::Changed {
                subscription: SubId(1),
                added: vec![item.clone()],
                updated: vec![],
                removed: vec!["907ce22f359894ef".into()],
            },
            json!({"method": "items/changed", "params": {
                "subscription": 1, "added": [item_json], "updated": [],
                "removed": ["907ce22f359894ef"],
            }}),
        );
        wire(
            News::Reset {
                subscription: SubId(1),
            },
            json!({"method": "items/reset", "params": {"subscription": 1}}),
        );
    }
}
