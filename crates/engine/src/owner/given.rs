//! What an owner in another process gives for one of its files (`file/read`), and how the engine
//! reads that into the workspace's items and links. A Markdown piece goes through the Markdown
//! owner, so the built-in `task` and the user's types match in it exactly as in a note; a typed
//! item is checked against its type. Each item's id comes from its file, its kind and the owner's
//! place for it, so it is the same in every process and after a restart.

use crate::gateway;
use crate::items::{Data, Item, Settings};
use crate::owner::markdown;
use crate::tasks::{FNV_START, fnv_from, line_id};
use crate::workspace::Ref;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as Json};
use std::collections::HashMap;

/// The answer to `file/read`: the file's items and links.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Given {
    #[serde(default)]
    pub items: Vec<GivenItem>,
    #[serde(default)]
    pub links: Vec<GivenLink>,
}

/// One item, at a place in the file the owner keeps stable (a row's key, a card's id).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum GivenItem {
    /// Markdown text, read as a note is: a task in it is a `task`.
    Markdown { place: String, text: String },
    /// An item of a loaded type, its fields by name.
    Typed {
        place: String,
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        fields: Map<String, Json>,
    },
}

/// A link outside the Markdown pieces (those are read with them): its target as written, as
/// `[[target]]` would name it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GivenLink {
    pub target: String,
    pub place: String,
    /// What it says around the link, for backlinks.
    #[serde(default)]
    pub text: String,
}

/// One change `file/edit` asks of an owner, at one of the places it gave.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Change {
    /// A Markdown piece's new text, which the engine made as it edits a note.
    Text { place: String, text: String },
    /// A typed item's field, in the form items carry it or as text its type reads; `null`
    /// removes it.
    Field {
        place: String,
        field: String,
        value: Json,
    },
    /// A link the owner reported: its target as written, and what it becomes (a rename).
    Link {
        place: String,
        link: String,
        to: String,
    },
}

/// What the owner should give for the file once `changes` are made to what it gave.
pub fn apply(given: &Given, changes: &[Change]) -> Given {
    let mut out = given.clone();
    for c in changes {
        match c {
            Change::Text { place, text } => {
                for item in &mut out.items {
                    if let GivenItem::Markdown { place: p, text: t } = item
                        && p == place
                    {
                        *t = text.clone();
                    }
                }
            }
            Change::Field {
                place,
                field,
                value,
            } => {
                for item in &mut out.items {
                    if let GivenItem::Typed {
                        place: p, fields, ..
                    } = item
                        && p == place
                    {
                        let removes =
                            value.is_null() || value.as_str().is_some_and(|v| v.trim().is_empty());
                        match removes {
                            true => fields.remove(field),
                            false => fields.insert(field.clone(), value.clone()),
                        };
                    }
                }
            }
            Change::Link { place, link, to } => {
                for l in &mut out.links {
                    if l.place == *place && l.target == *link {
                        l.target = to.clone();
                    }
                }
            }
        }
    }
    out
}

/// Whether what an owner gave reads as what was asked of it: the same items (their kinds and
/// fields, wherever they are and whatever their ids) and the same link targets. Places may
/// differ, since an owner's place can be part of what changed (a CSV row's first cell). Else
/// what differs, for a message.
pub fn same(asked: &Read, gave: &Read) -> Result<(), String> {
    let items = |r: &Read| -> Vec<String> {
        let mut v: Vec<String> = r.items.iter().map(data_key).collect();
        v.sort();
        v
    };
    let links = |r: &Read| -> Vec<String> {
        let mut v: Vec<String> = r.links.iter().map(|(l, _)| l.target.clone()).collect();
        v.sort();
        v
    };
    let only = |a: &[String], b: &[String]| -> Vec<String> {
        let mut b = b.to_vec();
        let mut out = Vec::new();
        for x in a {
            match b.iter().position(|y| y == x) {
                Some(i) => drop(b.remove(i)),
                None => out.push(x.clone()),
            }
        }
        out
    };
    let short = |v: Vec<String>| -> String {
        let s = v.join(", ");
        match s.chars().count() > 200 {
            true => format!("{}…", s.chars().take(200).collect::<String>()),
            false => s,
        }
    };
    let (want, got) = (items(asked), items(gave));
    if want != got {
        let (missing, extra) = (only(&want, &got), only(&got, &want));
        return Err(match (missing.is_empty(), extra.is_empty()) {
            (false, false) => format!("{} where {} was asked", short(extra), short(missing)),
            (false, true) => format!("{} is missing", short(missing)),
            _ => format!("{} wasn't asked for", short(extra)),
        });
    }
    let (want, got) = (links(asked), links(gave));
    if want != got {
        let (missing, extra) = (only(&want, &got), only(&got, &want));
        return Err(format!(
            "links to {} where links to {} were asked",
            short(extra),
            short(missing)
        ));
    }
    Ok(())
}

/// Whether two items say the same: their kinds and fields, not where they are.
pub fn same_data(a: &Item, b: &Item) -> bool {
    data_key(a) == data_key(b)
}

/// An item's kind and fields as JSON, without where they were read.
fn data_key(item: &Item) -> String {
    serde_json::to_string(&item.data).unwrap_or_default()
}

/// A file's items and links, read from what its owner gave, with the owner's place for each.
#[derive(Default)]
pub struct Read {
    pub items: Vec<Item>,
    /// By item id.
    pub places: HashMap<String, String>,
    pub links: Vec<(Ref, String)>,
    /// What didn't read, one line each: a type that isn't loaded, a field it doesn't have.
    pub problems: Vec<String>,
}

/// Read what the owner gave for the file at workspace-relative `file`.
pub fn read(file: &str, given: &Given, s: &Settings) -> Read {
    let mut out = Read::default();
    for item in &given.items {
        match item {
            GivenItem::Markdown { place, text } => {
                let key = format!("{file}\0{place}");
                let lines: Vec<&str> = text.lines().collect();
                for mut item in markdown::items(&key, text, s) {
                    let line = lines.get(item.line - 1).copied().unwrap_or_default();
                    item = gateway::answer_line(s, line, &item).into_owned();
                    item.note = file.to_string();
                    out.places.insert(item.id.clone(), place.clone());
                    out.items.push(item);
                }
                let refs = markdown::read(text).refs.into_iter();
                out.links.extend(refs.map(|r| (r, place.clone())));
            }
            GivenItem::Typed {
                place,
                kind,
                fields,
            } => match s.matcher.given(kind, fields) {
                Ok(typed) => {
                    let id = [file, "\0", kind, "\0", place]
                        .iter()
                        .fold(FNV_START, |h, part| fnv_from(h, part.as_bytes()));
                    let id = format!("{id:016x}");
                    out.places.insert(id.clone(), place.clone());
                    out.items.push(Item {
                        id,
                        note: file.to_string(),
                        line: 1,
                        end_line: 1,
                        section: Vec::new(),
                        parent: None,
                        block_id: None,
                        rev: line_id(&Json::Object(fields.clone()).to_string()),
                        data: Data::Typed(Box::new(typed)),
                        attributes: Default::default(),
                    });
                }
                Err(e) => out.problems.push(format!("{place}: {e}")),
            },
        }
    }
    for l in &given.links {
        let target = crate::workspace::clean_target(&l.target);
        let r = target.map(|target| Ref {
            target,
            line: 1,
            context: l.text.trim().chars().take(240).collect(),
        });
        out.links.extend(r.map(|r| (r, l.place.clone())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_an_owner_gives_is_json() {
        let wire = json!({
            "items": [
                {"place": "2", "text": "- [ ] Read the report 📅 2026-11-20"},
                {"place": "3", "type": "eruption", "fields": {"vei": 6}},
            ],
            "links": [{"target": "Krakatoa", "place": "3", "text": "[[Krakatoa]],1883,6"}],
        });
        let given: Given = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&given).unwrap(), wire);
        assert!(matches!(&given.items[1], GivenItem::Typed { kind, .. } if kind == "eruption"));
        let wrong = json!({"items": [{"place": "1", "text": "x", "colour": "red"}]});
        assert!(serde_json::from_value::<Given>(wrong).is_err());
    }
}
