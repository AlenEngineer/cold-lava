//! The owner of `.canvas` files (JSON Canvas, Obsidian's whiteboards), in the engine like
//! Markdown's: it answers at once, in-process. A canvas joins the workspace through it, in the
//! alphabet's terms (docs/extensions.md, "The canvas owner, as built"):
//!
//! - each card is a piece at the node's id, *placed* where the file says: a `text` node a
//!   `text`, a `file` node and a web page (`link`) an `embed`, a node of a type nobody knows
//!   an `opaque`. Its position, its size and its colour are attributes, carried and never
//!   interpreted;
//! - what is written on a `text` card is Markdown, so it is a Markdown piece at the same
//!   place, read exactly as before: a task on a sticky or a text card is a `task`, and its
//!   links and tags are the canvas's;
//! - each `file` node is a link from the canvas to its file, at the node's id, so the file's
//!   backlinks list the canvas and the graph draws the edge;
//! - each arrow is a link between two pieces, with its label. It joins two cards of the
//!   drawing, not two files, so it is no workspace link.
//!
//! A group is not given: as a `section` it would be a heading among the workspace's items,
//! which no board had before, so an arrow to or from a group is not given either.
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

use crate::alphabet::Role;
use crate::json_canvas::{self, CNode, Field};
use crate::owner::given::{Between, Change, Given, GivenItem, GivenLink, Piece, Places};
use serde_json::{Map, Value as Json};
use std::collections::{HashMap, HashSet};

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

/// What a node is, in the alphabet. `None` for a group, which is not given.
fn role(kind: &str) -> Option<Role> {
    match kind {
        "text" => Some(Role::Text),
        "file" | "link" => Some(Role::Embed),
        "group" => None,
        _ => Some(Role::Opaque),
    }
}

/// What a card carries beyond its contents, as the file says it. *Placed*: `x`, `y`, `width`
/// and `height`, 0 where the file leaves one out, which is where the whiteboard draws it.
/// Then its `color`, a web page's `url` and a file card's `subpath`, when it has one. Every
/// other field of the node stays in the file and is carried by nobody.
fn attributes(n: &CNode) -> Map<String, Json> {
    let mut out = Map::new();
    let sizes = [
        ("x", n.x),
        ("y", n.y),
        ("width", n.width),
        ("height", n.height),
    ];
    for (name, v) in sizes {
        // A whole number as the file writes one, not `40.0`.
        out.insert(name.to_string(), crate::matcher::number(v));
    }
    let subpath = n.extra.get("subpath").and_then(Json::as_str);
    let texts = [
        ("color", n.color.as_deref()),
        ("url", n.url.as_deref()),
        ("subpath", subpath),
    ];
    for (name, text) in texts {
        out.extend(text.map(|t| (name.to_string(), Json::from(t))));
    }
    out
}

/// A canvas's items and links: each card a piece, placed; a text card's text a Markdown piece
/// at the same place; a file card a link to its file; an arrow a link between two pieces.
///
/// A canvas is another program's file, so the owner gives only what the engine takes, and
/// asks the engine which that is (`given::Places`), the one rule for every owner: a node
/// whose id is empty or too long gets no piece, no Markdown piece and no link; a node whose
/// place another node already has gets no piece and no Markdown piece where the other has
/// one; a link whose file is empty or too long is not given; an arrow past the bounds or with
/// an end that is no piece (a group, a node that isn't there) is not given. All of it stays
/// in the file.
pub fn read(text: &str) -> Result<Given, String> {
    let doc =
        json_canvas::parse(text.as_bytes()).map_err(|e| format!("it isn't a JSON Canvas: {e}"))?;
    let places = places(doc.nodes.iter().map(|n| n.id.as_str()));
    // Each node's card, the Markdown written on it and its link to a file, when it has them.
    let mut nodes: Vec<Node> = Vec::with_capacity(doc.nodes.len());
    for (n, place) in doc.nodes.into_iter().zip(&places) {
        let card = role(&n.kind).map(|role| {
            GivenItem::Piece(Piece {
                place: place.as_str().into(),
                role,
                attributes: attributes(&n).into(),
                text: Default::default(),
                inside: None,
                cells: Default::default(),
            })
        });
        let markdown = match (n.kind.as_str(), n.text) {
            ("text", Some(text)) => Some(GivenItem::Markdown {
                place: place.as_str().into(),
                text: text.into(),
            }),
            _ => None,
        };
        let link = match (n.kind.as_str(), n.file) {
            ("file", Some(file)) => Some(GivenLink {
                target: file.as_str().into(),
                place: place.as_str().into(),
                text: file.into(),
            }),
            _ => None,
        };
        nodes.push(Node {
            items: [card, markdown],
            link,
        });
    }
    // What the engine takes, asked in the answer's order.
    let mut taken = Places::default();
    let mut kept: Vec<[bool; 3]> = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let mut keep = [false; 3];
        for (k, item) in node.items.iter().enumerate() {
            if let Some(item) = item.as_ref().filter(|item| taken.fits(item).is_ok()) {
                taken.claim(item);
                if let GivenItem::Piece(card) = item {
                    taken.take(card);
                }
                keep[k] = true;
            }
        }
        keep[2] = (node.link.as_ref()).is_some_and(|l| taken.link(l).is_ok());
        kept.push(keep);
    }
    // An arrow names two ids, and the first node of an id has the id as its place.
    let mut between: Vec<Between> = Vec::new();
    for e in doc.edges {
        let arrow = Between {
            from: e.from_node.into(),
            to: e.to_node.into(),
            label: e.label.unwrap_or_default().into(),
        };
        if taken.joins(&arrow, between.len()).is_ok() {
            between.push(arrow);
        }
    }
    let mut given = Given {
        between: between.into(),
        ..Given::default()
    };
    for (node, [card, markdown, link]) in nodes.into_iter().zip(kept) {
        let [card_item, markdown_item] = node.items;
        given.items.extend(card_item.filter(|_| card));
        given.items.extend(markdown_item.filter(|_| markdown));
        given.links.extend(node.link.filter(|_| link));
    }
    Ok(given)
}

/// What one node gives, before the engine is asked which of it it takes: its card and the
/// Markdown written on it, and its link to a file.
struct Node {
    items: [Option<GivenItem>; 2],
    link: Option<GivenLink>,
}

/// The canvas's new text with `changes` made: a Markdown piece's new text is its node's `text`,
/// a link's new target its node's `file`. A typed item's field is refused, since a canvas gives
/// none, and so are a card's attribute and runs: the owner writes a card's text and file only.
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
            Change::Attribute {
                place, attribute, ..
            } => {
                return Err(format!(
                    "the canvas owner doesn't write a card's {attribute} (node {place}): it \
                     writes a card's text and its file, and nothing else of a node"
                ));
            }
            Change::Runs { place, .. } => {
                return Err(format!(
                    "a card's text is Markdown and takes no runs (node {place})"
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
/// `read` gives anything for it, but a node whose place an earlier node's `text` or `file`
/// has: a change names a node by its place and `edit` writes the first node there, so the
/// later one's change would be written over the earlier one's text. `None` when nothing
/// changes; an error when the file isn't JSON with nodes that have ids, so nothing in it can
/// be found.
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
    let mut seen: HashSet<(&str, String)> = HashSet::new();
    for f in &fields {
        if !seen.insert((f.name, place(&f.node, f.nth))) {
            continue;
        }
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

    /// What the owner gave before cards were pieces: the Markdown pieces and the links to
    /// files, which every item, link and id of a board comes from.
    fn as_before(given: &Given) -> (Vec<GivenItem>, Vec<GivenLink>) {
        let markdown = |i: &&GivenItem| matches!(i, GivenItem::Markdown { .. });
        (
            given.items.iter().filter(markdown).cloned().collect(),
            given.links.to_vec(),
        )
    }

    #[test]
    fn text_nodes_are_pieces_and_file_nodes_links() {
        let given = read(CANVAS).unwrap();
        let (items, links) = as_before(&given);
        assert_eq!(
            items,
            vec![GivenItem::Markdown {
                place: "s".into(),
                text: "Masks\n- [ ] Two each [[Lava]]".into()
            }]
        );
        assert_eq!(
            links,
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

    /// The pinned JSON of what the owner gives for a board: each card a piece with its role
    /// and where it sits, a text card's Markdown at the same place, a file card's link, and
    /// each arrow between two cards with its label. A group is not given, and neither is an
    /// arrow to one or to a node that isn't there.
    #[test]
    fn a_board_is_given_in_the_alphabet() {
        let board = r##"{"nodes":[
            {"id":"a","type":"text","text":"- [ ] Buy masks","x":40,"y":-120.5,"width":250,"height":60,"color":"4","mine":{"x":1}},
            {"id":"b","type":"file","file":"Volcanoes/Lava.md","subpath":"#Flows","x":400,"y":0,"width":400,"height":400},
            {"id":"w","type":"link","url":"https://jsoncanvas.org","x":0,"y":500,"width":300,"height":200},
            {"id":"g","type":"group","label":"Trip","x":-20,"y":-200,"width":900,"height":900},
            {"id":"u","type":"sparkle","x":1,"y":2},
            {"id":"a","type":"text","text":"twin","x":0,"y":0,"width":1,"height":1}
        ],"edges":[
            {"id":"e1","fromNode":"a","fromSide":"right","toNode":"b","toSide":"left","label":"needs"},
            {"id":"e2","fromNode":"b","toNode":"w"},
            {"id":"e3","fromNode":"u","toNode":"u","color":"1"},
            {"id":"e4","fromNode":"a","toNode":"g","label":"in a group"},
            {"id":"e5","fromNode":"a","toNode":"gone"}
        ]}"##;
        let given = read(board).unwrap();
        assert_eq!(
            serde_json::to_value(&given).unwrap(),
            serde_json::json!({
                "items": [
                    {"place": "a", "role": "text",
                     "attributes": {"x": 40, "y": -120.5, "width": 250, "height": 60, "color": "4"}},
                    {"place": "a", "text": "- [ ] Buy masks"},
                    {"place": "b", "role": "embed",
                     "attributes": {"x": 400, "y": 0, "width": 400, "height": 400, "subpath": "#Flows"}},
                    {"place": "w", "role": "embed",
                     "attributes": {"x": 0, "y": 500, "width": 300, "height": 200,
                                    "url": "https://jsoncanvas.org"}},
                    {"place": "u", "role": "opaque",
                     "attributes": {"x": 1, "y": 2, "width": 0, "height": 0}},
                    {"place": "a\u{1f}1", "role": "text",
                     "attributes": {"x": 0, "y": 0, "width": 1, "height": 1}},
                    {"place": "a\u{1f}1", "text": "twin"},
                ],
                "links": [
                    {"target": "Volcanoes/Lava.md", "place": "b", "text": "Volcanoes/Lava.md"},
                    {"from": "a", "to": "b", "label": "needs"},
                    {"from": "b", "to": "w"},
                    {"from": "u", "to": "u"},
                ],
            })
        );
        // It reads back through its own JSON, as an extension's answer does.
        let wire = serde_json::to_string(&given).unwrap();
        assert_eq!(serde_json::from_str::<Given>(&wire).unwrap(), given);
    }

    /// With ids that repeat, or an id that is literally a repeated id's place, a board gives
    /// the Markdown pieces it gave before cards were pieces: one for each text written, at its
    /// node's place, and none for a card with no text. Two such boards whose tasks must stay
    /// editable, and a card with no text on its own.
    #[test]
    fn a_board_gives_the_markdown_it_gave_whatever_its_ids() {
        let markdown = |board: &str| -> Vec<(String, String)> {
            let items = read(board).unwrap().items.into_iter();
            let piece = |i: GivenItem| match i {
                GivenItem::Markdown { place, text } => {
                    Some((place.into_string(), text.into_string()))
                }
                _ => None,
            };
            items.filter_map(piece).collect()
        };
        let pair = |place: &str, text: &str| (place.to_string(), text.to_string());
        // A repeated id with no text, then a node whose id is literally that one's place.
        let first = r##"{"nodes":[
            {"id":"t","type":"text","text":"- [ ] first"},
            {"id":"t","type":"text"},
            {"id":"t\u001f1","type":"text","text":"- [ ] literal"}
        ]}"##;
        assert_eq!(
            markdown(first),
            [pair("t", "- [ ] first"), pair("t\u{1f}1", "- [ ] literal")]
        );
        // The literal one has no text, and no card of its own: its place is the twin's.
        let second = r##"{"nodes":[
            {"id":"t","type":"text","text":"- [ ] one"},
            {"id":"t","type":"text","text":"- [ ] two"},
            {"id":"t\u001f1","type":"text"}
        ]}"##;
        assert_eq!(
            markdown(second),
            [pair("t", "- [ ] one"), pair("t\u{1f}1", "- [ ] two")]
        );
        // A card with no text has a piece and nothing written.
        let alone = r##"{"nodes":[{"id":"n","type":"text"},{"id":"","type":"text"}]}"##;
        assert!(markdown(alone).is_empty());
        assert_eq!(read(alone).unwrap().items.len(), 1, "the card at n");
    }

    /// Every card, text, link and arrow the owner gives is one the engine takes: read as the
    /// engine reads an answer, a board of hostile nodes and arrows has nothing to report.
    #[test]
    fn the_engine_reports_nothing_of_what_the_owner_gives() {
        let long = "i".repeat(<crate::bounded::Place as crate::bounded::Limit>::MOST + 1);
        let label = "L".repeat(<crate::bounded::LinkLabel as crate::bounded::Limit>::MOST + 1);
        let board = format!(
            r##"{{"nodes":[
                {{"id":"","type":"text","text":"x"}},
                {{"id":"{long}","type":"file","file":"A.md"}},
                {{"id":"t","type":"text","text":"one"}},
                {{"id":"t","type":"text"}},
                {{"id":"t\u001f1","type":"sparkle"}},
                {{"id":"g","type":"group"}}
            ],"edges":[
                {{"id":"e","fromNode":"t","toNode":"t","label":"{label}"}},
                {{"id":"e","fromNode":"t","toNode":"g"}},
                {{"id":"e","fromNode":"","toNode":"t"}},
                {{"id":"e","fromNode":"t","toNode":"t","label":"kept"}}
            ]}}"##
        );
        let given = read(&board).unwrap();
        assert_eq!(given.between.len(), 1);
        let mut taken = Places::default();
        for item in &given.items {
            assert_eq!(taken.fits(item), Ok(()));
            taken.claim(item);
            if let GivenItem::Piece(card) = item {
                taken.take(card);
            }
        }
        for link in &given.links {
            assert_eq!(taken.link(link), Ok(()));
        }
        for (nth, arrow) in given.between.iter().enumerate() {
            assert_eq!(taken.joins(arrow, nth), Ok(()));
        }
    }

    /// A canvas is another program's file. Whatever its nodes say, everything the owner
    /// gives is what the engine takes (the owner's decision of 2026-11-13, one rule for a
    /// place in every form): a node whose id can't be a place gives no piece, no text and no
    /// link, a second node at a place gives no second piece or text there, and a file card
    /// whose file is empty gives no link.
    #[test]
    fn a_node_that_cant_be_a_piece_keeps_its_text() {
        let long = "i".repeat(<crate::bounded::Place as crate::bounded::Limit>::MOST + 1);
        let board = format!(
            r##"{{"nodes":[
                {{"id":"","type":"text","text":"- [ ] No id","x":1e300,"y":-1e300}},
                {{"id":"{long}","type":"text","text":"- [ ] Long id"}},
                {{"id":"t","type":"text","text":"one"}},
                {{"id":"t","type":"text","text":"two"}},
                {{"id":"t\u001f1","type":"text","text":"three"}},
                {{"id":"\u001b]0;x\u0007","type":"file","file":""}}
            ],"edges":[
                {{"id":"e","fromNode":"","toNode":"t"}},
                {{"id":"e","fromNode":"t","toNode":"t"}},
                {{"id":"e","fromNode":"{long}","toNode":"{long}"}}
            ]}}"##
        );
        let given = read(&board).unwrap();
        let (items, links) = as_before(&given);
        assert_eq!(
            items,
            [
                GivenItem::Markdown {
                    place: "t".into(),
                    text: "one".into()
                },
                GivenItem::Markdown {
                    place: "t\u{1f}1".into(),
                    text: "two".into()
                },
            ],
            "the text of the nodes that hold their places"
        );
        assert!(links.is_empty(), "a file card with no file gives no link");
        let places: Vec<&str> = (given.items.iter())
            .filter_map(|i| match i {
                GivenItem::Piece(p) => Some(p.place.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(places, ["t", "t\u{1f}1", "\u{1b}]0;x\u{7}"]);
        assert_eq!(given.between.len(), 1);
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
        // Where a card sits is carried, and the owner doesn't write it: refused, with the
        // reason, and nothing made.
        let moved = Change::Attribute {
            place: "s".into(),
            attribute: "x".into(),
            value: serde_json::json!(40),
        };
        let why = edit(CANVAS, &[moved]).unwrap_err();
        assert!(why.contains("doesn't write a card's x"), "{why}");
        let runs = Change::Runs {
            place: "s".into(),
            cell: None,
            text: Vec::new(),
        };
        assert!(edit(CANVAS, &[runs]).is_err());
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
                GivenItem::Markdown { place, .. } => Some(place.into_string()),
                GivenItem::Typed { .. } | GivenItem::Piece(_) => None,
            };
            items.filter_map(place).collect()
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

    /// Two nodes at one place (a repeated id, then a node whose id is literally its place): a
    /// change names a node by its place and `edit` writes the first node there, so a rename
    /// changes the first one's links and leaves the second as it is, rather than writing the
    /// second's relinked text over the first's.
    #[test]
    fn a_rename_never_writes_one_nodes_text_over_another_at_its_place() {
        let board = r##"{"nodes":[
            {"id":"t","type":"text","text":"one"},
            {"id":"t","type":"text","text":"two [[Lava]]"},
            {"id":"t\u001f1","type":"text","text":"three [[Lava]]"}
        ]}"##;
        let asked = relink(board, |_| None, lava_to_magma).unwrap().unwrap();
        let relinked = edit(board, &asked.changes).unwrap();
        assert_eq!(relinked, board.replace("two [[Lava]]", "two [[Magma]]"));
    }
}
