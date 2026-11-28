//! Where a block's words are in its source, and its text as an item holds it: a heading's, a
//! list item's, a table row's cells.

use crate::matcher;
use std::ops::Range;

/// A heading's words in the source: ATX without its `#`s, setext without its underline.
pub(super) fn heading_range(src: &str, range: Range<usize>) -> Range<usize> {
    let raw = &src[range.clone()];
    let first = raw.lines().next().unwrap_or_default();
    let indent = first.len() - first.trim_start().len();
    if first.trim_start().starts_with('#') {
        let after = first[indent..].trim_start_matches('#');
        let start = range.start + first.len() - after.trim_start().len();
        let mut words = after.trim();
        let closed = words.trim_end_matches('#');
        if closed.is_empty() || closed.ends_with([' ', '\t']) {
            words = closed.trim_end();
        }
        start..start + words.len()
    } else {
        let body = raw.trim_end();
        let end = body.rfind('\n').unwrap_or(0);
        let words = body[..end].trim();
        let start = range.start + body.find(words).unwrap_or(0);
        start..start + words.len()
    }
}

/// The cells of the table row starting at `start`, as pulldown-cmark split them: at each `|`
/// that isn't escaped, a wikilink's too, then cut to the table's `cols` or filled with empty
/// cells after the row's line break.
pub(super) fn row_cells(src: &str, start: usize, cols: usize) -> Vec<Range<usize>> {
    let b = src.as_bytes();
    let eol = src[start..]
        .find(['\n', '\r'])
        .map_or(src.len(), |i| start + i);
    let after = match &b[eol..] {
        [b'\r', b'\n', ..] => eol + 2,
        [_, ..] => eol + 1,
        [] => eol,
    };
    let mut cells = Vec::new();
    let mut i = start;
    loop {
        if b.get(i) == Some(&b'|') {
            i += 1;
        }
        let cell = i;
        while i < eol && matches!(b[i], b' ' | b'\t') {
            i += 1;
        }
        if i >= eol {
            break;
        }
        while i < eol && b[i] != b'|' {
            let escape = b[i] == b'\\' && b.get(i + 1).is_some_and(u8::is_ascii_punctuation);
            i += match (escape, b.get(i + 1..i + 3)) {
                (true, Some(b"\\|")) => 3,
                (true, _) => 2,
                _ => 1,
            };
        }
        cells.push(cell..i);
    }
    cells.truncate(cols);
    cells.resize(cols, after..after);
    cells
}

/// A table cell without its padding and pipes.
pub(super) fn trim_cell(src: &str, range: Range<usize>) -> Range<usize> {
    let pad = [' ', '\t', '|', '\r', '\n'];
    let cell = &src[range.clone()];
    let start = range.start + cell.len() - cell.trim_start_matches(pad).len();
    let end = range.end - (cell.len() - cell.trim_end_matches(pad).len());
    start..end.max(start)
}

/// Where a list item's words start on its first line: after its marker and the space.
pub(super) fn list_text_start(first: &str) -> usize {
    let t = first.trim_start();
    let marker = t.strip_prefix(['-', '*', '+']).unwrap_or_else(|| {
        t.trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')'])
    });
    first.len() - marker.trim_start().len()
}

/// A heading's text as written: ATX without its `#`s, setext without its underline.
pub(super) fn heading_text(raw: &str) -> String {
    let raw = raw.trim();
    let text = match raw.strip_prefix('#') {
        Some(_) => {
            let t = raw
                .lines()
                .next()
                .unwrap_or_default()
                .trim_start_matches('#');
            let closed = t.trim_end().trim_end_matches('#');
            if closed.is_empty() || closed.ends_with([' ', '\t']) {
                closed.to_string()
            } else {
                t.to_string()
            }
        }
        None => {
            let lines: Vec<&str> = raw.lines().collect();
            lines[..lines.len().saturating_sub(1)].join(" ")
        }
    };
    text.trim().to_string()
}

/// A list item's first line without its marker.
pub(super) fn list_text(first: &str) -> String {
    let t = first.trim_start();
    let t = t
        .strip_prefix(['-', '*', '+'])
        .unwrap_or_else(|| t.trim_start_matches(|c: char| c.is_ascii_digit()))
        .trim_start_matches(['.', ')']);
    let mut text = String::with_capacity(t.len());
    matcher::push_words(&mut text, t);
    text
}
