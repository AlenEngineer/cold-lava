//! How one list differs from another list of the same kind of thing, in as few changes as
//! explain it: what one holds more of than the other, and what stands elsewhere. The
//! read-back names what an owner changed with it (`given::same`).

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// How `got` differs from `want`, each entry by its position in its list, in order. The
/// k-th of a value in one list is the k-th of it in the other.
#[derive(Debug, Default, PartialEq)]
pub struct Diff {
    /// Of `want`, those `got` holds fewer of.
    pub dropped: Vec<usize>,
    /// Of `got`, those `want` holds fewer of.
    pub added: Vec<usize>,
    /// Of those both hold, the ones outside the longest run both hold in the same order: by
    /// their position in `want`, then in `got`.
    pub moved: Vec<(usize, usize)>,
}

/// How `got` differs from `want`: nothing when they are equal. Of the runs kept in order that
/// are as long as any, the one keeping the most entries `keep` holds to is kept, so those
/// are the last to be said to have moved. In O(n log n), so an answer of any size is
/// compared.
pub fn diff<K: Eq + Hash>(want: &[K], got: &[K], keep: impl Fn(&K) -> bool) -> Diff {
    let mut out = Diff::default();
    if want == got {
        return out;
    }
    let mut at: HashMap<&K, VecDeque<usize>> = HashMap::new();
    for (j, k) in got.iter().enumerate() {
        at.entry(k).or_default().push_back(j);
    }
    let mut both: Vec<(usize, usize)> = Vec::new();
    for (i, k) in want.iter().enumerate() {
        match at.get_mut(k).and_then(VecDeque::pop_front) {
            Some(j) => both.push((i, j)),
            None => out.dropped.push(i),
        }
    }
    out.added = at.into_values().flatten().collect();
    out.added.sort_unstable();
    // The run kept: of the entries both hold, in `want`'s order, the heaviest whose positions
    // in `got` rise. Each weighs more than all the `keep` entries together, so the longest
    // run is kept, and of those the one with the most `keep` entries.
    let one = both.len() as u64 + 1;
    let weight = |i: usize| one + u64::from(keep(&want[i]));
    // A Fenwick tree over the positions in `got`: the heaviest run that ends before each, and
    // the entry of `both` it ends at.
    let none: (u64, usize) = (0, usize::MAX);
    let mut tree = vec![none; got.len() + 1];
    let mut before = vec![usize::MAX; both.len()];
    let mut heaviest = none;
    for (n, &(i, j)) in both.iter().enumerate() {
        let mut run = none;
        let mut x = j;
        while x > 0 {
            run = run.max(tree[x]);
            x &= x - 1;
        }
        before[n] = run.1;
        let run = (run.0 + weight(i), n);
        let mut x = j + 1;
        while x < tree.len() {
            tree[x] = tree[x].max(run);
            x += x.isolate_lowest_one();
        }
        heaviest = heaviest.max(run);
    }
    let mut kept = vec![false; both.len()];
    let mut n = heaviest.1;
    while n != usize::MAX {
        kept[n] = true;
        n = before[n];
    }
    let moved = both.into_iter().zip(kept).filter(|(_, kept)| !kept);
    out.moved = moved.map(|(pair, _)| pair).collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(want: &str, got: &str) -> Diff {
        let (w, g): (Vec<char>, Vec<char>) = (want.chars().collect(), got.chars().collect());
        diff(&w, &g, |c| c.is_uppercase())
    }

    /// One change is named alone: an entry moved, added, dropped or given twice.
    #[test]
    fn one_change_is_named_alone() {
        assert_eq!(of("abcd", "abcd"), Diff::default());
        assert_eq!(of("abcd", "dabc").moved, [(3, 0)]);
        assert_eq!(of("abcd", "bcda").moved, [(0, 3)]);
        assert_eq!(of("abcd", "abxcd").added, [2]);
        assert_eq!(of("abcd", "abd").dropped, [2]);
        let twice = of("abcd", "abbcd");
        assert_eq!(
            (twice.added, twice.dropped, twice.moved),
            (vec![2], vec![], vec![])
        );
    }

    /// Where two entries could each be the one that moved, the one `keep` holds to stays.
    #[test]
    fn what_is_kept_stays_where_either_could_have_moved() {
        assert_eq!(of("abX", "aXb").moved, [(1, 2)]);
        assert_eq!(of("aXb", "abX").moved, [(2, 1)]);
        assert_eq!(of("ab", "ba").moved, [(0, 1)], "neither kept: the first");
    }

    /// The longest run two lists hold in the same order, and of those as long, the most
    /// uppercase entries one keeps, by the quadratic table.
    fn best(a: &[char], b: &[char]) -> (usize, usize) {
        let mut t = vec![vec![(0, 0); b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                let skip = t[i + 1][j].max(t[i][j + 1]);
                let (n, upper) = t[i + 1][j + 1];
                t[i][j] = match a[i] == b[j] {
                    true => skip.max((n + 1, upper + usize::from(a[i].is_uppercase()))),
                    false => skip,
                };
            }
        }
        t[0][0]
    }

    /// Every list of up to four of `a`, `b` and `X` against every other (121 lists, a value
    /// repeated or not): nothing is said of two equal lists and something of two that
    /// differ; each entry is dropped, added, moved or kept, once; what is dropped and added
    /// is what each list holds more of; what is kept is a run both lists hold in order, as
    /// long as any run the two share, and of those as long, one keeping the most `X`.
    #[test]
    fn every_short_list_is_told_apart_from_every_other() {
        let mut lists: Vec<Vec<char>> = vec![vec![]];
        let mut last: Vec<Vec<char>> = vec![vec![]];
        for _ in 0..4 {
            last = (last.iter())
                .flat_map(|l| "abX".chars().map(move |c| [l.clone(), vec![c]].concat()))
                .collect();
            lists.extend(last.clone());
        }
        let count = |l: &[char], c: char| l.iter().filter(|x| **x == c).count();
        let once = |mut at: Vec<usize>| {
            let n = at.len();
            at.sort_unstable();
            at.dedup();
            at.len() == n
        };
        let mut pairs = 0;
        for w in &lists {
            for g in &lists {
                let d = diff(w, g, |c| c.is_uppercase());
                assert_eq!(d == Diff::default(), w == g, "{w:?} {g:?}");
                for c in "abX".chars() {
                    let more = |a: &[char], b: &[char]| count(a, c).saturating_sub(count(b, c));
                    let dropped = d.dropped.iter().filter(|&&i| w[i] == c).count();
                    let added = d.added.iter().filter(|&&j| g[j] == c).count();
                    assert_eq!((dropped, added), (more(w, g), more(g, w)), "{w:?} {g:?}");
                }
                let moved_w: Vec<usize> = d.moved.iter().map(|m| m.0).collect();
                let moved_g: Vec<usize> = d.moved.iter().map(|m| m.1).collect();
                assert!(
                    once([d.dropped.clone(), moved_w.clone()].concat()),
                    "{w:?} {g:?}"
                );
                assert!(
                    once([d.added.clone(), moved_g.clone()].concat()),
                    "{w:?} {g:?}"
                );
                let kept_w: Vec<char> = (0..w.len())
                    .filter(|i| !d.dropped.contains(i) && !moved_w.contains(i))
                    .map(|i| w[i])
                    .collect();
                let kept_g: Vec<char> = (0..g.len())
                    .filter(|j| !d.added.contains(j) && !moved_g.contains(j))
                    .map(|j| g[j])
                    .collect();
                assert_eq!(kept_w, kept_g, "{w:?} {g:?}");
                assert!(d.moved.iter().all(|&(i, j)| w[i] == g[j]), "{w:?} {g:?}");
                let upper = kept_w.iter().filter(|c| c.is_uppercase()).count();
                assert_eq!((kept_w.len(), upper), best(w, g), "{w:?} {g:?}");
                pairs += 1;
            }
        }
        assert_eq!(pairs, 121 * 121);
    }
}
