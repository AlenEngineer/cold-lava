//! Words as every reader of a text joins them: one space between each, whatever was between
//! them. A leaf.

/// Adds `text`'s words to `out`, one space between each.
pub(crate) fn push(out: &mut String, text: &str) {
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
}
