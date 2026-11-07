//! Daily notes, following the vault's Obsidian "Daily notes" settings
//! (`.obsidian/daily-notes.json`: folder, moment.js date format, template).

use anyhow::{Context, Result};
use chrono::{Datelike, Local, NaiveDate, NaiveTime, Timelike, Weekday};
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

pub struct Settings {
    /// Folder relative to the vault root ("" = root).
    pub folder: String,
    /// moment.js format; may contain `/` for sub-folders, e.g. `YYYY/MM/YYYY-MM-DD`.
    pub format: String,
    /// Template note, relative to the vault root (without `.md` is fine).
    pub template: Option<String>,
}

impl Settings {
    pub fn load(root: &Path) -> Settings {
        let json: serde_json::Value =
            std::fs::read_to_string(root.join(".obsidian/daily-notes.json"))
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
        let get = |k: &str| {
            json.get(k)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        Settings {
            folder: get("folder").unwrap_or("").trim_matches('/').to_string(),
            format: get("format").unwrap_or("YYYY-MM-DD").to_string(),
            template: get("template").map(String::from),
        }
    }

    /// Vault-relative path of the daily note for `date`.
    pub fn rel_path(&self, date: NaiveDate) -> String {
        let name = format_date(date, &self.format);
        if self.folder.is_empty() {
            format!("{name}.md")
        } else {
            format!("{}/{name}.md", self.folder)
        }
    }

    /// The date of a note, if its vault-relative path is a daily note.
    pub fn date_of(&self, rel: &str) -> Option<NaiveDate> {
        let rel = rel.strip_suffix(".md")?;
        let inside = if self.folder.is_empty() {
            rel
        } else {
            rel.strip_prefix(&self.folder)?.strip_prefix('/')?
        };
        parse_date(inside, &self.format)
    }
}

/// Parse a date given on the command line: YYYY-MM-DD, today, tomorrow, yesterday.
pub fn parse_cli_date(s: &str) -> Result<NaiveDate> {
    let today = Local::now().date_naive();
    Ok(match s {
        "today" => today,
        "tomorrow" => today.succ_opt().unwrap_or(today),
        "yesterday" => today.pred_opt().unwrap_or(today),
        _ => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .with_context(|| format!("not a date (YYYY-MM-DD): {s}"))?,
    })
}

/// Create the daily note for `date` if needed (from the template). Returns its path.
pub fn ensure(root: &Path, date: NaiveDate) -> Result<PathBuf> {
    let s = Settings::load(root);
    let rel = s.rel_path(date);
    let path = root.join(&rel);
    if path.exists() {
        return Ok(path);
    }
    let title = rel
        .rsplit('/')
        .next()
        .unwrap_or(&rel)
        .trim_end_matches(".md")
        .to_string();
    let body = match &s.template {
        Some(t) => {
            let t = if t.ends_with(".md") {
                t.clone()
            } else {
                format!("{t}.md")
            };
            let text = std::fs::read_to_string(root.join(&t))
                .with_context(|| format!("reading the daily-note template {t}"))?;
            let now = date.and_time(Local::now().time());
            crate::templates::fill(&text, &title, now, "YYYY-MM-DD", "HH:mm")
        }
        None => String::new(),
    };
    crate::write::create(&path, body.as_bytes())?;
    Ok(path)
}

// ---------- moment.js formats ----------

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum Tok {
    Year4,
    Year2,
    MonthName,
    MonthShort,
    Month2,
    Month,
    DayOrd,
    Day2,
    Day,
    WeekdayName,
    WeekdayShort,
    IsoWeek2,
    IsoWeek,
    Hour2,
    Hour12,
    Minute2,
    Second2,
    AmPm,
    Lit(char),
}

/// moment.js tokens cold-lava reads, longest first so `MMMM` wins over `MM`.
const TOKENS: [(&str, Tok); 18] = [
    ("YYYY", Tok::Year4),
    ("MMMM", Tok::MonthName),
    ("dddd", Tok::WeekdayName),
    ("MMM", Tok::MonthShort),
    ("ddd", Tok::WeekdayShort),
    ("YY", Tok::Year2),
    ("MM", Tok::Month2),
    ("Do", Tok::DayOrd),
    ("DD", Tok::Day2),
    ("WW", Tok::IsoWeek2),
    ("HH", Tok::Hour2),
    ("mm", Tok::Minute2),
    ("ss", Tok::Second2),
    ("M", Tok::Month),
    ("D", Tok::Day),
    ("W", Tok::IsoWeek),
    ("h", Tok::Hour12),
    ("A", Tok::AmPm),
];

/// Letters in `fmt` that are moment.js tokens cold-lava doesn't read (`Q`, `X`, `gggg`), which
/// `tokenize` would keep as literal text. Text in `[brackets]` is literal on purpose.
pub fn unknown_tokens(fmt: &str) -> Vec<String> {
    let mut unknown: Vec<String> = Vec::new();
    let mut rest = fmt;
    'outer: while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']').unwrap_or(r.len());
            rest = r.get(end + 1..).unwrap_or("");
            continue;
        }
        for (pat, _) in TOKENS {
            if let Some(r) = rest.strip_prefix(pat) {
                rest = r;
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap();
        if c.is_ascii_alphabetic() {
            let run = rest.len() - rest.trim_start_matches(c).len();
            let token = rest[..run].to_string();
            if !unknown.contains(&token) {
                unknown.push(token);
            }
            rest = &rest[run..];
        } else {
            rest = &rest[c.len_utf8()..];
        }
    }
    unknown
}

fn tokenize(fmt: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut rest = fmt;
    'outer: while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('[') {
            // [literal text]
            let end = r.find(']').unwrap_or(r.len());
            out.extend(r[..end].chars().map(Tok::Lit));
            rest = r.get(end + 1..).unwrap_or("");
            continue;
        }
        for (pat, tok) in TOKENS {
            if let Some(r) = rest.strip_prefix(pat) {
                out.push(tok);
                rest = r;
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(Tok::Lit(c));
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn ordinal(d: u32) -> String {
    let suffix = match (d % 10, d % 100) {
        (1, n) if n != 11 => "st",
        (2, n) if n != 12 => "nd",
        (3, n) if n != 13 => "rd",
        _ => "th",
    };
    format!("{d}{suffix}")
}

pub fn format_date(date: NaiveDate, fmt: &str) -> String {
    format_datetime(date, NaiveTime::MIN, fmt)
}

pub fn format_datetime(date: NaiveDate, time: NaiveTime, fmt: &str) -> String {
    let mut s = String::new();
    for t in tokenize(fmt) {
        match t {
            Tok::Year4 => s += &format!("{:04}", date.year()),
            Tok::Year2 => s += &format!("{:02}", date.year().rem_euclid(100)),
            Tok::MonthName => s += MONTHS[date.month0() as usize],
            Tok::MonthShort => s += &MONTHS[date.month0() as usize][..3],
            Tok::Month2 => s += &format!("{:02}", date.month()),
            Tok::Month => s += &date.month().to_string(),
            Tok::DayOrd => s += &ordinal(date.day()),
            Tok::Day2 => s += &format!("{:02}", date.day()),
            Tok::Day => s += &date.day().to_string(),
            Tok::WeekdayName => s += DAYS[date.weekday().num_days_from_monday() as usize],
            Tok::WeekdayShort => s += &DAYS[date.weekday().num_days_from_monday() as usize][..3],
            Tok::IsoWeek2 => s += &format!("{:02}", date.iso_week().week()),
            Tok::IsoWeek => s += &date.iso_week().week().to_string(),
            Tok::Hour2 => s += &format!("{:02}", time.hour()),
            Tok::Hour12 => s += &(((time.hour() + 11) % 12) + 1).to_string(),
            Tok::Minute2 => s += &format!("{:02}", time.minute()),
            Tok::Second2 => s += &format!("{:02}", time.second()),
            Tok::AmPm => s += if time.hour() < 12 { "AM" } else { "PM" },
            Tok::Lit(c) => s.push(c),
        }
    }
    s
}

/// The regex that reads dates written in `fmt`, and what each of its groups holds. Compiled once
/// per format: every note's path is checked against the daily-note format.
fn pattern(fmt: &str) -> Option<Arc<(Regex, Vec<Tok>)>> {
    type Cache = Mutex<HashMap<String, Option<Arc<(Regex, Vec<Tok>)>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache
        .entry(fmt.to_string())
        .or_insert_with(|| compile(fmt).map(Arc::new))
        .clone()
}

fn compile(fmt: &str) -> Option<(Regex, Vec<Tok>)> {
    let toks = tokenize(fmt);
    let mut re = String::from("^");
    let mut groups = Vec::new();
    for t in &toks {
        let (pat, captured) = match t {
            Tok::Year4 => (r"(\d{4})".to_string(), true),
            Tok::Year2 => (r"(\d{2})".to_string(), true),
            Tok::MonthName => (format!("({})", MONTHS.join("|")), true),
            Tok::MonthShort => (format!("({})", MONTHS.map(|m| &m[..3]).join("|")), true),
            Tok::Month2 | Tok::Day2 => (r"(\d{2})".to_string(), true),
            Tok::Month | Tok::Day => (r"(\d{1,2})".to_string(), true),
            Tok::DayOrd => (r"(\d{1,2})(?:st|nd|rd|th)".to_string(), true),
            Tok::WeekdayName => (format!("(?:{})", DAYS.join("|")), false),
            Tok::WeekdayShort => (format!("(?:{})", DAYS.map(|d| &d[..3]).join("|")), false),
            Tok::IsoWeek2 | Tok::Hour2 | Tok::Minute2 | Tok::Second2 => {
                (r"\d{2}".to_string(), false)
            }
            Tok::IsoWeek | Tok::Hour12 => (r"\d{1,2}".to_string(), false),
            Tok::AmPm => ("(?:AM|PM)".to_string(), false),
            Tok::Lit(c) => (regex::escape(&c.to_string()), false),
        };
        if captured {
            groups.push(*t);
        }
        re += &pat;
    }
    re.push('$');
    Some((Regex::new(&re).ok()?, groups))
}

/// Parse `s` strictly with a moment.js format (it must format back to exactly `s`).
pub fn parse_date(s: &str, fmt: &str) -> Option<NaiveDate> {
    let pattern = pattern(fmt)?;
    let (re, groups) = &*pattern;
    let caps = re.captures(s)?;
    let (mut y, mut m, mut d) = (None, None, None);
    for (i, t) in groups.iter().enumerate() {
        let v = caps.get(i + 1)?.as_str();
        match t {
            Tok::Year4 => y = v.parse::<i32>().ok(),
            Tok::Year2 => y = v.parse::<i32>().ok().map(|yy| 2000 + yy),
            Tok::MonthName => m = MONTHS.iter().position(|x| *x == v).map(|i| i as u32 + 1),
            Tok::MonthShort => {
                m = MONTHS
                    .iter()
                    .position(|x| &x[..3] == v)
                    .map(|i| i as u32 + 1)
            }
            Tok::Month2 | Tok::Month => m = v.parse().ok(),
            Tok::Day2 | Tok::Day | Tok::DayOrd => d = v.parse().ok(),
            _ => {}
        }
    }
    let date = NaiveDate::from_ymd_opt(y?, m?, d?)?;
    (format_date(date, fmt) == s).then_some(date)
}

pub fn weekday_from_name(s: &str) -> Option<Weekday> {
    let s = s.to_lowercase();
    let i = DAYS
        .iter()
        .position(|d| d.to_lowercase() == s || d[..3].to_lowercase() == s)?;
    Weekday::try_from(i as u8).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn moment_formats_round_trip() {
        let date = d(2026, 11, 5);
        for (fmt, expect) in [
            ("YYYY-MM-DD", "2026-11-05"),
            ("YYYY/MM/YYYY-MM-DD", "2026/11/2026-11-05"),
            ("DD.MM.YY", "05.11.26"),
            ("dddd, MMMM Do YYYY", "Thursday, November 5th 2026"),
            ("YYYY-[W]WW", "2026-W45"),
            ("D MMM YYYY", "5 Nov 2026"),
        ] {
            assert_eq!(format_date(date, fmt), expect, "{fmt}");
            if !fmt.contains('W') {
                assert_eq!(parse_date(expect, fmt), Some(date), "{fmt}");
            }
        }
        assert_eq!(parse_date("2026-13-40", "YYYY-MM-DD"), None);
        assert_eq!(
            parse_date("2026-11-5", "YYYY-MM-DD"),
            None,
            "strict: must format back the same"
        );
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
    }

    #[test]
    fn settings_paths() {
        let s = Settings {
            folder: "Journal".into(),
            format: "YYYY/MM-DD".into(),
            template: None,
        };
        assert_eq!(s.rel_path(d(2026, 1, 9)), "Journal/2026/01-09.md");
        assert_eq!(s.date_of("Journal/2026/01-09.md"), Some(d(2026, 1, 9)));
        assert_eq!(s.date_of("Other/2026/01-09.md"), None);
        assert_eq!(s.date_of("Journal/notes.md"), None);
    }

    #[test]
    fn unknown_tokens_are_found_outside_brackets() {
        assert!(unknown_tokens("YYYY-MM-DD dddd [Week] WW, Do MMM h:mm A").is_empty());
        assert_eq!(unknown_tokens("YYYY-Q gggg [Q] x"), ["Q", "gggg", "x"]);
    }
}
