//! The two renames a write that must not replace unseen is made of: one refused when something
//! is at its target, and one that swaps two entries. Each is one call to the system, so no
//! other program's write can fall between a check and the rename. A leaf: std and rustix.

use std::io;
use std::path::Path;

/// Rename `from` to `to`, refused with `AlreadyExists` when something is at `to`.
pub(super) fn no_replace(from: &Path, to: &Path) -> io::Result<()> {
    match flagged(from, to, How::NoReplace) {
        Err(e) if e.kind() == io::ErrorKind::Unsupported => by_link(from, to),
        done => done,
    }
}

/// Swap the entries at `a` and `b`: each path holds one of the two at every moment.
/// `Unsupported` where the file system can't (NFS, most FUSE mounts, other systems).
pub(super) fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    flagged(a, b, How::Exchange)
}

enum How {
    NoReplace,
    Exchange,
}

/// `renameat2` on Linux, `renameatx_np` on macOS (the macOS branch is built nowhere on CI).
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn flagged(from: &Path, to: &Path, how: How) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;
    let flags = match how {
        How::NoReplace => RenameFlags::NOREPLACE,
        How::Exchange => RenameFlags::EXCHANGE,
    };
    let unsupported = [Errno::INVAL, Errno::NOSYS, Errno::NOTSUP, Errno::OPNOTSUPP];
    renameat_with(CWD, from, CWD, to, flags).map_err(|e| match unsupported.contains(&e) {
        true => io::ErrorKind::Unsupported.into(),
        false => e.into(),
    })
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn flagged(_: &Path, _: &Path, _: How) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// `no_replace` where the file system has no such rename. A file or a link is hard-linked at
/// `to`, which is refused when something is there, then unlinked at `from`; a save of `from`
/// landing between the two is lost, the one window left. A folder can't be linked: it is
/// renamed after a check, which can replace no more than an empty folder made in between.
/// Without hard links either (some FUSE mounts), a file is checked and renamed as a folder is.
pub(super) fn by_link(from: &Path, to: &Path) -> io::Result<()> {
    if !std::fs::symlink_metadata(from)?.is_dir() {
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                return std::fs::remove_file(from).inspect_err(|_| {
                    let _ = std::fs::remove_file(to);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
            Err(_) => {}
        }
    }
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::ErrorKind::AlreadyExists.into());
    }
    std::fs::rename(from, to)
}
