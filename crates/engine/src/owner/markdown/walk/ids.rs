//! A note's ids and revs: the id of each item's key (`keys`, `identity::id`), and the rev of its
//! lines (`identity::line_id`), or a twin's (`twins`). Changing either moves what clients hold.

use super::{Raw, keys, twins};
use crate::identity;
use crate::items::Item;

/// Gives each item its id and rev, in document order (a parent before its children): the id of
/// its key (`keys`), and the rev of its lines, or a twin's (`twins`).
pub(super) fn of(note: &str, text: &str, raws: Vec<Raw>) -> Vec<Item> {
    let keys = keys::of(&raws);
    let lines: Vec<&str> = text.lines().collect();
    let mut items: Vec<Item> = Vec::with_capacity(raws.len());
    // An item's own lines, joined, reused from item to item.
    let mut joined = String::new();
    for (raw, key) in raws.into_iter().zip(&keys.keys) {
        let id = identity::id(&[note, raw.data.kind(), key]);
        joined.clear();
        for (i, line) in (lines.get(raw.line - 1..raw.end_line.min(lines.len())))
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            if i > 0 {
                joined.push('\n');
            }
            joined.push_str(line);
        }
        items.push(Item {
            id,
            note: note.to_string(),
            line: raw.line,
            end_line: raw.end_line,
            section: raw.section,
            parent: raw.parent.map(|p| items[p].id.clone()),
            block_id: raw.block_id,
            rev: identity::line_id(&joined),
            data: raw.data,
        });
    }
    // Skipped for speed (#327): it cost every note about 40 µs, though with no anchor counted
    // twice every key is its anchor and no two of a kind share one, so no item has a twin.
    // A debug build runs it anyway and checks that it changes nothing, on every note a test reads.
    if keys.repeated {
        twin_revs(&mut items, &keys.anchors);
    } else if cfg!(debug_assertions) {
        let mut passed = items.clone();
        twin_revs(&mut passed, &keys.anchors);
        assert_eq!(passed, items, "the skipped twin pass would change {note}");
    }
    items.sort_by_key(|i| i.line);
    items
}

/// Gives the items one of whose twins has the same lines a twin's rev (`twins`): `items` as
/// `of` made them, a parent before its children, and what their anchors are made of.
fn twin_revs(items: &mut [Item], anchors: &[(Option<usize>, String)]) {
    let revs = {
        let keyed: Vec<twins::Keyed> = (items.iter().zip(anchors))
            .map(|(item, (under, own))| twins::Keyed {
                kind: item.data.kind(),
                under: *under,
                own,
                rev: &item.rev,
            })
            .collect();
        // A twin's rev for each twin. The item it is under comes before it, so that one's rev
        // is here when it is a twin's, and else is the rev of its lines.
        let mut revs: Vec<Option<String>> = Vec::with_capacity(keyed.len());
        for (item, twin) in keyed.iter().zip(twins::of(&keyed)) {
            let rev = twin.map(|twin| {
                let under = (twin.under).map(|p| revs[p].as_deref().unwrap_or(keyed[p].rev));
                identity::twin_rev(item.rev, twin.twins, under)
            });
            revs.push(rev);
        }
        revs
    };
    for (item, rev) in items.iter_mut().zip(revs) {
        if let Some(rev) = rev {
            item.rev = rev;
        }
    }
}
