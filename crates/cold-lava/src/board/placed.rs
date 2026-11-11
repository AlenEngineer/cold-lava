//! Placed shapes in a JSON Canvas file (docs/shapes.md, "Placed shapes and files"). A placed
//! shape is a node with a `cold-lava` field: which shape, what its inputs are bound to, and the
//! geometry Obsidian would round or lose. What the user placed is saved; everything the shape
//! shows is derived and never saved.
//!
//! The node stays a node Obsidian shows sensibly. A shape with an input bound to a note is a
//! `file` node (`file` is the note, so Obsidian shows the note and renames keep it); else one with
//! an input bound to text is a `text` node (`text` is that text, so editing it in Obsidian edits
//! the shape); else a `text` node with the shape's title. The node's own `x`, `y`, `width` and
//! `height` are the placement's box. Obsidian 1.15 keeps unknown fields when it saves a canvas
//! but sorts the nodes and rounds those four to integers, so nothing here depends on node order,
//! and the box's fractions and the rotation live in the field.

use super::CNode;
use crate::bind::Source;
use cold_lava_canvas::{Placement, Point};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The node field that makes a node a placed shape.
pub const FIELD: &str = "cold-lava";
/// The top-level field naming the version of `FIELD`'s format.
pub const VERSION_FIELD: &str = "cold-lava-version";
pub const VERSION: u64 = 1;
/// Canvas pixels per world unit of the canvas engine: a terminal cell is one unit wide at zoom 1.
pub const UNIT: f64 = 10.0;

/// What cold-lava keeps on a node that is a placed shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    pub shape: String,
    /// What each input is bound to, except the one the node itself holds (`node`).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub inputs: IndexMap<String, Source>,
    /// The input the node's own `file` (a note) or `text` holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// Degrees clockwise about the centre.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// `[x, y, width, height]` in canvas pixels when one isn't whole. It counts only while it
    /// rounds to the node's own box, so a node moved in Obsidian goes where Obsidian put it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<[f64; 4]>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Stored {
    /// The node's placed shape: `None` for a plain node, an error for a field that doesn't read.
    pub fn of(node: &CNode) -> Option<Result<Stored, String>> {
        let v = node.extra.get(FIELD)?;
        Some(serde_json::from_value(v.clone()).map_err(|e| format!("{}: {e}", node.id)))
    }

    /// What each input is bound to, the node's own included, in no particular order.
    pub fn sources(&self, node: &CNode) -> IndexMap<String, Source> {
        let mut out = self.inputs.clone();
        if let Some(name) = &self.node {
            let own = match node.kind.as_str() {
                "file" => node.file.clone().map(Source::Note),
                _ => node.text.clone().map(Source::Text),
            };
            if let Some(source) = own {
                out.insert(name.clone(), source);
            }
        }
        out
    }

    /// Where the shape sits, in the canvas engine's world units.
    pub fn placement(&self, node: &CNode) -> Placement {
        let own = [node.x, node.y, node.width, node.height];
        let whole = own.iter().all(|v| v.fract() == 0.0);
        let b = match self.exact {
            Some(e) if whole && e.iter().zip(own).all(|(e, v)| e.round() == v) => e,
            _ => own,
        };
        Placement::new(
            Point::new(b[0] / UNIT, b[1] / UNIT),
            Point::new(b[2] / UNIT, b[3] / UNIT),
        )
        .rotated(self.rotation)
    }
}

/// Makes `node` the placed shape `shape` bound to `sources` at `at`: its kind and content as the
/// mapping above says (`title` for a shape with neither a note nor text bound), its box, and the
/// field. `inputs` is the shape's inputs in order, which decides the node's own input.
pub fn place(
    node: &mut CNode,
    shape: &str,
    inputs: &[String],
    title: &str,
    sources: &IndexMap<String, Source>,
    at: Placement,
) {
    let in_order = || inputs.iter().filter_map(|n| Some((n, sources.get(n)?)));
    let note = in_order().find(|(_, s)| matches!(s, Source::Note(_)));
    let own = note.or_else(|| in_order().find(|(_, s)| matches!(s, Source::Text(_))));
    let (kind, file, text) = match own {
        Some((_, Source::Note(path))) => ("file", Some(path.clone()), None),
        Some((_, Source::Text(text))) => ("text", None, Some(text.clone())),
        _ => ("text", None, Some(title.to_string())),
    };
    node.kind = kind.to_string();
    node.file = file;
    node.text = text;
    let b = [
        at.at.x * UNIT,
        at.at.y * UNIT,
        at.size.x * UNIT,
        at.size.y * UNIT,
    ];
    [node.x, node.y, node.width, node.height] = b;
    let own_name = own.map(|(n, _)| n.clone());
    let stored = Stored {
        shape: shape.to_string(),
        inputs: (sources.iter())
            .filter(|(n, _)| Some(*n) != own_name.as_ref())
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect(),
        node: own_name,
        rotation: at.rotation.rem_euclid(360.0),
        exact: b.iter().any(|v| v.fract() != 0.0).then_some(b),
    };
    let field = serde_json::to_value(stored).expect("a placed shape serializes");
    node.extra.insert(FIELD.to_string(), field);
}

/// Moves or turns a placed shape, keeping what it is bound to.
pub fn replace(node: &mut CNode, at: Placement) {
    let Some(Ok(stored)) = Stored::of(node) else {
        return;
    };
    let sources = stored.sources(node);
    let mut inputs: Vec<String> = stored.node.iter().cloned().collect();
    inputs.extend(stored.inputs.keys().cloned());
    let title = node.text.clone().unwrap_or_default();
    place(node, &stored.shape, &inputs, &title, &sources, at);
}

/// Marks the file's format when it has a placed shape, and unmarks it when it has none.
pub fn mark_version(extra: &mut Map<String, Value>, nodes: &[CNode]) {
    match nodes.iter().any(|n| n.extra.contains_key(FIELD)) {
        true => {
            extra.insert(VERSION_FIELD.to_string(), Value::from(VERSION));
        }
        false => {
            extra.remove(VERSION_FIELD);
        }
    }
}

/// The ids of nodes that had cold-lava's field in `before` and are still in `after` without it:
/// a program that rewrote the file dropped it.
pub fn lost(before: &Value, after: &Value) -> Vec<String> {
    let nodes = |v: &Value| -> Vec<(String, bool)> {
        (v["nodes"].as_array().into_iter().flatten())
            .filter_map(|n| Some((n["id"].as_str()?.to_string(), n.get(FIELD).is_some())))
            .collect()
    };
    let after = nodes(after);
    (nodes(before).into_iter())
        .filter(|(id, had)| *had && after.iter().any(|(i, has)| i == id && !has))
        .map(|(id, _)| id)
        .collect()
}

/// Puts `ours`'s field back on each node `ids` names in `merged`, and the version with them.
pub fn restore(merged: &mut Value, ours: &Value, ids: &[String]) {
    let field = |id: &str| {
        (ours["nodes"].as_array().into_iter().flatten())
            .find(|n| n["id"] == id)
            .and_then(|n| n.get(FIELD).cloned())
    };
    let mut any = false;
    for node in merged["nodes"].as_array_mut().into_iter().flatten() {
        let Some(id) = node["id"].as_str().map(String::from) else {
            continue;
        };
        if ids.contains(&id)
            && let (Some(f), Some(n)) = (field(&id), node.as_object_mut())
        {
            n.insert(FIELD.to_string(), f);
            any = true;
        }
    }
    if any && let Some(top) = merged.as_object_mut() {
        top.insert(VERSION_FIELD.to_string(), Value::from(VERSION));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Doc, merge3, parse};
    use super::*;
    use crate::bind::Bound;
    use serde_json::json;
    use std::path::Path;

    fn node(kind: &str) -> CNode {
        serde_json::from_value(
            json!({"id": "n", "type": kind, "x": 0, "y": 0, "width": 1, "height": 1}),
        )
        .unwrap()
    }

    fn query(kind: &str, tag: &str) -> Source {
        let mut b = Bound::default();
        b.select.kind = kind.into();
        b.select.filter.tag = Some(tag.into());
        Source::Query(Box::new(b))
    }

    fn sources(list: &[(&str, Source)]) -> IndexMap<String, Source> {
        (list.iter())
            .map(|(n, s)| (n.to_string(), s.clone()))
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_card_is_a_file_node_a_sticky_a_text_node_and_others_show_their_title() {
        let at = Placement::new(Point::new(3.0, 4.0), Point::new(25.0, 10.0));
        let mut card = node("text");
        let note = Source::Note("Volcanoes/Krakatoa.md".into());
        place(
            &mut card,
            "card",
            &names(&["note"]),
            "Card",
            &sources(&[("note", note.clone())]),
            at,
        );
        assert_eq!(card.kind, "file");
        assert_eq!(card.file.as_deref(), Some("Volcanoes/Krakatoa.md"));
        assert_eq!(card.text, None);
        assert_eq!(
            (card.x, card.y, card.width, card.height),
            (30.0, 40.0, 250.0, 100.0)
        );
        let stored = Stored::of(&card).unwrap().unwrap();
        assert_eq!(stored.node.as_deref(), Some("note"));
        assert!(
            stored.inputs.is_empty(),
            "the note lives in the node's `file` alone"
        );
        assert_eq!(stored.sources(&card)["note"], note);

        let mut sticky = node("file");
        let text = Source::Text("- [ ] Pack ash masks".into());
        place(
            &mut sticky,
            "sticky",
            &names(&["text"]),
            "Sticky",
            &sources(&[("text", text)]),
            at,
        );
        assert_eq!(sticky.kind, "text");
        assert_eq!(sticky.text.as_deref(), Some("- [ ] Pack ash masks"));
        assert_eq!(sticky.file, None);
        // An edit of the text node, in Obsidian or here, is the sticky's text.
        sticky.text = Some("- [x] Pack ash masks".into());
        let stored = Stored::of(&sticky).unwrap().unwrap();
        assert_eq!(
            stored.sources(&sticky)["text"],
            Source::Text("- [x] Pack ash masks".into())
        );

        let mut venn = node("text");
        let bound = sources(&[
            ("a", query("volcano", "eruption")),
            ("b", query("volcano", "strato")),
        ]);
        place(
            &mut venn,
            "venn2",
            &names(&["a", "b"]),
            "Simple Venn diagram",
            &bound,
            at.rotated(-30.0),
        );
        assert_eq!(venn.kind, "text");
        assert_eq!(venn.text.as_deref(), Some("Simple Venn diagram"));
        let stored = Stored::of(&venn).unwrap().unwrap();
        assert_eq!(stored.node, None);
        assert_eq!(stored.sources(&venn), bound);
        assert_eq!(stored.rotation, 330.0);
        assert_eq!(stored.placement(&venn).rotation, 330.0);
    }

    #[test]
    fn fractions_and_rotation_survive_obsidians_rounding_and_sorting() {
        let mut venn = node("text");
        let at = Placement::new(Point::new(1.25, 2.0), Point::new(30.5, 30.0)).rotated(15.0);
        let bound = sources(&[("a", query("volcano", "eruption"))]);
        place(&mut venn, "venn2", &names(&["a", "b"]), "Venn", &bound, at);
        venn.id = "venn".into();
        let mut other = node("text");
        other.id = "a-plain-card".into();
        other.text = Some("plain".into());
        let doc = Doc {
            nodes: vec![venn, other],
            ..Doc::default()
        };
        // What Obsidian does on save: sort the nodes, round the box, keep unknown fields.
        let mut saved = serde_json::to_value(&doc).unwrap();
        saved["nodes"].as_array_mut().unwrap().reverse();
        let (doc, _) = parse(saved.to_string().as_bytes(), Path::new("t.canvas")).unwrap();
        let venn = doc.nodes.iter().find(|n| n.id == "venn").unwrap();
        assert_eq!((venn.x, venn.width), (13.0, 305.0), "whole numbers on disk");
        let stored = Stored::of(venn).unwrap().unwrap();
        assert_eq!(
            stored.placement(venn),
            at,
            "the exact box and the turn come back"
        );

        // Moved in Obsidian: the node's box wins over the stale exact one.
        let mut moved = venn.clone();
        moved.x = 200.0;
        let p = stored.placement(&moved);
        assert_eq!((p.at.x, p.size.x, p.rotation), (20.0, 30.5, 15.0));
    }

    #[test]
    fn replace_moves_a_shape_keeping_its_binding() {
        let mut sticky = node("text");
        let bound = sources(&[("text", Source::Text("hi".into()))]);
        let at = Placement::new(Point::new(0.0, 0.0), Point::new(20.0, 10.0));
        place(
            &mut sticky,
            "sticky",
            &names(&["text"]),
            "Sticky",
            &bound,
            at,
        );
        replace(&mut sticky, at.rotated(90.0));
        let stored = Stored::of(&sticky).unwrap().unwrap();
        assert_eq!((stored.shape.as_str(), stored.rotation), ("sticky", 90.0));
        assert_eq!(stored.sources(&sticky), bound);
    }

    #[test]
    fn a_field_that_doesnt_read_is_an_error_and_a_plain_node_is_none() {
        let mut n = node("text");
        assert_eq!(Stored::of(&n), None);
        n.extra.insert(FIELD.into(), json!({"shape": 3}));
        assert!(Stored::of(&n).unwrap().is_err());
    }

    #[test]
    fn the_guard_finds_and_restores_fields_another_program_dropped() {
        let shape = json!({"id": "s", "type": "text", "text": "Venn", "x": 0, "y": 0,
                           "width": 300, "height": 300, FIELD: {"shape": "venn2"}});
        let base = json!({"nodes": [shape.clone()], "edges": [], VERSION_FIELD: 1});
        let ours = base.clone();
        // Saved by a program that keeps nodes but not our fields, and moved the node.
        let theirs = json!({"nodes": [{"id": "s", "type": "text", "text": "Venn", "x": 40, "y": 0,
                                       "width": 300, "height": 300}], "edges": []});
        let lost = lost(&base, &theirs);
        assert_eq!(lost, ["s"]);
        let (mut merged, _) = merge3(&base, &ours, &theirs);
        assert!(
            merged["nodes"][0].get(FIELD).is_none(),
            "the merge alone loses it"
        );
        restore(&mut merged, &ours, &lost);
        assert_eq!(merged["nodes"][0][FIELD]["shape"], "venn2");
        assert_eq!(merged["nodes"][0]["x"], 40, "their move is kept");
        assert_eq!(merged[VERSION_FIELD], 1);
        // A node deleted outside is a deletion, not a loss.
        let gone = json!({"nodes": [], "edges": []});
        assert!(super::lost(&base, &gone).is_empty());
    }

    #[test]
    fn the_version_marks_files_with_shapes() {
        let mut extra = Map::new();
        let mut shaped = node("text");
        mark_version(&mut extra, std::slice::from_ref(&shaped));
        assert!(extra.get(VERSION_FIELD).is_none());
        shaped.extra.insert(FIELD.into(), json!({"shape": "rect"}));
        mark_version(&mut extra, &[shaped]);
        assert_eq!(extra[VERSION_FIELD], 1);
    }
}
