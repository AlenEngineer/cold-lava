//! The names a write's own files take beside the file it writes, and the one way one is
//! claimed (`reserved`). A staged write's name, `.Name.cold-lava-PID-N.tmp`, is made here
//! (`beside`) and read back here (`staged_for`, for the clearing of what a killed save left,
//! `stale`), so the two can't drift apart. A kept text's name is `kept.rs`'s. A leaf: std only.

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
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    file.with_file_name(format!(".{name}{STAGED}{}-{n}.tmp", std::process::id()))
}

/// The file a staged write's name is for, and the PID of the cold-lava that staged it: the
/// name `beside` makes gives `Name` and PID, and no other name reads (a kept text's,
/// `.Name.cold-lava-kept-…`, never does).
pub(super) fn staged_for(name: &str) -> Option<(&str, u32)> {
    let inner = name.strip_prefix('.')?.strip_suffix(".tmp")?;
    let (file, ids) = inner.rsplit_once(STAGED)?;
    let (pid, n) = ids.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if file.is_empty() || !digits(pid) || !digits(n) {
        return None;
    }
    Some((file, pid.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::{beside, staged_for};
    use std::path::Path;

    #[test]
    fn the_name_beside_makes_is_the_one_staged_for_reads() {
        for file in ["N.md", "a.cold-lava-b.md", "Board 1.canvas", "no-extension"] {
            let staged = beside(&Path::new("dir").join(file));
            let name = staged.file_name().unwrap().to_str().unwrap();
            assert_eq!(staged_for(name), Some((file, std::process::id())), "{name}");
        }
    }

    #[test]
    fn a_staged_name_reads_and_a_kept_one_doesnt() {
        assert_eq!(staged_for(".N.md.cold-lava-12-0.tmp"), Some(("N.md", 12)));
        assert_eq!(
            staged_for(".a.cold-lava-b.md.cold-lava-7-3.tmp"),
            Some(("a.cold-lava-b.md", 7))
        );
        for not in [
            ".N.cold-lava-kept-5-0.md",
            ".N.cold-lava-kept-5-0.tmp",
            ".N.md.cold-lava-12-.tmp",
            ".N.md.cold-lava--0.tmp",
            ".N.md.cold-lava-1x-0.tmp",
            "N.md.cold-lava-12-0.tmp",
            "..cold-lava-12-0.tmp",
        ] {
            assert_eq!(staged_for(not), None, "{not}");
        }
    }
}
