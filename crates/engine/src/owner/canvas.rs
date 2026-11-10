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

use crate::json_canvas::{self, Field};
use crate::owner::given::{Change, Given, GivenItem, GivenLink};

/// The kind of file it reads, and its name in messages.
pub const EXTENSION: &str = "canvas";
pub const OWNER: &str = "canvas";

/// A canvas's items and links: its text nodes as Markdown pieces, its file nodes as links.
pub fn read(text: &str) -> Result<Given, String> {
    let doc =
        json_canvas::parse(text.as_bytes()).map_err(|e| format!("it isn't a JSON Canvas: {e}"))?;
    let mut given = Given::default();
    for n in doc.nodes {
        match (n.kind.as_str(), n.text, n.file) {
            ("text", Some(text), _) => given.items.push(GivenItem::Markdown { place: n.id, text }),
            ("file", _, Some(file)) => given.links.push(GivenLink {
                target: file.clone(),
                place: n.id,
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
            .find(|f| f.node == place && f.name == name)
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

/// One field a rename changed: where it is (1-based line of the file) and its JSON before and
/// after.
pub struct Relinked {
    pub line: usize,
    pub before: String,
    pub after: String,
}

/// The canvas's text with links to moved files rewritten: each file node's `file` becomes what
/// `file` says (a workspace path), each text node's text what `piece` makes of it (the Markdown
/// owner's relink, which keeps each link's style), with how many links `piece` changed. `None`
/// when nothing changes or the file isn't a canvas.
pub fn relink(
    text: &str,
    file: impl Fn(&str) -> Option<String>,
    piece: impl Fn(&str) -> Option<(String, usize)>,
) -> Option<(String, Vec<Relinked>, usize)> {
    let fields = json_canvas::fields(text).ok()?;
    let (mut new, mut links) = (Vec::new(), 0);
    for f in &fields {
        let to = match f.name {
            "file" => file(&f.value).map(|to| (to, 1)),
            _ => piece(&f.value),
        };
        if let Some((to, n)) = to.filter(|(to, _)| *to != f.value) {
            links += n;
            new.push((f, to));
        }
    }
    if new.is_empty() {
        return None;
    }
    let changed = (new.iter())
        .map(|(f, to)| Relinked {
            line: text[..f.span.start].matches('\n').count() + 1,
            before: format!("\"{}\": {}", f.name, &text[f.span.clone()]),
            after: format!(
                "\"{}\": {}",
                f.name,
                serde_json::to_string(to).unwrap_or_default()
            ),
        })
        .collect();
    Some((json_canvas::set(text, &new), changed, links))
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

    #[test]
    fn relink_rewrites_file_nodes_and_pieces_in_place() {
        let (text, changed, links) = relink(
            CANVAS,
            |f| (f == "Volcanoes/Lava.md").then(|| "Hot/Lava.md".to_string()),
            |t| {
                t.contains("[[Lava]]")
                    .then(|| (t.replace("[[Lava]]", "[[Magma]]"), 1))
            },
        )
        .unwrap();
        let want = CANVAS
            .replace("Volcanoes/Lava.md", "Hot/Lava.md")
            .replace("[[Lava]]", "[[Magma]]");
        assert_eq!(text, want);
        assert_eq!((changed.len(), links), (2, 2));
        assert_eq!(changed[1].line, 4);
        assert_eq!(changed[1].after, "\"file\": \"Hot/Lava.md\"");
        assert!(relink(CANVAS, |_| None, |_| None).is_none());
    }
}
