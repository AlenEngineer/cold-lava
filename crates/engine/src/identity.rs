//! Identity (docs/foundations.md, 5): the one hash every id and rev is made with, and the last
//! step of every id (`id`: the file, the kind and the owner's key, hashed). Each owner makes its
//! own key and calls `id` itself: a note's items in the Markdown owner
//! (`owner/markdown/walk/keys.rs`, a typed item's key too, then `ids.rs`), an
//! owner's pieces and typed items from the place the owner gave (`owner/given.rs`). Changing a
//! key moves ids as surely as changing this module does, and either is the owner's decision.
//! An id is an address, where the item is now; `anchor` names the anchor the engine writes on an
//! item when something keeps a reference to it (#190). A leaf: it needs nothing but `std`.
//! The tests pin ids the engine gave before this module.

/// FNV-1a of some bytes: stable across runs and machines, unlike `std`'s hasher.
pub fn fnv(bytes: &[u8]) -> u64 {
    fnv_from(START, bytes)
}

const START: u64 = 0xcbf2_9ce4_8422_2325;

/// `fnv` of what was hashed into `h`, then `bytes`.
fn fnv_from(h: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(h, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

/// `fnv` of some bytes, as 16 hex digits.
pub fn hash(bytes: &[u8]) -> String {
    hex(fnv(bytes))
}

fn hex(h: u64) -> String {
    format!("{h:016x}")
}

/// An id: `hash` of its parts with a NUL between each two.
pub fn id(parts: &[&str]) -> String {
    hex(of_parts(parts))
}

fn of_parts(parts: &[&str]) -> u64 {
    let mut h = START;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            h = fnv_from(h, b"\0");
        }
        h = fnv_from(h, part.as_bytes());
    }
    h
}

/// The name of the anchor the engine writes on the item whose address is `address` (its id),
/// at the `attempt`th try (0 first) when the names before it are taken: six characters of
/// `a-z0-9`, the form Obsidian gives the names it writes. Derived, not random, so two
/// processes anchoring one item write the same name.
pub fn anchor(address: &str, attempt: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut h = of_parts(&[address, &attempt.to_string()]) % 36u64.pow(6);
    let mut name = [b'0'; 6];
    for digit in name.iter_mut().rev() {
        *digit = DIGITS[(h % 36) as usize];
        h /= 36;
    }
    name.iter().map(|&b| b as char).collect()
}

/// Fingerprint of some text, as 12 hex digits: a note item's lines or a toggled line, or JSON
/// text (a piece or a typed item an owner gave, an item as its answer wrote it). Line breaks
/// and carriage returns at its end are not hashed.
pub fn line_id(line: &str) -> String {
    rev(fnv(line.trim_end_matches(['\r', '\n']).as_bytes()))
}

/// The rev of a note's item that one of its twins could be taken for (`items::twins`, #327):
/// of the rev of its lines, how many there are of it and its twins and, when the item it is
/// under has twins too, that item's rev, with a NUL between each two, in `line_id`'s form.
pub fn twin_rev(rev_of_lines: &str, twins: usize, under: Option<&str>) -> String {
    let twins = twins.to_string();
    rev(match under {
        Some(under) => of_parts(&[rev_of_lines, &twins, under]),
        None => of_parts(&[rev_of_lines, &twins]),
    })
}

/// 48 bits of a hash, as 12 hex digits: a rev.
fn rev(h: u64) -> String {
    format!("{:012x}", h & 0xffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each id is one the engine gave before this module, copied from the test or golden that
    /// pins it, so a change here that moves an id fails.
    #[test]
    fn ids_given_before_are_given_again() {
        // A note's item with a block id, `tests/tasks/plain.jsonl`.
        assert_eq!(id(&["Plans.md", "task", "^rent"]), "a9c8726d8700a55e");
        assert_eq!(
            id(&["Daily/2026-11-05.md", "task", "^rent"]),
            "5603b580f1d302bf"
        );
        // An owner's piece and typed item, `owner::given`'s pinned ids.
        assert_eq!(
            id(&["Home.chores", "task", "t1", "piece"]),
            "e5b0fa4e80ce14e0"
        );
        assert_eq!(id(&["B.csv", "eruption", "3"]), "63d323788612c4e8");
    }

    /// Each name is computed apart from this module, from the addresses pinned above.
    #[test]
    fn an_anchors_name_is_derived_from_the_address_and_the_attempt() {
        assert_eq!(anchor("a9c8726d8700a55e", 0), "uk33oy");
        assert_eq!(anchor("a9c8726d8700a55e", 1), "ygx5ph");
        assert_eq!(anchor("8c129b98cb8ad550", 0), "79el6x");
        assert_eq!(anchor("8c129b98cb8ad550", 1), "3ckj6e");
        for attempt in 0..200 {
            let name = anchor("5603b580f1d302bf", attempt);
            let form = name.len() == 6
                && name
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
            assert!(form, "{name}");
        }
    }

    #[test]
    fn revs_given_before_are_given_again() {
        // `tests/tasks/plain.jsonl`, lines 5 and 29 of `tests/tasks/corpus.md`.
        assert_eq!(line_id("- [ ] Pay rent 📅 2026-11-12"), "559e02f5813d");
        assert_eq!(
            line_id("- [ ] Block id 📅 2026-11-12 ^rent\r\n"),
            "00c0a9b28be7"
        );
    }

    /// Each rev is computed apart from this module.
    #[test]
    fn a_twins_rev_is_derived_from_its_lines_its_twins_and_what_it_is_under() {
        let rent = line_id("- [ ] Pay rent");
        assert_eq!(rent, "022115ba59e3");
        assert_eq!(twin_rev(&rent, 3, None), "00121f818752");
        assert_eq!(twin_rev(&rent, 2, None), "01121f818905");
        let transfer = line_id("  - [ ] Transfer");
        let under = line_id("- [ ] Rent 📅 2026-11-01");
        assert_eq!((&*transfer, &*under), ("69ec5d6b8a55", "06886ce81dcb"));
        assert_eq!(twin_rev(&transfer, 2, Some(&under)), "1216e37efa72");
    }
}
