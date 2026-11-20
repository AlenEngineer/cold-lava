//! The anchor rule (docs/foundations.md, 5): an edit keeps every block id (`^rent`, the anchor
//! links point at) on the item that had it. None is lost, none moves to another item, and none
//! appears on an item that didn't have it, but one: the edited item may take back an id already
//! written in its own text (an untick that takes a done date off `^rent ✅ 2026-11-07`, which
//! cold-lava 0.6.0 wrote, gives the task its anchor again). Set-field and the tick both read
//! their edit back through `change`. A leaf: it reads items and text, and says nothing to
//! anyone.

use super::Item;
use std::collections::HashMap;
use std::fmt;

/// How an edit would change an anchor. `from` is where the item started before the edit;
/// `to` and `on` are where it starts after it.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Lost { id: String, from: usize },
    Moved { id: String, from: usize, to: usize },
    Made { id: String, on: usize },
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Change::Lost { id, from } => {
                write!(f, "the item on line {from} would lose its anchor ^{id}")
            }
            Change::Moved { id, from, to } => write!(
                f,
                "the anchor ^{id} would move from the item on line {from} to the one on line {to}"
            ),
            Change::Made { id, on } => {
                write!(f, "the item on line {on} would take ^{id} as a new anchor")
            }
        }
    }
}

/// The first way `after` breaks the anchor rule, if any. `same[i]` is the item of `after` that
/// `before[i]` became (none: it is gone), `edited` the index in `before` of the item the edit
/// was for, and `text` the note before the edit.
pub fn change(
    before: &[Item],
    after: &[Item],
    same: &[Option<usize>],
    edited: usize,
    text: &str,
) -> Option<Change> {
    let mut was = vec![None; after.len()];
    for (i, j) in same.iter().enumerate() {
        if let Some(j) = j {
            was[*j] = Some(i);
        }
    }
    let had = |j: usize| was[j].and_then(|i: usize| before[i].block_id.as_deref());
    for (i, b) in before.iter().enumerate() {
        let Some(id) = b.block_id.as_deref() else {
            continue;
        };
        if same[i].is_some_and(|j| after[j].block_id.as_deref() == Some(id)) {
            continue;
        }
        let to = (0..after.len())
            .find(|&j| after[j].block_id.as_deref() == Some(id) && had(j) != Some(id));
        let id = id.to_string();
        return Some(match to {
            Some(j) => Change::Moved {
                id,
                from: b.line,
                to: after[j].line,
            },
            None => Change::Lost { id, from: b.line },
        });
    }
    let restored = |j: usize, id: &str| {
        let e = &before[edited];
        same[edited] == Some(j) && e.block_id.is_none() && written(text, e, id)
    };
    after.iter().enumerate().find_map(|(j, a)| {
        let id = a.block_id.as_deref()?;
        (had(j) != Some(id) && !restored(j, id)).then(|| Change::Made {
            id: id.to_string(),
            on: a.line,
        })
    })
}

/// `same` for an edit that moved items by whole lines: `before[i]` became the item of
/// `after` of its kind that starts on `line(&before[i])`, the nth of them there as it is the
/// nth of the items of `before` that land there.
pub fn by_line(
    before: &[Item],
    after: &[Item],
    line: impl Fn(&Item) -> usize,
) -> Vec<Option<usize>> {
    let mut starts: HashMap<(usize, &str), Vec<usize>> = HashMap::new();
    for (j, a) in after.iter().enumerate() {
        starts.entry((a.line, a.data.kind())).or_default().push(j);
    }
    let mut seen: HashMap<(usize, &str), usize> = HashMap::new();
    before
        .iter()
        .map(|b| {
            let key = (line(b), b.data.kind());
            let nth = seen.entry(key).or_default();
            let j = starts.get(&key).and_then(|js| js.get(*nth));
            *nth += 1;
            j.copied()
        })
        .collect()
}

/// Whether `^id` is written in `item`'s own lines of `text`, as a word of its own.
fn written(text: &str, item: &Item, id: &str) -> bool {
    let tag = format!("^{id}");
    let own = text.lines().skip(item.line - 1);
    own.take(item.end_line + 1 - item.line).any(|line| {
        line.match_indices(&tag).any(|(at, _)| {
            let after = &line[at + tag.len()..];
            line[..at]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
                && !after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{self, tests::settings};

    fn read(text: &str) -> Vec<Item> {
        items::extract("N.md", text, &settings("obsidian"))
    }

    fn same_lines(before: &str, after: &str) -> Option<Change> {
        let (b, a) = (read(before), read(after));
        change(&b, &a, &by_line(&b, &a, |i| i.line), 0, before)
    }

    #[test]
    fn an_anchor_kept_on_its_item_is_no_change() {
        assert_eq!(
            same_lines("- one ^a\n- two\n", "- one more ^a\n- two\n"),
            None
        );
    }

    #[test]
    fn an_anchor_lost_moved_or_made_is_a_change() {
        assert_eq!(
            same_lines("- one ^a\n- two\n", "- one\n- two\n"),
            Some(Change::Lost {
                id: "a".into(),
                from: 1
            })
        );
        assert_eq!(
            same_lines("- one ^a\n- two\n", "- one\n- two ^a\n"),
            Some(Change::Moved {
                id: "a".into(),
                from: 1,
                to: 2
            })
        );
        assert_eq!(
            same_lines("- one\n- two\n", "- one\n- two ^b\n"),
            Some(Change::Made {
                id: "b".into(),
                on: 2
            })
        );
    }

    #[test]
    fn the_edited_item_takes_back_only_an_id_written_in_its_own_text() {
        let back = "- one ^a ✅ 2026-11-07\n- two ^b x\n";
        assert_eq!(
            same_lines(back, "- one ^a\n- two ^b x\n"),
            None,
            "written there, on the edited item"
        );
        assert_eq!(
            same_lines(back, "- one ^a ✅ 2026-11-07\n- two ^b\n"),
            Some(Change::Made {
                id: "b".into(),
                on: 2
            }),
            "written there, but not on the edited item"
        );
        assert_eq!(
            same_lines("- one ^ab ✅ 2026-11-07\n", "- one ^a\n"),
            Some(Change::Made {
                id: "a".into(),
                on: 1
            }),
            "^ab is not ^a"
        );
        assert_eq!(
            same_lines("- one ^z ✅ 2026-11-07\n", "- one ^a\n"),
            Some(Change::Made {
                id: "a".into(),
                on: 1
            }),
            "never written"
        );
    }

    #[test]
    fn items_that_moved_down_a_line_are_paired_by_where_they_start() {
        let (b, a) = (read("- one\n- two ^a\n"), read("- new\n- one\n- two ^a\n"));
        let same = by_line(&b, &a, |i| i.line + 1);
        assert_eq!(same, vec![Some(1), Some(2)]);
        assert_eq!(change(&b, &a, &same, 0, ""), None);
    }
}
