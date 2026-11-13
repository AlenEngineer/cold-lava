//! Which of serde's words about an answer that doesn't read may be shown. serde names the
//! engine's own Rust types in its messages in more ways than anyone can list (`untagged enum
//! NAME`, `expected struct NAME`, …), so a message is kept only in the forms known to carry
//! nothing but what the answer wrote and a kind of value (`only_the_answer`). A leaf: it
//! reads text, and knows serde_json's errors only to take off where in a text one was.

/// serde's message, without where it was in a text (` at line N column M`), which a value
/// read as a value never has.
pub fn message_of(e: &serde_json::Error) -> String {
    let whole = e.to_string();
    let at = format!(" at line {} column {}", e.line(), e.column());
    match e.line() {
        0 => whole,
        _ => whole.strip_suffix(&at).map(String::from).unwrap_or(whole),
    }
}

/// Whether serde's `message` is one of the forms that carry only what the answer wrote (a
/// field's or a variant's name, a value) and what a value should have been. The shapes are
/// serde 1.0.229's own (`serde_core::de::Error`'s defaults and `OneOf`):
///
/// - `missing field `NAME``, `duplicate field `NAME``;
/// - `unknown field `NAME`, expected LIST`, `unknown field `NAME`, there are no fields`, and
///   the same for `unknown variant` (`there are no variants`), where LIST is serde's list of
///   the names the answer may use: `` `a` ``, `` `a` or `b` `` or `` one of `a`, `b`, `c` ``
///   (every enum read from an answer renames its variants for the wire, and the list is of
///   those names);
/// - `invalid type: …, expected KIND`, `invalid value: …, expected KIND`,
///   `invalid length N, expected KIND`, where KIND, what serde writes after the last
///   `, expected ` (after the answer's own value, which may hold those words), is a kind of
///   value (`is_a_kind`).
///
/// serde writes NAME, the answer's own, between backticks without escaping it, so NAME is
/// what stands between the first backtick and the next, and the form is kept only when what
/// follows it is exactly serde's own continuation: a name that holds a backtick ends early,
/// and the rest of it is not serde's.
pub fn only_the_answer(message: &str) -> bool {
    for form in ["missing field `", "duplicate field `"] {
        if let Some(rest) = message.strip_prefix(form) {
            return rest
                .split_once('`')
                .is_some_and(|(_, after)| after.is_empty());
        }
    }
    for (form, none) in [
        ("unknown field `", ", there are no fields"),
        ("unknown variant `", ", there are no variants"),
    ] {
        if let Some(rest) = message.strip_prefix(form) {
            let Some((_, after)) = rest.split_once('`') else {
                return false;
            };
            return after == none
                || after
                    .strip_prefix(", expected ")
                    .is_some_and(is_serdes_list);
        }
    }
    let valued = ["invalid type: ", "invalid value: ", "invalid length "];
    if !valued.iter().any(|form| message.starts_with(form)) {
        return false;
    }
    message
        .rsplit_once(", expected ")
        .is_some_and(|(_, kind)| is_a_kind(kind))
}

/// Whether `list` is serde's list of names (`OneOf`): `` `a` ``, `` `a` or `b` ``, or
/// `` one of `a`, `b`, `c` ``, each name between backticks and holding none.
fn is_serdes_list(list: &str) -> bool {
    let one = |name: &str| {
        let inner = name.strip_prefix('`').and_then(|n| n.strip_suffix('`'));
        inner.is_some_and(|n| !n.is_empty() && !n.contains('`'))
    };
    match list.strip_prefix("one of ") {
        Some(names) => names.split(", ").count() > 2 && names.split(", ").all(one),
        None => match list.split_once(" or ") {
            Some((a, b)) => one(a) && one(b),
            None => one(list),
        },
    }
}

/// Whether serde's `expected` part is a kind of value, not a Rust item: made only of the words
/// serde and serde_json write for kinds of value in what an answer is read into (serde's
/// `de::impls` and `Unexpected`, serde_json's value deserializer: `a string`, `a map`, `a
/// sequence`, `an array of length 2`, `string or map`, `map with a single key`, `option`,
/// `unit`), and a number's type (`u8`, `f64`). A word it doesn't know, a struct, an enum or a
/// variant reads "as one"; so does serde's jargon (`variant identifier`), which names nothing
/// but helps no one.
fn is_a_kind(kind: &str) -> bool {
    const NUMBERS: [&str; 14] = [
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
        "f32", "f64",
    ];
    const WORDS: [&str; 24] = [
        "a",
        "an",
        "or",
        "of",
        "with",
        "string",
        "map",
        "sequence",
        "array",
        "boolean",
        "character",
        "byte",
        "borrowed",
        "empty",
        "single",
        "key",
        "unit",
        "option",
        "integer",
        "number",
        "null",
        "length",
        "elements",
        "element",
    ];
    let word = |w: &str| WORDS.contains(&w) || w.chars().all(|c| c.is_ascii_digit());
    NUMBERS.contains(&kind) || (!kind.is_empty() && kind.split(' ').all(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::{Error as _, Unexpected};

    #[test]
    fn only_the_forms_that_say_what_the_answer_wrote_pass() {
        assert!(only_the_answer(
            "unknown field `colour`, expected `place` or `text`"
        ));
        assert!(only_the_answer("missing field `name`"));
        assert!(only_the_answer(
            "invalid type: integer `3`, expected a string"
        ));
        assert!(!only_the_answer(
            "data did not match any variant of untagged enum Run"
        ));
        assert!(!only_the_answer(
            "invalid type: map, expected struct NewText"
        ));
        let at = serde_json::from_str::<u8>("\"x\"").unwrap_err();
        assert_eq!(message_of(&at), "invalid type: string \"x\", expected u8");
        let value = serde_json::Error::invalid_value(Unexpected::Signed(-1), &"u8");
        assert_eq!(
            message_of(&value),
            "invalid value: integer `-1`, expected u8"
        );
    }
}
