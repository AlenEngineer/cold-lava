//! Finding notes: fuzzy matching for the quick switcher, and full-text search.
//! Both ignore case and accents ("kil" finds "Kīlauea").

use crate::index::Index;
use crate::vault::search_key;
use serde::Serialize;

// ---------- folding with a map back to the original text ----------

/// `search_key` of `s`, plus, for each char of the result, the index of the original char.
fn fold_map(s: &str) -> (Vec<char>, Vec<usize>) {
    let mut out = Vec::new();
    let mut map = Vec::new();
    for (i, c) in s.chars().enumerate() {
        let mut buf = [0u8; 4];
        for f in search_key(c.encode_utf8(&mut buf)).chars() {
            out.push(f);
            map.push(i);
        }
    }
    (out, map)
}

fn is_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '/' | '.' | '(' | '[' | ',' | ':' | '\'')
}

// ---------- fuzzy matching ----------

#[derive(Clone, Debug, PartialEq)]
pub struct Fuzzy {
    pub score: i64,
    /// Indices of the matched chars in the candidate (for highlighting).
    pub positions: Vec<usize>,
}

/// Fuzzy-match `query` against `candidate`: all query chars in order. Rewards matches at the
/// start, at word starts and in runs; penalises gaps and long candidates.
pub fn fuzzy(query: &str, candidate: &str) -> Option<Fuzzy> {
    let q: Vec<char> = search_key(query)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Some(Fuzzy {
            score: 0,
            positions: Vec::new(),
        });
    }
    let (c, map) = fold_map(candidate);
    let mut best: Option<(i64, Vec<usize>)> = None;
    // Try every place the first char matches, then match the rest greedily, preferring
    // word starts when one is reachable before the next plain match.
    for start in (0..c.len()).filter(|&i| c[i] == q[0]) {
        let mut pos = vec![start];
        let mut i = start + 1;
        let mut ok = true;
        for &qc in &q[1..] {
            let next = (i..c.len()).find(|&j| c[j] == qc);
            let Some(n) = next else {
                ok = false;
                break;
            };
            // A word-start occurrence later on is worth more than a mid-word one now,
            // unless the plain one continues a run.
            let word_start =
                (n..c.len()).find(|&j| c[j] == qc && (j == 0 || is_boundary(c[j - 1])));
            let pick = match word_start {
                Some(w) if n != *pos.last().unwrap() + 1 => w,
                _ => n,
            };
            pos.push(pick);
            i = pick + 1;
        }
        if !ok {
            continue;
        }
        let mut score: i64 = 0;
        for (k, &p) in pos.iter().enumerate() {
            score += 10;
            if p == 0 {
                score += 30;
            } else if is_boundary(c[p - 1]) {
                score += 20;
            }
            if k > 0 {
                let prev = pos[k - 1];
                if p == prev + 1 {
                    score += 15;
                } else {
                    score -= ((p - prev - 1) as i64).min(10);
                }
            }
        }
        score -= (c.len() as i64 - q.len() as i64) / 3;
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, pos));
        }
    }
    let (mut score, pos) = best?;
    let folded: String = c.iter().collect();
    let qs: String = q.iter().collect();
    if folded == qs {
        score += 1000;
    } else if folded.starts_with(&qs) {
        score += 200;
    }
    // Map folded positions back to original chars (deduplicated).
    let mut positions: Vec<usize> = pos.into_iter().map(|p| map[p]).collect();
    positions.dedup();
    Some(Fuzzy { score, positions })
}

#[derive(Clone, Debug)]
pub struct SwitchHit {
    pub note: usize,
    pub score: i64,
    /// The alias that matched, if it was an alias rather than the name.
    pub alias: Option<String>,
    /// Matched char positions in the name (or alias).
    pub positions: Vec<usize>,
}

/// Notes for the quick switcher, best first. Matches names, aliases and (weaker) paths.
pub fn switcher(ix: &Index, query: &str) -> Vec<SwitchHit> {
    let mut hits: Vec<SwitchHit> = ix
        .vault
        .notes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let mut best: Option<SwitchHit> = fuzzy(query, &n.name).map(|f| SwitchHit {
                note: i,
                score: f.score,
                alias: None,
                positions: f.positions,
            });
            for a in &n.aliases {
                if let Some(f) = fuzzy(query, a) {
                    let s = f.score - 5; // the real name wins a tie
                    if best.as_ref().is_none_or(|b| s > b.score) {
                        best = Some(SwitchHit {
                            note: i,
                            score: s,
                            alias: Some(a.clone()),
                            positions: f.positions,
                        });
                    }
                }
            }
            if best.is_none() {
                // Folder + name, e.g. "volc hel" for Volcanoes/Mount St. Helens.
                if let Some(f) = fuzzy(query, &n.rel) {
                    best = Some(SwitchHit {
                        note: i,
                        score: f.score - 40,
                        alias: None,
                        positions: Vec::new(),
                    });
                }
            }
            best
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            let (na, nb) = (&ix.vault.notes[a.note].name, &ix.vault.notes[b.note].name);
            na.len()
                .cmp(&nb.len())
                .then(na.to_lowercase().cmp(&nb.to_lowercase()))
        })
    });
    hits
}

// ---------- full-text search ----------

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct LineHit {
    /// 1-based line number.
    pub line: usize,
    pub text: String,
    /// Matched char ranges in `text`: [start, end).
    pub ranges: Vec<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct NoteHits {
    pub note: usize,
    /// The title (file name) matches too.
    pub in_title: bool,
    /// Matching lines in the note (all of them, even if `lines` is capped).
    pub count: usize,
    pub lines: Vec<LineHit>,
}

/// Split a query into terms: words, and "quoted phrases". Folded for matching.
pub fn terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = query.trim();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('"') {
            let end = r.find('"').unwrap_or(r.len());
            out.push(search_key(&r[..end]));
            rest = r.get(end + 1..).unwrap_or("").trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push(search_key(&rest[..end]));
            rest = rest[end..].trim_start();
        }
    }
    out.retain(|t| !t.is_empty());
    out
}

/// Char ranges in `text` where any of `terms` occurs.
fn find_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let (f, map) = fold_map(text);
    let mut ranges = Vec::new();
    for t in terms {
        let t: Vec<char> = t.chars().collect();
        if t.is_empty() || t.len() > f.len() {
            continue;
        }
        let mut i = 0;
        while i + t.len() <= f.len() {
            if f[i..i + t.len()] == t[..] {
                ranges.push((map[i], map[i + t.len() - 1] + 1));
                i += t.len();
            } else {
                i += 1;
            }
        }
    }
    ranges.sort();
    // Merge overlaps.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.0 <= last.1 => last.1 = last.1.max(r.1),
            _ => merged.push(r),
        }
    }
    merged
}

/// Notes containing every term (in the text or the title), best first:
/// title matches, then most matching lines. Up to `max_lines` lines per note.
pub fn search(ix: &Index, query: &str, max_lines: usize) -> Vec<NoteHits> {
    let terms = terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let folded = ix.folded();
    let mut out: Vec<NoteHits> = Vec::new();
    for (i, n) in ix.vault.notes.iter().enumerate() {
        let f = &folded[i];
        if !terms
            .iter()
            .all(|t| f.whole.contains(t.as_str()) || f.name.contains(t.as_str()))
        {
            continue;
        }
        let in_title = terms.iter().any(|t| f.name.contains(t.as_str()));
        let mut lines = Vec::new();
        let mut count = 0;
        for (ln, (raw, fl)) in n.text.lines().zip(&f.lines).enumerate() {
            if terms.iter().any(|t| fl.contains(t.as_str())) {
                count += 1;
                if lines.len() < max_lines {
                    let text: String = raw.trim_end().to_string();
                    lines.push(LineHit {
                        line: ln + 1,
                        ranges: find_ranges(&text, &terms),
                        text,
                    });
                }
            }
        }
        out.push(NoteHits {
            note: i,
            in_title,
            count,
            lines,
        });
    }
    out.sort_by(|a, b| {
        b.in_title
            .cmp(&a.in_title)
            .then(b.count.cmp(&a.count))
            .then_with(|| {
                ix.vault.notes[a.note]
                    .name
                    .to_lowercase()
                    .cmp(&ix.vault.notes[b.note].name.to_lowercase())
            })
    });
    out
}

/// What a search result looks like as JSON (for `cold-lava search --json`).
#[derive(Serialize)]
pub struct JsonHit {
    pub file: String,
    pub path: String,
    pub title_match: bool,
    pub count: usize,
    pub lines: Vec<LineHit>,
}

pub fn to_json(ix: &Index, hits: &[NoteHits]) -> Vec<JsonHit> {
    hits.iter()
        .map(|h| {
            let n = &ix.vault.notes[h.note];
            JsonHit {
                file: format!("{}.md", n.rel),
                path: n.path.to_string_lossy().into_owned(),
                title_match: h.in_title,
                count: h.count,
                lines: h.lines.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn example() -> Index {
        Index::build(
            crate::vault::scan(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-vault"))
                .unwrap(),
        )
    }

    #[test]
    fn fuzzy_basics() {
        assert!(fuzzy("msh", "Mount St. Helens").is_some());
        assert!(fuzzy("hsm", "Mount St. Helens").is_none(), "order matters");
        assert_eq!(fuzzy("kil", "Kīlauea").unwrap().positions, [0, 1, 2]);
        let exact = fuzzy("lava", "Lava").unwrap().score;
        let prefix = fuzzy("lava", "Lava lake").unwrap().score;
        let inside = fuzzy("lava", "Volcanic lavas").unwrap().score;
        assert!(
            exact > prefix && prefix > inside,
            "{exact} {prefix} {inside}"
        );
        // Word starts beat scattered letters.
        let word_starts = fuzzy("vei", "Volcanic Explosivity Index").unwrap();
        assert_eq!(word_starts.positions, [0, 9, 21]);
    }

    #[test]
    fn switcher_ranks_names_aliases_and_paths() {
        let ix = example();
        let top = |q: &str| {
            switcher(&ix, q)
                .first()
                .map(|h| ix.vault.notes[h.note].name.clone())
        };
        assert_eq!(top("msh").as_deref(), Some("Mount St. Helens"));
        assert_eq!(top("kil").as_deref(), Some("Kīlauea"));
        assert_eq!(top("vei").as_deref(), Some("Volcanic Explosivity Index"));
        assert_eq!(top("people john").as_deref(), Some("David A. Johnston"));
        assert!(switcher(&ix, "").len() == ix.vault.notes.len());
    }

    #[test]
    fn full_text_search() {
        let ix = example();
        let hits = search(&ix, "pompeii", 5);
        assert_eq!(hits.len(), 1);
        let n = &ix.vault.notes[hits[0].note];
        assert_eq!(n.name, "Vesuvius");
        let l = &hits[0].lines[0];
        let matched: String = l
            .text
            .chars()
            .skip(l.ranges[0].0)
            .take(l.ranges[0].1 - l.ranges[0].0)
            .collect();
        assert_eq!(matched, "Pompeii");
        // All terms must be in the note; accents are ignored; phrases stay together.
        assert!(search(&ix, "pompeii hawaii", 5).is_empty());
        assert!(!search(&ix, "hawaii", 5).is_empty(), "matches Hawaiʻi");
        assert!(!search(&ix, "\"cornfield in Michoacán\"", 5).is_empty());
        assert!(search(&ix, "\"Michoacán cornfield\"", 5).is_empty());
        // Title matches come first.
        let lava = search(&ix, "lava", 3);
        assert_eq!(ix.vault.notes[lava[0].note].name, "Lava");
    }

    #[test]
    fn ranges_map_back_through_accents() {
        assert_eq!(
            find_ranges("Mauna Loa and Kīlauea", &terms("kilauea")),
            [(14, 21)]
        );
        assert_eq!(find_ranges("aaa", &terms("aa")), [(0, 2)]);
        assert_eq!(
            terms("one \"two three\"  four"),
            ["one", "two three", "four"]
        );
    }
}
