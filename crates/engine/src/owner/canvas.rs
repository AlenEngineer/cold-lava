//! The owner of `.canvas` files (JSON Canvas, Obsidian's whiteboards), in the engine like
//! Markdown's: it answers at once, in-process. A canvas joins the workspace through it:
//!
//! - each `text` node's text is a Markdown piece at the node's id, so a task on a sticky or a
//!   text card is a `task`, and its links and tags are the canvas's;
//! - each `file` node is a link from the canvas to its file, at the node's id, so the file's
//!   backlinks list the canvas and the graph draws the edge;
//! - `link` nodes (web pages) and `group` labels are not workspace links, and neither are edges:
//!   an arrow joins two nodes of the drawing, not two files.
//!
//! Edits and renames change one node's `text` or `file` in place (`json_canvas::set`), so every
//! other byte, unknown fields and the nodes' order included, stays as it was written.
//!
//! Reading items and rewriting links need different things of a file, and each asks for no more
//! than it needs. `read` wants the whole drawing to parse (`json_canvas::parse`: every node an
//! id and a type), since an item must be on a card the whiteboard can show. A rename (`relink`,
//! then `edit`) wants only the nodes' ids and their `text` and `file` strings
//! (`json_canvas::fields`), so it rewrites the links of a canvas `read` gives nothing for:
//! another program's file must not be left pointing at a note that moved.

use crate::json_canvas::{self, Field};
use crate::owner::given::{Change, Given, GivenItem, GivenLink};
use std::collections::HashMap;

/// The kind of file it reads, and its name in messages.
pub const EXTENSION: &str = "canvas";
pub const OWNER: &str = "canvas";

/// The owner's place for a node: its id. Ids should be unique; where another program repeated
/// one, the `nth` node with it (from 0) has its number after the id, so that every node has a
/// place of its own and a change reaches the node it is for.
fn place(id: &str, nth: usize) -> String {
    match nth {
        0 => id.to_string(),
        n => format!("{id}\u{1f}{n}"),
    }
}

/// Every node's place, from the nodes' ids in the file's order. The whiteboard names its nodes
/// by these, so a place the owner gave selects the node it is for. A place is an address in
/// memory and in messages: it is never written into the canvas.
pub fn places<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    ids.into_iter()
        .map(|id| {
            let nth = seen.entry(id).or_insert(0);
            *nth += 1;
            place(id, *nth - 1)
        })
        .collect()
}

/// A canvas's items and links: its text nodes as Markdown pieces, its file nodes as links.
pub fn read(text: &str) -> Result<Given, String> {
    let doc =
        json_canvas::parse(text.as_bytes()).map_err(|e| format!("it isn't a JSON Canvas: {e}"))?;
    let mut given = Given::default();
    let places = places(doc.nodes.iter().map(|n| n.id.as_str()));
    for (n, place) in doc.nodes.into_iter().zip(places) {
        match (n.kind.as_str(), n.text, n.file) {
            ("text", Some(text), _) => given.items.push(GivenItem::Markdown { place, text }),
            ("file", _, Some(file)) => given.links.push(GivenLink {
                target: file.clone(),
                place,
                text: file,
            }),
            _ => {}
        }
    }
    Ok(given)
}

/// The canvas's new text with `changes` made: a piece's new text is its node's `text`, a link's
/// new target its node's `file`. A typed item's field is refused: a canvas gives none.
pub fn edit(text: &str, changes: &[Change]) -> Result<String, String> {
    let fields = json_canvas::fields(text).map_err(|e| format!("it isn't a JSON Canvas: {e}"))?;
    let find = |place: &str, name: &str| {
        (fields.iter())
            .find(|f| f.name == name && self::place(&f.node, f.nth) == place)
            .ok_or_else(|| format!("it has no node {place} with a {name}"))
    };
    let mut new: Vec<(&Field, String)> = Vec::new();
    for c in changes {
        match c {
            Change::Text { place, text } => new.push((find(place, "text")?, text.clone())),
            Change::Link { place, link, to } => {
                let f = find(place, "file")?;
                if f.value != *link {
                    return Err(format!("node {place} shows {}, not {link}", f.value));
                }
                new.push((f, to.clone()));
            }
            Change::Field { place, .. } => {
                return Err(format!(
                    "node {place} has no fields: a canvas gives no typed items"
                ));
            }
        }
    }
    Ok(json_canvas::set(text, &new))
}

/// One field a rename changes: where it is (1-based line of the file) and its JSON before and
/// after.
pub struct Relinked {
    pub line: usize,
    pub before: String,
    pub after: String,
}

/// What a rename asks of a canvas: what the owner gives for it as it is (`None`: it can't read
/// it), the changes that make its links follow the move (`file/edit`, answered by `edit`), each
/// as it shows in the file, and how many links they rewrite.
pub struct Relink {
    pub given: Option<Given>,
    pub changes: Vec<Change>,
    pub shown: Vec<Relinked>,
    pub links: usize,
}

/// The changes that rewrite a canvas's links to moved files: every node's `file` becomes what
/// `file` says (a workspace path: a card holds a path, not a link written in a style), every
/// node's `text` what `piece` makes of it (the Markdown owner's relink, which keeps each link's
/// style), with how many links `piece` changed. Every node, whatever its type and whether or not
/// `read` gives anything for it. `None` when nothing changes; an error when the file isn't JSON
/// with nodes that have ids, so nothing in it can be found.
pub fn relink(
    text: &str,
    file: impl Fn(&str) -> Option<String>,
    piece: impl Fn(&str) -> Option<(String, usize)>,
) -> Result<Option<Relink>, String> {
    let fields = json_canvas::fields(text).map_err(|e| format!("it isn't a JSON Canvas: {e}"))?;
    let mut out = Relink {
        given: read(text).ok(),
        changes: Vec::new(),
        shown: Vec::new(),
        links: 0,
    };
    for f in &fields {
        let to = match f.name {
            "file" => file(&f.value).map(|to| (to, 1)),
            _ => piece(&f.value),
        };
        let Some((to, n)) = to.filter(|(to, _)| *to != f.value) else {
            continue;
        };
        out.links += n;
        out.shown.push(Relinked {
            line: text[..f.span.start].matches('\n').count() + 1,
            before: format!("\"{}\": {}", f.name, &text[f.span.clone()]),
            after: format!(
                "\"{}\": {}",
                f.name,
                serde_json::to_string(&to).unwrap_or_default()
            ),
        });
        let place = place(&f.node, f.nth);
        out.changes.push(match f.name {
            "file" => Change::Link {
                place,
                link: f.value.clone(),
                to,
            },
            _ => Change::Text { place, text: to },
        });
    }
    Ok((!out.changes.is_empty()).then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: &str = "{\n\t\"nodes\":[\n\t\t{\"id\":\"s\",\"type\":\"text\",\"text\":\"Masks\\n- [ ] Two each [[Lava]]\",\"x\":0,\"y\":0,\"width\":1,\"height\":1,\"cold-lava\":{\"shape\":\"sticky\",\"node\":\"text\"}},\n\t\t{\"id\":\"c\",\"type\":\"file\",\"file\":\"Volcanoes/Lava.md\",\"x\":0,\"y\":0,\"width\":1,\"height\":1},\n\t\t{\"id\":\"w\",\"type\":\"link\",\"url\":\"https://jsoncanvas.org\",\"x\":0,\"y\":0,\"width\":1,\"height\":1},\n\t\t{\"id\":\"g\",\"type\":\"group\",\"label\":\"[[Not a link]]\",\"x\":0,\"y\":0,\"width\":1,\"height\":1}\n\t],\n\t\"edges\":[{\"id\":\"e\",\"fromNode\":\"s\",\"toNode\":\"c\"}]\n}\n";

    #[test]
    fn text_nodes_are_pieces_and_file_nodes_links() {
        let given = read(CANVAS).unwrap();
        assert_eq!(
            given.items,
            vec![GivenItem::Markdown {
                place: "s".into(),
                text: "Masks\n- [ ] Two each [[Lava]]".into()
            }]
        );
        assert_eq!(
            given.links,
            vec![GivenLink {
                target: "Volcanoes/Lava.md".into(),
                place: "c".into(),
                text: "Volcanoes/Lava.md".into()
            }],
            "a web page, a group's label and an edge are not workspace links"
        );
        assert!(read("").unwrap().items.is_empty());
        assert!(read("{\"nodes\": [").is_err());
    }

    #[test]
    fn an_edit_changes_only_its_node() {
        let ticked = edit(
            CANVAS,
            &[Change::Text {
                place: "s".into(),
                text: "Masks\n- [x] Two each [[Lava]]".into(),
            }],
        )
        .unwrap();
        assert_eq!(ticked, CANVAS.replace("- [ ] Two", "- [x] Two"));
        let moved = edit(
            CANVAS,
            &[Change::Link {
                place: "c".into(),
                link: "Volcanoes/Lava.md".into(),
                to: "Hot/Lava.md".into(),
            }],
        )
        .unwrap();
        assert_eq!(moved, CANVAS.replace("Volcanoes/Lava.md", "Hot/Lava.md"));
        let field = Change::Field {
            place: "s".into(),
            field: "due".into(),
            value: "2026-11-12".into(),
        };
        assert!(edit(CANVAS, &[field]).is_err());
        let gone = Change::Text {
            place: "nope".into(),
            text: "x".into(),
        };
        assert!(edit(CANVAS, &[gone]).is_err());
    }

    fn lava_to_magma(t: &str) -> Option<(String, usize)> {
        let n = t.matches("[[Lava]]").count();
        (n > 0).then(|| (t.replace("[[Lava]]", "[[Magma]]"), n))
    }

    fn lava_to_hot(f: &str) -> Option<String> {
        (f == "Volcanoes/Lava.md").then(|| "Hot/Lava.md".to_string())
    }

    #[test]
    fn relink_asks_for_file_nodes_and_pieces_in_place() {
        let asked = relink(CANVAS, lava_to_hot, lava_to_magma).unwrap().unwrap();
        assert_eq!(asked.given, read(CANVAS).ok());
        let want = CANVAS
            .replace("Volcanoes/Lava.md", "Hot/Lava.md")
            .replace("[[Lava]]", "[[Magma]]");
        assert_eq!(edit(CANVAS, &asked.changes).unwrap(), want);
        assert_eq!((asked.shown.len(), asked.links), (2, 2));
        assert_eq!(asked.shown[1].line, 4);
        assert_eq!(asked.shown[1].after, "\"file\": \"Hot/Lava.md\"");
        assert!(relink(CANVAS, |_| None, |_| None).unwrap().is_none());
        assert!(relink("{\"nodes\": [", lava_to_hot, lava_to_magma).is_err());
        assert!(relink("", lava_to_hot, lava_to_magma).unwrap().is_none());
    }

    /// Another program's canvas may repeat an id. Each node has a place of its own, so a change
    /// reaches the card it is for: both cards, or only the second.
    #[test]
    fn nodes_of_one_id_are_read_and_relinked_each_on_its_own() {
        let both = CANVAS.replace(
            "{\"id\":\"w\",\"type\":\"link\",\"url\":\"https://jsoncanvas.org\"",
            "{\"id\":\"s\",\"type\":\"text\",\"text\":\"Also [[Lava]]\"",
        );
        let places = |text: &str| -> Vec<String> {
            let items = read(text).unwrap().items.into_iter();
            let place = |i: GivenItem| match i {
                GivenItem::Markdown { place, .. } | GivenItem::Typed { place, .. } => place,
            };
            items.map(place).collect()
        };
        assert_eq!(places(&both), ["s", "s\u{1f}1"]);
        let asked = relink(&both, |_| None, lava_to_magma).unwrap().unwrap();
        assert_eq!(asked.changes.len(), 2, "{:?}", asked.changes);
        let relinked = edit(&both, &asked.changes).unwrap();
        assert_eq!(relinked, both.replace("[[Lava]]", "[[Magma]]"));
        let applied = crate::owner::given::apply(asked.given.as_ref().unwrap(), &asked.changes);
        assert_eq!(read(&relinked).unwrap().items, applied.items);

        let second = both.replace("Two each [[Lava]]", "Two each");
        let asked = relink(&second, |_| None, lava_to_magma).unwrap().unwrap();
        assert_eq!(asked.changes.len(), 1, "{:?}", asked.changes);
        let relinked = edit(&second, &asked.changes).unwrap();
        assert_eq!(relinked, second.replace("Also [[Lava]]", "Also [[Magma]]"));
        let applied = crate::owner::given::apply(asked.given.as_ref().unwrap(), &asked.changes);
        assert_eq!(read(&relinked).unwrap().items, applied.items);
    }

    /// The whiteboard names its nodes by their places: every node has one of its own.
    #[test]
    fn every_node_has_a_place_of_its_own() {
        let named = places(["a", "s", "s", "b", "s"]);
        assert_eq!(named, ["a", "s", "s\u{1f}1", "b", "s\u{1f}2"]);
    }

    /// A node without a type: `read` gives nothing for the file, and a rename still rewrites
    /// its card's path and its text's links.
    #[test]
    fn a_canvas_that_gives_no_items_is_still_relinked() {
        let untyped = CANVAS.replace("\"id\":\"c\",\"type\":\"file\",", "\"id\":\"c\",");
        assert!(read(&untyped).is_err());
        let asked = relink(&untyped, lava_to_hot, lava_to_magma)
            .unwrap()
            .unwrap();
        assert_eq!(asked.given, None);
        let want = untyped
            .replace("Volcanoes/Lava.md", "Hot/Lava.md")
            .replace("[[Lava]]", "[[Magma]]");
        assert_eq!(edit(&untyped, &asked.changes).unwrap(), want);
    }
}
