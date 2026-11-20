//! Identity (docs/foundations.md, 5): the one hash every id and rev is made with, and the one
//! formula for an id. A leaf: it needs nothing but `std`. Whatever changes here moves ids,
//! which is the owner's decision; the tests pin ids the engine gave before this module.

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

/// An id: `fnv` of its parts with a NUL between each two, as 16 hex digits.
pub fn id(parts: &[&str]) -> String {
    let mut h = START;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            h = fnv_from(h, b"\0");
        }
        h = fnv_from(h, part.as_bytes());
    }
    format!("{h:016x}")
}

/// Fingerprint of a line, as 12 hex digits.
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
