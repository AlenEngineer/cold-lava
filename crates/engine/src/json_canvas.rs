//! JSON Canvas (<https://jsoncanvas.org>), the format of Obsidian's `.canvas` files: the one
//! reader and writer the whiteboard and the canvas owner share. `Doc` is the whole file, with
//! every field it doesn't know kept (`extra`), written back tab-indented as Obsidian does.
//! `fields` and `set` edit one node's `text` or `file` in place, so every other byte stays as
//! it was written.

use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::ops::Range;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Doc {
    #[serde(default)]
    pub nodes: Vec<CNode>,
    #[serde(default)]
    pub edges: Vec<CEdge>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CNode {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, serialize_with = "round")]
    pub x: f64,
    #[serde(default, serialize_with = "round")]
    pub y: f64,
    #[serde(default, serialize_with = "round")]
    pub width: f64,
    #[serde(default, serialize_with = "round")]
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CEdge {
    pub id: String,
    pub from_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_end: Option<String>,
    pub to_node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn round<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_i64(v.round() as i64)
}

/// Read a canvas file. An empty one is an empty canvas.
pub fn parse(bytes: &[u8]) -> serde_json::Result<Doc> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Doc::default());
    }
    serde_json::from_slice(bytes)
}

/// A canvas as a file: tab-indented, ending in a newline.
pub fn to_bytes(doc: &Doc) -> serde_json::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let fmt = serde_json::ser::PrettyFormatter::with_indent(b"\t");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
    doc.serialize(&mut ser)?;
    buf.push(b'\n');
    Ok(buf)
}

/// A node's `text` or `file` as written in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The node's id.
    pub node: String,
    /// Which of the nodes with that id it is, from 0: ids should be unique, and a file another
    /// program wrote may repeat one.
    pub nth: usize,
    /// `text` or `file`.
    pub name: &'static str,
    /// Where its JSON string is in the file's text, quotes included.
    pub span: Range<usize>,
    pub value: String,
}

#[derive(Deserialize)]
struct Spans<'a> {
    #[serde(borrow, default)]
    nodes: Vec<NodeSpans<'a>>,
}

#[derive(Deserialize)]
struct NodeSpans<'a> {
    id: String,
    #[serde(borrow, default)]
    text: Option<&'a RawValue>,
    #[serde(borrow, default)]
    file: Option<&'a RawValue>,
}

/// Every node's `text` and `file` that is a string, in the order of the nodes, with where each
/// is written in `text`.
pub fn fields(text: &str) -> serde_json::Result<Vec<Field>> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let spans: Spans = serde_json::from_str(text)?;
    let mut out = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for n in spans.nodes {
        let nth = seen.entry(n.id.clone()).or_insert(0);
        *nth += 1;
        let nth = *nth - 1;
        for (name, raw) in [("text", n.text), ("file", n.file)] {
            let Some(raw) = raw else { continue };
            let Ok(value) = serde_json::from_str::<String>(raw.get()) else {
                continue;
            };
            let start = raw.get().as_ptr() as usize - text.as_ptr() as usize;
            out.push(Field {
                node: n.id.clone(),
                nth,
                name,
                span: start..start + raw.get().len(),
                value,
            });
        }
    }
    Ok(out)
}

/// `text` with each field's value replaced by the string given for it, written as JSON in
/// place: every other byte stays as it was. Each field is given once: spans must not
/// overlap, and `canvas::edit`, the caller, refuses two changes to one field.
pub fn set(text: &str, new: &[(&Field, String)]) -> String {
    let mut new: Vec<&(&Field, String)> = new.iter().collect();
    new.sort_by_key(|(f, _)| f.span.start);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (f, value) in new {
        out.push_str(&text[last..f.span.start]);
        out.push_str(&serde_json::to_string(value).unwrap_or_default());
        last = f.span.end;
    }
    out.push_str(&text[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_found_where_they_are_written_and_set_in_place() {
        let text = "{\"nodes\":[\n\t{\"id\":\"a\",\"type\":\"text\",\"text\":\"- [ ] \\\"hot\\\" \u{e9}\",\"x\":0,\"y\":0,\"width\":1,\"height\":1,\"mine\":{\"text\":\"no\"}},\n\t{\"id\":\"b\",\"type\":\"file\",\"file\":\"Lava.md\",\"x\":0,\"y\":0,\"width\":1,\"height\":1}\n],\"edges\":[{\"id\":\"e\",\"fromNode\":\"a\",\"toNode\":\"b\",\"label\":\"text\"}]}";
        let f = fields(text).unwrap();
        assert_eq!(
            f.len(),
            2,
            "a node's own fields only, not those inside other fields"
        );
        assert_eq!((f[0].node.as_str(), f[0].name), ("a", "text"));
        assert_eq!(f[0].value, "- [ ] \"hot\" \u{e9}");
        assert_eq!(&text[f[0].span.clone()], "\"- [ ] \\\"hot\\\" \u{e9}\"");
        assert_eq!((f[1].node.as_str(), f[1].value.as_str()), ("b", "Lava.md"));
        let out = set(
            text,
            &[
                (&f[1], "Hot/Lava.md".into()),
                (&f[0], "- [x] \"hot\"\nnext".into()),
            ],
        );
        let want = text
            .replace(
                "\"- [ ] \\\"hot\\\" \u{e9}\"",
                "\"- [x] \\\"hot\\\"\\nnext\"",
            )
            .replace("\"Lava.md\"", "\"Hot/Lava.md\"");
        assert_eq!(out, want);
        assert!(parse(out.as_bytes()).is_ok());
        assert!(fields("  ").unwrap().is_empty());
        assert!(fields("{\"nodes\": [").is_err());
    }
}
