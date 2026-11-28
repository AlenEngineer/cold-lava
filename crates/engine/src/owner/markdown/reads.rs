//! What a type's sources read in a note's text by Markdown's rules (and the plugins' it reads
//! as): Dataview's fields, a checkbox, the checkbox that stands for the `checkbox` attribute,
//! and the block id that ends a block. Leaves, asked by the matcher (`matcher.rs`) for a
//! note's blocks only: a piece an owner gave is never read as Markdown (`matcher/piece.rs`).

use std::ops::Range;

use regex::Regex;

use crate::markdown;
use crate::sources::On;

/// The forms Dataview writes the field `key` in: `[key:: value]`, `(key:: value)`, and
/// `key:: value` at the start of a line.
pub(crate) fn field(key: &str) -> Regex {
    let k = regex::escape(key);
    Regex::new(&format!(
        r"(?m)\[{k}::[ \t]*([^\]\n]*)\]|\({k}::[ \t]*([^)\n]*)\)|^[ \t]*{k}::[ \t]*([^\n]*)"
    ))
    .expect("an escaped key makes a valid pattern")
}

/// Each field `re` (made by `field`) finds in `text`, in order: the whole of it as written, and
/// its value.
pub(crate) fn fields<'t>(
    re: &'t Regex,
    text: &'t str,
) -> impl Iterator<Item = (Range<usize>, Range<usize>)> + 't {
    // Every form has `key::`; most blocks have none.
    let any = text.contains("::") && re.is_match(text);
    (any.then(|| re.captures_iter(text)).into_iter().flatten()).map(|c| {
        let whole = c.get(0).unwrap().range();
        let m = (1..=3).find_map(|i| c.get(i)).expect("one form matched");
        (whole, m.range())
    })
}

/// A checkbox where `text` starts, whatever follows it: one character between brackets, and
/// where its `]` is.
pub(crate) fn checkbox_at_start(text: &str) -> Option<(char, usize)> {
    let symbol = text
        .strip_prefix('[')
        .and_then(|t| t.chars().next())
        .filter(|&c| c != '\n' && text[1 + c.len_utf8()..].starts_with(']'))?;
    Some((symbol, 1 + symbol.len_utf8()))
}

/// An item's checkbox as the attribute: one character between brackets, then whitespace or
/// nothing, and with `with`, only when the text holds it after that.
pub(crate) fn boxed(text: &str, with: Option<&str>) -> Option<(char, usize)> {
    let (symbol, end) = super::checkbox(text)?;
    if let Some(with) = with {
        let after = &text[end + 1..];
        let space = after.chars().next().map_or(0, char::len_utf8);
        if !after[space..].contains(with) {
            return None;
        }
    }
    Some((symbol, end))
}

/// Where the block id that ends a block's text is written, which is not words. A list item's
/// text comes without its block id; a `^x` left on its first line, when more lines follow,
/// names nothing and is words.
pub(crate) fn block_id(on: On, text: &str) -> Option<Range<usize>> {
    if on == On::Item {
        return None;
    }
    let cut = markdown::split_block_id(text)?;
    Some(cut.before.len()..cut.before.len() + cut.written.len())
}
