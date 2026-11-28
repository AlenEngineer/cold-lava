//! A cold-lava killed during a save (a crash, `kill -9`, a closed terminal) leaves its staged
//! file beside the file it was saving: `.Name.cold-lava-PID-N.tmp` (#443). The first write this
//! process stages in a folder clears the ones a dead cold-lava left there (`once`).
//!
//! What a staged file holds after a kill depends on when the kill came: the killed save's new
//! text (before the swap), the text the swap took out (after it), the file's only text (after
//! `aside` moved it there, where nothing is at the file's path), or another program's text
//! `put_back` was handing back. So one that isn't empty goes back to its file's path when
//! nothing is there (`os_rename::no_replace`, which never replaces); one is removed only when
//! it holds what its file holds now, or nothing (a kill before the bytes went in); any other
//! is kept beside the file under a kept name (`kept::keep`), never removed, and the core
//! lists it (`kept::find`). The file is read only when it is a regular file of
//! the staged file's length (`same_length`): a pipe is never read, nor a file that can't be a
//! copy, whatever its size.
//!
//! Which files are a dead cold-lava's is told by the name (`name::staged_for`: no kept text's
//! name reads as one), the PID in it (no process has it here, `dead`) and its age (`AGE`). A
//! PID can't tell a live cold-lava this process can't see (in a container, on another machine
//! sharing the folder) from a dead one, so what keeps such a save's staged file from being
//! taken is this:
//!
//! - From its compare to its last swap, a `replace` holds its turn on the folder
//!   (`lock::take`), and the staged name then holds the file's old text, whose time can be
//!   years old. The clearing takes the same turn, so it never runs inside one: it waits for
//!   it (`lock::WAIT`), and when the turn can't be had (the wait ran out, a file system without
//!   the lock, as NFS is, a system without `flock`) nothing is cleared in that folder. That
//!   holds across processes and containers on one machine; across machines, only as far as the
//!   file system shares the lock.
//! - Before that, while it is written and synced (and in `write_atomic` and `create`, which
//!   take no turn, until their rename), a staged file holds the new text and was just written:
//!   `AGE` keeps it, by the later of when it was written and when it last changed name.
//!
//! This process's own staged files have a live PID, and are never touched.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use super::name::staged_for;
use super::{kept, lock, os_rename};

/// How long ago a dead cold-lava's staged file must have been written, or renamed, before it
/// is cleared: what keeps a file staged by a save this process can't see before that save
/// takes its turn on the folder (`replace`), or renames it into place (`write_atomic`,
/// `create`). Such a save holds it for milliseconds (the time to write and sync its text). It
/// says nothing once a `replace` has swapped (the name then holds the old text, as old as
/// that text): the folder's turn covers that. A clock more than this apart between two
/// machines sharing a folder defeats it.
pub(super) const AGE: Duration = Duration::from_secs(60);

/// Clear the staged files dead cold-lavas left in `dir`, the first time this process stages a
/// write there. The folders already swept are remembered so a folder is listed once per
/// process, not on every save (a rename writes hundreds of files in one folder); one that held
/// a file too young to clear is swept again by a later write, and one whose turn couldn't be
/// had, or whose file's age can't be read or is ahead of the clock, is not. The memory is
/// locked only to check and mark a folder: the listing runs outside it, so a sweep holds up
/// only the writes in its own folder (by the turn it holds), not every write in the process.
pub(super) fn once(dir: &Path) {
    static SWEPT: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
    let mark = |claim: bool| {
        SWEPT.lock().is_ok_and(|mut swept| {
            let swept = swept.get_or_insert_default();
            match claim {
                true => swept.insert(dir.to_path_buf()),
                false => swept.remove(dir),
            }
        })
    };
    if !mark(true) {
        return;
    }
    if clear(dir, lock::WAIT, AGE) == Swept::Again {
        mark(false);
    }
}

/// What a sweep of a folder leaves for later.
#[derive(Debug, PartialEq)]
enum Swept {
    /// Nothing: the folder is not swept again in this process.
    Done,
    /// A dead cold-lava's staged file too young to clear: a later write sweeps again.
    Again,
}

/// Clear `dir`'s staged files left by dead cold-lavas older than `age`, holding the folder's
/// turn (waiting for it at most `wait`); when the turn can't be had, nothing.
fn clear(dir: &Path, wait: Duration, age: Duration) -> Swept {
    let Some(_turn) = lock::locked(dir, wait) else {
        return Swept::Done;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Swept::Done;
    };
    let mut swept = Swept::Done;
    for entry in entries.flatten() {
        let file = match left(&entry, age) {
            Left::Not => continue,
            Left::Young => {
                swept = Swept::Again;
                continue;
            }
            Left::Old(file) => dir.join(file),
        };
        let staged = entry.path();
        let len = entry.metadata().map_or(0, |m| m.len());
        if len > 0 && os_rename::no_replace(&staged, &file).is_ok() {
            continue;
        }
        let now = match len {
            0 => Some(Vec::new()),
            len => same_length(&file, len),
        };
        let droppable: Vec<&[u8]> = now.iter().map(Vec::as_slice).collect();
        kept::keep(&file, &file, &staged, &droppable);
    }
    swept
}

/// What a folder's entry is to the clearing of staged files.
pub(super) enum Left {
    /// Not a dead cold-lava's staged file (another name, a live one's, not a regular file), or
    /// one whose age can't be read or is ahead of the clock (`since_changed`).
    Not,
    /// A dead cold-lava's, written or renamed within the age asked for.
    Young,
    /// A dead cold-lava's, older: the name of the file it was staged for.
    Old(String),
}

/// Whether `entry` is a staged file a dead cold-lava left, older than `age` (`AGE` for the
/// sweep). The one test the sweep and the finder of kept texts (`kept::find`) both ask.
pub(super) fn left(entry: &std::fs::DirEntry, age: Duration) -> Left {
    let name = entry.file_name();
    let Some((file, pid)) = name.to_str().and_then(staged_for) else {
        return Left::Not;
    };
    let Ok(meta) = entry.metadata() else {
        return Left::Not;
    };
    if !meta.is_file() || !dead(pid) {
        return Left::Not;
    }
    match since_changed(&meta) {
        Some(ago) if ago > age => Left::Old(file.to_string()),
        Some(_) => Left::Young,
        None => Left::Not,
    }
}

/// What `file` holds, when it is a regular file of `len` bytes (not a link, a pipe or a
/// device: opened without following a link and without waiting, then checked by what was
/// opened); None otherwise, and then the staged file it is compared with counts as different
/// and is kept. A pipe read whole never returns, and a file of another length can't be a copy,
/// so neither is read.
fn same_length(file: &Path, len: u64) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
        options.custom_flags(flags.bits() as i32);
    }
    let opened = options.open(file).ok()?;
    let meta = opened.metadata().ok()?;
    if !meta.is_file() || meta.len() != len {
        return None;
    }
    let mut now = Vec::new();
    opened.take(len + 1).read_to_end(&mut now).ok()?;
    (now.len() as u64 == len).then_some(now)
}

/// How long ago a file was last written or renamed: the later of its mtime and its ctime
/// (which a rename sets on ext4, btrfs, xfs and APFS). None when that can't be read, or is
/// ahead of the clock.
fn since_changed(meta: &std::fs::Metadata) -> Option<Duration> {
    let written = meta.modified().ok()?;
    #[cfg(unix)]
    let written = {
        use std::os::unix::fs::MetadataExt;
        let secs = u64::try_from(meta.ctime()).ok();
        let nanos = u32::try_from(meta.ctime_nsec()).ok();
        match secs.zip(nanos) {
            Some((s, n)) => written.max(SystemTime::UNIX_EPOCH + Duration::new(s, n)),
            None => written,
        }
    };
    SystemTime::now().duration_since(written).ok()
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

#[cfg(all(test, any(target_os = "linux", target_os = "android")))]
mod tests {
    use super::{AGE, Swept, clear, lock};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cold-lava-stale-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn files(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// A PID no process has: one past the largest Linux gives out.
    fn dead() -> u32 {
        i32::MAX as u32
    }

    /// `name` in `d`, holding `text`, written `ago` (its ctime stays now).
    fn leave(d: &Path, name: &str, text: &str, ago: Duration) {
        std::fs::write(d.join(name), text).unwrap();
        let f = std::fs::File::options()
            .write(true)
            .open(d.join(name))
            .unwrap();
        f.set_modified(SystemTime::now() - ago).unwrap();
    }

    /// A cold-lava killed during a save leaves its staged file (#443). A sweep takes it away:
    /// removed when it holds what its file holds or nothing, kept under a kept name otherwise
    /// (it may be the killed save's text, or another program's it took out). One renamed or
    /// written within `AGE` stays, even when its mtime is old (a staged name a live `replace`
    /// swapped the file's old text into); so does a staged file of a live cold-lava and a kept
    /// text.
    #[test]
    fn a_staged_file_a_killed_save_left_is_cleared() {
        let d = dir("cleared");
        let dead = dead();
        let live = std::process::id();
        let long = Duration::from_secs(600);
        std::fs::write(d.join("N.md"), "now").unwrap();
        let copy = format!(".N.md.cold-lava-{dead}-0.tmp");
        let other = format!(".N.md.cold-lava-{dead}-1.tmp");
        let empty = format!(".N.md.cold-lava-{dead}-3.tmp");
        let alive = format!(".N.md.cold-lava-{live}-900.tmp");
        let kept = ".N.cold-lava-kept-5-0.md".to_string();
        leave(&d, &copy, "now", long);
        leave(&d, &other, "the killed save", long);
        leave(&d, &empty, "", long);
        leave(&d, &alive, "a save under way", long);
        leave(&d, &kept, "kept", long);
        let all = files(&d);
        assert_eq!(clear(&d, lock::WAIT, AGE), Swept::Again);
        assert_eq!(
            files(&d),
            all,
            "changed within AGE (ctime), so nothing goes"
        );
        assert_eq!(clear(&d, lock::WAIT, Duration::ZERO), Swept::Done);
        let left = files(&d);
        assert!(!left.contains(&copy), "a copy is removed: {left:?}");
        assert!(!left.contains(&empty), "an empty one is removed: {left:?}");
        assert!(!left.contains(&other), "{left:?}");
        let keeps: Vec<_> = left
            .iter()
            .filter(|f| f.starts_with(".N.cold-lava-kept-") && **f != kept)
            .collect();
        assert_eq!(keeps.len(), 1, "{left:?}");
        assert_eq!(std::fs::read(d.join(keeps[0])).unwrap(), b"the killed save");
        for stays in [&alive, &kept] {
            assert!(left.contains(stays), "{stays} stays: {left:?}");
        }
        assert_eq!(std::fs::read(d.join("N.md")).unwrap(), b"now");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A save killed after it moved the note aside (where files can't be swapped, `aside`)
    /// leaves the note's only text under the staged name, and nothing at the note's path
    /// (#427): the sweep puts it back there, never under a kept name.
    #[test]
    fn a_staged_file_that_is_a_notes_only_text_goes_back_to_its_path() {
        let d = dir("only");
        let staged = format!(".N.md.cold-lava-{}-0.tmp", dead());
        leave(&d, &staged, "the only text", Duration::from_secs(600));
        assert_eq!(clear(&d, lock::WAIT, Duration::ZERO), Swept::Done);
        assert_eq!(files(&d), ["N.md"]);
        assert_eq!(std::fs::read(d.join("N.md")).unwrap(), b"the only text");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A leftover named for a pipe (or anything not a regular file of its length) is kept,
    /// and the pipe is never read: a read with no writer would never return.
    #[test]
    fn a_leftover_for_a_pipe_is_kept_without_reading_it() {
        let d = dir("pipe");
        let mode = rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR;
        rustix::fs::mkfifoat(rustix::fs::CWD, d.join("pipe"), mode).unwrap();
        let staged = format!(".pipe.cold-lava-{}-0.tmp", dead());
        leave(&d, &staged, "a text", Duration::from_secs(600));
        assert_eq!(clear(&d, lock::WAIT, Duration::ZERO), Swept::Done);
        let left = files(&d);
        assert!(!left.contains(&staged), "{left:?}");
        let keeps: Vec<_> = left
            .iter()
            .filter(|f| f.starts_with(".pipe.cold-lava-kept-"))
            .collect();
        assert_eq!(keeps.len(), 1, "{left:?}");
        assert_eq!(std::fs::read(d.join(keeps[0])).unwrap(), b"a text");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A staged file under a turn held on its folder (a `replace` between its compare and its
    /// last swap, in a process whose PID reads as dead here) is never touched: the sweep waits
    /// for the turn and, when it can't have it, clears nothing. Once the turn is over, it does.
    #[test]
    fn a_staged_file_a_live_turn_holds_is_not_touched() {
        let d = dir("turn");
        std::fs::write(d.join("N.md"), "the new text").unwrap();
        let staged = format!(".N.md.cold-lava-{}-0.tmp", dead());
        leave(
            &d,
            &staged,
            "the old text, swapped out",
            Duration::from_secs(600),
        );
        let (held, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel::<()>());
        let turn = {
            let d = d.clone();
            let (held, release) = (held.0, release.1);
            std::thread::spawn(move || {
                let folder = lock::locked(&d, lock::WAIT);
                held.send(folder.is_some()).unwrap();
                let _ = release.recv();
                drop(folder);
            })
        };
        assert!(held.1.recv().unwrap(), "the turn is held");
        let swept = clear(&d, Duration::from_millis(50), Duration::ZERO);
        assert_eq!(swept, Swept::Done);
        assert!(files(&d).contains(&staged), "{:?}", files(&d));
        assert_eq!(
            std::fs::read(d.join(&staged)).unwrap(),
            b"the old text, swapped out"
        );
        release.0.send(()).unwrap();
        turn.join().unwrap();
        clear(&d, lock::WAIT, Duration::ZERO);
        assert!(!files(&d).contains(&staged), "{:?}", files(&d));
        let _ = std::fs::remove_dir_all(&d);
    }
}
