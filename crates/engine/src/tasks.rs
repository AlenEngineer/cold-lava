//! Tasks in the Obsidian Tasks plugin's format, for `cold-lava agenda` and `cold-lava task toggle`.
//!
//! A task is an item of the built-in `task` type (`types/task.toml`), or of a type that extends
//! it: the definition reads the emoji format (`- [ ] Pay rent 📅 2026-11-07 🔁 every month`) and
//! the Dataview format (`- [ ] Pay rent [due:: 2026-11-07]`). This module reads the plugin's
//! settings (global filter, statuses, "set done date", "next recurrence on the line below"), and
//! ticks a task the way the plugin does: done dates, recurrence and status cycles.

use crate::daily;
use crate::items::{self, Data, Item};
use crate::workspace::Workspace;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, Days, Local, Months, NaiveDate, Weekday};
use regex::Regex;
use serde::Serialize;
use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ---------- settings ----------

pub struct TasksSettings {
    pub global_filter: String,
    pub set_done_date: bool,
    pub set_cancelled_date: bool,
    pub recurrence_below: bool,
    pub dataview: bool,
    /// What each status symbol means: the defaults, then the plugin's `statusSettings`.
    pub statuses: BTreeMap<char, StatusType>,
    /// The symbol each status's toggle goes to, where the plugin's `statusSettings` say.
    pub next: BTreeMap<char, char>,
}

impl TasksSettings {
    pub fn load(root: &Path) -> TasksSettings {
        let json: serde_json::Value =
            std::fs::read_to_string(root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
        TasksSettings::from_json(&json)
    }

    pub fn from_json(json: &serde_json::Value) -> TasksSettings {
        let mut statuses: BTreeMap<char, StatusType> = [
            (' ', StatusType::Todo),
            ('x', StatusType::Done),
            ('X', StatusType::Done),
            ('-', StatusType::Cancelled),
            ('/', StatusType::InProgress),
        ]
        .into();
        let mut next = BTreeMap::new();
        let one = |v: Option<&serde_json::Value>| {
            let mut chars = v?.as_str()?.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(c),
                _ => None,
            }
        };
        let configured = ["coreStatuses", "customStatuses"]
            .iter()
            .filter_map(|k| json.pointer(&format!("/statusSettings/{k}"))?.as_array())
            .flatten();
        for s in configured {
            let kind = s.get("type").and_then(|v| v.as_str());
            if let (Some(c), Some(kind)) = (one(s.get("symbol")), kind)
                && let Some(t) = StatusType::from_plugin(kind)
            {
                statuses.insert(c, t);
                if let Some(n) = one(s.get("nextStatusSymbol")) {
                    next.insert(c, n);
                }
            }
        }
        TasksSettings {
            global_filter: json
                .get("globalFilter")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string(),
            set_done_date: json
                .get("setDoneDate")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            set_cancelled_date: json
                .get("setCancelledDate")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            recurrence_below: json
                .get("recurrenceOnNextLine")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            dataview: json.get("taskFormat").and_then(|v| v.as_str()) == Some("dataview"),
            statuses,
            next,
        }
    }

    /// A symbol the settings don't name is not a task status Tasks would act on.
    pub fn status_type(&self, symbol: char) -> StatusType {
        self.statuses
            .get(&symbol)
            .copied()
            .unwrap_or(StatusType::NonTask)
    }

    /// The symbol a toggle sets: the status's next one in the plugin's settings, else done for
    /// an open task and todo for a closed one.
    pub fn next_symbol(&self, symbol: char) -> char {
        match self.next.get(&symbol) {
            Some(&n) => n,
            None if matches!(
                self.status_type(symbol),
                StatusType::Done | StatusType::Cancelled
            ) =>
            {
                ' '
            }
            None => 'x',
        }
    }
}

/// The Tasks plugin's status types.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusType {
    Todo,
    InProgress,
    Done,
    Cancelled,
    NonTask,
}

impl StatusType {
    fn from_plugin(name: &str) -> Option<StatusType> {
        Some(match name {
            "TODO" => StatusType::Todo,
            "IN_PROGRESS" => StatusType::InProgress,
            "DONE" => StatusType::Done,
            "CANCELLED" => StatusType::Cancelled,
            "NON_TASK" => StatusType::NonTask,
            _ => return None,
        })
    }

    /// Its name in a task item's `status_type`.
    pub fn name(self) -> &'static str {
        match self {
            StatusType::Todo => "todo",
            StatusType::InProgress => "in_progress",
            StatusType::Done => "done",
            StatusType::Cancelled => "cancelled",
            StatusType::NonTask => "non_task",
        }
    }

    /// The name `agenda --json` and `task toggle --json` have always used.
    pub fn agenda_name(self) -> &'static str {
        match self {
            StatusType::NonTask => "other",
            t => t.name(),
        }
    }
}

/// A task as the agenda and toggle use it: the fields of an item that answers to `task`. A
/// subtype that doesn't read a field leaves it empty.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// The character between the brackets.
    pub status: char,
    pub status_type: StatusType,
    /// Description without dates, recurrence, priority and the global filter.
    pub text: String,
    pub due: Option<NaiveDate>,
    pub scheduled: Option<NaiveDate>,
    pub start: Option<NaiveDate>,
    pub created: Option<NaiveDate>,
    pub done: Option<NaiveDate>,
    pub cancelled: Option<NaiveDate>,
    pub recurrence: Option<String>,
    pub priority: Option<String>,
    /// With the `#`, as the agenda has always listed them.
    pub tags: Vec<String>,
    /// The date of the daily note the task is in.
    pub daily_note: Option<NaiveDate>,
}

impl Task {
    /// The task `item` is, if it answers to `task`; `line` is its first line as written, where
    /// the status is read when its type doesn't read one.
    pub fn of(item: &Item, line: &str, settings: &TasksSettings) -> Option<Task> {
        let Data::Typed(t) = &item.data else {
            return None;
        };
        if !item.data.answers_to("task") {
            return None;
        }
        let field = |name: &str| t.fields.get(name).and_then(Json::as_str);
        let date =
            |name: &str| field(name).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        let status = match field("status") {
            Some(s) => s.chars().next()?,
            None => re().task.captures(split_quote(line).1)?[2].chars().next()?,
        };
        let tags = match t.fields.get("tags") {
            Some(Json::Array(tags)) => tags
                .iter()
                .filter_map(Json::as_str)
                .map(|t| format!("#{t}"))
                .collect(),
            _ => Vec::new(),
        };
        Some(Task {
            status,
            status_type: settings.status_type(status),
            text: field("text").unwrap_or_default().to_string(),
            due: date("due"),
            scheduled: date("scheduled"),
            start: date("start"),
            created: date("created"),
            done: date("done"),
            cancelled: date("cancelled"),
            recurrence: field("recurrence").map(String::from),
            priority: field("priority").map(String::from),
            tags,
            daily_note: date("daily_note"),
        })
    }

    /// The date the agenda files it under, and which one that is.
    pub fn agenda_date(&self) -> Option<(NaiveDate, &'static str)> {
        [
            (self.due, "due"),
            (self.scheduled, "scheduled"),
            (self.start, "start"),
            (self.daily_note, "daily_note"),
        ]
        .into_iter()
        .find_map(|(d, source)| Some((d?, source)))
    }
}

struct Re {
    /// "- [ ] text", "* [x] text", "1. [ ] text"
    task: Regex,
}

fn re() -> &'static Re {
    static RE: OnceLock<Re> = OnceLock::new();
    RE.get_or_init(|| Re {
        task: Regex::new(r"^(\s*(?:[-*+]|\d+[.)])\s+\[)(.)(\]\s+)(.*)$").unwrap(),
    })
}

/// FNV-1a of some bytes: stable across runs and machines, unlike `std`'s hasher.
pub fn fnv(bytes: &[u8]) -> u64 {
    fnv_from(FNV_START, bytes)
}

pub const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

/// `fnv` of what was hashed into `h`, then `bytes`.
pub fn fnv_from(h: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(h, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// Fingerprint of a line, as 12 hex digits.
pub fn line_id(line: &str) -> String {
    let h = fnv(line.trim_end_matches(['\r', '\n']).as_bytes());
    format!("{:012x}", h & 0xffff_ffff_ffff)
}

/// Splits a line into its blockquote / callout markers (`> > `) and the rest.
fn split_quote(line: &str) -> (&str, &str) {
    let mut at = 0;
    for (i, c) in line.char_indices() {
        match c {
            '>' => at = i + 1,
            ' ' | '\t' if at > 0 => at = i + 1,
            ' ' | '\t' => {}
            _ => break,
        }
    }
    line.split_at(at)
}

// ---------- agenda ----------

/// A task as `agenda --json` lists it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AgendaTask {
    pub text: String,
    /// "todo", "done", "cancelled", "in_progress", or "other".
    pub status: &'static str,
    pub status_char: String,
    pub done: bool,
    /// The date the agenda files it under (due, else scheduled, else start, else the daily note's).
    pub date: Option<String>,
    pub date_source: Option<&'static str>,
    pub due: Option<String>,
    pub scheduled: Option<String>,
    pub start: Option<String>,
    pub created: Option<String>,
    pub done_date: Option<String>,
    pub cancelled_date: Option<String>,
    pub recurrence: Option<String>,
    pub priority: Option<String>,
    pub tags: Vec<String>,
    /// Workspace-relative file, absolute path and 1-based line.
    pub file: String,
    pub path: String,
    pub line: usize,
    /// The task's `rev`, to pass to `task toggle --id` (guards against edits in between).
    pub id: String,
    pub raw: String,
}

impl AgendaTask {
    fn new(item: &Item, t: Task, root: &Path, raw: &str) -> AgendaTask {
        let s = |d: Option<NaiveDate>| d.map(|d| d.to_string());
        let date = t.agenda_date();
        AgendaTask {
            status: t.status_type.agenda_name(),
            status_char: t.status.to_string(),
            done: t.status_type == StatusType::Done,
            date: s(date.map(|d| d.0)),
            date_source: date.map(|d| d.1),
            due: s(t.due),
            scheduled: s(t.scheduled),
            start: s(t.start),
            created: s(t.created),
            done_date: s(t.done),
            cancelled_date: s(t.cancelled),
            recurrence: t.recurrence,
            priority: t.priority,
            tags: t.tags,
            text: t.text,
            file: item.note.clone(),
            path: root.join(&item.note).to_string_lossy().into_owned(),
            line: item.line,
            id: item.rev.clone(),
            raw: raw.to_string(),
        }
    }
}

#[derive(Serialize)]
pub struct DailyNote {
    pub date: String,
    pub file: String,
    pub path: String,
}

#[derive(Serialize, Default)]
pub struct Day {
    pub tasks: usize,
    pub open: usize,
    pub daily_note: bool,
}

#[derive(Serialize)]
pub struct Agenda {
    pub workspace: String,
    pub from: String,
    pub to: String,
    pub tasks: Vec<AgendaTask>,
    pub daily_notes: Vec<DailyNote>,
    /// Per-day summary for calendar markers (only days with something on them).
    pub days: BTreeMap<String, Day>,
}

/// The workspace's dated tasks and daily notes from `from` to `to`, read from its items.
pub fn agenda(workspace: &Workspace, from: NaiveDate, to: NaiveDate) -> Agenda {
    let in_range = |d: NaiveDate| d >= from && d <= to;
    let settings = workspace.item_settings();
    let mut tasks = Vec::new();
    let mut notes = Vec::new();
    for note in &workspace.notes {
        let file = format!("{}.md", note.rel);
        if let Some(d) = settings.daily.date_of(&file)
            && in_range(d)
        {
            notes.push(DailyNote {
                date: d.to_string(),
                file,
                path: note.path.to_string_lossy().into_owned(),
            });
        }
        let lines: Vec<&str> = note.text.lines().collect();
        for item in workspace.items_once(note).iter() {
            let raw = lines.get(item.line - 1).copied().unwrap_or_default();
            if let Some(t) = Task::of(item, raw, &settings.tasks)
                && t.agenda_date().is_some_and(|(d, _)| in_range(d))
            {
                tasks.push(AgendaTask::new(item, t, &workspace.root, raw));
            }
        }
    }
    tasks.sort_by(|a, b| (&a.date, &a.file, a.line).cmp(&(&b.date, &b.file, b.line)));
    notes.sort_by(|a, b| a.date.cmp(&b.date));
    let mut days: BTreeMap<String, Day> = BTreeMap::new();
    for t in &tasks {
        let d = days.entry(t.date.clone().unwrap_or_default()).or_default();
        d.tasks += 1;
        if t.status == "todo" || t.status == "in_progress" {
            d.open += 1;
        }
    }
    for n in &notes {
        days.entry(n.date.clone()).or_default().daily_note = true;
    }
    Agenda {
        workspace: workspace.root.to_string_lossy().into_owned(),
        from: from.to_string(),
        to: to.to_string(),
        tasks,
        daily_notes: notes,
        days,
    }
}

// ---------- toggling ----------

#[derive(Serialize, Debug)]
pub struct Toggled {
    pub path: String,
    pub line: usize,
    pub status: &'static str,
    pub raw: String,
    pub id: String,
    /// For recurring tasks: the next occurrence that was inserted.
    pub next: Option<NextTask>,
    pub warning: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct NextTask {
    pub line: usize,
    pub raw: String,
    pub id: String,
}

#[derive(Debug)]
pub struct Changed(pub String);
impl std::fmt::Display for Changed {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Changed {}

/// Toggle the task at `path:line` (1-based) like Obsidian Tasks does: to the status's next
/// symbol, with a done date and the next occurrence of a recurring task when that is done, and
/// without its done and cancelled dates when it isn't.
pub fn toggle(
    path: &Path,
    line: usize,
    expect_id: Option<&str>,
    today: NaiveDate,
) -> Result<Toggled> {
    let path = path
        .canonicalize()
        .with_context(|| format!("no such file: {}", path.display()))?;
    let root = crate::workspace::find_workspace_root(&path);
    let item_settings = items::Settings::load(&root);
    let settings = &item_settings.tasks;
    let rel = path
        .strip_prefix(&root)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");
    let text = std::fs::read_to_string(&path)?;
    let crlf = text.contains("\r\n");
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let ends_with_newline = text.ends_with('\n');
    if line == 0 || line > lines.len() {
        return Err(Changed(format!(
            "{} has no line {line} (it has {})",
            path.display(),
            lines.len()
        ))
        .into());
    }
    let old = lines[line - 1].clone();
    let note_items = items::extract(&rel, &text, &item_settings);
    let item = task_at(&note_items, line, &old, settings);
    let rev = item
        .as_ref()
        .map_or_else(|| line_id(&old), |(i, _)| i.rev.clone());
    if expect_id.is_some_and(|id| id != rev) {
        return Err(Changed(format!(
            "line {line} changed since the agenda was read; refresh and try again"
        ))
        .into());
    }
    let Some((_, task)) = item else {
        bail!("line {line} of {} is not a task", path.display());
    };
    let dataview = settings.dataview || old.contains("::");
    let (quote, rest) = split_quote(&old);
    let caps = re()
        .task
        .captures(rest)
        .with_context(|| format!("line {line} of {} is not a task", path.display()))?;
    let (prefix, mid, body) = (
        format!("{quote}{}", &caps[1]),
        caps[3].to_string(),
        caps[4].to_string(),
    );
    let mut warning = None;
    let mut next = None;

    let new_char = settings.next_symbol(task.status);
    let without = |b: &str, emoji, dv| strip_field(b, emoji, dv);
    let new_body = match settings.status_type(new_char) {
        StatusType::Done => {
            let mut b = without(&body, "❌", "cancelled");
            if settings.set_done_date {
                b = add_field(&b, "✅", "completion", today, dataview);
            }
            if let Some(rule) = &task.recurrence {
                match next_occurrence(&task, rule, today) {
                    Some(dates) => {
                        let mut nb =
                            without(&without(&body, "✅", "completion"), "❌", "cancelled");
                        for (emoji, field, new) in dates {
                            nb = replace_date(&nb, emoji, field, new);
                        }
                        next = Some(format!("{prefix} {mid}{nb}"));
                    }
                    None => {
                        warning = Some(format!(
                            "couldn't work out the next date for \"{rule}\"; no new occurrence was added"
                        ))
                    }
                }
            }
            b
        }
        StatusType::Cancelled => {
            let b = without(&body, "✅", "completion");
            match settings.set_cancelled_date {
                true => add_field(&b, "❌", "cancelled", today, dataview),
                false => b,
            }
        }
        // Open again: drop the done / cancelled date.
        _ => without(&without(&body, "✅", "completion"), "❌", "cancelled"),
    };
    let new_line = format!("{prefix}{new_char}{mid}{new_body}");
    lines[line - 1] = new_line.clone();
    let mut toggled_line = line;
    let mut next_at = None;
    if let Some(n) = next {
        let at = if settings.recurrence_below {
            line
        } else {
            line - 1
        };
        lines.insert(at, n);
        if !settings.recurrence_below {
            toggled_line += 1;
        }
        next_at = Some(at + 1);
    }
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(nl);
    if ends_with_newline {
        out.push_str(nl);
    }
    match crate::write::replace(&path, text.as_bytes(), out.as_bytes()) {
        Err(crate::write::WriteError::Changed(_)) => {
            return Err(Changed(format!(
                "{} changed while the task was being toggled; refresh and try again",
                path.display()
            ))
            .into());
        }
        r => r?,
    }
    let new_items = items::extract(&rel, &out, &item_settings);
    let rev_at = |at: usize| {
        task_at(&new_items, at, &lines[at - 1], settings)
            .map_or_else(|| line_id(&lines[at - 1]), |(i, _)| i.rev.clone())
    };
    Ok(Toggled {
        path: path.to_string_lossy().into_owned(),
        line: toggled_line,
        status: settings.status_type(new_char).agenda_name(),
        id: rev_at(toggled_line),
        raw: new_line,
        next: next_at.map(|at| NextTask {
            line: at,
            raw: lines[at - 1].clone(),
            id: rev_at(at),
        }),
        warning,
    })
}

/// The task item that starts on `line`, whose text is `raw`.
fn task_at<'a>(
    items: &'a [Item],
    line: usize,
    raw: &str,
    settings: &TasksSettings,
) -> Option<(&'a Item, Task)> {
    items
        .iter()
        .filter(|i| i.line == line)
        .find_map(|i| Some((i, Task::of(i, raw, settings)?)))
}

fn strip_field(body: &str, emoji: &str, dv: &str) -> String {
    let e = Regex::new(&format!(r"\s*{emoji}\u{{FE0F}}?\s*\d{{4}}-\d{{2}}-\d{{2}}")).unwrap();
    let d = Regex::new(&format!(r"\s*[\[(]{dv}::[^\])]*[\])]")).unwrap();
    d.replace_all(&e.replace_all(body, ""), "").into_owned()
}

fn add_field(body: &str, emoji: &str, dv: &str, date: NaiveDate, dataview: bool) -> String {
    let b = strip_field(body, emoji, dv);
    let b = b.trim_end();
    if dataview {
        format!("{b} [{dv}:: {date}]")
    } else {
        format!("{b} {emoji} {date}")
    }
}

fn replace_date(body: &str, emoji: &str, dv: &str, date: NaiveDate) -> String {
    let e = Regex::new(&format!(r"({emoji}\u{{FE0F}}?\s*)\d{{4}}-\d{{2}}-\d{{2}}")).unwrap();
    let d = Regex::new(&format!(r"([\[(]{dv}::\s*)\d{{4}}-\d{{2}}-\d{{2}}")).unwrap();
    let b = e.replace_all(body, format!("${{1}}{date}"));
    d.replace_all(&b, format!("${{1}}{date}")).into_owned()
}

/// New dates for the next occurrence of a recurring task: (emoji, dataview field, date).
/// All of the task's dates move by the same offset, like Obsidian Tasks.
fn next_occurrence(
    t: &Task,
    rule: &str,
    today: NaiveDate,
) -> Option<Vec<(&'static str, &'static str, NaiveDate)>> {
    let fields = [
        ("📅", "due", t.due),
        ("⏳", "scheduled", t.scheduled),
        ("🛫", "start", t.start),
    ];
    let reference = fields.iter().find_map(|f| f.2);
    let rule_l = rule.to_lowercase();
    let when_done = rule_l.contains("when done");
    let base = if when_done { today } else { reference? };
    let next = advance(base, rule_l.replace("when done", "").trim())?;
    let delta = next.signed_duration_since(reference.unwrap_or(base));
    Some(
        fields
            .into_iter()
            .filter_map(|(e, f, d)| Some((e, f, d? + delta)))
            .collect(),
    )
}

/// The next date after `base` for a Tasks recurrence rule (a common subset of them).
pub fn advance(base: NaiveDate, rule: &str) -> Option<NaiveDate> {
    let words: Vec<&str> = rule
        .trim()
        .trim_start_matches("every")
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();
    let (n, unit_at) = match words.first()?.parse::<u32>() {
        Ok(n) => (n, 1),
        Err(_) => (1, 0),
    };
    let unit = *words.get(unit_at)?;
    match unit.trim_end_matches('s') {
        "day" => base.checked_add_days(Days::new(n as u64)),
        "week" => {
            // "every week on Monday, Friday"
            let days: Vec<Weekday> = words
                .iter()
                .skip(unit_at + 1)
                .filter_map(|w| daily::weekday_from_name(w))
                .collect();
            if days.is_empty() {
                base.checked_add_days(Days::new(7 * n as u64))
            } else {
                next_weekday(base, &days)
            }
        }
        "month" => base.checked_add_months(Months::new(n)),
        "year" => base.checked_add_months(Months::new(12 * n)),
        "weekday" => next_weekday(
            base,
            &[
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
        ),
        _ => {
            // "every Monday", "every Tuesday and Thursday"
            let days: Vec<Weekday> = words
                .iter()
                .filter_map(|w| daily::weekday_from_name(w))
                .collect();
            if days.is_empty() {
                None
            } else {
                next_weekday(base, &days)
            }
        }
    }
}

fn next_weekday(base: NaiveDate, days: &[Weekday]) -> Option<NaiveDate> {
    (1..=7)
        .filter_map(|k| base.checked_add_days(Days::new(k)))
        .find(|d| days.contains(&d.weekday()))
}

/// Today on the local clock, or the date `COLD_LAVA_TODAY` pins (YYYY-MM-DD), so that scripted
/// runs and their tests read the same day whenever they run.
pub fn today() -> NaiveDate {
    std::env::var("COLD_LAVA_TODAY")
        .ok()
        .and_then(|d| NaiveDate::parse_from_str(d.trim(), "%Y-%m-%d").ok())
        .unwrap_or_else(|| Local::now().date_naive())
}

pub fn first_of_month(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

pub fn last_of_month(d: NaiveDate) -> NaiveDate {
    first_of_month(d)
        .checked_add_months(Months::new(1))
        .and_then(|n| n.pred_opt())
        .unwrap_or(d)
}

pub fn locate(spec: &str) -> Result<(PathBuf, usize)> {
    let (file, line) = spec.rsplit_once(':').context("expected FILE:LINE")?;
    let line: usize = line
        .parse()
        .context("the line after ':' must be a number")?;
    Ok((PathBuf::from(file), line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::Flavour;
    use crate::types::{self, Layer};
    use crate::{daily, templates};

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn settings(json: serde_json::Value, more: &str) -> items::Settings {
        let mut layers = vec![Layer::built_in()];
        if !more.is_empty() {
            layers.push(Layer::config(
                Path::new("config.toml"),
                toml::from_str(more).unwrap(),
            ));
        }
        let types = types::load(layers);
        assert_eq!(types.errors, Vec::<String>::new());
        items::Settings::new(
            Flavour::built_in("obsidian").unwrap(),
            TasksSettings::from_json(&json),
            daily::Settings::default(),
            templates::Settings::default(),
            types,
        )
    }

    fn parse_with(s: &items::Settings, line: &str) -> Option<Task> {
        let all = items::extract("Note.md", line, s);
        all.iter().find_map(|i| Task::of(i, line, &s.tasks))
    }

    fn parse(line: &str) -> Option<Task> {
        parse_with(&settings(serde_json::json!({}), ""), line)
    }

    #[test]
    fn reads_tasks_in_the_emoji_format() {
        let t = parse(
            "- [ ] Pay rent #bills [[Landlord]] 📅 2026-11-07 ⏫ 🔁 every month ➕ 2026-10-01",
        )
        .unwrap();
        assert_eq!(t.text, "Pay rent #bills [[Landlord]]");
        assert_eq!((t.status, t.status_type), (' ', StatusType::Todo));
        assert_eq!(t.due, Some(d("2026-11-07")));
        assert_eq!(t.created, Some(d("2026-10-01")));
        assert_eq!(t.agenda_date(), Some((d("2026-11-07"), "due")));
        assert_eq!(t.recurrence.as_deref(), Some("every month"));
        assert_eq!(t.priority.as_deref(), Some("high"));
        assert_eq!(t.tags, ["#bills"]);
        let s = parse("  * [x] Call back ⏳ 2026-11-03 ✅ 2026-11-04").unwrap();
        assert_eq!(s.status_type, StatusType::Done);
        assert_eq!(s.agenda_date(), Some((d("2026-11-03"), "scheduled")));
        assert_eq!(s.done, Some(d("2026-11-04")));
        assert!(parse("- not a task").is_none());
        assert!(parse("- [ ] ").is_none(), "empty checkbox");
        assert!(parse("- [ ] 📅 2026-11-07").is_none(), "only a date");
        let tagged = settings(serde_json::json!({"globalFilter": "#task"}), "");
        assert!(parse_with(&tagged, "- [ ] untagged").is_none());
        let t = parse_with(&tagged, "- [/] Halfway #task").unwrap();
        assert_eq!(
            (t.text.as_str(), t.status_type),
            ("Halfway", StatusType::InProgress)
        );
    }

    #[test]
    fn custom_statuses_come_from_the_plugin_settings() {
        let type_of = |s: &items::Settings, line: &str| parse_with(s, line).unwrap().status_type;
        let default = settings(serde_json::json!({}), "");
        assert_eq!(type_of(&default, "- [-] Dropped"), StatusType::Cancelled);
        assert_eq!(type_of(&default, "- [X] Shouted"), StatusType::Done);
        assert_eq!(type_of(&default, "- [?] Maybe"), StatusType::NonTask);
        let custom = settings(
            serde_json::json!({"statusSettings": {
                "coreStatuses": [{"symbol": "x", "name": "Done", "type": "DONE"}],
                "customStatuses": [
                    {"symbol": "?", "name": "Question", "type": "TODO"},
                    {"symbol": "-", "name": "Skipped", "type": "NON_TASK"},
                    {"symbol": "ab", "name": "Two symbols", "type": "DONE"},
                    {"symbol": "!", "name": "Unknown type", "type": "SOMEDAY"}
                ]
            }}),
            "",
        );
        assert_eq!(type_of(&custom, "- [?] Maybe"), StatusType::Todo);
        assert_eq!(type_of(&custom, "- [-] Skipped"), StatusType::NonTask);
        assert_eq!(type_of(&custom, "- [/] Halfway"), StatusType::InProgress);
        assert_eq!(type_of(&custom, "- [!] Odd"), StatusType::NonTask);
    }

    #[test]
    fn reads_the_dataview_format() {
        let t = parse("- [ ] Dentist [due:: 2026-11-12] [repeat:: every year] [priority:: high]")
            .unwrap();
        assert_eq!(t.text, "Dentist");
        assert_eq!(t.due, Some(d("2026-11-12")));
        assert_eq!(t.recurrence.as_deref(), Some("every year"));
        assert_eq!(t.priority.as_deref(), Some("high"));
    }

    #[test]
    fn recurrence_rules() {
        let b = d("2026-11-07"); // a Saturday
        assert_eq!(advance(b, "every day"), Some(d("2026-11-08")));
        assert_eq!(advance(b, "every 3 days"), Some(d("2026-11-10")));
        assert_eq!(advance(b, "every week"), Some(d("2026-11-14")));
        assert_eq!(advance(b, "every 2 weeks"), Some(d("2026-11-21")));
        assert_eq!(advance(b, "every month"), Some(d("2026-12-07")));
        assert_eq!(
            advance(d("2026-01-31"), "every month"),
            Some(d("2026-02-28"))
        );
        assert_eq!(advance(b, "every year"), Some(d("2027-11-07")));
        assert_eq!(advance(b, "every weekday"), Some(d("2026-11-09")));
        assert_eq!(advance(b, "every Tuesday"), Some(d("2026-11-10")));
        assert_eq!(
            advance(b, "every week on Monday, Friday"),
            Some(d("2026-11-09"))
        );
        assert_eq!(advance(b, "every blue moon"), None);
    }

    fn temp_workspace(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        dir
    }

    #[test]
    fn toggle_done_undone_and_guard() {
        let v = temp_workspace("toggle");
        let f = v.join("Todo.md");
        std::fs::write(&f, "# Todo\n- [ ] Pay rent 📅 2026-11-07\n").unwrap();
        let id = line_id("- [ ] Pay rent 📅 2026-11-07");
        let today = d("2026-11-05");
        let r = toggle(&f, 2, Some(&id), today).unwrap();
        assert_eq!(r.status, "done");
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "# Todo\n- [x] Pay rent 📅 2026-11-07 ✅ 2026-11-05\n"
        );
        // The old id no longer matches: refuse.
        let err = toggle(&f, 2, Some(&id), today).unwrap_err();
        assert!(err.downcast_ref::<Changed>().is_some());
        // Toggle back with the new id.
        toggle(&f, 2, Some(&r.id), today).unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "# Todo\n- [ ] Pay rent 📅 2026-11-07\n"
        );
        assert!(toggle(&f, 1, None, today).is_err(), "headings aren't tasks");
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn toggle_in_a_callout_keeps_its_markers() {
        let v = temp_workspace("callout");
        let f = v.join("Plan.md");
        std::fs::write(&f, "> [!todo] This week\n> - [ ] Pack boots\n").unwrap();
        let r = toggle(&f, 2, Some(&line_id("> - [ ] Pack boots")), d("2026-11-05")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "> [!todo] This week\n> - [x] Pack boots ✅ 2026-11-05\n"
        );
        assert_eq!(r.id, line_id("> - [x] Pack boots ✅ 2026-11-05"));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn toggle_recurring_inserts_next_occurrence() {
        let v = temp_workspace("recur");
        let f = v.join("Bills.md");
        std::fs::write(
            &f,
            "- [ ] Rent 🔁 every month 📅 2026-11-01 ⏳ 2026-10-28\n",
        )
        .unwrap();
        let r = toggle(&f, 1, None, d("2026-11-02")).unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(
            text,
            "- [ ] Rent 🔁 every month 📅 2026-12-01 ⏳ 2026-11-27\n- [x] Rent 🔁 every month 📅 2026-11-01 ⏳ 2026-10-28 ✅ 2026-11-02\n"
        );
        assert_eq!((r.line, r.next.as_ref().unwrap().line), (2, 1));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn toggle_follows_the_statuses_next_symbols() {
        let v = temp_workspace("cycle");
        std::fs::create_dir_all(v.join(".obsidian/plugins/obsidian-tasks-plugin")).unwrap();
        std::fs::write(
            v.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"),
            r#"{"statusSettings": {"coreStatuses": [
                {"symbol": " ", "type": "TODO", "nextStatusSymbol": "/"},
                {"symbol": "/", "type": "IN_PROGRESS", "nextStatusSymbol": "x"},
                {"symbol": "x", "type": "DONE", "nextStatusSymbol": " "}
            ]}}"#,
        )
        .unwrap();
        let f = v.join("Cycle.md");
        std::fs::write(&f, "- [ ] Paint 📅 2026-11-07\n").unwrap();
        let today = d("2026-11-05");
        let steps: Vec<(&str, String)> = (0..3)
            .map(|_| {
                let r = toggle(&f, 1, None, today).unwrap();
                (r.status, std::fs::read_to_string(&f).unwrap())
            })
            .collect();
        assert_eq!(
            steps,
            [
                ("in_progress", "- [/] Paint 📅 2026-11-07\n".to_string()),
                (
                    "done",
                    "- [x] Paint 📅 2026-11-07 ✅ 2026-11-05\n".to_string()
                ),
                ("todo", "- [ ] Paint 📅 2026-11-07\n".to_string()),
            ]
        );
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn a_status_cancelled_by_its_next_symbol_gets_the_cancelled_date() {
        let v = temp_workspace("cancel");
        std::fs::create_dir_all(v.join(".obsidian/plugins/obsidian-tasks-plugin")).unwrap();
        let data = v.join(".obsidian/plugins/obsidian-tasks-plugin/data.json");
        let statuses = r#"{"statusSettings": {"coreStatuses": [
                {"symbol": "x", "type": "DONE", "nextStatusSymbol": "-"},
                {"symbol": "-", "type": "CANCELLED", "nextStatusSymbol": " "}
            ]}"#;
        std::fs::write(&data, format!("{statuses}}}")).unwrap();
        let f = v.join("Plan.md");
        std::fs::write(&f, "- [x] Hike 📅 2026-11-07 ✅ 2026-11-04\n").unwrap();
        let today = d("2026-11-05");
        let r = toggle(&f, 1, None, today).unwrap();
        assert_eq!(r.status, "cancelled");
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "- [-] Hike 📅 2026-11-07 ❌ 2026-11-05\n",
            "the done date goes, the cancelled date comes"
        );
        toggle(&f, 1, None, today).unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "- [ ] Hike 📅 2026-11-07\n"
        );
        std::fs::write(&data, format!("{statuses}, \"setCancelledDate\": false}}")).unwrap();
        std::fs::write(&f, "- [x] Hike [completion:: 2026-11-04]\n").unwrap();
        toggle(&f, 1, None, today).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "- [-] Hike\n");
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn agenda_finds_tasks_and_daily_notes() {
        let v = temp_workspace("agenda");
        std::fs::write(
            v.join(".obsidian/daily-notes.json"),
            r#"{"folder":"Daily","format":"YYYY-MM-DD"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(v.join("Daily")).unwrap();
        std::fs::write(
            v.join("Daily/2026-11-05.md"),
            "- [ ] Water plants\n- [x] Gym\n",
        )
        .unwrap();
        std::fs::write(v.join("Projects.md"), "- [ ] Ship it 📅 2026-11-20\n- [ ] Someday\n```\n- [ ] in code 📅 2026-11-20\n```\n- [ ] Next month 📅 2026-12-02\n").unwrap();
        let a = agenda(
            &crate::workspace::scan(&v).unwrap(),
            d("2026-11-01"),
            d("2026-11-30"),
        );
        let summary: Vec<(String, &str, Option<&str>)> = a
            .tasks
            .iter()
            .map(|t| (t.text.clone(), t.date.as_deref().unwrap(), t.date_source))
            .collect();
        assert_eq!(
            summary,
            [
                ("Water plants".to_string(), "2026-11-05", Some("daily_note")),
                ("Gym".to_string(), "2026-11-05", Some("daily_note")),
                ("Ship it".to_string(), "2026-11-20", Some("due")),
            ]
        );
        assert_eq!(a.daily_notes.len(), 1);
        assert_eq!(a.days["2026-11-05"].open, 1);
        assert!(a.days["2026-11-05"].daily_note);
        assert_eq!(a.tasks[2].line, 1);
        let _ = std::fs::remove_dir_all(&v);
    }

    const IDEA: &str = r#"
        [idea]
        extends = "task"
        on = "list-item"
        where.tag = ["idea"]
        match = '^\[[^\n]\]\s'
        fields.status = { checkbox = true }
        fields.text = { rest = true }
        fields.due = { marker = "📅", type = "date" }
    "#;

    #[test]
    fn a_subtype_of_task_is_in_the_agenda_and_toggles() {
        let s = settings(serde_json::json!({}), IDEA);
        let line = "- [ ] Glass from sand #idea 📅 2026-11-12";
        let all = items::extract("Ideas.md", line, &s);
        assert_eq!(all[0].data.kind(), "idea");
        let t = Task::of(&all[0], line, &s.tasks).expect("an idea is a task");
        assert_eq!((t.status, t.due), (' ', Some(d("2026-11-12"))));

        let v = temp_workspace("idea");
        std::fs::create_dir_all(v.join(".cold-lava")).unwrap();
        std::fs::write(
            v.join(".cold-lava/types.toml"),
            format!(
                "version = 1\n[types.{}",
                IDEA.trim().trim_start_matches('[')
            ),
        )
        .unwrap();
        let f = v.join("Ideas.md");
        std::fs::write(&f, format!("{line}\n- [ ] Pay rent 📅 2026-11-12\n")).unwrap();
        let workspace = crate::workspace::scan(&v).unwrap();
        assert_eq!(workspace.item_settings().types.errors, Vec::<String>::new());
        let a = agenda(&workspace, d("2026-11-01"), d("2026-11-30"));
        let texts: Vec<&str> = a.tasks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[0].starts_with("Glass from sand"), "{texts:?}");
        toggle(&f, 1, Some(&a.tasks[0].id), d("2026-11-05")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            "- [x] Glass from sand #idea 📅 2026-11-12 ✅ 2026-11-05\n- [ ] Pay rent 📅 2026-11-12\n"
        );
        let _ = std::fs::remove_dir_all(&v);
    }
}
