//! cold-lava's own writers take turns: every `replace` holds an advisory lock on the folder
//! its file is in, from its compare to its last swap (`replace::checked`), so two cold-lavas
//! (two processes, or two threads of one) never swap one file in each other's moment. Without
//! it, a third writer's swap back could take out a save another had already reported done and
//! put a refused one in its place (#425). Programs that don't take the lock (Obsidian, an
//! editor, a sync tool) still go through `replace`'s exchange and check, which is what keeps
//! their writes.
//!
//! Why the folder and how: `flock` on the folder's own descriptor (opened read-only) leaves no
//! file anywhere, so nothing new shows in the workspace, in Obsidian or in the scan, and no
//! other program can trip over a lock file; the kernel lets go of it when the holder exits,
//! however it exits, so a crash leaves nothing held. The file itself can't hold the lock: the
//! swap puts another file at its path, which a second writer would open and lock unhindered. The
//! folder is the one the swap happens in, a symlink's target's when a write follows a link
//! (`replace_through_link`), so a write through a link and one to the file it leads to wait for
//! each other. Writes of other files in the same folder wait too: each holds the lock for a few
//! milliseconds (a staged write and two swaps).
//!
//! It never deadlocks: a write waits at most `WAIT`, then goes ahead without the lock, as
//! every write did before it, and so does a write where the lock can't be taken (a folder that
//! can't be opened, a file system without `flock`, a system without it). A write that goes
//! ahead so is checked as an outside program's is: what that leaves open is `put_back`'s to
//! say. A leaf: std and rustix.

use std::fs::File;
use std::path::Path;
use std::time::Duration;

/// How long a write waits for another cold-lava's turn on the same folder to end. A turn is a
/// few milliseconds; one that lasts this long is a writer stopped (a debugger, `SIGSTOP`, a
/// file system that hangs), and waiting longer would hang this one with it.
const WAIT: Duration = Duration::from_secs(5);

/// A writer's turn on a folder: the lock, held until this is dropped. Empty when the write went
/// ahead without it (`take`).
pub(super) struct Turn {
    _folder: Option<File>,
}

impl Turn {
    /// Whether the lock is held: false when the wait ran out or the lock couldn't be taken.
    #[cfg(test)]
    pub(super) fn held(&self) -> bool {
        self._folder.is_some()
    }
}

/// Wait for this writer's turn on the folder `file` is in, at most `WAIT`.
pub(super) fn take(file: &Path) -> Turn {
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Turn {
        _folder: locked(dir, WAIT),
    }
}

/// The folder `dir`, opened and locked (`flock`, exclusive), tried again until `wait` is up.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub(super) fn locked(dir: &Path, wait: Duration) -> Option<File> {
    use rustix::fs::{FlockOperation, flock};
    use rustix::io::Errno;
    use std::time::Instant;
    let folder = File::open(dir).ok()?;
    let until = Instant::now() + wait;
    let mut pause = Duration::from_millis(1);
    loop {
        match flock(&folder, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => return Some(folder),
            Err(e) if [Errno::AGAIN, Errno::INTR].contains(&e) && Instant::now() < until => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// Elsewhere no lock is taken (the macOS branch above is built nowhere on CI).
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
pub(super) fn locked(_: &Path, _: Duration) -> Option<File> {
    None
}
