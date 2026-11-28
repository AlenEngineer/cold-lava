//! `replace`'s one job: put a new text in over a file only if it still holds what the caller
//! read, so that another program's write landing after the compare is never replaced unseen
//! (#395). A caller of `os_rename` and of `write.rs`'s staging; what it can't put back it keeps
//! under a name of its own (`kept::keep`) and names (`WriteError::Kept`).
//!
//! The rule throughout: the engine deletes only its own staged bytes, or a text it took out that
//! is exactly what the caller read (`expected`) or what the path holds as it is deleted. Every
//! swap checks what came out, and a text put back over a newer one is swapped out again
//! (`put_back`). Where that can't go on (a writer that doesn't stop, a swap that fails), the
//! path may hold an older text: the newer one is never removed then, but kept beside it, named.
//!
//! cold-lava's own replaces never race here: each holds its turn on the file's folder from its
//! compare to its last swap (`lock`), so the other writers `checked` meets are other programs,
//! a cold-lava whose wait ran out, and cold-lava's own moves and deletes (`move_file`,
//! `trash::delete`), which take no turn. A move or delete during a turn makes the swap fail, or
//! the moved file holds the new text: no text is lost.

use std::io;
use std::path::Path;

use super::kept::keep;
use super::name::{self, reserved};
use super::{WriteError, io_err, lock, os_rename, staged, unstaged};

/// Where a test lands another write in `checked`: after the compare (`compared`), and after the
/// swap took out another write's text, before each round that puts one back (`swapped`, with
/// the round's number, from 0). The engine's own writes pass two closures that do nothing.
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
/// out was compared. An open still under way seems to count too: a whiteboard saving every
/// 25 ms beside a script that rewrites the file in place every 15 ms lost 3 of the script's
/// 363 writes in one run under load, each open returning 0.6 to 6 ms after what came out was
/// compared (#443; 15 to 17 a run before #425, none without load). That the system found the
/// file by its path before the swap, then emptied it (`O_TRUNC`) so the script wrote into the
/// text the swap took out, which was removed as the caller's, is inferred from the trace: it
/// shows when each open returned, not when its path was looked up. A program that saves by
/// renaming a new file over the path never loses a write this way.
/// Another cold-lava's replace of a file in the same folder waits until
/// this one is done, from the compare to the last swap (`lock::take`); the new text is staged
/// (written and synced) before the turn is taken, so the turn holds no write to disk. Errors
/// name `path`.
pub(super) fn checked(
    path: &Path,
    file: &Path,
    expected: &[u8],
    bytes: &[u8],
    hooks: Hooks<impl FnOnce(), impl FnMut(usize)>,
) -> Result<(), WriteError> {
    // Staging makes no folder (`staged`): in a folder that isn't there it is refused as `Io`,
    // and a file that isn't there is refused as `Io` by the read, with nothing left staged.
    let new = staged(path, file, bytes)?;
    let _turn = lock::take(file);
    let now = std::fs::read(file).map_err(|e| unstaged(&new, io_err(path)(e)))?;
    if now != expected {
        return Err(unstaged(&new, WriteError::Changed(path.to_path_buf())));
    }
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
    swapped: impl FnMut(usize),
) -> Result<(), WriteError> {
    if std::fs::read(out).is_ok_and(|was| was == expected) {
        let _ = std::fs::remove_file(out);
        return Ok(());
    }
    put_back(path, file, out, expected, bytes, swapped)
}

/// Swap another write's text in `out` back into `file`, and refuse (`Changed`). The swap back
/// is checked as the first swap was: what comes out should be what this write put in. When it
/// isn't, another program wrote the path in the moment between, and the text just put back is
/// older than it: so the newer one is swapped in once more, and the older one comes out. That
/// repeats while each round takes out a text it didn't put in (`swap_back`). When a round
/// settles (`Ours`, `Same`), the last text another program wrote is at the path, and what is
/// left in `out` is removed only when it is this write's own `bytes`, `expected`, or the text
/// the path now holds; anything else is kept (`keep`). When the rounds run out (`ROUNDS`) or a
/// round stops (`Stopped`: a swap or a read failed), the path may hold an older text than the
/// one in `out`: nothing is removed then, and what is in `out` is kept beside the path and
/// named, whatever it holds. `between` runs before each round, with its number: tests land
/// writes there.
///
/// What this can't tell apart is a text written later from one written earlier that another
/// writer swapped back in: every text it didn't put in counts as newer. Another cold-lava is
/// never that writer while it holds its turn (`lock`), but one that went ahead without it (its
/// wait ran out, or the lock couldn't be taken), or another program that also swaps a file out
/// and back, can be: then a save that writer was told was done may end up beside the path in a
/// kept file, and the path may hold a text whose writer was refused. No text is removed either
/// way: each is at the path or kept beside it, named. A program that writes into the file it
/// opened before the swap is the gap `checked` leaves open.
pub(super) fn put_back(
    path: &Path,
    file: &Path,
    out: &Path,
    expected: &[u8],
    bytes: &[u8],
    mut between: impl FnMut(usize),
) -> Result<(), WriteError> {
    let mut there = bytes.to_vec();
    for round in 0..ROUNDS {
        between(round);
        match swap_back(out, file, &there) {
            Round::Ours => return Err(keep(path, file, out, &[bytes, expected])),
            Round::Same(same) => return Err(keep(path, file, out, &[bytes, expected, &same])),
            Round::Newer(held) => there = held,
            Round::Stopped => break,
        }
    }
    Err(keep(path, file, out, &[]))
}

/// How many rounds `put_back` swaps before it gives up. Each round past the first means another
/// write landed at the path in the moment between two swaps; 16 in a row is no save that races
/// a person's, but a writer that doesn't stop (a sync loop, a script), and the loop must end.
/// Reaching it, the path holds the text the last round put in, and the newer one taken out is
/// kept beside it and named, never removed. Each round is two reads and a swap.
const ROUNDS: usize = 16;

/// What one round of `put_back` found.
enum Round {
    /// What came out is what the last round put in: nothing newer had landed, and the text that
    /// was in `out` is at the path.
    Ours,
    /// What came out holds the same as what went in: the path holds it, so `out` is a copy.
    Same(Vec<u8>),
    /// What came out is newer than what went in, which it holds: the next round puts it back.
    Newer(Vec<u8>),
    /// `out` couldn't be read, or swapped: what is at the path and in `out` is not known.
    Stopped,
}

/// One round of `put_back`: read `out`, swap it with `file`, read what came out, and say which
/// it was, given `there`, what the last round put in at the path.
fn swap_back(out: &Path, file: &Path, there: &[u8]) -> Round {
    let Ok(held) = std::fs::read(out) else {
        return Round::Stopped;
    };
    if os_rename::exchange(out, file).is_err() {
        return Round::Stopped;
    }
    match std::fs::read(out) {
        Ok(came) if came == there => Round::Ours,
        Ok(came) if came == held => Round::Same(came),
        Ok(_) => Round::Newer(held),
        Err(_) => Round::Stopped,
    }
}

/// `checked` where the file system can't swap (NFS, most FUSE mounts): the file is moved aside
/// by one plain rename onto a name this write reserved (`name::aside`, so it replaces only its
/// own empty file, and a killed save's leftover there reads as the file's only text), so a write after that can't land in it unseen, and compared there; the new text goes
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
    let old = reserved(file, name::aside).map_err(|e| unstaged(new, io_err(path)(e)))?;
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

/// `Changed` when the file is no longer there, as a write that removed it changed it.
fn gone(path: &Path, e: io::Error) -> WriteError {
    match e.kind() {
        io::ErrorKind::NotFound => WriteError::Changed(path.to_path_buf()),
        _ => WriteError::Io(path.to_path_buf(), e),
    }
}
