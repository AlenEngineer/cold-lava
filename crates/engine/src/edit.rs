//! Changing items in place: `set_field` puts one value of one typed item back into its note
//! (docs/extensions.md, "Writing, as built").
//!
//! The value goes through the source its field is read from: only the value's bytes change, a
//! missing value is added the way the line already writes such fields, and frontmatter is edited
//! line by line. Then the note is read again in memory, and the edit is refused before anything
//! is written unless the item reads back exactly the new value with every other field and every
//! other item in the note unchanged.

use std::fmt;
use std::ops::Range;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use regex::Regex;
use serde::Serialize;
use serde_json::Value as Json;

use crate::frontmatter;
use crate::items::{self, Data, Item, Settings};
use crate::matcher::{self, Typed};
use crate::types::{On, Shape, Source, Value};
use crate::{daily, vault, write};

/// Why an edit wasn't made. Nothing was written.
#[derive(Clone, Debug, PartialEq)]
pub enum EditError {
    /// The item is gone, its `rev` differs, or the file changed while writing: read it again.
    Changed(String),
    /// The edit can't be made this way: an unknown field, a read-only source, a value that
    /// doesn't fit, a field there is no safe place for.
    Refused(String),
    /// The note wouldn't read back as asked.
    ReadBack(String),
    Io(String),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EditError::Changed(m) | EditError::Refused(m) | EditError::Io(m) => f.write_str(m),
            EditError::ReadBack(m) => write!(f, "refused, the note would not read back: {m}"),
        }
    }
}

impl std::error::Error for EditError {}

use EditError::{Changed, ReadBack, Refused};

/// Set `field` of the item `id` to `value`, in the form items carry it (an ISO date, minutes
/// for a duration, an enum's name, a link's target); a string is also read as the field's type
/// reads it. `null` or `""` removes the value. With `rev`, refused if the item changed since.
pub struct SetField<'a> {
    pub id: &'a str,
    pub rev: Option<&'a str>,
    pub field: &'a str,
    pub value: Json,
}

/// What `set_field` did, or would do.
#[derive(Debug, Serialize)]
pub struct Set {
    /// The note's text after the edit.
    #[serde(skip)]
    pub text: String,
    /// The item as it reads back. Its id is new when the field is part of its key.
    pub item: Item,
    /// False when the note already said this, and nothing was written.
    pub changed: bool,
}

/// `set_field` on the note at vault-relative `note` in `root`, written with `write::replace`
/// unless `dry_run`.
pub fn set_field(root: &Path, note: &str, req: &SetField, dry_run: bool) -> Result<Set, EditError> {
    let path = root.join(note);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| EditError::Io(format!("{}: {e}", path.display())))?;
    let set = plan(note, &text, &Settings::load(root), req)?;
    if set.changed && !dry_run {
        write::replace(&path, text.as_bytes(), set.text.as_bytes()).map_err(|e| match e {
            write::WriteError::Changed(_) => Changed(e.to_string()),
            e => EditError::Io(e.to_string()),
        })?;
    }
    Ok(set)
}

/// The edit `set_field` would make to `text`, the note at vault-relative `note`, checked by
/// reading it back.
pub fn plan(note: &str, text: &str, s: &Settings, req: &SetField) -> Result<Set, EditError> {
    let before = items::extract(note, text, s);
    let k = before
        .iter()
        .position(|i| i.id == req.id)
        .ok_or_else(|| Changed(format!("{note} has no item {}: read it again", req.id)))?;
    let item = &before[k];
    if req.rev.is_some_and(|r| r != item.rev) {
        return Err(Changed(format!(
            "{note}:{} changed since it was read: read it again",
            item.line
        )));
    }
    let Data::Typed(typed) = &item.data else {
        return Err(Refused(format!(
            "{note}:{} is a {}, not a typed item",
            item.line,
            item.data.kind()
        )));
    };
    let ty = s
        .types
        .types
        .iter()
        .find(|t| t.name == typed.kind)
        .ok_or_else(|| Refused(format!("no type called {}", typed.kind)))?;
    let def = &ty.definition;
    let field = req.field;
    let value_type = def.fields.get(field).ok_or_else(|| {
        let names: Vec<&str> = def.fields.keys().map(String::as_str).collect();
        Refused(format!(
            "{} has no field {field} (it has {})",
            typed.kind,
            names.join(", ")
        ))
    })?;
    let shape = &def.from[typed.place.shape];
    let sources: &[Source] = shape.fields.get(field).map_or(&[], |s| &s.0);
    if sources.is_empty() {
        return Err(Refused(format!(
            "this {} is a {} that doesn't read {field}",
            typed.kind,
            shape.on.name()
        )));
    }
    let dataview = s.tasks.dataview || text[typed.place.text.clone()].contains("::");
    let (src, existing) = target(typed, shape, field, sources, dataview)?;
    let value = input(src, value_type, &req.value, &s.daily.format)
        .map_err(|e| Refused(format!("{field}: {e}")))?;
    let unchanged = || Set {
        text: text.to_string(),
        item: item.clone(),
        changed: false,
    };
    if typed.fields[field] == value {
        return Ok(unchanged());
    }
    let w = Writer {
        text,
        typed,
        on: shape.on,
        field,
        existing,
        value: &value,
        value_type,
        others: enum_forms_in(text, &before, &typed.kind, field),
        daily_format: &s.daily.format,
    };
    let splice = w.splice(src)?;
    let mut new = String::with_capacity(text.len() + splice.with.len());
    new.push_str(&text[..splice.range.start]);
    new.push_str(&splice.with);
    new.push_str(&text[splice.range.end..]);
    if new == text {
        return Ok(unchanged());
    }
    let item = read_back(note, &new, s, &before, k, field, &value)?;
    Ok(Set {
        text: new,
        item,
        changed: true,
    })
}

/// The source a value is written through, and the span of the value it replaces. The source
/// that gave the field its value, when it can be written. For a field with no value, a writable
/// source before that one (it is empty, or it would have given the value): of a kind the item
/// already writes its other fields with, else a marker or a Dataview field as the vault's tasks
/// are written (`dataview`), else the first.
fn target<'a>(
    typed: &Typed,
    shape: &Shape,
    field: &str,
    sources: &'a [Source],
    dataview: bool,
) -> Result<(&'a Source, Option<Range<usize>>), EditError> {
    let read_from = typed.place.sources.get(field).copied();
    let spans = typed.spans.get(field).map_or(&[][..], Vec::as_slice);
    if spans.len() > 1 {
        return Err(Refused(format!(
            "{field} is written {} times here; set-field writes one value",
            spans.len()
        )));
    }
    if let Some(j) = read_from
        && writable(&sources[j])
    {
        return Ok((&sources[j], spans.first().cloned()));
    }
    let before = read_from.unwrap_or(sources.len());
    let candidates: Vec<&Source> = sources[..before].iter().filter(|s| writable(s)).collect();
    let used: Vec<&str> = (typed.place.sources.iter())
        .filter(|(f, _)| &***f != field && !typed.fields[&***f].is_null())
        .filter_map(|(f, &j)| shape.fields.get(&**f)?.0.get(j))
        .map(Source::kind)
        .collect();
    let preferred = if dataview { "inline" } else { "marker" };
    let pick = (candidates.iter().find(|s| used.contains(&s.kind())))
        .or_else(|| candidates.iter().find(|s| s.kind() == preferred))
        .or(candidates.first());
    match pick {
        Some(s) => Ok((s, None)),
        None => {
            let s = &sources[read_from.unwrap_or(0)];
            Err(Refused(format!(
                "{field} is read from {}, which can't be written",
                describe(s)
            )))
        }
    }
}

/// Whether set-field can write through it; a fence's `info` is writable by design but isn't
/// read yet.
fn writable(s: &Source) -> bool {
    s.writable() && !matches!(s, Source::Info(_))
}

fn describe(s: &Source) -> String {
    match s {
        Source::Rest(_) => "the rest of the text".into(),
        Source::Note(_) => "the note's name or path".into(),
        Source::Section(_) => "the headings above it".into(),
        Source::Inlines(_) => "the tags or links in the text".into(),
        Source::Children(_) => "the items under it".into(),
        Source::Info(_) => "a fence's info, which isn't read yet".into(),
        s => format!("{} = …", s.kind()),
    }
}

/// The value asked for, as the field carries it.
fn input(src: &Source, v: &Value, value: &Json, daily_format: &str) -> Result<Json, String> {
    // A checkbox holds one character as written, and a space is a status.
    if let (Source::Checkbox(_), Json::String(s)) = (src, value)
        && s.chars().count() == 1
    {
        return matcher::parse_as_written(v, s, daily_format)
            .ok_or_else(|| format!("{s:?} is not a {}", v.name()));
    }
    let text = match value {
        Json::Null => return Ok(Json::Null),
        Json::String(s) if s.trim().is_empty() => return Ok(Json::Null),
        Json::String(s) => s.trim().to_string(),
        other => other.to_string(),
    };
    if let Source::Flags(map) = src {
        return match map.values().find(|n| **n == text) {
            Some(n) => Ok(Json::String(n.clone())),
            None => Err(format!(
                "{text:?} is not one of {}",
                map.values().cloned().collect::<Vec<_>>().join(", ")
            )),
        };
    }
    let not = || format!("{text:?} is not a {}", v.name());
    match v {
        Value::Enum { values } => {
            let values = values.map();
            if values.values().any(|n| *n == text) {
                Ok(Json::String(text))
            } else if let Some(n) = values.get(&text) {
                Ok(Json::String(n.clone()))
            } else {
                let mut names: Vec<&str> = values.values().map(String::as_str).collect();
                names.dedup();
                Err(format!("{text:?} is not one of {}", names.join(", ")))
            }
        }
        Value::Date { .. } => NaiveDate::parse_from_str(&text, "%Y-%m-%d")
            .ok()
            .map(|d| Json::String(d.format("%Y-%m-%d").to_string()))
            .or_else(|| matcher::parse(v, &text, daily_format))
            .ok_or_else(not),
        Value::Duration => match text.parse::<f64>() {
            Ok(n) if n >= 0.0 && n.is_finite() => Ok(matcher::number(n)),
            _ => matcher::parse(v, &text, daily_format).ok_or_else(not),
        },
        _ => matcher::parse(v, &text, daily_format).ok_or_else(not),
    }
}

/// The written forms of `field`'s enum values in the note's other items of this type, for
/// writing a value the way the note already does.
fn enum_forms_in(text: &str, items: &[Item], kind: &str, field: &str) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match &i.data {
            Data::Typed(t) if t.kind == kind => t.spans.get(field),
            _ => None,
        })
        .flatten()
        .map(|r| text[r.clone()].to_string())
        .collect()
}

/// One change to the note: `range` becomes `with`.
#[derive(Debug)]
struct Splice {
    range: Range<usize>,
    with: String,
}

fn insert(at: usize, with: String) -> Splice {
    Splice {
        range: at..at,
        with,
    }
}

struct Writer<'a> {
    text: &'a str,
    typed: &'a Typed,
    on: On,
    field: &'a str,
    /// The value being replaced.
    existing: Option<Range<usize>>,
    value: &'a Json,
    value_type: &'a Value,
    /// Forms the note already writes this field in.
    others: Vec<String>,
    daily_format: &'a str,
}

impl Writer<'_> {
    fn splice(&self, src: &Source) -> Result<Splice, EditError> {
        let field = self.field;
        let block = self.typed.place.text.clone();
        let remove = self.value.is_null();
        let existing = self.existing.clone();
        match src {
            Source::Capture(_) | Source::Pattern(_) | Source::Checkbox(_) => match existing {
                _ if remove => Err(Refused(format!(
                    "{field} is read by a pattern; taking its value out would leave the text \
                     around it, so edit the note instead"
                ))),
                Some(span) => Ok(Splice {
                    range: span,
                    with: self.written(false)?,
                }),
                None => Err(Refused(format!(
                    "{field} has no value to replace, and its pattern says where a value is \
                     read, not where to add one"
                ))),
            },
            Source::Marker(glyph) => match existing {
                Some(span) if remove => {
                    // `⏳️`: a variation selector after the glyph is part of it.
                    let lead = self.text[..span.start].trim_end_matches([' ', '\t']);
                    let lead = lead.strip_suffix('\u{FE0F}').unwrap_or(lead);
                    if !lead.ends_with(glyph.as_str()) {
                        return Err(Refused(format!("{field}'s {glyph} isn't before its value")));
                    }
                    Ok(self.cut(lead.len() - glyph.len()..span.end))
                }
                Some(span) => Ok(Splice {
                    range: span,
                    with: self.written(false)?,
                }),
                None => {
                    let value = self.written(false)?;
                    let line = &self.text[block.clone()];
                    Ok(match line.find(glyph.as_str()) {
                        Some(i) => {
                            let end = i + glyph.len();
                            let end = end + line[end..].strip_prefix('\u{FE0F}').map_or(0, |_| 3);
                            insert(block.start + end, format!(" {value}"))
                        }
                        None => insert(block.end, format!(" {glyph} {value}")),
                    })
                }
            },
            Source::Inline(key) => match existing {
                Some(span) if remove => self.cut_inline(key, span),
                Some(span) => Ok(Splice {
                    range: span,
                    with: self.written(false)?,
                }),
                None if self.on == On::Note => {
                    let nl = newline(self.text);
                    let at = block.start;
                    let gap = if at > 0 && !self.text[..at].ends_with('\n') {
                        nl
                    } else {
                        ""
                    };
                    Ok(insert(
                        at,
                        format!("{gap}{key}:: {}{nl}", self.written(false)?),
                    ))
                }
                None => {
                    let value = self.written(false)?;
                    let line = &self.text[block.clone()];
                    let field = match dataview_form(line) {
                        '(' => format!("({key}:: {value})"),
                        _ => format!("[{key}:: {value}]"),
                    };
                    Ok(insert(block.end, format!(" {field}")))
                }
            },
            Source::Flag(glyph) => match (self.value, existing) {
                (Json::Bool(true), None) => Ok(insert(block.end, format!(" {glyph}"))),
                (Json::Bool(false), Some(span)) => Ok(self.cut(span)),
                (Json::Bool(_), _) => Ok(insert(block.end, String::new())),
                _ => Err(Refused(format!("{field} is a flag: it is true or false"))),
            },
            Source::Flags(map) => {
                let glyph = || {
                    let name = self.value.as_str().unwrap_or_default();
                    map.iter()
                        .find(|(_, n)| *n == name)
                        .map(|(g, _)| g.clone())
                        .expect("input() checked the name")
                };
                match existing {
                    Some(span) if remove => Ok(self.cut(span)),
                    Some(span) => Ok(Splice {
                        range: span,
                        with: glyph(),
                    }),
                    None => Ok(insert(block.end, format!(" {}", glyph()))),
                }
            }
            Source::Column(column) => {
                let cell = self
                    .typed
                    .place
                    .cells
                    .iter()
                    .find(|(h, _)| h == column)
                    .map(|(_, c)| c.clone())
                    .ok_or_else(|| Refused(format!("this row has no cell under {column}")))?;
                let value = if remove {
                    String::new()
                } else {
                    self.written(false)?
                };
                if value.contains(['|', '\n']) {
                    return Err(Refused(format!(
                        "{field}: a table cell can't hold {value:?}"
                    )));
                }
                Ok(fill_cell(self.text, cell, &value))
            }
            Source::Property(key) => self.property(key),
            _ => Err(Refused(format!(
                "{field} is read from {}, which can't be written",
                describe(src)
            ))),
        }
    }

    /// The new value as text, written the way the value it replaces was.
    fn written(&self, yaml: bool) -> Result<String, EditError> {
        let field = self.field;
        let old = self.existing.clone().map(|r| &self.text[r]);
        let s = self.value.as_str().unwrap_or_default();
        let text = match self.value_type {
            Value::Text => s.to_string(),
            Value::Number => self.value.to_string(),
            Value::Date { format } => {
                let format = format.as_deref().unwrap_or("YYYY-MM-DD");
                let format = format.replace("${daily.format}", self.daily_format);
                let date = NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("input() made it");
                daily::format_date(date, &format)
            }
            Value::Datetime { format: Some(_) } => {
                return Err(Refused(format!(
                    "{field} is a datetime with a format, which isn't read yet"
                )));
            }
            Value::Datetime { format: None } => {
                let t =
                    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").expect("input() made it");
                let sep = if old.is_some_and(|o| o.contains(' ')) {
                    ' '
                } else {
                    'T'
                };
                let seconds = t.second() != 0 || old.is_some_and(|o| o.matches(':').count() > 1);
                let time = if seconds { "%H:%M:%S" } else { "%H:%M" };
                t.format(&format!("%Y-%m-%d{sep}{time}")).to_string()
            }
            Value::Bool => {
                let b = self.value.as_bool().unwrap_or_default();
                let yes_no = old.is_some_and(|o| matches!(o.to_lowercase().as_str(), "yes" | "no"));
                match (yes_no, b) {
                    (true, true) => "yes",
                    (true, false) => "no",
                    (false, true) => "true",
                    (false, false) => "false",
                }
                .to_string()
            }
            Value::Link => match old {
                Some(o) if !o.starts_with("[[") && o.starts_with('[') && o.contains("](") => {
                    let original = o.split_once("](").map_or("", |(_, t)| t);
                    format!("[{s}]({}.md)", vault::encode_target_like(s, original))
                }
                Some(o) if !o.starts_with('[') => s.to_string(),
                _ => format!("[[{s}]]"),
            },
            Value::Tag => match old {
                Some(o) if o.starts_with('#') => format!("#{s}"),
                Some(_) => s.to_string(),
                None if yaml => s.to_string(),
                None => format!("#{s}"),
            },
            Value::Enum { values } => {
                let values = values.map();
                let forms: Vec<&str> = values
                    .iter()
                    .filter(|(_, n)| *n == s)
                    .map(|(w, _)| w.as_str())
                    .collect();
                [old.map(str::to_string)]
                    .into_iter()
                    .flatten()
                    .chain(self.others.iter().cloned())
                    .find(|w| forms.contains(&w.as_str()))
                    .unwrap_or_else(|| forms[0].to_string())
            }
            Value::Duration => duration(self.value.as_f64().unwrap_or_default(), old),
        };
        if text.contains('\n') {
            return Err(Refused(format!("{field} can't hold a line break")));
        }
        Ok(text)
    }

    /// `range` taken out, with one space beside it (the one before, else the one after).
    fn cut(&self, range: Range<usize>) -> Splice {
        let t = self.text;
        let (mut start, mut end) = (range.start, range.end);
        if t[..start].ends_with([' ', '\t']) {
            start -= 1;
        } else if t[end..].starts_with([' ', '\t']) {
            end += 1;
        }
        Splice {
            range: start..end,
            with: String::new(),
        }
    }

    /// A Dataview field taken out whole: `[key:: v]` and `(key:: v)` with their brackets,
    /// `key:: v` to the end of its line, and the line too when nothing else is on it.
    fn cut_inline(&self, key: &str, span: Range<usize>) -> Result<Splice, EditError> {
        let t = self.text;
        let lead = t[..span.start].trim_end_matches([' ', '\t']);
        let name = format!("{key}::");
        let Some(start) = lead.strip_suffix(name.as_str()).map(str::len) else {
            return Err(Refused(format!(
                "{}: no {name} before its value",
                self.field
            )));
        };
        if let Some(open) = t[..start]
            .chars()
            .next_back()
            .filter(|c| matches!(c, '[' | '('))
        {
            let close = if open == '[' { ']' } else { ')' };
            let after = &t[span.end..];
            let gap = after.len() - after.trim_start_matches([' ', '\t']).len();
            if !after[gap..].starts_with(close) {
                return Err(Refused(format!(
                    "{}: no {close} after its value",
                    self.field
                )));
            }
            return Ok(self.cut(start - 1..span.end + gap + 1));
        }
        let line_start = t[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = t[span.end..].find('\n').map_or(t.len(), |i| span.end + i);
        if t[line_start..start].trim().is_empty() {
            let end = (line_end + 1).min(t.len());
            return Ok(Splice {
                range: line_start..end,
                with: String::new(),
            });
        }
        let end = span.end + t[span.end..line_end].trim_end().len();
        Ok(self.cut(start..end.max(span.end)))
    }

    /// A frontmatter property set, added or taken out, line by line.
    fn property(&self, key: &str) -> Result<Splice, EditError> {
        let field = self.field;
        let t = self.text;
        let fm = frontmatter::read(t);
        let found = fm.as_ref().and_then(|f| f.get(key));
        let nl = newline(t);
        match found {
            Some(frontmatter::Value::Nested) => {
                return Err(Refused(format!(
                    "{key} is nested YAML, which cold-lava doesn't rewrite"
                )));
            }
            Some(frontmatter::Value::List(l)) if l.len() > 1 => {
                return Err(Refused(format!(
                    "{key} lists {} values; set-field writes one",
                    l.len()
                )));
            }
            _ => {}
        }
        if self.value.is_null() {
            let lines = frontmatter::property_lines(t, key)
                .ok_or_else(|| Refused(format!("{key} isn't in the frontmatter")))?;
            return Ok(Splice {
                range: lines,
                with: String::new(),
            });
        }
        let value = self.written(true)?;
        match found {
            Some(frontmatter::Value::Scalar(_) | frontmatter::Value::List(_))
                if let Some(span) = self.existing.clone() =>
            {
                let quoted = t[..span.start].ends_with(['"', '\'']);
                Ok(Splice {
                    range: span,
                    with: if quoted { value } else { yaml(&value)? },
                })
            }
            Some(frontmatter::Value::Empty) => {
                let lines = frontmatter::property_lines(t, key).expect("the key is there");
                let line = t[lines.clone()].lines().next().unwrap_or_default();
                let end = lines.start + line.trim_end().len();
                Ok(insert(end, format!(" {}", yaml(&value)?)))
            }
            Some(_) => Err(Refused(format!(
                "{field}: {key} is there but its value isn't read"
            ))),
            None => {
                let line = format!("{key}: {}{nl}", yaml(&value)?);
                Ok(match &fm {
                    Some(fm) => {
                        let at = t
                            .split_inclusive('\n')
                            .take(fm.end_line - 1)
                            .map(str::len)
                            .sum();
                        insert(at, line)
                    }
                    None => insert(0, format!("---{nl}{line}---{nl}")),
                })
            }
        }
    }
}

/// A scalar as YAML reads it back: plain when it can be, else quoted.
fn yaml(value: &str) -> Result<String, EditError> {
    if !frontmatter::needs_quotes(value) {
        Ok(value.to_string())
    } else if !value.contains(['"', '\\']) {
        Ok(format!("\"{value}\""))
    } else if !value.contains('\'') {
        Ok(format!("'{value}'"))
    } else {
        Err(Refused(format!(
            "{value:?} has both kinds of quote, which a property can't hold as written"
        )))
    }
}

/// The bracket the block's other Dataview fields use: `(` for `(key:: v)`, else `[`.
fn dataview_form(text: &str) -> char {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"([\[(])[^\[\]()\n]*::").unwrap());
    re.captures(text)
        .and_then(|c| c[1].chars().next())
        .unwrap_or('[')
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// A table cell's content replaced, keeping the cell as wide as it was when the value fits, so
/// the table's other rows don't need padding again.
fn fill_cell(text: &str, cell: Range<usize>, value: &str) -> Splice {
    let line_start = text[..cell.start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[cell.start..]
        .find('\n')
        .map_or(text.len(), |i| cell.start + i);
    let open = text[line_start..cell.start]
        .rfind('|')
        .map_or(line_start, |i| line_start + i + 1);
    let close = text[cell.end..line_end]
        .find('|')
        .map_or(cell.end, |i| cell.end + i);
    let inside = &text[open..close];
    let lead = inside.len() - inside.trim_start().len();
    let trail = inside.len() - inside.trim_end().len();
    let width = inside.chars().count();
    let value_width = value.chars().count();
    let lead = if lead + value_width + trail.min(1) <= width {
        lead
    } else {
        lead.min(1)
    };
    let pad = width.saturating_sub(lead + value_width).max(trail.min(1));
    Splice {
        range: open..close,
        with: format!("{}{value}{}", " ".repeat(lead), " ".repeat(pad)),
    }
}

/// Minutes as `1h30m`: whole minutes alone when the old value was written that way (`90m`).
fn duration(minutes: f64, old: Option<&str>) -> String {
    let secs = (minutes * 60.0).round() as i64;
    if secs % 60 == 0
        && old.is_some_and(|o| {
            o.strip_suffix('m')
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
    {
        return format!("{}m", secs / 60);
    }
    let days = old.is_some_and(|o| o.contains('d'));
    let mut rest = secs;
    let mut out = String::new();
    for (unit, size) in [('d', 86_400), ('h', 3_600), ('m', 60), ('s', 1)] {
        if unit == 'd' && !days {
            continue;
        }
        let n = rest / size;
        rest %= size;
        if n > 0 {
            out.push_str(&format!("{n}{unit}"));
        }
    }
    if out.is_empty() { "0m".into() } else { out }
}

/// The note read again: the same item (by place; its id changes with its key) must have
/// exactly the new value and its other fields, and every other item must be unchanged but for
/// what follows from the edit: a new parent id, a renamed heading above it.
fn read_back(
    note: &str,
    text: &str,
    s: &Settings,
    before: &[Item],
    k: usize,
    field: &str,
    value: &Json,
) -> Result<Item, EditError> {
    let after = items::extract(note, text, s);
    if after.len() != before.len() {
        return Err(ReadBack(format!(
            "it would have {} items instead of {}",
            after.len(),
            before.len()
        )));
    }
    let old = &before[k];
    // The same item: by id, else (its key changed) the one of its kind that starts on its line,
    // the nth of them as before. Its edit never moves where it starts.
    let at_line = |items: &[Item]| -> Vec<usize> {
        (0..items.len())
            .filter(|&i| items[i].line == old.line && items[i].data.kind() == old.data.kind())
            .collect()
    };
    let nth = at_line(before).iter().position(|&i| i == k);
    let j = after
        .iter()
        .position(|a| a.id == old.id)
        .or_else(|| at_line(&after).get(nth?).copied())
        .ok_or_else(|| ReadBack("the item would not be there any more".into()))?;
    let new = &after[j];
    let (Data::Typed(was), Data::Typed(is)) = (&old.data, &new.data) else {
        return Err(ReadBack(format!(
            "line {} would read as a {}",
            new.line,
            new.data.kind()
        )));
    };
    if is.kind != was.kind {
        return Err(ReadBack(format!(
            "it would read as a {} instead of a {}",
            is.kind, was.kind
        )));
    }
    if is.fields[field] != *value {
        return Err(ReadBack(format!(
            "{field} would read {} instead of {value}",
            is.fields[field]
        )));
    }
    // A field read from the very text that was set (a status's type, from its checkbox) may
    // change with it.
    let shares_text = |name: &str| {
        was.spans
            .get(name)
            .is_some_and(|s| Some(s) == was.spans.get(field))
    };
    if let Some((name, v)) = was
        .fields
        .iter()
        .map(|(name, v)| (&**name, v))
        .find(|&(name, v)| name != field && is.fields[name] != *v && !shares_text(name))
    {
        return Err(ReadBack(format!(
            "{name} would change too, from {v} to {}",
            is.fields[name]
        )));
    }
    let renamed = |id: &Option<String>| -> Option<String> {
        let id = id.as_ref()?;
        if *id == old.id {
            return Some(new.id.clone());
        }
        let i = before.iter().position(|b| &b.id == id)?;
        let i = i - (i > k) as usize;
        let i = i + (i >= j) as usize;
        Some(after[i].id.clone())
    };
    // A heading's own text is the section of what is under it; setting a field written in it
    // renames that one section, the same way for every item.
    let mut section_change: Option<(String, String)> = None;
    let others = |items: &'_ [Item], skip: usize| -> Vec<usize> {
        (0..items.len()).filter(|&i| i != skip).collect()
    };
    for (bi, ai) in others(before, k).into_iter().zip(others(&after, j)) {
        let (b, a) = (&before[bi], &after[ai]);
        let mut ok = same_data(&b.data, &a.data)
            && b.block_id == a.block_id
            && renamed(&b.parent) == a.parent;
        let resectioned = b.section != a.section;
        if resectioned {
            let diff: Vec<(&String, &String)> = b
                .section
                .iter()
                .zip(&a.section)
                .filter(|(x, y)| x != y)
                .collect();
            ok &= b.section.len() == a.section.len() && diff.len() == 1 && {
                let pair = (diff[0].0.clone(), diff[0].1.clone());
                *section_change.get_or_insert_with(|| pair.clone()) == pair
            };
        }
        let parent_moved = b.parent.is_some() && renamed(&b.parent) != b.parent;
        ok &= b.id == a.id || resectioned || parent_moved;
        if !ok {
            return Err(ReadBack(format!(
                "the {} on line {} would change too",
                b.data.kind(),
                a.line
            )));
        }
    }
    Ok(new.clone())
}

/// The same item content: a typed item's kind and fields, not where they are.
fn same_data(a: &Data, b: &Data) -> bool {
    match (a, b) {
        (Data::Typed(a), Data::Typed(b)) => {
            a.kind == b.kind && a.is == b.is && a.fields == b.fields
        }
        (a, b) => a == b,
    }
}

#[cfg(test)]
mod tests;
