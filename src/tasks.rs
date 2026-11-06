//! Tasks in the Obsidian Tasks plugin's format, for `cold-lava agenda` and `cold-lava task toggle`.
//!
//! Understands the emoji format (`- [ ] Pay rent 📅 2026-11-07 🔁 every month`) and the Dataview
//! format (`- [ ] Pay rent [due:: 2026-11-07]`), the plugin's global filter, its "set done date"
//! and "next recurrence on the line below" settings, and daily notes as an implicit date.

use crate::daily;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, Days, Local, Months, NaiveDate, Weekday};
use regex::Regex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use walkdir::WalkDir;

// ---------- settings ----------

pub struct TasksSettings {
    pub global_filter: String,
    pub set_done_date: bool,
    pub recurrence_below: bool,
    pub dataview: bool,
}

impl TasksSettings {
    pub fn load(root: &Path) -> TasksSettings {
        let json: serde_json::Value =
            std::fs::read_to_string(root.join(".obsidian/plugins/obsidian-tasks-plugin/data.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
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
        }
    }
}

// ---------- parsing ----------

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Task {
    /// Description without dates, recurrence and priority markers.
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
    /// Fingerprint of the line, to pass to `task toggle --id` (guards against edits in between).
    pub id: String,
    pub raw: String,
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

/// FNV-1a fingerprint of a line, as 12 hex digits.
pub fn line_id(line: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in line.trim_end_matches(['\r', '\n']).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:012x}", h & 0xffff_ffff_ffff)
}

fn status_of(c: char) -> (&'static str, bool) {
    match c {
        ' ' => ("todo", false),
        'x' | 'X' => ("done", true),
        '-' => ("cancelled", false),
        '/' => ("in_progress", false),
        _ => ("other", false),
    }
}

/// Parse one line; `None` if it isn't a task (or doesn't carry the global filter).
pub fn parse_line(line: &str, global_filter: &str) -> Option<Task> {
    let r = re();
    let c = r.task.captures(line)?;
    let status_char = c[2].to_string();
    let body = c[4].to_string();
    if !global_filter.is_empty() && !body.contains(global_filter) {
        return None;
    }
    let (status, done) = status_of(status_char.chars().next().unwrap_or(' '));
    let mut t = Task {
        text: String::new(),
        status,
        status_char,
        done,
        date: None,
        date_source: None,
        due: None,
        scheduled: None,
        start: None,
        created: None,
        done_date: None,
        cancelled_date: None,
        recurrence: None,
        priority: None,
        tags: Vec::new(),
        file: String::new(),
        path: String::new(),
        line: 0,
        id: line_id(line),
        raw: line.to_string(),
    };
    for m in r.date_field.captures_iter(&body) {
        let v = Some(m[2].to_string());
        match &m[1] {
            "📅" | "📆" | "🗓" => t.due = v,
            "⏳" | "⌛" => t.scheduled = v,
            "🛫" => t.start = v,
            "➕" => t.created = v,
            "✅" => t.done_date = v,
            _ => t.cancelled_date = v,
        }
    }
    if let Some(m) = r.recur.captures(&body) {
        t.recurrence = Some(m[1].trim().to_string());
    }
    for m in r.dv_field.captures_iter(&body) {
        let v = m[2].trim().to_string();
        let date = NaiveDate::parse_from_str(&v, "%Y-%m-%d")
            .is_ok()
            .then(|| v.clone());
        match &m[1] {
            "due" => t.due = date.or(t.due.take()),
            "scheduled" => t.scheduled = date.or(t.scheduled.take()),
            "start" => t.start = date.or(t.start.take()),
            "created" => t.created = date.or(t.created.take()),
            "completion" => t.done_date = date.or(t.done_date.take()),
            "cancelled" => t.cancelled_date = date.or(t.cancelled_date.take()),
            "repeat" => t.recurrence = Some(v),
            "priority" => {
                t.priority = match v.as_str() {
                    "highest" => Some("highest"),
                    "high" => Some("high"),
                    "medium" => Some("medium"),
                    "low" => Some("low"),
                    "lowest" => Some("lowest"),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    for (emoji, p) in [
        ("🔺", "highest"),
        ("⏫", "high"),
        ("🔼", "medium"),
        ("🔽", "low"),
        ("⏬", "lowest"),
    ] {
        if body.contains(emoji) {
            t.priority = Some(p);
        }
    }
    t.tags = r
        .tag
        .captures_iter(&body)
        .map(|m| m[1].to_string())
        .collect();
    // Clean description: drop the markers.
    let mut text = r.date_field.replace_all(&body, "").into_owned();
    text = r.dv_field.replace_all(&text, "").into_owned();
    text = r.recur_text.replace_all(&text, "").into_owned();
    for e in ["🔺", "⏫", "🔼", "🔽", "⏬"] {
        text = text.replace(e, "");
    }
    if !global_filter.is_empty() {
        text = text.replace(global_filter, "");
    }
    t.text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.text.is_empty() {
        return None; // an empty checkbox (e.g. from a template) isn't a task yet
    }
    let (date, src) = if let Some(d) = &t.due {
        (Some(d.clone()), Some("due"))
    } else if let Some(d) = &t.scheduled {
        (Some(d.clone()), Some("scheduled"))
    } else if let Some(d) = &t.start {
        (Some(d.clone()), Some("start"))
    } else {
        (None, None)
    };
    t.date = date;
    t.date_source = src;
    Some(t)
}

// ---------- agenda ----------

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
    pub tasks: Vec<Task>,
    pub daily_notes: Vec<DailyNote>,
    /// Per-day summary for calendar markers (only days with something on them).
    pub days: BTreeMap<String, Day>,
}

pub fn agenda(root: &Path, from: NaiveDate, to: NaiveDate) -> Result<Agenda> {
    let root = root
        .canonicalize()
        .with_context(|| format!("vault not found: {}", root.display()))?;
    let settings = TasksSettings::load(&root);
    let daily = daily::Settings::load(&root);
    let in_range = |d: NaiveDate| d >= from && d <= to;
    let mut tasks = Vec::new();
    let mut notes = Vec::new();
    let walker = WalkDir::new(&root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for e in walker.filter_map(|e| e.ok()) {
        if !e.file_type().is_file() || e.path().extension().is_none_or(|x| x != "md") {
            continue;
        }
        let rel = e
            .path()
            .strip_prefix(&root)
            .unwrap_or(e.path())
            .to_string_lossy()
            .replace('\\', "/");
        let note_date = daily.date_of(&rel);
        if let Some(d) = note_date.filter(|d| in_range(*d)) {
            notes.push(DailyNote {
                date: d.to_string(),
                file: rel.clone(),
                path: e.path().to_string_lossy().into_owned(),
            });
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        if !text.contains('[') {
            continue;
        }
        let mut in_code = false;
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            let Some(mut t) = parse_line(line, &settings.global_filter) else {
                continue;
            };
            if t.date.is_none()
                && let Some(d) = note_date
            {
                t.date = Some(d.to_string());
                t.date_source = Some("daily_note");
            }
            let Some(d) = t
                .date
                .as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            else {
                continue;
            };
            if !in_range(d) {
                continue;
            }
            t.file = rel.clone();
            t.path = e.path().to_string_lossy().into_owned();
            t.line = i + 1;
            tasks.push(t);
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
    Ok(Agenda {
        vault: root.to_string_lossy().into_owned(),
        from: from.to_string(),
        to: to.to_string(),
        tasks,
        daily_notes: notes,
        days,
    })
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
    let settings = TasksSettings::load(&root);
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
    if let Some(id) = expect_id
        && line_id(&old) != id
    {
        return Err(Changed(format!(
            "line {line} changed since the agenda was read; refresh and try again"
        ))
        .into());
    }
    let Some(task) = parse_line(&old, &settings.global_filter) else {
        bail!("line {line} of {} is not a task", path.display());
    };
    let dataview = settings.dataview || old.contains("::");
    let caps = re().task.captures(&old).unwrap();
    let (prefix, mid, body) = (
        caps[1].to_string(),
        caps[3].to_string(),
        caps[4].to_string(),
    );
    let mut warning = None;
    let mut next = None;

    let (new_char, new_body) = if task.done || task.status == "cancelled" {
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
            match next_occurrence(&task, rule, today) {
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
    let mut next_out = None;
    if let Some(n) = next {
        let at = if settings.recurrence_below {
            line
        } else {
            line - 1
        };
        lines.insert(at, n.clone());
        if !settings.recurrence_below {
            toggled_line += 1;
        }
        next_out = Some(NextTask {
            line: at + 1,
            id: line_id(&n),
            raw: n,
        });
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
    let status = status_of(new_char).0;
    Ok(Toggled {
        path: path.to_string_lossy().into_owned(),
        line: toggled_line,
        status,
        id: line_id(&new_line),
        raw: new_line,
        next: next_out,
        warning,
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
    let parse = |s: &Option<String>| {
        s.as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    };
    let fields = [
        ("📅", "due", parse(&t.due)),
        ("⏳", "scheduled", parse(&t.scheduled)),
        ("🛫", "start", parse(&t.start)),
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

    #[test]
    fn parses_tasks_emoji_format() {
        let t = parse_line(
            "- [ ] Pay rent #bills 📅 2026-11-07 ⏫ 🔁 every month ➕ 2026-10-01",
            "",
        )
        .unwrap();
        assert_eq!(t.text, "Pay rent #bills");
        assert_eq!((t.status, t.done), ("todo", false));
        assert_eq!(t.due.as_deref(), Some("2026-11-07"));
        assert_eq!(t.created.as_deref(), Some("2026-10-01"));
        assert_eq!(
            (t.date.as_deref(), t.date_source),
            (Some("2026-11-07"), Some("due"))
        );
        assert_eq!(t.recurrence.as_deref(), Some("every month"));
        assert_eq!(t.priority, Some("high"));
        assert_eq!(t.tags, ["#bills"]);
        let s = parse_line("  * [x] Call back ⏳ 2026-11-03 ✅ 2026-11-04", "").unwrap();
        assert_eq!(
            (s.status, s.done, s.date_source),
            ("done", true, Some("scheduled"))
        );
        assert_eq!(s.done_date.as_deref(), Some("2026-11-04"));
        assert!(parse_line("- not a task", "").is_none());
        assert!(parse_line("- [ ] ", "").is_none(), "empty checkbox");
        assert!(
            parse_line("- [ ] 📅 2026-11-07", "").is_none(),
            "only a date"
        );
        assert!(parse_line("- [ ] untagged", "#task").is_none());
        assert_eq!(
            parse_line("- [/] Halfway #task", "#task").unwrap().status,
            "in_progress"
        );
    }

    #[test]
    fn parses_dataview_format() {
        let t = parse_line(
            "- [ ] Dentist [due:: 2026-11-12] [repeat:: every year] [priority:: high]",
            "",
        )
        .unwrap();
        assert_eq!(t.text, "Dentist");
        assert_eq!(t.due.as_deref(), Some("2026-11-12"));
        assert_eq!(t.recurrence.as_deref(), Some("every year"));
        assert_eq!(t.priority, Some("high"));
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
        let a = agenda(&v, d("2026-11-01"), d("2026-11-30")).unwrap();
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
