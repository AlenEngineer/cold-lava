//! The list view's words: the "list / tree" of the view vocabulary (docs/extensions.md,
//! "Views"). The sender says what the groups and rows are, and hears `Event`s about rows by
//! id. It knows nothing of tasks: a checkbox is a state to draw and a key to report. Every type
//! derives serde; a frontend draws it its own way.

use cold_lava_engine::types::Look;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct List {
    pub title: String,
    pub groups: Vec<Group>,
    /// Keys the sender gives a meaning to. The view's own keys come first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<KeyCommand>,
    /// What an empty list says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<String>,
    #[serde(default, skip_serializing_if = "Filter::is_slash")]
    pub filter: Filter,
}

/// How a view starts filtering: with `/` (the default), or as soon as you type, for a view
/// whose whole job is picking a row. Typing then takes every character, so the view's letter
/// keys and the sender's keys are out of reach; the arrows, Enter and Esc still work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Filter {
    #[default]
    Slash,
    Typing,
}

impl Filter {
    pub fn is_slash(&self) -> bool {
        *self == Filter::Slash
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub title: String,
    /// Folded when it first shows; folding it by hand then wins.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    pub rows: Vec<Row>,
}

/// One row: an id the sender knows it by, its text, and what goes beside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub text: Vec<Piece>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    /// Drawn at the right edge: a date, a note's name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub right: Vec<Piece>,
    /// How far it is nested under the row before it.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub depth: usize,
    /// A look over the whole row (a done task's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
}

/// Text with a look of its own: a priority's chip, an overdue date.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Piece {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<Look>,
}

impl Piece {
    pub fn plain(text: impl Into<String>) -> Piece {
        Piece {
            text: text.into(),
            look: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Check {
    Unchecked,
    Partial,
    Checked,
    Crossed,
}

/// A key the sender binds: pressing it sends `Event::Command` with the selected row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyCommand {
    /// One character.
    pub key: char,
    pub command: String,
    pub title: String,
}

/// What the view tells its sender.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum Event {
    /// Enter on a row.
    Activated { row: String },
    /// `x` on a row with a checkbox.
    Toggled { row: String },
    /// One of the sender's keys.
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        row: Option<String>,
    },
}

fn is_false(b: &bool) -> bool {
    !b
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_list_json_is_pinned() {
        let mut l = List {
            title: "agenda".into(),
            groups: vec![Group {
                id: "today".into(),
                title: "Today".into(),
                collapsed: false,
                rows: vec![Row {
                    id: "a".into(),
                    text: vec![Piece::plain("Pay rent")],
                    check: Some(Check::Unchecked),
                    right: vec![Piece::plain("Journal")],
                    depth: 0,
                    look: None,
                }],
            }],
            keys: vec![KeyCommand {
                key: 'd',
                command: "show-done".into(),
                title: "show done".into(),
            }],
            empty: None,
            filter: Default::default(),
        };
        assert_eq!(
            serde_json::to_value(&l).unwrap(),
            json!({
                "title": "agenda",
                "groups": [{
                    "id": "today",
                    "title": "Today",
                    "rows": [{
                        "id": "a",
                        "text": [{"text": "Pay rent"}],
                        "check": "unchecked",
                        "right": [{"text": "Journal"}],
                    }],
                }],
                "keys": [{"key": "d", "command": "show-done", "title": "show done"}],
            })
        );
        l.filter = Filter::Typing;
        assert_eq!(serde_json::to_value(&l).unwrap()["filter"], "typing");
        let e = Event::Command {
            command: "show-done".into(),
            row: None,
        };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({"event": "command", "command": "show-done"})
        );
        assert_eq!(
            serde_json::to_value(Event::Toggled { row: "a".into() }).unwrap(),
            json!({"event": "toggled", "row": "a"})
        );
    }
}
