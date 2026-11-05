//! Colour groups, like Obsidian's graph "Groups" setting.
//!
//! Sources: the vault's own `.obsidian/graph.json` colour groups (same queries Obsidian uses),
//! or automatic grouping by top-level folder or by first tag.

use crate::graph::{Graph, Kind, Node};
use ratatui::style::Color;
use std::collections::HashMap;
use std::path::Path;

pub const PALETTE: [Color; 8] = [
    Color::Rgb(239, 98, 98),   // red
    Color::Rgb(255, 167, 64),  // orange
    Color::Rgb(250, 214, 90),  // yellow
    Color::Rgb(110, 200, 120), // green
    Color::Rgb(70, 200, 210),  // teal
    Color::Rgb(90, 160, 250),  // blue
    Color::Rgb(240, 110, 170), // pink
    Color::Rgb(190, 150, 120), // brown
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Obsidian,
    Folder,
    Tag,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Obsidian => "obsidian groups",
            Mode::Folder => "by folder",
            Mode::Tag => "by tag",
        }
    }
}

pub struct Group {
    pub label: String,
    pub color: Color,
    query: Option<Query>,
}

/// The result of applying a grouping to a graph.
pub struct Assignment {
    pub groups: Vec<Group>,
    pub of_node: Vec<Option<usize>>,
}

impl Assignment {
    pub fn none(n: usize) -> Self {
        Assignment { groups: Vec::new(), of_node: vec![None; n] }
    }

    pub fn color(&self, i: usize) -> Option<Color> {
        self.of_node.get(i).copied().flatten().map(|g| self.groups[g].color)
    }

    pub fn count(&self, g: usize) -> usize {
        self.of_node.iter().filter(|x| **x == Some(g)).count()
    }
}

pub fn assign(mode: Mode, graph: &Graph, obsidian: &[Group]) -> Assignment {
    let n = graph.nodes.len();
    match mode {
        Mode::Off => Assignment::none(n),
        Mode::Obsidian => {
            let groups: Vec<Group> = obsidian
                .iter()
                .map(|g| Group { label: g.label.clone(), color: g.color, query: g.query.clone() })
                .collect();
            // First matching group wins, like Obsidian.
            let of_node = graph
                .nodes
                .iter()
                .map(|node| {
                    groups.iter().position(|g| g.query.as_ref().is_some_and(|q| q.matches(node)))
                })
                .collect();
            Assignment { groups, of_node }
        }
        Mode::Folder => auto(graph, |node| {
            let rel = node.rel.as_ref()?;
            rel.split_once('/').map(|(top, _)| top.to_string())
        }),
        Mode::Tag => auto(graph, |node| node.tags.first().map(|t| format!("#{t}"))),
    }
}

/// Group by an extracted key; the 8 most common keys get palette colours.
fn auto(graph: &Graph, key: impl Fn(&Node) -> Option<String>) -> Assignment {
    let keys: Vec<Option<String>> = graph.nodes.iter().map(|n| if n.kind == Kind::Note { key(n) } else { None }).collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for k in keys.iter().flatten() {
        *counts.entry(k.as_str()).or_default() += 1;
    }
    let mut ranked: Vec<(&str, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    ranked.truncate(PALETTE.len());
    let groups: Vec<Group> = ranked
        .iter()
        .enumerate()
        .map(|(i, (k, _))| Group { label: k.to_string(), color: PALETTE[i], query: None })
        .collect();
    let of_node = keys
        .iter()
        .map(|k| k.as_ref().and_then(|k| groups.iter().position(|g| &g.label == k)))
        .collect();
    Assignment { groups, of_node }
}

/// Reads `colorGroups` from `.obsidian/graph.json`.
pub fn load_obsidian(root: &Path) -> Vec<Group> {
    let Ok(text) = std::fs::read_to_string(root.join(".obsidian/graph.json")) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(list) = json.get("colorGroups").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|g| {
            let query = g.get("query")?.as_str()?.trim().to_string();
            let rgb = g.get("color")?.get("rgb")?.as_u64()? as u32;
            let color = Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
            Some(Group { label: query.clone(), color, query: Some(Query::parse(&query)) })
        })
        .collect()
}

// ---------- a small subset of Obsidian's search syntax ----------
// Supports: `path:x`, `file:x`, `tag:#x`, plain words, `-negation`, `"quoted words"`, `OR`.
// Space-separated terms must all match; `OR` separates alternatives.

#[derive(Clone)]
struct Query {
    alternatives: Vec<Vec<Term>>,
}

#[derive(Clone)]
struct Term {
    negate: bool,
    field: Field,
    value: String,
}

#[derive(Clone, Copy)]
enum Field {
    Path,
    File,
    Tag,
    Any,
}

impl Query {
    fn parse(q: &str) -> Query {
        let mut alternatives = vec![Vec::new()];
        for tok in tokenize(q) {
            if tok == "OR" {
                alternatives.push(Vec::new());
                continue;
            }
            let (negate, tok) = match tok.strip_prefix('-') {
                Some(t) if !t.is_empty() => (true, t.to_string()),
                _ => (false, tok),
            };
            let (field, value) = if let Some(v) = tok.strip_prefix("path:") {
                (Field::Path, v)
            } else if let Some(v) = tok.strip_prefix("file:") {
                (Field::File, v)
            } else if let Some(v) = tok.strip_prefix("tag:") {
                (Field::Tag, v.trim_start_matches('#'))
            } else if let Some(v) = tok.strip_prefix('#') {
                (Field::Tag, v)
            } else {
                (Field::Any, tok.as_str())
            };
            let value = value.trim_matches('"').to_lowercase();
            if !value.is_empty() {
                alternatives.last_mut().unwrap().push(Term { negate, field, value });
            }
        }
        alternatives.retain(|a| !a.is_empty());
        Query { alternatives }
    }

    fn matches(&self, n: &Node) -> bool {
        if n.kind == Kind::Tag {
            return false;
        }
        let path = n.rel.clone().unwrap_or_else(|| n.label.clone()).to_lowercase();
        let file = n.label.to_lowercase();
        self.alternatives.iter().any(|terms| {
            terms.iter().all(|t| {
                let hit = match t.field {
                    Field::Path => path.contains(&t.value),
                    Field::File => file.contains(&t.value),
                    Field::Tag => n.tags.iter().any(|tag| tag == &t.value || tag.starts_with(&format!("{}/", t.value))),
                    Field::Any => file.contains(&t.value) || path.contains(&t.value),
                };
                hit != t.negate
            })
        })
    }
}

fn tokenize(q: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in q.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
