//! What a tag's name is, asked by the syntax tree (so the reader, the index, the items and
//! renames), the reader's check that a cut keeps its tags, the Markdown owner's reading of the
//! frontmatter's `tags`, and a name given on its own (the tag dialog, a `tags` edit,
//! rename-tag's options).
//!
//! Two readings stand apart, as they always have. A name given on its own (`is_name`) takes
//! letters and digits as Rust's `char::is_alphanumeric` does, which is more than the
//! pattern's: marks that are part of a word, as `हिंदी`'s vowel signs, and circled letters
//! (`Ⓐ`). In a note's text `#हिंदी` is the tag `ह`, and `#Ⓐ` no tag. And a value of the
//! frontmatter's `tags` is a tag whatever its characters, unless it `is_number`.
//!
//! Two more places still decide what a tag is without asking this module, recorded on #272
//! for D-4's later steps: the matcher's note gate (`matcher.rs`, `Note::new`, which reads the
//! frontmatter's `tags` and `tag` with no number rule), and the matcher's `Value::Tag`
//! (`matcher::parse`: not empty, no whitespace), which gates a `set-field` on a tag field and
//! so can write `#123`, which the tree doesn't read as a tag.

use std::sync::OnceLock;

use regex::Regex;

/// A tag's name in a note's text: letters, digits, `_`, `-` and `/`.
const NAME: &str = r"[\p{L}\p{N}_/\-]+";

/// `#tag` in text: after the start of the text, a space, `(` or `,`. Its name is group 1, and
/// a tag's unless it `is_number`.
pub fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"(?:^|[\s(,])#({NAME})")).unwrap())
}

/// `pattern` in a note's own text, where the markers that open emphasis, a strikethrough or a
/// highlight may stand between what a tag follows and its `#` (`**#tag**`). The syntax tree
/// finds tags with it, and checks the markers, which it reads back from the `#`, against the
/// flavour.
pub fn marked_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(&format!(r"(?:^|[\s(,])[*_~=]*#({NAME})")).unwrap())
}

/// `#123` is no tag: a name needs a character that isn't an ASCII digit.
pub fn is_number(name: &str) -> bool {
    name.chars().all(|c| c.is_ascii_digit())
}

/// Whether `name`, given on its own, is a tag's as Obsidian takes one: letters, digits, `_`,
/// `-` and `/`, and not only digits.
pub fn is_name(name: &str) -> bool {
    (name.chars()).all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/')) && !is_number(name)
}
