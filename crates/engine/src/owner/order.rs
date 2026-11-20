//! How one list differs from another list of the same kind of thing, in as few changes as
//! explain it (short of a value repeated a thousand times over, `MORE_PAIRS`): what one holds
//! more of than the other, and what stands elsewhere. The read-back names what an owner
//! changed with it (`given::same`).

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

/// How `got` differs from `want`, each entry by its position in its list, in order.
#[derive(Debug, Default, PartialEq)]
pub struct Diff {
    /// Of `want`, those `got` holds fewer of.
    pub dropped: Vec<usize>,
    /// Of `got`, those `want` holds fewer of.
    pub added: Vec<usize>,
    /// Of those both hold, the ones outside the run `diff` keeps: by their position in
    /// `want`, then in `got`.
    pub moved: Vec<(usize, usize)>,
}

/// How many pairs of equal entries `diff` weighs beyond one for each entry of `want`: about a
/// million. Lists in which no value repeats have at most one pair an entry, so they are
/// always within it, and a diff takes at most about as long as one of lists a million
/// entries longer. Past it (a value given a thousand times in both lists, which an owner
/// that gives what its file holds doesn't), the k-th of each value in `want` is paired with
/// the k-th of it in `got` alone: the run kept is one both lists hold in order, and not
/// always the longest.
const MORE_PAIRS: usize = 1 << 20;

/// How `got` differs from `want`: nothing when they are equal. The run kept is the longest
/// both hold in order, and of those as long, the one keeping the most entries `keep` holds
/// to: what is said to have moved is what the fewest moves explain, and when as few explain
/// either of two entries moving, it is the one `keep` doesn't hold to. Past `MORE_PAIRS`, the
/// run kept is one both hold in order. In O((n + p) log n) for `p` pairs of equal entries,
/// at most `n` and `MORE_PAIRS` together, so an answer of any size is compared.
pub fn diff<K: Eq + Hash>(want: &[K], got: &[K], keep: impl Fn(&K) -> bool) -> Diff {
    within(want, got, keep, MORE_PAIRS)
}

/// What each list holds more of than the other, whatever the order: of `want`, those `got`
/// holds fewer of, and of `got`, those `want` holds fewer of, each value's last ones.
pub fn counts<K: Eq + Hash>(want: &[K], got: &[K]) -> (Vec<usize>, Vec<usize>) {
    let d = pair(want, 0..want.len(), got, 0..got.len());
    (d.dropped, d.added)
}

/// `diff`, weighing every pair of equal entries while there are at most `more` more of them
/// than entries in `want`.
fn within<K: Eq + Hash>(want: &[K], got: &[K], keep: impl Fn(&K) -> bool, more: usize) -> Diff {
    if want == got {
        return Diff::default();
    }
    let mut at: HashMap<&K, Vec<usize>> = HashMap::new();
    for (j, k) in got.iter().enumerate() {
        at.entry(k).or_default().push(j);
    }
    let of = |k: &K| at.get(k).map(Vec::as_slice).unwrap_or_default();
    let pairs = (want.iter()).fold(0usize, |n, k| n.saturating_add(of(k).len()));
    let pairs: Vec<(usize, usize)> = match pairs <= want.len().saturating_add(more) {
        // Each entry of `want` with every equal one of `got`, the last first, so that a run
        // rising in both takes one of them at most.
        true => (want.iter().enumerate())
            .flat_map(|(i, k)| of(k).iter().rev().map(move |&j| (i, j)))
            .collect(),
        false => pair(want, 0..want.len(), got, 0..got.len()).moved,
    };
    let (mut in_w, mut in_g) = (vec![false; want.len()], vec![false; got.len()]);
    for (i, j) in heaviest(&pairs, got.len(), |i| keep(&want[i])) {
        in_w[i] = true;
        in_g[j] = true;
    }
    let rest_w = (0..want.len()).filter(|&i| !in_w[i]);
    let rest_g = (0..got.len()).filter(|&j| !in_g[j]);
    pair(want, rest_w, got, rest_g)
}

/// Of `pairs` (a position in `want` and one in `got`, in `want`'s order, and for one entry of
/// `want` from the last in `got` back), the heaviest run whose positions rise in both. Each
/// pair weighs more than all those `keep` holds to together, so the longest run is kept,
/// and of those, the one with the most of them.
fn heaviest(
    pairs: &[(usize, usize)],
    got: usize,
    keep: impl Fn(usize) -> bool,
) -> Vec<(usize, usize)> {
    let one = pairs.len() as u64 + 1;
    // A Fenwick tree over the positions in `got`: the heaviest run that ends before each, and
    // the pair it ends at.
    let none: (u64, usize) = (0, usize::MAX);
    let mut tree = vec![none; got + 1];
    let mut before = vec![usize::MAX; pairs.len()];
    let mut best = none;
    for (n, &(i, j)) in pairs.iter().enumerate() {
        let mut run = none;
        let mut x = j;
        while x > 0 {
            run = run.max(tree[x]);
            x &= x - 1;
        }
        before[n] = run.1;
        let run = (run.0 + one + u64::from(keep(i)), n);
        let mut x = j + 1;
        while x < tree.len() {
            tree[x] = tree[x].max(run);
            x += x.isolate_lowest_one();
        }
        best = best.max(run);
    }
    let mut run = Vec::new();
    let mut n = best.1;
    while n != usize::MAX {
        run.push(pairs[n]);
        n = before[n];
    }
    run
}

/// How `got` differs from `want` among the entries at `w` and at `g` when no run is kept: the
/// k-th of each value at `w` is paired with the k-th of it at `g`, in `w`'s order, and the
/// rest are dropped or added.
fn pair<K: Eq + Hash>(
    want: &[K],
    w: impl Iterator<Item = usize>,
    got: &[K],
    g: impl Iterator<Item = usize>,
) -> Diff {
    let mut at: HashMap<&K, VecDeque<usize>> = HashMap::new();
    for j in g {
        at.entry(&got[j]).or_default().push_back(j);
    }
    let mut out = Diff::default();
    for i in w {
        match at.get_mut(&want[i]).and_then(VecDeque::pop_front) {
            Some(j) => out.moved.push((i, j)),
            None => out.dropped.push(i),
        }
    }
    out.added = at.into_values().flatten().collect();
    out.added.sort_unstable();
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
    /// is what each list holds more of (`counts` too); what is kept is a run both lists hold
    /// in order, as long as any run the two share, and of those as long, one keeping the
    /// most `X`. Past the bound on pairs, all but the last hold.
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
        let keep = |c: &char| c.is_uppercase();
        let (mut pairs, mut apart) = (0, 0);
        for w in &lists {
            for g in &lists {
                let more = |dropped: &[usize], added: &[usize]| {
                    "abX".chars().all(|c| {
                        let extra =
                            |a: &[char], b: &[char]| count(a, c).saturating_sub(count(b, c));
                        let dropped = dropped.iter().filter(|&&i| w[i] == c).count();
                        let added = added.iter().filter(|&&j| g[j] == c).count();
                        (dropped, added) == (extra(w, g), extra(g, w))
                    })
                };
                let (dropped, added) = counts(w, g);
                assert!(more(&dropped, &added), "{w:?} {g:?}");
                let (d, past) = (diff(w, g, keep), within(w, g, keep, 0));
                apart += usize::from(d != past);
                for (d, longest) in [(d, true), (past, false)] {
                    assert_eq!(d == Diff::default(), w == g, "{w:?} {g:?}");
                    assert!(more(&d.dropped, &d.added), "{w:?} {g:?}");
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
                    if longest {
                        let upper = kept_w.iter().filter(|c| c.is_uppercase()).count();
                        assert_eq!((kept_w.len(), upper), best(w, g), "{w:?} {g:?}");
                    }
                }
                pairs += 1;
            }
        }
        assert_eq!(pairs, 121 * 121);
        assert!(apart > 0, "past the bound, another run is kept");
    }
}
