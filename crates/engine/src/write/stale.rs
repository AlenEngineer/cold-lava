//! A cold-lava killed during a save (a crash, `kill -9`, a closed terminal) leaves its staged
//! file beside the file it was saving: `.Name.cold-lava-PID-N.tmp` (#443). The first write this
//! process stages in a folder clears the ones a dead cold-lava left there (`once`).
//!
//! What a staged file holds after a kill depends on when the kill came: the killed save's new
//! text (before the swap), the text the swap took out (after it), or another program's text
//! `put_back` was handing back. So one is removed only when it holds what its file holds now;
//! any other is kept beside the file under a kept name (`replace::keep`), never removed.
//!
//! Which files are a dead cold-lava's is told by the name alone: `.Name.cold-lava-PID-N.tmp`
//! with digits for PID and N (`staged_for`), where no process has that PID (`dead`), last written
//! more than `AGE` ago. A kept text's name (`.Name.cold-lava-kept-…`) never reads as one. A
//! staged file of a live cold-lava is never touched, this process's own included.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use super::replace;

/// How long ago a dead cold-lava's staged file must have been written before it is cleared. A
/// save holds a staged file for milliseconds (seconds while its turn waits, `lock::WAIT`); the
/// age keeps a file from being taken from a save under way in a process this one can't see (a
/// container's, another machine's on a shared folder), whose PID reads as dead here.
const AGE: Duration = Duration::from_secs(60);

/// Clear the staged files dead cold-lavas left in `dir`, the first time this process stages a
/// write there. The folders already cleared are remembered so a folder is listed once per
/// process, not on every save (a rename writes hundreds of files in one folder); a folder
/// that still held one too young to clear is listed again on the next write.
pub(super) fn once(dir: &Path) {
    static CLEARED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
    let Ok(mut cleared) = CLEARED.lock() else {
        return;
    };
    let cleared = cleared.get_or_insert_default();
    if cleared.contains(dir) {
        return;
    }
    if clear(dir) {
        cleared.insert(dir.to_path_buf());
    }
}

/// Clear `dir`'s staged files left by dead cold-lavas; false when one was too young to clear.
fn clear(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    let mut done = true;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some((file, pid)) = name.to_str().and_then(staged_for) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() || !dead(pid) {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age > AGE);
        if !old {
            done = false;
            continue;
        }
        let file = dir.join(file);
        let now = std::fs::read(&file).unwrap_or_default();
        replace::keep(&file, &file, &entry.path(), &[&now]);
    }
    done
}

/// The file a staged file's name is for, and the PID of the cold-lava that staged it:
/// `.Name.cold-lava-PID-N.tmp` (`write::beside`) gives `Name` and PID.
fn staged_for(name: &str) -> Option<(&str, u32)> {
    let inner = name.strip_prefix('.')?.strip_suffix(".tmp")?;
    let (file, ids) = inner.rsplit_once(".cold-lava-")?;
    let (pid, n) = ids.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if file.is_empty() || !digits(pid) || !digits(n) {
        return None;
    }
    Some((file, pid.parse().ok()?))
}

/// Whether no process has `pid` (`kill(pid, 0)` says there is none). Where that can't be asked,
/// every PID counts as alive, and nothing is cleared.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn dead(pid: u32) -> bool {
    use rustix::process::{Pid, test_kill_process};
    let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return false;
    };
    test_kill_process(pid) == Err(rustix::io::Errno::SRCH)
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn dead(_: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::staged_for;

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
