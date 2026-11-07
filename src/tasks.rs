//! Tasks in the Obsidian Tasks plugin's format, for `cold-lava agenda` and `cold-lava task toggle`.
//!
//! Understands the emoji format (`- [ ] Pay rent 📅 2026-11-07 🔁 every month`) and the Dataview
//! format (`- [ ] Pay rent [due:: 2026-11-07]`), the plugin's global filter, its "set done date"
//! and "next recurrence on the line below" settings, and daily notes as an implicit date.

use crate::daily;
use crate::items::{self, Data, Item};
use crate::vault::Vault;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, Days, Local, Months, NaiveDate, Weekday};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ---------- settings ----------

pub struct TasksSettings {
    pub global_filter: String,
    pub set_done_date: bool,
    pub recurrence_below: bool,
    pub dataview: bool,
    /// What each status symbol means: the defaults, then the plugin's `statusSettings`.
    pub statuses: BTreeMap<char, StatusType>,
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

    fn from_json(json: &serde_json::Value) -> TasksSettings {
        let mut statuses: BTreeMap<char, StatusType> = [
            (' ', StatusType::Todo),
            ('x', StatusType::Done),
            ('X', StatusType::Done),
            ('-', StatusType::Cancelled),
            ('/', StatusType::InProgress),
        ]
        .into();
        let configured = ["coreStatuses", "customStatuses"]
            .iter()
            .filter_map(|k| json.pointer(&format!("/statusSettings/{k}"))?.as_array())
            .flatten();
        for s in configured {
            let symbol = s.get("symbol").and_then(|v| v.as_str()).map(str::chars);
            let kind = s.get("type").and_then(|v| v.as_str());
            if let (Some(mut symbol), Some(kind)) = (symbol, kind)
                && let (Some(c), None) = (symbol.next(), symbol.next())
                && let Some(t) = StatusType::from_plugin(kind)
            {
                statuses.insert(c, t);
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
            recurrence_below: json
                .get("recurrenceOnNextLine")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            dataview: json.get("taskFormat").and_then(|v| v.as_str()) == Some("dataview"),
            statuses,
        }
    }

    /// A symbol the settings don't name is not a task status Tasks would act on.
    pub fn status_type(&self, symbol: char) -> StatusType {
        self.statuses
            .get(&symbol)
            .copied()
            .unwrap_or(StatusType::NonTask)
    }
}

// ---------- parsing ----------

/// The Tasks plugin's status types.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
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

    /// The name `agenda --json` and `task toggle --json` have always used.
    fn agenda_name(self) -> &'static str {
        match self {
            StatusType::Todo => "todo",
            StatusType::InProgress => "in_progress",
            StatusType::Done => "done",
            StatusType::Cancelled => "cancelled",
            StatusType::NonTask => "other",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Highest,
    High,
    Medium,
    Low,
    Lowest,
}

impl Priority {
    const EMOJI: [(&str, Priority); 5] = [
        ("🔺", Priority::Highest),
        ("⏫", Priority::High),
        ("🔼", Priority::Medium),
        ("🔽", Priority::Low),
        ("⏬", Priority::Lowest),
    ];

    fn from_name(name: &str) -> Option<Priority> {
        serde_json::from_value(serde_json::Value::from(name)).ok()
    }

    fn name(self) -> &'static str {
        match self {
            Priority::Highest => "highest",
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
            Priority::Lowest => "lowest",
        }
    }
}

/// One task line, read the way the Tasks plugin reads it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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
    pub priority: Option<Priority>,
    /// As written, with the `#`.
    pub tags: Vec<String>,
    /// Link targets, as `Note::links` has them.
    pub links: Vec<String>,
    /// The date of the daily note the task is in.
    pub daily_note: Option<NaiveDate>,
}

impl Task {
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
    task: Regex,
    date_field: Regex,
    recur: Regex,
    dv_field: Regex,
    tag: Regex,
    recur_text: Regex,
}

fn re() -> &'static Re {
    static RE: OnceLock<Re> = OnceLock::new();
    RE.get_or_init(|| Re {
        // "- [ ] text", "* [x] text", "1. [ ] text"
        task: Regex::new(r"^(\s*(?:[-*+]|\d+[.)])\s+\[)(.)(\]\s+)(.*)$").unwrap(),
        date_field: Regex::new(r"(📅|📆|🗓|⏳|⌛|🛫|➕|✅|❌)\u{FE0F}?\s*(\d{4}-\d{2}-\d{2})").unwrap(),
        recur: Regex::new(r"🔁\u{FE0F}?\s*([a-zA-Z0-9, !]+?)\s*(?:$|[📅📆🗓⏳⌛🛫➕✅❌⏫🔼🔽🔺⏬🆔⛔🏁#\[])").unwrap(),
        dv_field: Regex::new(r"[\[(](due|scheduled|start|created|completion|cancelled|repeat|priority)::\s*([^\])]*)[\])]").unwrap(),
        tag: Regex::new(r"(?:^|\s)(#[\p{L}\p{N}_/\-]+)").unwrap(),
        recur_text: Regex::new(r"🔁\u{FE0F}?\s*[a-zA-Z0-9, !]+").unwrap(),
    })
}

/// FNV-1a of some bytes: stable across runs and machines, unlike `std`'s hasher.
pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
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

/// Parse a list item's first line, from its marker (`- [ ] …`); `None` if it isn't a task
/// (or doesn't carry the global filter).
pub fn parse_line(line: &str, settings: &TasksSettings) -> Option<Task> {
    let r = re();
    let c = r.task.captures(line)?;
    let symbol = c[2].chars().next()?;
    let body = &c[4];
    let global_filter = settings.global_filter.as_str();
    if !global_filter.is_empty() && !body.contains(global_filter) {
        return None;
    }
    let mut t = Task {
        status: symbol,
        status_type: settings.status_type(symbol),
        text: String::new(),
        due: None,
        scheduled: None,
        start: None,
        created: None,
        done: None,
        cancelled: None,
        recurrence: None,
        priority: None,
        tags: Vec::new(),
        links: crate::vault::line_links(body),
        daily_note: None,
    };
    let date = |s: &str| NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok();
    for m in r.date_field.captures_iter(body) {
        let field = match &m[1] {
            "📅" | "📆" | "🗓" => &mut t.due,
            "⏳" | "⌛" => &mut t.scheduled,
            "🛫" => &mut t.start,
            "➕" => &mut t.created,
            "✅" => &mut t.done,
            _ => &mut t.cancelled,
        };
        *field = date(&m[2]);
    }
    if let Some(m) = r.recur.captures(body) {
        t.recurrence = Some(m[1].trim().to_string());
    }
    for m in r.dv_field.captures_iter(body) {
        let v = m[2].trim();
        let field = match &m[1] {
            "due" => &mut t.due,
            "scheduled" => &mut t.scheduled,
            "start" => &mut t.start,
            "created" => &mut t.created,
            "completion" => &mut t.done,
            "cancelled" => &mut t.cancelled,
            "repeat" => {
                t.recurrence = Some(v.to_string());
                continue;
            }
            _ => {
                t.priority = Priority::from_name(v);
                continue;
            }
        };
        *field = date(v).or(field.take());
    }
    for (emoji, p) in Priority::EMOJI {
        if body.contains(emoji) {
            t.priority = Some(p);
        }
    }
    t.tags = r
        .tag
        .captures_iter(body)
        .map(|m| m[1].to_string())
        .collect();
    // Clean description: drop the markers.
    let mut text = r.date_field.replace_all(body, "").into_owned();
    text = r.dv_field.replace_all(&text, "").into_owned();
    text = r.recur_text.replace_all(&text, "").into_owned();
    for (e, _) in Priority::EMOJI {
        text = text.replace(e, "");
    }
    if !global_filter.is_empty() {
        text = text.replace(global_filter, "");
    }
    t.text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.text.is_empty() {
        return None; // an empty checkbox (e.g. from a template) isn't a task yet
    }
    Some(t)
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
    pub priority: Option<&'static str>,
    pub tags: Vec<String>,
    /// Vault-relative file, absolute path and 1-based line.
    pub file: String,
    pub path: String,
    pub line: usize,
    /// The task's `rev`, to pass to `task toggle --id` (guards against edits in between).
    pub id: String,
    pub raw: String,
}

impl AgendaTask {
    fn new(item: &Item, t: &Task, root: &Path, raw: &str) -> AgendaTask {
        let s = |d: Option<NaiveDate>| d.map(|d| d.to_string());
        let date = t.agenda_date();
        AgendaTask {
            text: t.text.clone(),
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
            recurrence: t.recurrence.clone(),
            priority: t.priority.map(Priority::name),
            tags: t.tags.clone(),
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
    pub vault: String,
    pub from: String,
    pub to: String,
    pub tasks: Vec<AgendaTask>,
    pub daily_notes: Vec<DailyNote>,
    /// Per-day summary for calendar markers (only days with something on them).
    pub days: BTreeMap<String, Day>,
}

/// The vault's dated tasks and daily notes from `from` to `to`, read from its items.
pub fn agenda(vault: &Vault, from: NaiveDate, to: NaiveDate) -> Agenda {
    let in_range = |d: NaiveDate| d >= from && d <= to;
    let mut tasks = Vec::new();
    let mut notes = Vec::new();
    for note in &vault.notes {
        let file = format!("{}.md", note.rel);
        if let Some(d) = vault.item_settings().daily.date_of(&file)
            && in_range(d)
        {
            notes.push(DailyNote {
                date: d.to_string(),
                file,
                path: note.path.to_string_lossy().into_owned(),
            });
        }
        let lines: Vec<&str> = note.text.lines().collect();
        for item in vault.items(note) {
            if let Data::Task(t) = &item.data
                && t.agenda_date().is_some_and(|(d, _)| in_range(d))
            {
                let raw = lines.get(item.line - 1).copied().unwrap_or_default();
                tasks.push(AgendaTask::new(item, t, &vault.root, raw));
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
        vault: vault.root.to_string_lossy().into_owned(),
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

/// Toggle the task at `path:line` (1-based) like Obsidian Tasks does.
pub fn toggle(
    path: &Path,
    line: usize,
    expect_id: Option<&str>,
    today: NaiveDate,
) -> Result<Toggled> {
    let path = path
        .canonicalize()
        .with_context(|| format!("no such file: {}", path.display()))?;
    let root = crate::board::find_vault_root(&path);
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
    let item = task_at(&note_items, line);
    let rev = item.map_or_else(|| line_id(&old), |(i, _)| i.rev.clone());
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

    let (new_char, new_body) = if matches!(
        task.status_type,
        StatusType::Done | StatusType::Cancelled
    ) {
        // Back to todo: drop the done / cancelled date.
        (
            ' ',
            strip_field(&strip_field(&body, "✅", "completion"), "❌", "cancelled"),
        )
    } else {
        // Done (from todo, in progress, or a custom status).
        let mut b = body.clone();
        if settings.set_done_date {
            b = add_field(&b, "✅", "completion", today, dataview);
        }
        if let Some(rule) = &task.recurrence {
            match next_occurrence(task, rule, today) {
                Some(dates) => {
                    let mut nb = strip_field(&body, "✅", "completion");
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
        ('x', b)
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
        task_at(&new_items, at).map_or_else(|| line_id(&lines[at - 1]), |(i, _)| i.rev.clone())
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

/// The task item that starts on `line`.
fn task_at(items: &[Item], line: usize) -> Option<(&Item, &Task)> {
    items.iter().find_map(|i| match &i.data {
        Data::Task(t) if i.line == line => Some((i, t)),
        _ => None,
    })
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

pub fn today() -> NaiveDate {
    Local::now().date_naive()
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

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn settings(json: serde_json::Value) -> TasksSettings {
        TasksSettings::from_json(&json)
    }

    fn parse(line: &str) -> Option<Task> {
        parse_line(line, &settings(serde_json::json!({})))
    }

    #[test]
    fn parses_tasks_emoji_format() {
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
        assert_eq!(t.priority, Some(Priority::High));
        assert_eq!(t.tags, ["#bills"]);
        assert_eq!(t.links, ["Landlord"]);
        let s = parse("  * [x] Call back ⏳ 2026-11-03 ✅ 2026-11-04").unwrap();
        assert_eq!(s.status_type, StatusType::Done);
        assert_eq!(s.agenda_date(), Some((d("2026-11-03"), "scheduled")));
        assert_eq!(s.done, Some(d("2026-11-04")));
        assert!(parse("- not a task").is_none());
        assert!(parse("- [ ] ").is_none(), "empty checkbox");
        assert!(parse("- [ ] 📅 2026-11-07").is_none(), "only a date");
        let tagged = settings(serde_json::json!({"globalFilter": "#task"}));
        assert!(parse_line("- [ ] untagged", &tagged).is_none());
        let t = parse_line("- [/] Halfway #task", &tagged).unwrap();
        assert_eq!(
            (t.text.as_str(), t.status_type),
            ("Halfway", StatusType::InProgress)
        );
    }

    #[test]
    fn custom_statuses_come_from_the_plugin_settings() {
        let type_of = |s: &TasksSettings, line: &str| parse_line(line, s).unwrap().status_type;
        let default = settings(serde_json::json!({}));
        assert_eq!(type_of(&default, "- [-] Dropped"), StatusType::Cancelled);
        assert_eq!(type_of(&default, "- [X] Shouted"), StatusType::Done);
        assert_eq!(type_of(&default, "- [?] Maybe"), StatusType::NonTask);
        let custom = settings(serde_json::json!({"statusSettings": {
            "coreStatuses": [{"symbol": "x", "name": "Done", "type": "DONE"}],
            "customStatuses": [
                {"symbol": "?", "name": "Question", "type": "TODO"},
                {"symbol": "-", "name": "Skipped", "type": "NON_TASK"},
                {"symbol": "ab", "name": "Two symbols", "type": "DONE"},
                {"symbol": "!", "name": "Unknown type", "type": "SOMEDAY"}
            ]
        }}));
        assert_eq!(type_of(&custom, "- [?] Maybe"), StatusType::Todo);
        assert_eq!(type_of(&custom, "- [-] Skipped"), StatusType::NonTask);
        assert_eq!(type_of(&custom, "- [/] Halfway"), StatusType::InProgress);
        assert_eq!(type_of(&custom, "- [!] Odd"), StatusType::NonTask);
    }

    #[test]
    fn parses_dataview_format() {
        let t = parse("- [ ] Dentist [due:: 2026-11-12] [repeat:: every year] [priority:: high]")
            .unwrap();
        assert_eq!(t.text, "Dentist");
        assert_eq!(t.due, Some(d("2026-11-12")));
        assert_eq!(t.recurrence.as_deref(), Some("every year"));
        assert_eq!(t.priority, Some(Priority::High));
    }

    #[test]
    fn task_json_is_pinned() {
        let mut t = parse("- [/] Dentist #health 📅 2026-11-12 🔼").unwrap();
        t.daily_note = Some(d("2026-11-05"));
        let json = serde_json::json!({
            "status": "/",
            "status_type": "in_progress",
            "text": "Dentist #health",
            "due": "2026-11-12",
            "scheduled": null,
            "start": null,
            "created": null,
            "done": null,
            "cancelled": null,
            "recurrence": null,
            "priority": "medium",
            "tags": ["#health"],
            "links": [],
            "daily_note": "2026-11-05"
        });
        assert_eq!(serde_json::to_value(&t).unwrap(), json);
        assert_eq!(serde_json::from_value::<Task>(json).unwrap(), t);
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

    fn temp_vault(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cold-lava-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".obsidian")).unwrap();
        dir
    }

    #[test]
    fn toggle_done_undone_and_guard() {
        let v = temp_vault("toggle");
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
        let v = temp_vault("callout");
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
        let v = temp_vault("recur");
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
    fn agenda_finds_tasks_and_daily_notes() {
        let v = temp_vault("agenda");
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
            &crate::vault::scan(&v).unwrap(),
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
}
