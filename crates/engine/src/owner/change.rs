//! `items/edit` on an item an owner gave as a piece, or as a typed item, said as changes in the
//! model's terms (docs/neutral-model.md, "The owner's side"): set an attribute, replace a
//! piece's text, set a typed item's field. Nothing here writes or reads Markdown, and nothing
//! is written at all: the owner makes its file's new text from the changes (`file/edit`), and
//! the engine reads that back before it writes (`owner/edits.rs`).
//!
//! A field is written where its type reads it. The source that gave the value, else the first
//! the type has that a piece can hold, says which change it is:
//!
//! - `attribute = "due"`, and `checkbox = true` and `inline = "due"`, which read the attribute
//!   on a piece: `{place, attribute, value}`;
//! - `rest`, the piece's words: `{place, text}`, its new runs (`given::reword`);
//! - `column = "VEI"`: `{place, cell, text}`, the cell's new runs.
//!
//! Every other source is derived (a label, a link, the file's name) and is refused.
//!
//! New words replace a piece's text whole, so they are refused when another field of the item
//! has its value from those words (a `marker`, a `pattern`, a `flag`): the edit would drop
//! that value without saying so.

use crate::gateway::{Error, Op, Reason};
use crate::items::{Data, Item, Settings};
use crate::matcher::Typed;
use crate::owner::given::{CHECKBOX, Change, Piece, clears, reword};
use crate::tasks::StatusType;
use crate::types::{Source, Value};
use serde_json::Value as Json;

/// The changes an edit asks of an owner, and what the edit did, for `Event::ItemEdited`.
pub struct Made {
    pub changes: Vec<Change>,
    pub what: String,
}

fn refused(message: String) -> Error {
    Error::new(Reason::Refused, message)
}

/// What `set-field` says it did.
fn what(field: &str, value: &Json) -> String {
    match clears(value) {
        true => format!("removed {field}"),
        false => format!("set {field}"),
    }
}

/// The task `item` is and its status symbol as it is now, for a tick: refused when it is no
/// task, has no status, or recurs. Ticking a recurring task adds its next occurrence, a new
/// piece, and nothing adds a piece yet.
fn ticked(item: &Item) -> Result<(&Typed, char), Error> {
    let typed: &Typed = match &item.data {
        Data::Typed(typed) if item.data.answers_to("task") => typed,
        other => {
            let message = format!("{} is a {}, not a task", item.id, other.kind());
            return Err(refused(message));
        }
    };
    let field = |name: &str| typed.fields.get(name).filter(|v| !v.is_null());
    if field("recurrence").is_some() {
        return Err(refused(format!(
            "{} recurs, and ticking it would add its next occurrence: nothing can add an item \
             to a file another owner reads yet. Tick it in {}",
            item.id, item.note
        )));
    }
    let symbol = field("status")
        .and_then(Json::as_str)
        .and_then(|status| status.chars().next());
    let symbol = symbol.ok_or_else(|| {
        let message = format!("{} has no status to tick: set one with set-field", item.id);
        refused(message)
    })?;
    Ok((typed, symbol))
}

/// The word a piece's `checkbox` says for a status symbol of the workspace.
fn word_of(symbol: char, s: &Settings) -> Option<&'static str> {
    let status = s.tasks.status_type(symbol);
    let word = CHECKBOX.iter().find(|(_, t)| *t == status);
    word.map(|(word, _)| *word)
}

/// The status a tick moves a piece's task to. A piece's checkbox says one of four words, so a
/// tick follows the workspace's statuses (the Tasks plugin's `nextStatusSymbol`) to the first
/// one that is another word: a next status of the same kind (`!`, another open one) would
/// change nothing in the piece and is passed over. A next status that is no kind a checkbox
/// can say is refused, and names the setting.
fn next_for_a_piece(symbol: char, s: &Settings) -> Result<char, String> {
    let from = word_of(symbol, s);
    let mut at = symbol;
    for _ in 0..=s.tasks.statuses.len() {
        let next = s.tasks.next_symbol(at);
        match word_of(next, s) {
            None => {
                return Err(format!(
                    "the workspace's Tasks settings send the status {at:?} on to {next:?}, \
                     which is none of the kinds a piece's checkbox can say (open, done, \
                     in-progress, cancelled): set its status with set-field, or change that \
                     status's next symbol"
                ));
            }
            Some(word) if Some(word) != from => return Ok(next),
            Some(_) => at = next,
        }
    }
    Err(format!(
        "every status the workspace's Tasks settings go to from {symbol:?} is the same kind, \
         so a tick would change nothing: set its status with set-field"
    ))
}

/// `edit` on the item `item` that the piece `p` is.
pub fn piece(p: &Piece, item: &Item, op: &Op, s: &Settings) -> Result<Made, Error> {
    match op {
        Op::Toggle => {
            let (typed, now) = ticked(item)?;
            let next = next_for_a_piece(now, s).map_err(refused)?;
            let next = Json::String(next.to_string());
            let changes = vec![set(p, typed, "status", &next, s)?];
            let what = "ticked".to_string();
            Ok(Made { changes, what })
        }
        Op::SetField { field, value } => {
            let Data::Typed(typed) = &item.data else {
                return Err(refused(format!(
                    "{} is a {} no type reads, so it has no field {field}",
                    item.id,
                    item.data.kind()
                )));
            };
            let changes = vec![set(p, typed, field, value, s)?];
            let what = what(field, value);
            Ok(Made { changes, what })
        }
    }
}

/// The change that sets `field` of the item `typed`, read from the piece `p`, to `value`.
fn set(p: &Piece, typed: &Typed, field: &str, value: &Json, s: &Settings) -> Result<Change, Error> {
    let kind = &typed.kind;
    let definition = (s.types.types.iter()).find(|t| t.name == *kind);
    let definition = definition.map(|t| &t.definition);
    let shape = definition.and_then(|d| d.from.get(typed.place.shape));
    let value_type = definition.and_then(|d| d.fields.get(field));
    let (Some(shape), Some(value_type)) = (shape, value_type) else {
        return Err(refused(format!("{kind} has no field {field}")));
    };
    let sources = shape.fields.get(field).map_or(&[][..], |s| s.0.as_slice());
    let holds = |s: &Source| {
        matches!(
            s,
            Source::Attribute(_)
                | Source::Checkbox(_)
                | Source::Inline(_)
                | Source::Rest(_)
                | Source::Column(_)
        )
    };
    let read_from = (typed.place.sources.iter()).find_map(|(f, i)| (&**f == field).then_some(*i));
    let source = match read_from.and_then(|i| sources.get(i)) {
        Some(source) => Some(source).filter(|s| holds(s)),
        None => sources.iter().find(|s| holds(s)),
    };
    let Some(source) = source else {
        return Err(refused(format!(
            "{field} of a {kind} is read from what a piece doesn't hold as a value (its labels, \
             its links, its file or a pattern over its words), so it can't be set"
        )));
    };
    // The other fields of the item whose values are read from the piece's words: new words
    // would take them away.
    let in_words = |s: &Source| {
        matches!(
            s,
            Source::Pattern(_) | Source::Capture(_) | Source::Marker(_) | Source::Flags(_)
        )
    };
    let from_words: Vec<&str> = (typed.place.sources.iter())
        .filter(|(name, _)| &**name != field)
        .filter(|(name, i)| {
            let source = shape.fields.get(&**name).and_then(|s| s.0.get(*i));
            let value = typed.fields.get(name);
            let set = value.is_some_and(|v| !v.is_null() && *v != Json::Bool(false));
            let flag = matches!(source, Some(Source::Flag(_)));
            set && (flag || source.is_some_and(in_words))
        })
        .map(|(name, _)| &**name)
        .collect();
    let keeps_words = |what: &str| match from_words.is_empty() {
        true => Ok(()),
        false => Err(refused(format!(
            "{field} is {what}, and {} of this {kind} {} read from those words: new words \
             would drop {}. Change the words in the file itself",
            from_words.join(", "),
            if from_words.len() == 1 { "is" } else { "are" },
            if from_words.len() == 1 { "it" } else { "them" },
        ))),
    };
    let place = p.place.clone();
    let format = &s.daily.format;
    let text_of = |value: &Json| match value {
        Json::Null => String::new(),
        Json::String(text) => text.trim().to_string(),
        other => other.to_string(),
    };
    match source {
        Source::Attribute(name) | Source::Inline(name) if name != "checkbox" => {
            let value = match value_type {
                // A link an owner holds is its target as written: `[[x]]` in the value is
                // characters, not a Markdown link to read.
                Value::Link => match text_of(value) {
                    text if text.is_empty() => Json::Null,
                    text => Json::String(text),
                },
                _ => crate::edit::input(source, value_type, value, format).map_err(refused)?,
            };
            let attribute = name.clone();
            Ok(Change::Attribute {
                place,
                attribute,
                value,
            })
        }
        Source::Attribute(_) | Source::Inline(_) | Source::Checkbox(_) => {
            let word = checkbox_word(value, s).ok_or_else(|| {
                let words: Vec<&str> = CHECKBOX.iter().map(|(w, _)| *w).collect();
                refused(format!(
                    "{} is not a status a piece's checkbox can say: {}, or a status symbol of \
                     one of those kinds",
                    value,
                    words.join(", ")
                ))
            })?;
            Ok(Change::Attribute {
                place,
                attribute: "checkbox".to_string(),
                value: Json::String(word.to_string()),
            })
        }
        Source::Rest(_) if !p.cells.is_empty() => Err(refused(format!(
            "{field} is all of a row's words: set the field of the cell instead"
        ))),
        Source::Rest(_) => {
            keeps_words("the piece's words")?;
            Ok(Change::Runs {
                place,
                cell: None,
                text: reword(&p.text, &text_of(value)),
            })
        }
        Source::Column(name) => {
            let cell = p.cells.iter().find(|c| c.name == *name).ok_or_else(|| {
                refused(format!(
                    "{field} is read from the cell {name}, which this row doesn't have"
                ))
            })?;
            keeps_words("a cell of the row's words")?;
            let text = text_of(value);
            let reads = crate::matcher::parse(value_type, &text, format).is_some();
            if !text.is_empty() && !reads {
                return Err(refused(format!("{text:?} is not a {}", value_type.name())));
            }
            Ok(Change::Runs {
                place,
                cell: Some(name.clone()),
                text: reword(&cell.text, &text),
            })
        }
        _ => unreachable!("`holds` passed it"),
    }
}

/// The word a piece's `checkbox` says for `value`: one of the words, a status type's name
/// (`done`, `in_progress`) or a status symbol of the workspace (`x`, a space).
fn checkbox_word(value: &Json, s: &Settings) -> Option<&'static str> {
    let text = value.as_str()?;
    let mut chars = text.chars();
    let status = match (chars.next(), chars.next()) {
        (Some(symbol), None) => s.tasks.status_type(symbol),
        _ => {
            let text = text.trim();
            if let Some((word, _)) = CHECKBOX.iter().find(|(w, _)| *w == text) {
                return Some(*word);
            }
            let named = [
                StatusType::Todo,
                StatusType::Done,
                StatusType::InProgress,
                StatusType::Cancelled,
            ];
            named.into_iter().find(|t| t.name() == text)?
        }
    };
    let word = CHECKBOX.iter().find(|(_, t)| *t == status);
    word.map(|(word, _)| *word)
}

/// `edit` on a typed item an owner gave at `place`. A field is set as asked. A tick sets the
/// task's `status`, and its `status_type` when it has one, to the next status symbol, in the
/// form the type reads both.
pub fn typed(place: &str, item: &Item, op: &Op, s: &Settings) -> Result<Made, Error> {
    let place = place.to_string();
    match op {
        Op::SetField { field, value } => {
            let (field, value) = (field.clone(), value.clone());
            let what = what(&field, &value);
            let changes = vec![Change::Field {
                place,
                field,
                value,
            }];
            Ok(Made { changes, what })
        }
        Op::Toggle => {
            let (typed, now) = ticked(item)?;
            let next = s.tasks.next_symbol(now);
            let has = |name: &str| typed.fields.get(name).is_some_and(|v| !v.is_null());
            let fields = ["status", "status_type"].into_iter().filter(|f| has(f));
            let changes = fields
                .map(|field| Change::Field {
                    place: place.clone(),
                    field: field.to_string(),
                    value: Json::String(next.to_string()),
                })
                .collect();
            let what = "ticked".to_string();
            Ok(Made { changes, what })
        }
    }
}
