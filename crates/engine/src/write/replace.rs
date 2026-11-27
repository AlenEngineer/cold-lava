//! `replace`'s one job: put a new text in over a file only if it still holds what the caller
//! read, so that another program's write landing after the compare is never replaced unseen
//! (#395). A caller of `os_rename` and of `write.rs`'s staging; what it can't put back it keeps
//! under a name of its own (`kept_name`) and names (`WriteError::Kept`).
//!
//! The rule throughout: the engine deletes only its own staged bytes, or a text it took out that
//! is exactly what the caller read (`expected`) or what the path holds as it is deleted. And a
//! text is never put back over a newer one: every swap checks what came out (`put_back`).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{WriteError, beside, io_err, os_rename, reserved, staged, unstaged};

/// Where a test lands another write in `checked`: after the compare (`compared`), and after the
/// swap took out another write's text, before it goes back (`swapped`). The engine's own
/// writes pass two closures that do nothing.
pub(super) struct Hooks<A, B> {
    pub compared: A,
    pub swapped: B,
}

/// Compare `file` with `expected`, then put `bytes` in. The new text is staged, then swapped
/// with the file in one step (`os_rename::exchange`), so the path always holds a file, and
/// what came out is compared again: unless it is still `expected`, another write landed after
/// the compare, and it goes back in (`put_back`). Where the file system can't swap, the file is
/// moved aside instead (`aside`). Both compares read through a symlink: a link swapped in at
/// `file` after the caller's check is compared by what it leads to, and is what comes out, so
/// it is removed or put back as a link and never written through. Left open, as on any POSIX
/// system: a program that opened the file before the swap and writes into it after what came
/// out was compared. Errors name `path`.
pub(super) fn checked(
    path: &Path,
    file: &Path,
    expected: &[u8],
    bytes: &[u8],
    hooks: Hooks<impl FnOnce(), impl FnOnce()>,
) -> Result<(), WriteError> {
    let now = std::fs::read(file).map_err(io_err(path))?;
    if now != expected {
        return Err(WriteError::Changed(path.to_path_buf()));
    }
    let new = staged(path, file, bytes)?;
    (hooks.compared)();
    match os_rename::exchange(&new, file) {
        Ok(()) => settle(path, file, &new, expected, bytes, hooks.swapped),
        Err(e) if e.kind() == io::ErrorKind::Unsupported => aside(path, file, &new, expected),
        Err(e) => Err(unstaged(&new, gone(path, e))),
    }
}

/// After the swap, `out` holds what `file` held at that moment. The write is done when that is
/// what the caller read; anything else is another write's, and goes back.
fn settle(
    path: &Path,
    file: &Path,
    out: &Path,
    expected: &[u8],
    bytes: &[u8],
    swapped: impl FnOnce(),
) -> Result<(), WriteError> {
    if std::fs::read(out).is_ok_and(|was| was == expected) {
        let _ = std::fs::remove_file(out);
        return Ok(());
    }
    put_back(path, file, out, expected, bytes, swapped)
}

/// Swap another write's text in `out` back into `file`, and refuse (`Changed`). The swap back
/// is checked as the first swap was: what comes out should be what this write put in. When it
/// isn't, a newer write landed at the path in the moment between (another program's save, or
/// another cold-lava's replace, which may already have said it was done), and the text just put
/// back is older than it: so the newer one is swapped in once more, and the older one comes
/// out. That repeats while each swap takes out a text newer than the one it put in (at most
/// `ROUNDS` times), so the newest text ends at the path and no write's text is lost. What is
/// left in `out` is removed only when it is this write's own `bytes` (or `expected`), or the
/// text the path now holds; anything else is kept (`keep`). `swapped` runs before the swap
/// back: tests land writes there.
pub(super) fn put_back(
    path: &Path,
    file: &Path,
    out: &Path,
    expected: &[u8],
    bytes: &[u8],
    swapped: impl FnOnce(),
) -> Result<(), WriteError> {
    const ROUNDS: usize = 16;
    swapped();
    let mut there = bytes.to_vec();
    let mut same: Option<Vec<u8>> = None;
    for _ in 0..ROUNDS {
        let Ok(held) = std::fs::read(out) else { break };
        if os_rename::exchange(out, file).is_err() {
            break;
        }
        match std::fs::read(out) {
            Ok(came) if came == there => break,
            Ok(came) if came == held => {
                same = Some(came);
                break;
            }
            Ok(_) => there = held,
            Err(_) => break,
        }
    }
    let mut droppable = vec![bytes, expected];
    droppable.extend(same.as_deref());
    Err(keep(path, file, out, &droppable))
}

/// `checked` where the file system can't swap (NFS, most FUSE mounts): the file is moved aside
/// by one plain rename onto a name this write reserved (so it replaces only its own empty
/// file), so a write after that can't land in it unseen, and compared there; the new text goes
/// in only where nothing is (`os_rename::no_replace`). The path holds no file for that moment.
/// What was moved aside goes back where nothing is; when something is at the path by then, it
/// is removed if it is `expected`, and kept otherwise (`keep`). Left open where the file system
/// has neither rename flags nor hard links: both `no_replace`s here check, then rename
/// (`os_rename::by_link`), so a save that lands at the empty path between the two is replaced,
/// by the new text or by the text put back.
pub(super) fn aside(
    path: &Path,
    file: &Path,
    new: &Path,
    expected: &[u8],
) -> Result<(), WriteError> {
    let old = reserved(file, beside).map_err(|e| unstaged(new, io_err(path)(e)))?;
    if let Err(e) = std::fs::rename(file, &old) {
        let _ = std::fs::remove_file(&old);
        return Err(unstaged(new, gone(path, e)));
    }
    let refused = match std::fs::read(&old).is_ok_and(|was| was == expected) {
        true => match os_rename::no_replace(new, file) {
            Ok(()) => {
                let _ = std::fs::remove_file(&old);
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                WriteError::Changed(path.to_path_buf())
            }
            Err(e) => WriteError::Io(path.to_path_buf(), e),
        },
        false => WriteError::Changed(path.to_path_buf()),
    };
    let refused = unstaged(new, refused);
    match os_rename::no_replace(&old, file) {
        Ok(()) => Err(refused),
        Err(_) => match keep(path, file, &old, &[expected]) {
            WriteError::Changed(_) => Err(refused),
            kept => Err(kept),
        },
    }
}

/// The refusal for a text this write took out of `file` (where `path` lands) and holds in
/// `out`, a name of its own beside it that nothing else writes. When it is one of `droppable`
/// (this write's bytes, or what the caller read) nobody's text is lost: it is removed, and the
/// refusal is `Changed`. Otherwise it is another program's text: moved to a name that says it
/// is kept (`kept_name`) and named by `Kept`; if that move fails, it stays in `out`, which is
/// named instead.
pub(super) fn keep(path: &Path, file: &Path, out: &Path, droppable: &[&[u8]]) -> WriteError {
    if std::fs::read(out).is_ok_and(|now| droppable.contains(&now.as_slice())) {
        let _ = std::fs::remove_file(out);
        return WriteError::Changed(path.to_path_buf());
    }
    let kept = match reserved(file, kept_name) {
        Ok(kept) => match std::fs::rename(out, &kept) {
            Ok(()) => kept,
            Err(_) => {
                let _ = std::fs::remove_file(&kept);
                out.to_path_buf()
            }
        },
        Err(_) => out.to_path_buf(),
    };
    WriteError::Kept(path.to_path_buf(), kept)
}

/// The name a kept text goes under: `.Name.cold-lava-kept-SECONDS-N.ext` beside the file (the
/// file's stem and extension, the Unix time it was kept, a count), hidden, so the scan and
/// Obsidian pass it by, and apart from a staged write's `.Name.cold-lava-PID-N.tmp`, so a sweep
/// of staged files left by a crash never takes it. Where the user is shown kept texts is #427.
fn kept_name(file: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    let ext = file
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    file.with_file_name(format!(".{stem}.cold-lava-kept-{secs}-{n}{ext}"))
}

/// `Changed` when the file is no longer there, as a write that removed it changed it.
fn gone(path: &Path, e: io::Error) -> WriteError {
    match e.kind() {
        io::ErrorKind::NotFound => WriteError::Changed(path.to_path_buf()),
        _ => WriteError::Io(path.to_path_buf(), e),
    }
}
