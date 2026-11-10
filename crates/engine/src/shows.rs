//! What a region shows: an expression over a placed shape's inputs (docs/shapes.md, "Bindings,
//! as built"). `a & b` is what both inputs have, `a | b` what either has, `a - b` what `a` has
//! and `b` doesn't, matched on the shape's `key`.
//!
//! The set expression reads like a region's `area`, so the canvas crate's area parser reads it,
//! with the inputs standing in for parts; what it gives is a tree over the inputs' indices.

use std::collections::HashSet;

use cold_lava_canvas::{Area, AreaError, Kind, Part, Rect};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value as Json;

use crate::items::{Data, Item};
use crate::workspace::search_key;

/// One thing in an input: an item of a query, a note, or a label written out.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Member {
    /// An item's id, a note's path, or the label itself.
    pub id: String,
    /// The item's version, for an edit of it; `None` for what isn't an item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Its fields by name, as items carry them.
    pub fields: IndexMap<String, Json>,
}

impl Member {
    /// An item as a member: its fields, its text for a list item or a heading, and its note.
    pub fn of_item(item: &Item) -> Member {
        let mut fields = IndexMap::new();
        match &item.data {
            Data::Typed(t) => {
                for (k, v) in &t.fields {
                    fields.insert(k.to_string(), v.clone());
                }
            }
            Data::List { text } | Data::Heading { text, .. } => {
                fields.insert("text".into(), Json::String(text.clone()));
            }
            Data::Block => {}
        }
        fields
            .entry("note".into())
            .or_insert_with(|| Json::String(item.note.clone()));
        Member {
            id: item.id.clone(),
            rev: Some(item.rev.clone()),
            fields,
        }
    }

    /// A label written out: its `name` is the label.
    pub fn label(label: &str) -> Member {
        let fields = IndexMap::from([("name".to_string(), Json::String(label.to_string()))]);
        Member {
            id: label.to_string(),
            rev: None,
            fields,
        }
    }

    /// `field` as text: a list's values joined with ", "; `None` when it has no value.
    pub fn text(&self, field: &str) -> Option<String> {
        text_of(self.fields.get(field)?)
    }

    /// How it is labelled when nothing says: its `name`, `title` or `text`, else its first text
    /// field, else its id.
    pub fn default_label(&self) -> String {
        (["name", "title", "text"].iter())
            .find_map(|f| self.text(f))
            .or_else(|| {
                self.fields
                    .values()
                    .find_map(|v| v.as_str().map(String::from))
            })
            .unwrap_or_else(|| self.id.clone())
    }

    /// What makes it the same as a member of another input: its `key` field, folded with
    /// `search_key`; its id without a key, or when it has no value for it.
    fn key(&self, key: Option<&str>) -> String {
        match key.and_then(|k| self.text(k)) {
            Some(v) => format!("={}", search_key(&v)),
            None => format!("#{}", self.id),
        }
    }
}

/// A value as text: a string as it is, a number or a bool as written, a list joined with ", ".
pub fn text_of(v: &Json) -> Option<String> {
    match v {
        Json::Null => None,
        Json::String(s) => Some(s.clone()),
        Json::Array(a) if a.is_empty() => None,
        Json::Array(a) => Some(a.iter().filter_map(text_of).collect::<Vec<_>>().join(", ")),
        v => Some(v.to_string()),
    }
}

/// A region's `shows`, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Shows {
    /// Over the inputs: `Area::Part(i)` is input `i`.
    sets: Area,
}

impl Shows {
    /// Reads `text` over the shape's `inputs`; the error is one line.
    pub fn parse(text: &str, inputs: &[String]) -> Result<Shows, String> {
        let stand_ins: Vec<Part> = (inputs.iter())
            .map(|i| Part::new(Some(i), Kind::Rect(Rect::UNIT)))
            .collect();
        let sets = Area::parse(text, &stand_ins).map_err(|e| match e {
            AreaError::UnknownPart(n) => format!("{n} is not one of its inputs"),
            e => e.to_string(),
        })?;
        Ok(Shows { sets })
    }

    /// The members it shows, given each input's (in the order of the shape's inputs), matched
    /// across inputs on `key`. A member comes from the left of `&` and `-`, and from the left
    /// of `|` before the right's.
    pub fn eval<'m>(&self, inputs: &[&'m [Member]], key: Option<&str>) -> Vec<&'m Member> {
        sets(&self.sets, inputs, key)
    }
}

fn sets<'m>(area: &Area, inputs: &[&'m [Member]], key: Option<&str>) -> Vec<&'m Member> {
    let keys = |ms: &[&Member]| -> HashSet<String> { ms.iter().map(|m| m.key(key)).collect() };
    match area {
        Area::Part(i) => inputs.get(*i).map_or(Vec::new(), |ms| ms.iter().collect()),
        Area::And(a, b) => {
            let right = keys(&sets(b, inputs, key));
            let left = sets(a, inputs, key);
            left.into_iter()
                .filter(|m| right.contains(&m.key(key)))
                .collect()
        }
        Area::Minus(a, b) => {
            let right = keys(&sets(b, inputs, key));
            let left = sets(a, inputs, key);
            left.into_iter()
                .filter(|m| !right.contains(&m.key(key)))
                .collect()
        }
        Area::Or(a, b) => {
            let mut out = sets(a, inputs, key);
            let mut seen = keys(&out);
            for m in sets(b, inputs, key) {
                if seen.insert(m.key(key)) {
                    out.push(m);
                }
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(names: &[&str]) -> Vec<Member> {
        names.iter().map(|n| Member::label(n)).collect()
    }

    fn shown(
        expr: &str,
        a: &[Member],
        b: &[Member],
        c: &[Member],
        key: Option<&str>,
    ) -> Vec<String> {
        let inputs = ["a", "b", "c"].map(String::from);
        let shows = Shows::parse(expr, &inputs).unwrap();
        (shows.eval(&[a, b, c], key).iter())
            .map(|m| m.default_label())
            .collect()
    }

    #[test]
    fn set_operations_match_on_the_key_with_folding() {
        let a = labels(&["Kīlauea", "Krakatoa", "Fuji"]);
        let b = labels(&["kilauea", "Vesuvius", "FUJI"]);
        let c = labels(&["Fuji"]);
        let name = Some("name");
        assert_eq!(shown("a & b", &a, &b, &c, name), ["Kīlauea", "Fuji"]);
        assert_eq!(shown("a - b", &a, &b, &c, name), ["Krakatoa"]);
        assert_eq!(shown("b - a", &a, &b, &c, name), ["Vesuvius"]);
        assert_eq!(
            shown("a | b", &a, &b, &c, name),
            ["Kīlauea", "Krakatoa", "Fuji", "Vesuvius"]
        );
        assert_eq!(shown("a & b - c", &a, &b, &c, name), ["Kīlauea"]);
        assert_eq!(shown("(a | b) & c", &a, &b, &c, name), ["Fuji"]);
        // Without a key, a member is the same only as itself: labels differing in case differ.
        assert_eq!(shown("a & b", &a, &b, &c, None), Vec::<String>::new());
        assert_eq!(shown("a & c", &a, &b, &c, None), ["Fuji"]);
    }

    #[test]
    fn a_shows_naming_no_input_is_refused() {
        let inputs = ["a".to_string()];
        assert_eq!(
            Shows::parse("a & z", &inputs).unwrap_err(),
            "z is not one of its inputs"
        );
        assert!(Shows::parse("a &", &inputs).is_err());
    }
}
