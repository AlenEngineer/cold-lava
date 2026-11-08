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
