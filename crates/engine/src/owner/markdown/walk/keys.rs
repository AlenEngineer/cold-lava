//! A note's keys (`identity::id`'s last part, before the note and the kind): a block id, a
//! heading's path, or a list item's or a typed item's text hashed under the item it is in, then
//! `.n` for the nth repeat of a kind's anchor. Changing a key moves ids.

use crate::identity;
use crate::items::{Data, Keys, Raw};
use std::collections::HashMap;

/// Each item's key, in document order (a parent before its children).
pub(super) fn of(raws: &[Raw]) -> Keys {
    let mut keys: Vec<String> = Vec::with_capacity(raws.len());
    // What each key's anchor is made of (`anchor_of`).
    let mut anchors: Vec<(Option<usize>, String)> = Vec::with_capacity(raws.len());
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    // Whether an anchor was counted twice: only then can two items be twins.
    let mut repeated = false;
    for raw in raws {
        let kind = raw.data.kind();
        let (under, own) = anchor_of(raw);
        let anchor = format!("{}{own}", under.map_or("", |p| keys[p].as_str()));
        let n = seen.entry((kind.to_string(), anchor.clone())).or_default();
        let key = match *n {
            0 => anchor,
            n => format!("{anchor}.{n}"),
        };
        repeated |= *n > 0;
        *n += 1;
        keys.push(key);
        anchors.push((under, own));
    }
    Keys {
        keys,
        anchors,
        repeated,
    }
}

/// What an item's anchor, its key before `of` counts it, is made of: the item it is
/// keyed under, whose key the anchor starts with, and what the anchor has after that key (all
/// of it, under none).
fn anchor_of(raw: &Raw) -> (Option<usize>, String) {
    let hashed = |text: &str| format!("/{}", identity::hash(text.as_bytes()));
    match (&raw.block_id, &raw.data) {
        (Some(id), _) => (None, format!("^{id}")),
        (None, Data::Heading { text, .. }) => {
            let mut path = raw.section.join("#");
            if !raw.section.is_empty() {
                path.push('#');
            }
            path.push_str(text);
            (None, path)
        }
        (None, Data::List { text }) => (raw.parent, hashed(text)),
        (None, Data::Typed(_)) if raw.key.is_empty() => (raw.parent, String::new()),
        (None, Data::Typed(_)) => (raw.parent, hashed(&raw.key)),
        (None, Data::Block) => (None, String::new()),
    }
}
