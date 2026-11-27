//! The one way cold-lava writes to a workspace.
//!
//! Every write is atomic (a hidden temporary file in the same folder, then a rename), so Obsidian,
//! Neovim or a sync tool never sees a half-written note. Writes that replace existing content
//! can say what they expect to be there, and are refused if the file changed in the meantime, so
//! an edit made elsewhere is never overwritten, nor one that lands while the write is under way:
//! the new file goes in by a swap or a rename that never replaces (`os_rename`), and what came
//! out is checked.

mod os_rename;

use std::fmt;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub enum WriteError {
    /// The file's content isn't what the caller read earlier: someone else changed it.
    Changed(PathBuf),
    /// Another write landed at the path (the first) while this one was under way, and what
    /// was taken out of the file couldn't be put back: it is kept in the second path, a
    /// hidden file beside it. Read the file again: this write may or may not be in it.
    Kept(PathBuf, PathBuf),
    /// Creating a file where something already is (`taken`).
    Exists(PathBuf),
    /// A symlink that leads to no file, to nothing or to a folder: a write never puts a file
    /// where a link is, and never makes one through it (`landing`).
    NotAFile(PathBuf),
    /// A symlink at the path of a write that never follows one (`replace`): refused, so a link
    /// is never turned into a file without a word.
    Link(PathBuf),
    Io(PathBuf, io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WriteError::Changed(p) => write!(f, "{} changed since it was read", p.display()),
            WriteError::Kept(p, kept) => write!(
                f,
                "{} changed while it was being written: the text taken out of it is kept in {}",
                p.display(),
                kept.display()
            ),
            WriteError::Exists(p) => write!(f, "{} already exists", p.display()),
            WriteError::NotAFile(p) => f.write_str(&not_a_file(p.display())),
            WriteError::Link(p) => write!(
                f,
                "{} is a symbolic link, which this write never follows",
                p.display()
            ),
            WriteError::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for WriteError {}

/// `NotAFile` in its one wording, naming the link as the caller does.
pub(crate) fn not_a_file(name: impl fmt::Display) -> String {
    format!("{name} is a symbolic link that doesn't lead to a file")
}

fn io_err(path: &Path) -> impl Fn(io::Error) -> WriteError + '_ {
    move |e| WriteError::Io(path.to_path_buf(), e)
}

/// Replace (or create) `path` with `bytes`, atomically. Creates missing folders. A symlink is
/// written through: the file it leads to is replaced and the link stays (`landing`).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    write_at(path, &landing(path)?, bytes)
}

/// Where a write to `path` lands: `path`, or the file a symlink there leads to, every link on
/// the way followed, so the rename replaces that file, beside it, and the link stays a link. A
/// link to nothing, or to something that isn't a file, is `NotAFile`. Where a link may lead is
/// the caller's to bound: this follows it anywhere. Only `write_atomic` and
/// `replace_through_link` land here; `replace` and `create` put their file at the path itself,
/// so a link swapped in there after the check is never followed: `replace` swaps it out,
/// `create` is refused. A folder on the way swapped for a link is still followed (#366).
fn landing(path: &Path) -> Result<PathBuf, WriteError> {
    let link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if !link {
        return Ok(path.to_path_buf());
    }
    match std::fs::canonicalize(path) {
        Ok(file) if file.is_file() => Ok(file),
        Ok(_) => Err(WriteError::NotAFile(path.to_path_buf())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Err(WriteError::NotAFile(path.to_path_buf()))
        }
        Err(e) => Err(WriteError::Io(path.to_path_buf(), e)),
    }
}

/// Replace `file`, where a write to `path` lands, with `bytes`: a temporary file beside it,
/// then a rename. Errors name `path`, as the caller does.
fn write_at(path: &Path, file: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let new = staged(path, file, bytes)?;
    std::fs::rename(&new, file).map_err(|e| unstaged(&new, io_err(path)(e)))
}

/// A new file beside `file` holding `bytes`, on disk and with `file`'s permissions: what a
/// write renames into place. Errors name `path`.
fn staged(path: &Path, file: &Path, bytes: &[u8]) -> Result<PathBuf, WriteError> {
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    let (new, mut f) = loop {
        let new = beside(file);
        match std::fs::File::create_new(&new) {
            Ok(f) => break (new, f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err(path)(e)),
        }
    };
    let written = f.write_all(bytes).and_then(|()| f.sync_all());
    written.map_err(|e| unstaged(&new, io_err(path)(e)))?;
    if let Ok(meta) = std::fs::metadata(file) {
        let _ = std::fs::set_permissions(&new, meta.permissions());
    }
    Ok(new)
}

/// A name beside `file` that this process hasn't used: hidden, so file watchers (ours and
/// Obsidian's) ignore it, and new each time, so a text kept there (`Kept`) is never written
/// over by a later write.
fn beside(file: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    file.with_file_name(format!(".{name}.cold-lava-{}-{n}.tmp", std::process::id()))
}

/// `e`, once the file `staged` made is removed: it holds only this write's bytes.
fn unstaged(new: &Path, e: WriteError) -> WriteError {
    let _ = std::fs::remove_file(new);
    e
}

/// Replace the file at `path` only if it still contains `expected` (what the caller read
/// before). Outside the engine, `read::Seen::replace` is the way. It never follows a symlink: a
/// link at `path` is refused (`Link`), and one swapped in after that check is taken out by the
/// swap, never written through. Every path the engine found by its scan, or the gateway let
/// through, is a file (`workspace::first_link`), so this is the write for all of them.
pub(crate) fn replace(path: &Path, expected: &[u8], bytes: &[u8]) -> Result<(), WriteError> {
    refuse_link(path)?;
    replace_at(path, path, expected, bytes)
}

/// `Link` when a symlink is at `path`: `replace`'s refusal, which a plan checked before it
/// writes anything (`rename::check_unchanged`) asks too, so it is refused whole.
pub(crate) fn refuse_link(path: &Path) -> Result<(), WriteError> {
    let link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    match link {
        true => Err(WriteError::Link(path.to_path_buf())),
        false => Ok(()),
    }
}

/// As `replace`, but a symlink at `path` is followed (`landing`): the file it leads to is
/// replaced, and the link stays. Only for the user's own save of a file the scan doesn't read
/// and that the user opened by its path: a board's canvas, `shapes.toml`, a note the menu's
/// tree lists. Never for a path the gateway or an extension gave.
pub(crate) fn replace_through_link(
    path: &Path,
    expected: &[u8],
    bytes: &[u8],
) -> Result<(), WriteError> {
    replace_at(path, &landing(path)?, expected, bytes)
}

fn replace_at(path: &Path, file: &Path, expected: &[u8], bytes: &[u8]) -> Result<(), WriteError> {
    replace_with(path, file, expected, bytes, || {})
}

/// Compare `file` with `expected`, then put `bytes` in so that another program's write landing
/// after the compare is never replaced unseen (#395). The new text is staged, then swapped
/// with the file in one step (`os_rename::exchange`), so the path always holds a file, and
/// what came out is compared again: unless it is still `expected`, another write landed after
/// the compare, and it goes back in (`put_back`). Where the file system can't swap, the file is
/// moved aside instead (`aside`). Left open, as on any POSIX system: a program that opened the
/// file before the swap and writes into it after what came out was compared.
fn replace_with(
    path: &Path,
    file: &Path,
    expected: &[u8],
    bytes: &[u8],
    between: impl FnOnce(),
) -> Result<(), WriteError> {
    let now = std::fs::read(file).map_err(io_err(path))?;
    if now != expected {
        return Err(WriteError::Changed(path.to_path_buf()));
    }
    let new = staged(path, file, bytes)?;
    between();
    match os_rename::exchange(&new, file) {
        Ok(()) => settle(path, file, &new, expected, bytes),
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
) -> Result<(), WriteError> {
    if std::fs::read(out).is_ok_and(|was| was == expected) {
        let _ = std::fs::remove_file(out);
        return Ok(());
    }
    put_back(path, file, out, bytes)
}

/// Swap another write's text in `out` back into `file`, and refuse (`Changed`). What comes out
/// is removed only when it is this write's own `bytes`; anything else (a third write, landed in
/// the moment between) stays in `out` and is named (`Kept`).
fn put_back(path: &Path, file: &Path, out: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let back = os_rename::exchange(out, file).is_ok();
    if back && std::fs::read(out).is_ok_and(|now| now == bytes) {
        let _ = std::fs::remove_file(out);
        return Err(WriteError::Changed(path.to_path_buf()));
    }
    Err(WriteError::Kept(path.to_path_buf(), out.to_path_buf()))
}

/// `replace_with` where the file system can't swap (NFS, most FUSE mounts): the file is moved
/// aside, so a write after that can't land in it unseen, and compared there; the new text goes
/// in only where nothing is (`os_rename::no_replace`). The path holds no file for that moment.
/// Only a text that is `expected` is removed: another write's goes back, or is kept (`Kept`)
/// when something is at the path by then.
fn aside(path: &Path, file: &Path, new: &Path, expected: &[u8]) -> Result<(), WriteError> {
    let old = beside(file);
    os_rename::no_replace(file, &old).map_err(|e| unstaged(new, gone(path, e)))?;
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
        Err(_) => Err(WriteError::Kept(path.to_path_buf(), old)),
    }
}

/// `Changed` when the file is no longer there, as a write that removed it changed it.
fn gone(path: &Path, e: io::Error) -> WriteError {
    match e.kind() {
        io::ErrorKind::NotFound => WriteError::Changed(path.to_path_buf()),
        _ => WriteError::Io(path.to_path_buf(), e),
    }
}

/// Whether something is at `path`: a file, a folder or a symlink, a link to nothing included.
/// A new file never goes where something is.
pub fn taken(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Create a new file; refused if something is already there (`taken`), or is by the time the
/// file goes in (`os_rename::no_replace`): a file or a link made there after the check is
/// neither replaced nor written through.
pub fn create(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    if taken(path) {
        return Err(WriteError::Exists(path.to_path_buf()));
    }
    let new = staged(path, path, bytes)?;
    os_rename::no_replace(&new, path).map_err(|e| unstaged(&new, refused_at(path, path, e)))
}

/// `Exists` naming `to` when a rename onto it found something there, else `Io` naming `named`.
fn refused_at(named: &Path, to: &Path, e: io::Error) -> WriteError {
    match e.kind() {
        io::ErrorKind::AlreadyExists => WriteError::Exists(to.to_path_buf()),
        _ => WriteError::Io(named.to_path_buf(), e),
    }
}

/// Create a new file unless it already exists (then leave it alone). Returns whether it was made.
pub fn create_if_missing(path: &Path, bytes: &[u8]) -> Result<bool, WriteError> {
    match create(path, bytes) {
        Ok(()) => Ok(true),
        Err(WriteError::Exists(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Create a new, empty folder (and any missing parents); refused if something is already
/// there (`taken`).
pub fn create_dir(path: &Path) -> Result<(), WriteError> {
    if taken(path) {
        return Err(WriteError::Exists(path.to_path_buf()));
    }
    std::fs::create_dir_all(path).map_err(io_err(path))
}

/// Move a file, a folder or a symlink (the link itself); refused unless it `may_move`, or when
/// something is at `to` by the time it moves (`os_rename::no_replace`, #230).
pub fn move_file(from: &Path, to: &Path) -> Result<(), WriteError> {
    move_with(from, to, || {})
}

fn move_with(from: &Path, to: &Path, between: impl FnOnce()) -> Result<(), WriteError> {
    let at = at(from, to);
    if at == At::Other {
        return Err(WriteError::Exists(to.to_path_buf()));
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    }
    between();
    // Onto itself only the plain rename goes: to the system, the new spelling is taken.
    let moved = match at {
        At::Itself => std::fs::rename(from, to),
        _ => os_rename::no_replace(from, to),
    };
    moved.map_err(|e| refused_at(from, to, e))
}

/// Whether `from` may move to `to`: nothing is there (`taken`), or `to` differs from `from`
/// only in case and names the very entry `from` does, as on a file system that ignores case.
/// Where case tells names apart, that spelling can be another file, which a move would replace.
pub(crate) fn may_move(from: &Path, to: &Path) -> bool {
    at(from, to) != At::Other
}

/// What a move from `from` finds at `to`.
#[derive(PartialEq)]
enum At {
    Nothing,
    /// `from` itself, under a spelling that differs only in case.
    Itself,
    Other,
}

fn at(from: &Path, to: &Path) -> At {
    let Ok(there) = std::fs::symlink_metadata(to) else {
        return At::Nothing;
    };
    let case_only = from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
    match case_only && std::fs::symlink_metadata(from).is_ok_and(|here| same_entry(&here, &there)) {
        true => At::Itself,
        false => At::Other,
    }
}

#[cfg(unix)]
fn same_entry(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    (a.dev(), a.ino()) == (b.dev(), b.ino())
}

/// Windows' std gives no stable file id yet; its file systems ignore case by default.
#[cfg(not(unix))]
fn same_entry(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cold-lava-write-{}-{name}", std::process::id()));
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

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let d = dir("atomic");
        write_atomic(&d.join("sub/Note.md"), b"hello").unwrap();
        write_atomic(&d.join("sub/Note.md"), b"again").unwrap();
        assert_eq!(std::fs::read(d.join("sub/Note.md")).unwrap(), b"again");
        assert_eq!(files(&d.join("sub")), ["Note.md"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn replace_refuses_outside_changes() {
        let d = dir("replace");
        let p = d.join("N.md");
        std::fs::write(&p, "v1").unwrap();
        replace(&p, b"v1", b"v2").unwrap();
        assert!(matches!(
            replace(&p, b"v1", b"v3"),
            Err(WriteError::Changed(_))
        ));
        assert_eq!(std::fs::read(&p).unwrap(), b"v2");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn theirs_in_place(p: &Path) {
        std::fs::write(p, "theirs").unwrap();
    }

    fn theirs_by_a_rename(p: &Path) {
        let other = p.with_file_name("theirs.tmp");
        std::fs::write(&other, "theirs").unwrap();
        std::fs::rename(&other, p).unwrap();
    }

    const THEIRS: [(&str, fn(&Path)); 2] = [
        ("in place", theirs_in_place),
        ("by a rename", theirs_by_a_rename),
    ];

    /// Another program saves the file after `replace` compared it and before the new text is
    /// in place (#395): the replace is refused, the other program's text stays, and nothing
    /// of ours is left beside it.
    #[test]
    fn a_save_that_lands_while_a_replace_writes_is_kept() {
        let d = dir("race");
        let p = d.join("N.md");
        for (how, theirs) in THEIRS {
            std::fs::write(&p, "v1").unwrap();
            let refused = replace_with(&p, &p, b"v1", b"ours", || theirs(&p));
            assert!(
                matches!(&refused, Err(WriteError::Changed(at)) if *at == p),
                "{how}: {refused:?}"
            );
            assert_eq!(std::fs::read(&p).unwrap(), b"theirs", "{how}");
            assert_eq!(files(&d), ["N.md"], "{how}: nothing left beside it");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The same on a file system that can't swap two files: the file is moved aside, compared
    /// there and put back (`aside`). With no other write, the new text goes in.
    #[test]
    fn a_save_that_lands_where_files_cant_be_swapped_is_kept() {
        let d = dir("race-aside");
        let p = d.join("N.md");
        for (how, theirs) in THEIRS {
            std::fs::write(&p, "v1").unwrap();
            let new = staged(&p, &p, b"ours").unwrap();
            theirs(&p);
            let refused = aside(&p, &p, &new, b"v1");
            assert!(
                matches!(&refused, Err(WriteError::Changed(at)) if *at == p),
                "{how}: {refused:?}"
            );
            assert_eq!(std::fs::read(&p).unwrap(), b"theirs", "{how}");
            assert_eq!(files(&d), ["N.md"], "{how}: nothing left beside it");
        }
        let new = staged(&p, &p, b"ours").unwrap();
        aside(&p, &p, &new, b"theirs").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"ours");
        assert_eq!(files(&d), ["N.md"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A third write lands on the new text in the moment before the other write's text goes
    /// back: the other text is put back, and the third, which is neither, stays beside the file
    /// and is named.
    #[test]
    fn a_text_that_cant_be_put_back_is_kept_and_named() {
        let d = dir("put-back");
        let p = d.join("N.md");
        std::fs::write(&p, "third").unwrap();
        let out = staged(&p, &p, b"theirs").unwrap();
        let kept = put_back(&p, &p, &out, b"ours");
        assert!(
            matches!(&kept, Err(WriteError::Kept(at, beside)) if *at == p && *beside == out),
            "{kept:?}"
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&out).unwrap(), b"third");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The rename that never replaces, with the system's flag and by a hard link: refused onto
    /// a file or a folder that is there, both left as they were; done where nothing is.
    #[test]
    fn a_rename_never_replaces_what_is_there() {
        let d = dir("no-replace");
        let (a, b) = (d.join("A.md"), d.join("B.md"));
        let (empty, full) = (d.join("Empty"), d.join("Full"));
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(full.join("Inside")).unwrap();
        type Rename = fn(&Path, &Path) -> io::Result<()>;
        let renames: [(&str, Rename); 2] = [
            ("no_replace", os_rename::no_replace),
            ("by_link", os_rename::by_link),
        ];
        for (how, rename) in renames {
            std::fs::write(&a, "a").unwrap();
            std::fs::write(&b, "b").unwrap();
            let refused = rename(&a, &b).unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists, "{how}");
            let folder = rename(&empty, &full).unwrap_err();
            assert_eq!(folder.kind(), io::ErrorKind::AlreadyExists, "{how}");
            assert!(full.join("Inside").is_dir(), "{how}");
            assert_eq!(std::fs::read(&a).unwrap(), b"a", "{how}");
            assert_eq!(std::fs::read(&b).unwrap(), b"b", "{how}");
            std::fs::remove_file(&b).unwrap();
            rename(&a, &b).unwrap();
            assert_eq!(files(&d), ["B.md", "Empty", "Full"], "{how}");
            assert_eq!(std::fs::read(&b).unwrap(), b"a", "{how}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Another program makes a file where a move goes, after `move_file` found nothing there
    /// (#230): the move is refused, and both files keep their text.
    #[test]
    fn a_file_made_where_a_move_goes_is_never_replaced() {
        let d = dir("move-race");
        let (a, b) = (d.join("A.md"), d.join("B.md"));
        std::fs::write(&a, "ours").unwrap();
        let refused = move_with(&a, &b, || std::fs::write(&b, "theirs").unwrap());
        assert!(
            matches!(&refused, Err(WriteError::Exists(at)) if *at == b),
            "{refused:?}"
        );
        assert_eq!(std::fs::read(&a).unwrap(), b"ours");
        assert_eq!(std::fs::read(&b).unwrap(), b"theirs");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn create_and_move() {
        let d = dir("create");
        let (a, b) = (d.join("A.md"), d.join("x/B.md"));
        create(&a, b"a").unwrap();
        assert!(matches!(create(&a, b"again"), Err(WriteError::Exists(_))));
        assert!(!create_if_missing(&a, b"again").unwrap());
        assert_eq!(std::fs::read(&a).unwrap(), b"a");
        move_file(&a, &b).unwrap();
        assert!(!a.exists() && b.exists());
        create(&a, b"new a").unwrap();
        assert!(
            matches!(move_file(&a, &b), Err(WriteError::Exists(_))),
            "won't overwrite"
        );
        create_dir(&d.join("x/Empty/Inner")).unwrap();
        assert!(d.join("x/Empty/Inner").is_dir());
        assert!(matches!(
            create_dir(&d.join("x/Empty")),
            Err(WriteError::Exists(_))
        ));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Where case tells names apart, `lava.md` beside `Lava.md` is another entry: a case-only
    /// move onto it is refused, and with nothing there it goes ahead.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_case_only_move_never_replaces_another_entry() {
        let d = dir("case");
        let (upper, lower) = (d.join("Lava.md"), d.join("lava.md"));
        std::fs::write(&upper, "upper").unwrap();
        std::fs::write(&lower, "lower").unwrap();
        let refused = move_file(&upper, &lower);
        assert!(
            matches!(&refused, Err(WriteError::Exists(p)) if *p == lower),
            "{refused:?}"
        );
        assert_eq!(std::fs::read(&upper).unwrap(), b"upper");
        assert_eq!(std::fs::read(&lower).unwrap(), b"lower");

        std::fs::remove_file(&lower).unwrap();
        std::os::unix::fs::symlink("nowhere.md", &lower).unwrap();
        assert!(
            matches!(move_file(&upper, &lower), Err(WriteError::Exists(_))),
            "a link to nothing at the new spelling is another entry too"
        );
        assert!(is_link(&lower));

        std::fs::remove_file(&lower).unwrap();
        move_file(&upper, &lower).unwrap();
        assert_eq!(files(&d), ["lava.md"]);
        assert_eq!(std::fs::read(&lower).unwrap(), b"upper");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    fn is_link(path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    }

    #[cfg(unix)]
    #[test]
    fn a_write_to_a_symlink_lands_in_the_file_it_leads_to() {
        let d = dir("through");
        std::fs::create_dir_all(d.join("Elsewhere")).unwrap();
        std::fs::create_dir_all(d.join("Notes")).unwrap();
        let target = d.join("Elsewhere/Lava.md");
        std::fs::write(&target, "v1").unwrap();
        let (link, hop) = (d.join("Notes/Lava.md"), d.join("Notes/Hop.md"));
        std::os::unix::fs::symlink("../Elsewhere/Lava.md", &link).unwrap();
        std::os::unix::fs::symlink("Lava.md", &hop).unwrap();

        write_atomic(&link, b"v2").unwrap();
        replace_through_link(&hop, b"v2", b"v3").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"v3");
        let led = |p: &Path| std::fs::read_link(p).unwrap();
        assert_eq!(
            led(&link),
            Path::new("../Elsewhere/Lava.md"),
            "still a link"
        );
        assert_eq!(led(&hop), Path::new("Lava.md"), "still a link");
        assert!(
            matches!(
                replace_through_link(&link, b"v2", b"lost"),
                Err(WriteError::Changed(_))
            ),
            "a stale read is refused through a link too"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"v3");
        assert_eq!(files(&d.join("Notes")), ["Hop.md", "Lava.md"]);
        assert_eq!(
            files(&d.join("Elsewhere")),
            ["Lava.md"],
            "no temp file left"
        );

        move_file(&hop, &d.join("Notes/Moved.md")).unwrap();
        assert!(
            is_link(&d.join("Notes/Moved.md")),
            "a move takes the link itself"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `replace` never follows a link: one at the path is refused, and the file it leads to
    /// keeps its bytes.
    #[cfg(unix)]
    #[test]
    fn a_replace_refuses_a_link_at_the_path() {
        let d = dir("refuse-link");
        let (target, at) = (d.join("config"), d.join("Note.md"));
        std::fs::write(&target, "v1").unwrap();
        std::os::unix::fs::symlink("config", &at).unwrap();
        let refused = replace(&at, b"v1", b"lost");
        assert!(
            matches!(&refused, Err(WriteError::Link(p)) if *p == at),
            "{refused:?}"
        );
        let words = format!(
            "{} is a symbolic link, which this write never follows",
            at.display()
        );
        assert_eq!(refused.unwrap_err().to_string(), words);
        assert!(is_link(&at), "the link stays");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"v1",
            "nothing written through"
        );
        assert_eq!(files(&d), ["Note.md", "config"], "no temp file left");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A link swapped in at the path after `replace` checked it: the rename replaces the link,
    /// and the file it leads to keeps its bytes.
    #[cfg(unix)]
    #[test]
    fn a_link_swapped_in_after_the_check_is_replaced_and_never_followed() {
        let d = dir("entry");
        let (target, at) = (d.join("config"), d.join("Note.md"));
        std::fs::write(&target, "v1").unwrap();
        std::os::unix::fs::symlink("config", &at).unwrap();
        let stale = replace_at(&at, &at, b"other", b"lost");
        assert!(matches!(stale, Err(WriteError::Changed(_))), "{stale:?}");
        assert!(is_link(&at), "a stale read writes nothing");
        replace_at(&at, &at, b"v1", b"v2").unwrap();
        assert!(!is_link(&at), "the link is replaced");
        assert_eq!(std::fs::read(&at).unwrap(), b"v2");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"v1",
            "nothing written through"
        );
        assert_eq!(files(&d), ["Note.md", "config"], "no temp file left");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A move onto a link, or a folder made at one, finds the path `taken` as a new file does,
    /// a link to nothing included (#356).
    #[cfg(unix)]
    #[test]
    fn nothing_is_written_over_a_symlink_to_nothing_or_to_a_folder() {
        let d = dir("not-through");
        std::fs::create_dir(d.join("Folder")).unwrap();
        let note = d.join("Note.md");
        std::fs::write(&note, "kept").unwrap();
        std::os::unix::fs::symlink("Gone.md", d.join("Dangling.md")).unwrap();
        std::os::unix::fs::symlink("Folder", d.join("Folder.md")).unwrap();
        for name in ["Dangling.md", "Folder.md"] {
            let at = d.join(name);
            let refused =
                |e: Result<(), WriteError>| matches!(e, Err(WriteError::NotAFile(p)) if p == at);
            let there =
                |e: Result<(), WriteError>| matches!(e, Err(WriteError::Exists(p)) if p == at);
            assert!(refused(write_atomic(&at, b"x")), "write {name}");
            assert!(
                refused(replace_through_link(&at, b"", b"x")),
                "replace {name}"
            );
            assert!(
                matches!(replace(&at, b"", b"x"), Err(WriteError::Link(p)) if p == at),
                "replace {name}"
            );
            assert!(taken(&at), "{name} is taken");
            assert!(there(create(&at, b"x")), "create {name}");
            assert!(!create_if_missing(&at, b"x").unwrap(), "{name}");
            assert!(there(create_dir(&at)), "folder {name}");
            assert!(there(move_file(&note, &at)), "move onto {name}");
            assert!(is_link(&at), "{name} is still a link");
        }
        let said = write_atomic(&d.join("Dangling.md"), b"x").unwrap_err();
        let words = format!(
            "{} is a symbolic link that doesn't lead to a file",
            d.join("Dangling.md").display()
        );
        assert_eq!(said.to_string(), words);
        assert_eq!(std::fs::read(&note).unwrap(), b"kept", "nothing moved");
        assert_eq!(
            files(&d),
            ["Dangling.md", "Folder", "Folder.md", "Note.md"],
            "nothing made through a link to nothing"
        );
        assert!(files(&d.join("Folder")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
