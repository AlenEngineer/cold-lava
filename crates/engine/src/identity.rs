//! Identity (docs/foundations.md, 5): the one hash every id and rev is made with, and the last
//! step of every id (`id`: the file, the kind and the owner's key, hashed). Each owner makes its
//! own key: a note's items in `items::identify` (and a typed item's key in the matcher), an
//! owner's pieces and typed items from the place the owner gave. Changing a key moves ids as
//! surely as changing this module does, and either is the owner's decision. Composing a
//! note's key moves with the Markdown walk into its owner in track D's step D-3 (#272). An id
//! is an address, where the item is now; `anchor` names the anchor the engine writes on an
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
    let _ = (address, attempt);
    String::new()
}

/// Fingerprint of some text, as 12 hex digits: a note item's lines or a toggled line, or JSON
/// text (a piece or a typed item an owner gave, an item as its answer wrote it). Line breaks
/// and carriage returns at its end are not hashed.
pub fn line_id(line: &str) -> String {
    let h = fnv(line.trim_end_matches(['\r', '\n']).as_bytes());
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
}
