//! The names a write's own files take beside the file it writes, and the one way one is
//! claimed (`reserved`). A staged write's name, `.Name.cold-lava-PID-N.tmp`, and the name a
//! file moved aside takes where files can't be swapped, `.Name.cold-lava-PID-N.aside`, are
//! made here (`beside`, `aside`) and read back here (`staged_for`, for the clearing of what a
//! killed save left, `stale`), so they can't drift apart. A kept text's name is `kept.rs`'s.
//! A leaf: std only.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A new, empty file beside `file`, at the first name `name` gives where nothing is
/// (`create_new`): a name no other file has, so only this write ever writes over it.
pub(super) fn reserved(file: &Path, name: fn(&Path) -> PathBuf) -> io::Result<PathBuf> {
    loop {
        let at = name(file);
        match std::fs::File::create_new(&at) {
            Ok(_) => return Ok(at),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// The part of a staged write's name between the file's name and the ids.
const STAGED: &str = ".cold-lava-";

/// A staged write's name beside `file`, `.Name.cold-lava-PID-N.tmp`: hidden, so file watchers
/// (ours and Obsidian's) ignore it, and new each time this process asks (`reserved` makes sure
/// no one else has it).
pub(super) fn beside(file: &Path) -> PathBuf {
    named(file, NEW)
}

/// The name `replace::aside` moves a file to, `.Name.cold-lava-PID-N.aside`: a staged name
/// of its own, so the clearing of a killed save's leftovers can tell the file's only text,
/// moved out by cold-lava itself, from a text a swap took out (`Staged::aside`).
pub(super) fn aside(file: &Path) -> PathBuf {
    named(file, ASIDE)
}

/// The ends of a staged name: a new text's, and a file moved aside.
const NEW: &str = ".tmp";
const ASIDE: &str = ".aside";

fn named(file: &Path, end: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    file.with_file_name(format!(".{name}{STAGED}{}-{n}{end}", std::process::id()))
}

/// What a staged name says: the file it is for, the PID of the cold-lava that staged it, and
/// whether it is a file `replace::aside` moved out (`aside`) rather than a new text or what a
/// swap took out.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Staged<'a> {
    pub file: &'a str,
    pub pid: u32,
    pub aside: bool,
}

/// What a staged write's name says (`Staged`): the names `beside` and `aside` make read, and
/// no other name does (a kept text's, `.Name.cold-lava-kept-…`, never does).
pub(super) fn staged_for(name: &str) -> Option<Staged<'_>> {
    let inner = name.strip_prefix('.')?;
    let (inner, aside) = match inner.strip_suffix(NEW) {
        Some(inner) => (inner, false),
        None => (inner.strip_suffix(ASIDE)?, true),
    };
    let (file, ids) = inner.rsplit_once(STAGED)?;
    let (pid, n) = ids.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if file.is_empty() || !digits(pid) || !digits(n) {
        return None;
    }
    Some(Staged {
        file,
        pid: pid.parse().ok()?,
        aside,
    })
}

#[cfg(test)]
mod tests {
    use super::{Staged, aside, beside, staged_for};
    use std::path::Path;

    #[test]
    fn the_name_beside_makes_is_the_one_staged_for_reads() {
        for file in ["N.md", "a.cold-lava-b.md", "Board 1.canvas", "no-extension"] {
            for (made, aside) in [
                (beside(&Path::new("dir").join(file)), false),
                (aside(&Path::new("dir").join(file)), true),
            ] {
                let name = made.file_name().unwrap().to_str().unwrap();
                let pid = std::process::id();
                assert_eq!(
                    staged_for(name),
                    Some(Staged { file, pid, aside }),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn a_staged_name_reads_and_a_kept_one_doesnt() {
        let staged = |file, pid, aside| Some(Staged { file, pid, aside });
        assert_eq!(
            staged_for(".N.md.cold-lava-12-0.tmp"),
            staged("N.md", 12, false)
        );
        assert_eq!(
            staged_for(".N.md.cold-lava-12-0.aside"),
            staged("N.md", 12, true)
        );
        assert_eq!(
            staged_for(".a.cold-lava-b.md.cold-lava-7-3.tmp"),
            staged("a.cold-lava-b.md", 7, false)
        );
        for not in [
            ".N.cold-lava-kept-5-0.md",
            ".N.cold-lava-kept-5-0.tmp",
            ".N.md.cold-lava-12-.tmp",
            ".N.md.cold-lava--0.tmp",
            ".N.md.cold-lava-1x-0.tmp",
            "N.md.cold-lava-12-0.tmp",
            ".N.md.cold-lava-12-0.aside.tmp",
            "..cold-lava-12-0.tmp",
        ] {
            assert_eq!(staged_for(not), None, "{not}");
        }
    }
}
