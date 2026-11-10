//! A shape written back into a workspace's `.cold-lava/shapes.toml`, as text. The shape editor
//! saves through here: one shape's `[[shapes]]` block is written in the format people write by
//! hand, and the rest of the file (other shapes, glyphs, comments, blank lines) stays as it was,
//! byte for byte, the way a rename keeps a note's formatting.
//!
//! A block is its `[[shapes]]` line and what follows up to the next table that isn't one of its
//! own (`[[shapes.on]]` is); comments and blank lines just before the next table go with that
//! table. Comments inside a rewritten block are not kept.

use std::ops::Range;
use std::path::Path;

use super::{Definition, VERSION, WORKSPACE_FILE};
use crate::write::{self, WriteError};

/// `d` as a `[[shapes]]` block with id `id`, ending in a line break.
pub fn block(id: &str, d: &Definition) -> String {
    let mut table = match toml::Value::try_from(d) {
        Ok(toml::Value::Table(t)) => t,
        _ => unreachable!("a definition is a table of plain values"),
    };
    let on = table.remove("on");
    let mut out = String::from("[[shapes]]\n");
    out += &format!("id = {}\n", value(&toml::Value::String(id.into()), 0));
    for (key, v) in &table {
        out += &format!("{} = {}\n", bare(key), lines(key, v));
    }
    if let Some(toml::Value::Array(entries)) = on {
        for entry in entries {
            out += "\n[[shapes.on]]\n";
            if let toml::Value::Table(t) = entry {
                for (key, v) in &t {
                    out += &format!("{} = {}\n", bare(key), lines(key, v));
                }
            }
        }
    }
    out
}

/// A top-level value: `parts`, `regions` and a behaviour's `do` one entry per line, the rest on
/// one line.
fn lines(key: &str, v: &toml::Value) -> String {
    match v {
        toml::Value::Array(items)
            if matches!(key, "parts" | "regions" | "do") && !items.is_empty() =>
        {
            let mut s = String::from("[\n");
            for item in items {
                s += &format!("  {},\n", value(item, 1));
            }
            s + "]"
        }
        _ => value(v, 0),
    }
}

/// One value inline. A path's `segments` go one to a line, as hand-written paths do.
fn value(v: &toml::Value, depth: usize) -> String {
    match v {
        toml::Value::String(s) => toml::Value::String(s.clone()).to_string(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => number(*f),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(d) => d.to_string(),
        toml::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(|i| value(i, depth)).collect();
            format!("[{}]", inner.join(", "))
        }
        toml::Value::Table(t) if t.is_empty() => "{}".into(),
        toml::Value::Table(t) => {
            let fields: Vec<String> = (t.iter())
                .map(|(k, v)| match (k.as_str(), v) {
                    ("segments", toml::Value::Array(segs)) if segs.len() > 1 => {
                        let pad = "  ".repeat(depth + 1);
                        let each: String = (segs.iter())
                            .map(|s| format!("{pad}{},\n", value(s, depth + 1)))
                            .collect();
                        format!("segments = [\n{each}{}]", "  ".repeat(depth))
                    }
                    _ => format!("{} = {}", bare(k), value(v, depth)),
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
    }
}

fn has_version(text: &str) -> bool {
    toml::from_str::<toml::Table>(text).is_ok_and(|t| t.contains_key("version"))
}

/// A coordinate or size, always with a point: `0.38`, `1.0`. A number written with up to six
/// places stays as it is; one computed by the editor (`0.43301270…`) is kept to four.
fn number(f: f64) -> String {
    let short = (f * 1e6).round() / 1e6;
    let r = match (short - f).abs() < 1e-12 {
        true => short,
        false => (f * 10_000.0).round() / 10_000.0,
    };
    let r = if r == 0.0 { 0.0 } else { r };
    let s = format!("{r}");
    if s.contains('.') || s.contains('e') || !r.is_finite() {
        s
    } else {
        format!("{s}.0")
    }
}

fn bare(key: &str) -> String {
    let plain = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    match plain {
        true => key.to_string(),
        false => toml::Value::String(key.into()).to_string(),
    }
}

/// One `[[shapes]]` block of a file: its bytes and the id it gives.
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub id: Option<String>,
    pub span: Range<usize>,
}

/// The `[[shapes]]` blocks of a shapes file, in order.
pub fn blocks(text: &str) -> Vec<Block> {
    let headers = headers(text);
    let mut out = Vec::new();
    for (i, h) in headers.iter().enumerate() {
        if !(h.array && h.name == "shapes") {
            continue;
        }
        let next = (headers[i + 1..].iter())
            .find(|n| !n.name.starts_with("shapes."))
            .map_or(text.len(), |n| n.at);
        let mut end = next;
        // Comments and blank lines just before the next table are that table's.
        while end > h.at {
            let before = text[..end].strip_suffix('\n').unwrap_or(&text[..end]);
            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
            let line = text[line_start..end].trim();
            if line_start <= h.at || !(line.is_empty() || line.starts_with('#')) {
                break;
            }
            end = line_start;
        }
        let id = toml::from_str::<toml::Table>(&text[h.at..end])
            .ok()
            .and_then(|t| {
                let shapes = t.get("shapes")?.as_array()?;
                Some(shapes.first()?.get("id")?.as_str()?.to_string())
            });
        out.push(Block {
            id,
            span: h.at..end,
        });
    }
    out
}

struct Header {
    /// Where its line starts.
    at: usize,
    name: String,
    array: bool,
}

/// The table headers of a TOML text: lines starting with `[` outside any string or value.
fn headers(text: &str) -> Vec<Header> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut multi: Option<&str> = None;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let trimmed = line.trim_start();
        if multi.is_none() && depth == 0 && trimmed.starts_with('[') {
            let array = trimmed.starts_with("[[");
            let open = if array { 2 } else { 1 };
            let close = if array { "]]" } else { "]" };
            if let Some(end) = trimmed[open..].find(close) {
                let name: String = trimmed[open..open + end]
                    .split('.')
                    .map(|p| p.trim().trim_matches('"'))
                    .collect::<Vec<_>>()
                    .join(".");
                out.push(Header {
                    at: start,
                    name,
                    array,
                });
            }
            continue;
        }
        let mut rest = line;
        while !rest.is_empty() {
            if let Some(q) = multi {
                match rest.find(q) {
                    Some(i) => {
                        rest = &rest[i + 3..];
                        multi = None;
                    }
                    None => break,
                }
                continue;
            }
            let c = rest.chars().next().expect("not empty");
            match c {
                '#' => break,
                '"' | '\'' => {
                    let triple = if c == '"' { "\"\"\"" } else { "'''" };
                    if rest.starts_with(triple) {
                        multi = Some(triple);
                        rest = &rest[3..];
                        continue;
                    }
                    let mut chars = rest[1..].char_indices();
                    let mut end = rest.len();
                    while let Some((i, ch)) = chars.next() {
                        if c == '"' && ch == '\\' {
                            chars.next();
                        } else if ch == c {
                            end = i + 2;
                            break;
                        }
                    }
                    rest = &rest[end.min(rest.len())..];
                    continue;
                }
                '[' | '{' => depth += 1,
                ']' | '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The file's text with the shape `replacing` (or a new one, appended) written as `id` = `d`.
/// Refused when `replacing` isn't in the file, when another shape there already has `id`, or
/// when anything but that one shape would read differently afterwards.
pub fn splice(
    text: &str,
    replacing: Option<&str>,
    id: &str,
    d: &Definition,
) -> Result<String, String> {
    let found = blocks(text);
    let new = block(id, d);
    let clash = found
        .iter()
        .any(|b| b.id.as_deref() == Some(id) && Some(id) != replacing);
    if clash {
        return Err(format!(
            "{WORKSPACE_FILE} already has a shape called {id}: choose another id"
        ));
    }
    let spliced = match replacing {
        Some(old) => {
            let b = (found.iter())
                .find(|b| b.id.as_deref() == Some(old))
                .ok_or_else(|| format!("{WORKSPACE_FILE} has no shape called {old} any more"))?;
            format!("{}{new}{}", &text[..b.span.start], &text[b.span.end..])
        }
        None if text.trim().is_empty() => format!("version = {VERSION}\n\n{new}"),
        None if !has_version(text) => {
            return Err(format!(
                "not saved: {WORKSPACE_FILE} has no version = {VERSION}, so cold-lava skips it; add that line at its top"
            ));
        }
        None => {
            let sep = if text.ends_with("\n\n") {
                ""
            } else if text.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            format!("{text}{sep}{new}")
        }
    };
    check(text, &spliced, replacing, id)?;
    Ok(spliced)
}

/// Whether `after` reads as `before` with only the shape `replacing` changed (or `id` added).
fn check(before: &str, after: &str, replacing: Option<&str>, id: &str) -> Result<(), String> {
    let refuse = |why: String| Err(format!("not saved: {why}; the file is left as it was"));
    let read = |t: &str| toml::from_str::<toml::Table>(t).map_err(|e| e.message().to_string());
    let (old, new) = match (read(before), read(after)) {
        (Ok(o), Ok(n)) => (o, n),
        (_, Err(e)) => return refuse(format!("the new text doesn't read ({e})")),
        (Err(e), _) if !before.trim().is_empty() => {
            return refuse(format!("{WORKSPACE_FILE} doesn't read ({e})"));
        }
        (Err(_), Ok(n)) => (toml::Table::new(), n),
    };
    let without = |mut t: toml::Table, name: Option<&str>| {
        if let Some(toml::Value::Array(shapes)) = t.get_mut("shapes") {
            shapes.retain(|s| s.get("id").and_then(toml::Value::as_str) != name);
        }
        t.remove("version");
        t
    };
    let entry = (new.get("shapes").and_then(toml::Value::as_array))
        .and_then(|s| {
            s.iter()
                .find(|s| s.get("id").and_then(toml::Value::as_str) == Some(id))
        })
        .cloned();
    let mut old_rest = without(old, replacing);
    let mut new_rest = without(new, Some(id));
    for t in [&mut old_rest, &mut new_rest] {
        if t.get("shapes")
            .and_then(toml::Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            t.remove("shapes");
        }
    }
    if old_rest != new_rest {
        return refuse("the rest of the file would have changed".into());
    }
    let Some(mut entry) = entry.and_then(|e| e.as_table().cloned()) else {
        return refuse(format!("the shape {id} isn't in the new text"));
    };
    entry.remove("id");
    if let Err(e) = toml::Value::Table(entry).try_into::<Definition>() {
        return refuse(format!("the shape doesn't read back ({})", e.message()));
    }
    Ok(())
}

/// What happened to the workspace's shapes file.
#[derive(Debug)]
pub enum Saved {
    /// Written; these are its bytes now, the version the next save expects.
    Written(Vec<u8>),
    /// Refused, with why.
    Refused(String),
}

/// Saves `d` as `id` into the workspace's shapes file, replacing the shape `replacing` (or adding
/// a new one). `read` is the file as it was read (`None`: it didn't exist): if it changed since,
/// nothing is written.
pub fn save(
    root: &Path,
    read: Option<&[u8]>,
    replacing: Option<&str>,
    id: &str,
    d: &Definition,
) -> Saved {
    let path = root.join(WORKSPACE_FILE);
    let text = match read.map(std::str::from_utf8) {
        None => "",
        Some(Ok(t)) => t,
        Some(Err(_)) => return Saved::Refused(format!("{WORKSPACE_FILE} isn't UTF-8 text")),
    };
    let new = match splice(text, replacing, id, d) {
        Ok(n) => n,
        Err(e) => return Saved::Refused(e),
    };
    let written = match read {
        Some(before) => write::replace(&path, before, new.as_bytes()),
        None => write::create(&path, new.as_bytes()),
    };
    match written {
        Ok(()) => Saved::Written(new.into_bytes()),
        Err(WriteError::Changed(_) | WriteError::Exists(_)) => Saved::Refused(format!(
            "{WORKSPACE_FILE} changed since it was read: not saved; your shape is still here, and reopening the editor reads the file again"
        )),
        Err(e) => Saved::Refused(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::{Layer, load};

    const FILE: &str = r#"# My shapes.
version = 1

# Two circles.
[[shapes]]
id = "pair"   # the pair
title = "Pair"
inputs = ["a", "b"]
parts = [
  { circle = { id = "left", center = [0.38, 0.5], radius = 0.32 } },
  { circle = { id = "right", center = [0.62, 0.5], radius = 0.32 } },
]
regions = [
  { id = "both", area = "left & right", shows = "a & b" },  # shared
]

[[shapes.on]]
click = "both"
edit = { item = "clicked", toggle = true }

# A tick: "[[shapes]]" in a string isn't a table.
[[shapes]]
id = "tick"
title = """
[[shapes]]
"""
parts = [{ line = { id = "mark", from = [0.5, 0.3], to = [0.5, 0.7] } }]

[glyphs]
lava = "♨"
"#;

    fn def(text: &str) -> Definition {
        let mut t: toml::Table = toml::from_str(text).unwrap();
        let mut entry = t.remove("shapes").unwrap().as_array().unwrap()[0].clone();
        entry.as_table_mut().unwrap().remove("id");
        entry.try_into().unwrap()
    }

    #[test]
    fn finds_each_block_with_its_own_tables_and_not_the_comments_after() {
        let b = blocks(FILE);
        let ids: Vec<_> = b.iter().map(|b| b.id.as_deref()).collect();
        assert_eq!(ids, [Some("pair"), Some("tick")]);
        let pair = &FILE[b[0].span.clone()];
        assert!(pair.starts_with("[[shapes]]\nid = \"pair\""));
        assert!(pair.contains("[[shapes.on]]"));
        assert!(pair.trim_end().ends_with("toggle = true }"));
        assert!(FILE[b[1].span.clone()].trim_end().ends_with("0.7] } }]"));
    }

    #[test]
    fn a_block_reads_back_as_the_same_definition() {
        let d = def(&FILE[blocks(FILE)[0].span.clone()]);
        let written = block("pair", &d);
        assert_eq!(def(&written), d);
        assert!(written.contains(
            "parts = [\n  { circle = { id = \"left\", center = [0.38, 0.5], radius = 0.32 } },\n"
        ));
        assert!(written.contains("\n[[shapes.on]]\nclick = \"both\"\n"));
    }

    #[test]
    fn every_shape_cold_lava_ships_or_shows_reads_back_from_its_block() {
        let example = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../example-workspace/.cold-lava/shapes.toml"),
        )
        .unwrap();
        for text in [crate::shapes::BUILT_IN, example.as_str()] {
            let found = blocks(text);
            assert!(found.len() > 3);
            for b in found {
                let id = b.id.clone().expect("an id");
                let d = def(&text[b.span.clone()]);
                assert_eq!(def(&block(&id, &d)), d, "{id}");
                let same = splice(text, Some(&id), &id, &d).unwrap();
                let read = |t: &str| {
                    let o = crate::types::Origin::BuiltIn;
                    load([Layer::from_file(o, Ok(t.to_string()))]).shapes
                };
                let defs = |t: &str| read(t).into_iter().map(|l| (l.id, l.definition));
                assert!(defs(text).eq(defs(&same)), "{id}");
            }
        }
    }

    #[test]
    fn rewriting_one_shape_keeps_every_other_byte() {
        let mut d = def(&FILE[blocks(FILE)[0].span.clone()]);
        d.title = "Pair, changed".into();
        let after = splice(FILE, Some("pair"), "pair", &d).unwrap();
        let b = blocks(FILE)[0].span.clone();
        let new_block = block("pair", &d);
        assert_eq!(&after[..b.start], &FILE[..b.start]);
        assert_eq!(&after[b.start + new_block.len()..], &FILE[b.end..]);
        let loaded = load([Layer::from_file(
            crate::types::Origin::BuiltIn,
            Ok(after.clone()),
        )]);
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        assert_eq!(
            loaded.get("pair").unwrap().definition.title,
            "Pair, changed"
        );
        assert_eq!(loaded.glyphs.len(), 1);
    }

    #[test]
    fn a_new_shape_is_appended_and_an_id_is_never_taken_twice() {
        let d = def(&FILE[blocks(FILE)[1].span.clone()]);
        let after = splice(FILE, None, "tock", &d).unwrap();
        assert!(after.starts_with(FILE));
        assert!(after.ends_with(&block("tock", &d)));
        assert!(
            splice(FILE, None, "tick", &d)
                .unwrap_err()
                .contains("already has")
        );
        assert!(
            splice(FILE, Some("pair"), "tick", &d)
                .unwrap_err()
                .contains("already has")
        );
        let fresh = splice("", None, "tock", &d).unwrap();
        assert!(fresh.starts_with("version = 1\n\n[[shapes]]\nid = \"tock\"\n"));
    }

    #[test]
    fn odd_files_are_read_or_refused_never_broken() {
        let d = def(&FILE[blocks(FILE)[1].span.clone()]);
        let ends = format!("{}# done ✓", &FILE[..blocks(FILE)[1].span.end]);
        assert_eq!(blocks(&ends).len(), 2);
        assert!(splice(&ends, Some("tick"), "tick", &d).is_ok());
        let why = splice("# my shapes\n", None, "tick", &d).unwrap_err();
        assert!(why.contains("no version"), "{why}");
        assert_eq!(number(0.33333), "0.33333");
        assert_eq!(number(0.433_012_701_9), "0.433");
        assert_eq!(number(1.0), "1.0");
    }

    #[test]
    fn saving_refuses_a_file_changed_since_it_was_read() {
        let root =
            std::env::temp_dir().join(format!("cold-lava-shape-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join(WORKSPACE_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, FILE).unwrap();
        let d = def(&FILE[blocks(FILE)[1].span.clone()]);
        let Saved::Written(now) = save(&root, Some(FILE.as_bytes()), Some("tick"), "tick", &d)
        else {
            panic!("saved")
        };
        assert_eq!(std::fs::read(&path).unwrap(), now);
        std::fs::write(&path, format!("{FILE}\n# edited elsewhere\n")).unwrap();
        let refused = save(&root, Some(&now), Some("tick"), "tick", &d);
        assert!(matches!(refused, Saved::Refused(ref why) if why.contains("changed since")));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with("# edited elsewhere\n")
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
