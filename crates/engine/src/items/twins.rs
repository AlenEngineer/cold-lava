//! Twins (#327). `identify` keys the items of a kind with one anchor `anchor`, `anchor.1`,
//! `anchor.2` and so on, and the items keyed under an item (its list items, the inline items
//! in it) by its key. Twins are items of one kind whose keys differ only in those counts. A
//! count is a place among twins, so when a twin comes or goes, each twin after it takes the
//! address before its own, with the same rev when the lines are the same: an edit held by that
//! address would land on another item, and no subscriber would hear that anything changed. So
//! an item one of whose twins has the same lines has a rev that also says how many there are
//! of it and its twins, and, when the item it is keyed under has twins too, that item's rev
//! (`identity::twin_rev`): such an edit is refused as changed. Every other item keeps the rev
//! of its lines, a twin whose lines tell it apart included. A leaf: it reads what each anchor
//! is made of and the revs, and says nothing to anyone.

use std::collections::HashMap;

/// An item of a note as `identify` keyed it.
pub(super) struct Keyed<'a> {
    pub kind: &'a str,
    /// The item it is keyed under: its anchor is that item's key, then `own`.
    pub under: Option<usize>,
    /// What its anchor has after the key of the item it is under; under none, all of it.
    pub own: &'a str,
    /// The rev of its own lines.
    pub rev: &'a str,
}

/// What an item's rev says besides its lines.
#[derive(Debug, PartialEq)]
pub(super) struct Twin {
    /// How many there are of it and its twins.
    pub twins: usize,
    /// The item it is keyed under, when that one has twins too: its rev says which it is.
    pub under: Option<usize>,
}

/// For each of `items` (an item before the items keyed under it), what its rev says besides
/// its lines, when one of its twins has the same lines.
pub(super) fn of(items: &[Keyed]) -> Vec<Option<Twin>> {
    // Each key without the counts in it, those of the items it is under included: what an
    // item and its twins share.
    let mut bare: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let under = item.under.map_or("", |p| bare[p].as_str());
        let key = format!("{under}{}", item.own);
        bare.push(key);
    }
    let mut twins: HashMap<(&str, &str), usize> = HashMap::new();
    let mut alike: HashMap<(&str, &str, &str), usize> = HashMap::new();
    for (item, key) in items.iter().zip(&bare) {
        let key = key.as_str();
        *twins.entry((item.kind, key)).or_default() += 1;
        *alike.entry((item.kind, key, item.rev)).or_default() += 1;
    }
    let count = |i: usize| twins[&(items[i].kind, bare[i].as_str())];
    (items.iter().enumerate())
        .map(|(i, item)| {
            let alone = alike[&(item.kind, bare[i].as_str(), item.rev)] == 1;
            (!alone).then(|| Twin {
                twins: count(i),
                under: item.under.filter(|&p| count(p) > 1),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(kind, under, own, rev)`, as `identify` keys items.
    fn of_these(items: &[(&str, Option<usize>, &str, &str)]) -> Vec<Option<Twin>> {
        let keyed: Vec<Keyed> = (items.iter())
            .map(|&(kind, under, own, rev)| Keyed {
                kind,
                under,
                own,
                rev,
            })
            .collect();
        of(&keyed)
    }

    fn twin(twins: usize, under: Option<usize>) -> Option<Twin> {
        Some(Twin { twins, under })
    }

    #[test]
    fn twins_with_the_same_lines_are_counted_and_twins_told_apart_by_theirs_are_not() {
        let same = of_these(&[
            ("task", None, "/a", "r"),
            ("task", None, "/a", "r"),
            ("task", None, "/a", "r"),
            ("list", None, "/a", "r"),
        ]);
        assert_eq!(same, [twin(3, None), twin(3, None), twin(3, None), None]);
        let told_apart = of_these(&[
            ("task", None, "/a", "r"),
            ("task", None, "/a", "s"),
            ("task", None, "/a", "r"),
        ]);
        assert_eq!(told_apart, [twin(3, None), None, twin(3, None)]);
        let alone = of_these(&[("task", None, "/a", "r"), ("task", None, "/b", "r")]);
        assert_eq!(alone, [None, None]);
    }

    /// The items keyed under two twins are twins too, and each says which of them it is
    /// under; under an item with no twins, not.
    #[test]
    fn what_is_keyed_under_twins_is_twins_and_says_which_it_is_under() {
        let under_twins = of_these(&[
            ("task", None, "/p", "p1"),
            ("task", Some(0), "/c", "c"),
            ("task", None, "/p", "p2"),
            ("task", Some(2), "/c", "c"),
        ]);
        let want = [None, twin(2, Some(0)), None, twin(2, Some(2))];
        assert_eq!(under_twins, want);
        let under_one = of_these(&[
            ("task", None, "/p", "p"),
            ("list", Some(0), "/c", "c"),
            ("list", Some(0), "/c", "c"),
        ]);
        assert_eq!(under_one, [None, twin(2, None), twin(2, None)]);
        // Three deep: under twins whose own lines differ, what is under each is told apart
        // by its parent's rev.
        let deep = of_these(&[
            ("list", None, "/p", "p"),
            ("list", Some(0), "/c", "c1"),
            ("list", Some(1), "/g", "g"),
            ("list", None, "/p", "p"),
            ("list", Some(3), "/c", "c2"),
            ("list", Some(4), "/g", "g"),
        ]);
        let want = [
            twin(2, None),
            None,
            twin(2, Some(1)),
            twin(2, None),
            None,
            twin(2, Some(4)),
        ];
        assert_eq!(deep, want);
    }
}
