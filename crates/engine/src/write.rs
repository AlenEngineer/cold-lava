//! The one way cold-lava writes to a workspace.
//!
//! Every write is atomic (a hidden temporary file in the same folder, then a rename), so Obsidian,
//! Neovim or a sync tool never sees a half-written note. Writes that replace existing content
//! can say what they expect to be there, and are refused if the file changed in the meantime, so
//! an edit made elsewhere is never overwritten, nor one that lands while the write is under way:
//! the new file goes in by a swap or a rename that never replaces (`os_rename`), and what came
//! out is checked.

mod lock;
mod os_rename;
mod replace;

use std::fmt;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub enum WriteError {
    /// The file's content isn't what the caller read earlier: someone else changed it.
    Changed(PathBuf),
    /// Another write landed at the path (the first) while this one was under way, and a text
    /// taken out of the file couldn't be put back: it is kept in the second path, a hidden
    /// file beside it named `.Name.cold-lava-kept-…` (`replace::kept_name`). It is never the
    /// caller's own old text or this write's. Read the file again: this write may or may not
    /// be in it. Not "read again" alone: a text now sits outside the file (gateway `kept`).
    Kept(PathBuf, PathBuf),
    /// Creating a file where something already is (`taken`).
    Exists(PathBuf),
    /// A symlink that leads to no file, to nothing or to a folder: a write never puts a file
    /// where a link is, and never makes one through it (`landing`).
    NotAFile(PathBuf),
    /// A symlink at the path, or on the way to it, of a write or a move that never follows one
    /// (`replace`; the workspace's `.trash`): refused, so a link is never turned into a file or
    /// gone through without a word.
    Link(PathBuf),
    Io(PathBuf, io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WriteError::Changed(p) => write!(f, "{} changed since it was read", p.display()),
            WriteError::Kept(p, kept) => f.write_str(&kept_words(p.display(), kept.display())),
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

/// `Kept` in its one wording, naming the file and where its text is kept as the caller does.
/// The kept file comes first, so a status line cut at 100 columns still shows where it is.
pub(crate) fn kept_words(name: impl fmt::Display, kept: impl fmt::Display) -> String {
    format!("other text kept in {kept} · {name} changed while saving")
}

/// `Kept`'s words naming the file by `rel`, its workspace path, and the kept text by the
/// workspace path of the hidden file beside it, as a write that follows no link keeps it.
/// Only for a caller that knows the workspace path but not the root (the edit desk,
/// `Edits::wrote`); one that has the root names the error with [`WriteError::named_from`].
pub(crate) fn kept_beside(rel: &str, kept: &Path) -> String {
    let name = kept.file_name().unwrap_or_default().to_string_lossy();
    let at = match rel.rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/{name}"),
        None => name.into_owned(),
    };
    kept_words(rel, at)
}

impl WriteError {
    /// The error's words, with a kept text's two paths named from `root`, the workspace's
    /// (`rel_path::or_whole`: a path outside it, as a link's target may be, is named whole).
    /// What a person is shown; the other refusals read as `Display` says them.
    pub fn named_from(&self, root: &Path) -> String {
        match self {
            WriteError::Kept(p, kept) => {
                let (p, kept) = kept_named(root, p, kept);
                kept_words(p, kept)
            }
            _ => self.to_string(),
        }
    }

    /// The same error, with a kept text's two paths named from `root` as
    /// [`WriteError::named_from`] names them, for a caller that passes the error on (the core
    /// reads its reason back, `Reason::of`) and whose words a person is shown.
    pub fn named_in(self, root: &Path) -> WriteError {
        match self {
            WriteError::Kept(p, kept) => {
                let (p, kept) = kept_named(root, &p, &kept);
                WriteError::Kept(p.into(), kept.into())
            }
            e => e,
        }
    }
}

/// A kept text's two paths, the file's and the kept text's, as a person is shown them: from
/// `root`, the workspace's (`rel_path::or_whole`: a path outside it is named whole). The one
/// rule `named_from` and `named_in` name them by.
fn kept_named(root: &Path, p: &Path, kept: &Path) -> (String, String) {
    (
        crate::rel_path::or_whole(root, p),
        crate::rel_path::or_whole(root, kept),
    )
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
    folder_made(file)?;
    let new = staged(path, file, bytes)?;
    std::fs::rename(&new, file).map_err(|e| unstaged(&new, io_err(path)(e)))
}

/// The folder `file` goes in, and any missing above it, made: what `write_atomic` and `create`
/// do first. `replace` never does: a file it replaces is in a folder already.
fn folder_made(file: &Path) -> Result<(), WriteError> {
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io_err(dir))
}

/// A new file beside `file` holding `bytes`, on disk and with `file`'s permissions: what a
/// write renames into place. It makes no folder: one that isn't there is `Io`. Errors name
/// `path`.
fn staged(path: &Path, file: &Path, bytes: &[u8]) -> Result<PathBuf, WriteError> {
    let new = reserved(file, beside).map_err(io_err(path))?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(&new)
        .map_err(|e| unstaged(&new, io_err(path)(e)))?;
    let written = f.write_all(bytes).and_then(|()| f.sync_all());
    written.map_err(|e| unstaged(&new, io_err(path)(e)))?;
    if let Ok(meta) = std::fs::metadata(file) {
        let _ = std::fs::set_permissions(&new, meta.permissions());
    }
    Ok(new)
}

/// A new, empty file beside `file`, at the first name `name` gives where nothing is
/// (`create_new`): a name no other file has, so only this write ever writes over it.
fn reserved(file: &Path, name: fn(&Path) -> PathBuf) -> io::Result<PathBuf> {
    loop {
        let at = name(file);
        match std::fs::File::create_new(&at) {
            Ok(_) => return Ok(at),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// A staged write's name beside `file`: hidden, so file watchers (ours and Obsidian's) ignore
/// it, and new each time this process asks (`reserved` makes sure no one else has it).
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
/// before), never over a write that lands meanwhile (`replace::checked`). Outside the engine,
/// `read::Seen::replace` is the way. It never follows a symlink: a link at `path` is refused
/// (`Link`), and one swapped in after that check is compared by what it leads to, then taken
/// out by the swap as a link, never written through. Every path the engine found by its scan,
/// or the gateway let through, is a file (`workspace::first_link`), so this is the write for
/// all of them.
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
    #[cfg(test)]
    if let Some((compared, swapped)) = lands::take() {
        let hooks = replace::Hooks { compared, swapped };
        return replace::checked(path, file, expected, bytes, hooks);
    }
    let hooks = replace::Hooks {
        compared: || {},
        swapped: |_| {},
    };
    replace::checked(path, file, expected, bytes, hooks)
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
    folder_made(path)?;
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

/// Tests only: other writes landed in one `replace` of this thread where `replace::Hooks`
/// lands them, for a test of a caller that can't hand `checked` its hooks (`rename::Written`).
#[cfg(test)]
pub(crate) mod lands {
    use std::cell::RefCell;

    type Compared = Box<dyn FnOnce()>;
    type Swapped = Box<dyn FnMut(usize)>;
    type Next = (usize, Option<(Compared, Swapped)>);

    thread_local! {
        static NEXT: RefCell<Next> = const { RefCell::new((0, None)) };
    }

    /// Land `compared` and `swapped` in this thread's `replace` after the next `skip` ones.
    pub(crate) fn in_replace(
        skip: usize,
        compared: impl FnOnce() + 'static,
        swapped: impl FnMut(usize) + 'static,
    ) {
        let hooks: (Compared, Swapped) = (Box::new(compared), Box::new(swapped));
        NEXT.with(|n| *n.borrow_mut() = (skip, Some(hooks)));
    }

    pub(super) fn take() -> Option<(Compared, Swapped)> {
        NEXT.with(|n| {
            let mut n = n.borrow_mut();
            match n.0 {
                _ if n.1.is_none() => None,
                0 => n.1.take(),
                _ => {
                    n.0 -= 1;
                    None
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::replace::{Hooks, aside, checked, keep, put_back};
    use super::*;
    use std::time::Duration;

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

    type Theirs = fn(&Path);

    const THEIRS: [(&str, Theirs); 2] = [
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
            let hooks = Hooks {
                compared: || theirs(&p),
                swapped: |_| {},
            };
            let refused = checked(&p, &p, b"v1", b"ours", hooks);
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

    /// The kept text `refused` names, checked to be beside `p` under a kept text's name (never
    /// a staged write's), and its file name.
    fn kept_beside(refused: &Result<(), WriteError>, p: &Path) -> (PathBuf, String) {
        let Err(WriteError::Kept(at, beside)) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(at, p);
        let name = beside.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with(".N.cold-lava-kept-") && name.ends_with(".md"),
            "{name}"
        );
        (beside.clone(), name)
    }

    fn third_in_place(p: &Path) {
        std::fs::write(p, "third").unwrap();
    }

    fn third_by_a_rename(p: &Path) {
        let other = p.with_file_name("third.tmp");
        std::fs::write(&other, "third").unwrap();
        std::fs::rename(&other, p).unwrap();
    }

    /// A third write lands on the new text after the swap took another write's text out and
    /// before it goes back. The swap back takes the third out, sees it isn't this write's text,
    /// and swaps it in again: the newest text stays at the path, the older one is kept beside
    /// it and named, and nothing of ours is left.
    #[test]
    fn a_write_between_the_swap_and_the_swap_back_stays_at_the_path() {
        let d = dir("between-swaps");
        let p = d.join("N.md");
        let thirds: [(&str, Theirs); 2] = [
            ("in place", third_in_place),
            ("by a rename", third_by_a_rename),
        ];
        for (how, third) in thirds {
            std::fs::write(&p, "v1").unwrap();
            let hooks = Hooks {
                compared: || theirs_by_a_rename(&p),
                swapped: |round| {
                    if round == 0 {
                        third(&p)
                    }
                },
            };
            let refused = checked(&p, &p, b"v1", b"ours", hooks);
            let (beside, name) = kept_beside(&refused, &p);
            assert_eq!(std::fs::read(&p).unwrap(), b"third", "{how}");
            assert_eq!(std::fs::read(&beside).unwrap(), b"theirs", "{how}");
            assert_eq!(files(&d), [name.as_str(), "N.md"], "{how}");
            std::fs::remove_file(&beside).unwrap();
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `put_back` on its own: the text in `out` goes in, the newer text that comes out goes in
    /// again, and the older one is kept under a kept text's name; the staged name is gone.
    #[test]
    fn a_text_that_cant_be_put_back_is_kept_and_named() {
        let d = dir("put-back");
        let p = d.join("N.md");
        std::fs::write(&p, "third").unwrap();
        let out = staged(&p, &p, b"theirs").unwrap();
        let kept = put_back(&p, &p, &out, b"v1", b"ours", |_| {});
        let (beside, name) = kept_beside(&kept, &p);
        assert_eq!(std::fs::read(&p).unwrap(), b"third");
        assert_eq!(std::fs::read(beside).unwrap(), b"theirs");
        assert_eq!(
            files(&d),
            [name.as_str(), "N.md"],
            "the staged name is gone"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A writer that doesn't stop: before each round of `put_back` another text lands at the
    /// path, so every round takes out a newer one, until the rounds run out. The last to land
    /// is the caller's old text (`expected`): it is newer than what the path holds, so it is
    /// never removed, but kept beside the path and named.
    #[test]
    fn when_the_rounds_run_out_the_newest_text_is_kept_and_named() {
        let d = dir("put-back-rounds");
        let p = d.join("N.md");
        std::fs::write(&p, "ours").unwrap();
        let out = staged(&p, &p, b"theirs").unwrap();
        let mut last = 0;
        let kept = put_back(&p, &p, &out, b"v1", b"ours", |round| {
            last = round;
            let text = if round == 15 {
                "v1".to_string()
            } else {
                format!("w{round}")
            };
            std::fs::write(&p, text).unwrap();
        });
        assert_eq!(last, 15, "sixteen rounds ran");
        let (beside, name) = kept_beside(&kept, &p);
        assert_eq!(std::fs::read(&beside).unwrap(), b"v1", "the newest is kept");
        assert_eq!(
            std::fs::read(&p).unwrap(),
            b"w14",
            "the path holds an older text"
        );
        assert_eq!(
            files(&d),
            [name.as_str(), "N.md"],
            "the staged name is gone"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A swap back that fails after a round took out a newer text: here the caller's old text
    /// (`expected`) landed, then the file was removed, so the next swap can't be made. What
    /// was taken out is newer than anything else, so it is never removed: it is kept and named.
    #[test]
    fn when_a_swap_back_fails_the_newest_text_is_kept_and_named() {
        let d = dir("put-back-stopped");
        let p = d.join("N.md");
        std::fs::write(&p, "ours").unwrap();
        let out = staged(&p, &p, b"theirs").unwrap();
        let kept = put_back(&p, &p, &out, b"v1", b"ours", |round| match round {
            0 => std::fs::write(&p, "v1").unwrap(),
            _ => std::fs::remove_file(&p).unwrap(),
        });
        let (beside, name) = kept_beside(&kept, &p);
        assert_eq!(std::fs::read(&beside).unwrap(), b"v1", "the newest is kept");
        assert_eq!(files(&d), [name.as_str()], "the staged name is gone");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Another program saves twice. B compared, the program saved, and B's swap took that save
    /// out; before B puts it back, the program reads the file (B's text) and saves again over
    /// it. B's swap back never undoes the later save: it stays at the path, the earlier one is
    /// kept beside it, and B's own text is gone.
    #[test]
    fn a_later_save_is_never_undone_by_the_swap_back() {
        let d = dir("two-writers");
        let p = d.join("N.md");
        std::fs::write(&p, "v1").unwrap();
        let later = |p: &Path| {
            let other = p.with_file_name("later.tmp");
            std::fs::write(&other, "c").unwrap();
            std::fs::rename(&other, p).unwrap();
        };
        let hooks = Hooks {
            compared: || theirs_by_a_rename(&p),
            swapped: |round| {
                if round == 0 {
                    assert_eq!(std::fs::read(&p).unwrap(), b"b");
                    later(&p);
                }
            },
        };
        let refused = checked(&p, &p, b"v1", b"b", hooks);
        let (beside, name) = kept_beside(&refused, &p);
        assert_eq!(std::fs::read(&p).unwrap(), b"c");
        assert_eq!(std::fs::read(&beside).unwrap(), b"theirs");
        assert_eq!(files(&d), [name.as_str(), "N.md"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Three cold-lava writers replace one file at once, each over the text all three read
    /// (#425: three `cold-lava item set` on one task). They take turns (`lock`): the one told it
    /// is done is the one at the path, the other two are refused as changed, and nothing is
    /// kept. Each waits after its compare, so without turns all three would swap at once.
    #[test]
    fn of_three_cold_lava_writers_the_one_told_done_is_at_the_path() {
        let d = dir("three-writers");
        let p = d.join("N.md");
        for round in 0..5 {
            std::fs::write(&p, "v1").unwrap();
            let start = std::sync::Barrier::new(3);
            let results: Vec<(&[u8], Result<(), WriteError>)> = std::thread::scope(|s| {
                let writers: Vec<_> = [&b"a"[..], &b"b"[..], &b"c"[..]]
                    .into_iter()
                    .map(|ours| {
                        let (p, start) = (&p, &start);
                        s.spawn(move || {
                            start.wait();
                            let hooks = Hooks {
                                compared: || std::thread::sleep(Duration::from_millis(30)),
                                swapped: |_| {},
                            };
                            (ours, checked(p, p, b"v1", ours, hooks))
                        })
                    })
                    .collect();
                writers.into_iter().map(|w| w.join().unwrap()).collect()
            });
            let done: Vec<&[u8]> = (results.iter())
                .filter(|(_, r)| r.is_ok())
                .map(|(ours, _)| *ours)
                .collect();
            assert_eq!(done.len(), 1, "round {round}: {results:?}");
            assert_eq!(std::fs::read(&p).unwrap(), done[0], "round {round}");
            for (_, r) in &results {
                let told = matches!(r, Ok(()) | Err(WriteError::Changed(_)));
                assert!(told, "round {round}: {r:?}");
            }
            assert_eq!(files(&d), ["N.md"], "round {round}: nothing kept");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A writer's turn holds the folder until it is dropped; another waits for it at most as
    /// long as it is told, then goes ahead without the lock instead of hanging.
    #[test]
    fn a_turn_is_waited_for_and_never_forever() {
        let d = dir("turn");
        let turn = lock::take(&d.join("N.md"));
        assert!(turn.held());
        let started = std::time::Instant::now();
        assert!(lock::locked(&d, Duration::from_millis(50)).is_none());
        assert!(started.elapsed() >= Duration::from_millis(50));
        drop(turn);
        assert!(lock::locked(&d, Duration::from_millis(50)).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// B as above, then another cold-lava writer, C, after B's swap back: B put the other
    /// program's text back, so C, which read B's text, is refused too, and nothing is kept.
    #[test]
    fn a_replace_after_the_swap_back_is_refused() {
        let d = dir("two-writers-after");
        let p = d.join("N.md");
        std::fs::write(&p, "v1").unwrap();
        let hooks = Hooks {
            compared: || theirs_by_a_rename(&p),
            swapped: |_| {},
        };
        let b = checked(&p, &p, b"v1", b"b", hooks);
        assert!(matches!(&b, Err(WriteError::Changed(_))), "{b:?}");
        let c = replace(&p, b"b", b"c");
        assert!(matches!(&c, Err(WriteError::Changed(_))), "{c:?}");
        assert_eq!(std::fs::read(&p).unwrap(), b"theirs");
        assert_eq!(files(&d), ["N.md"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// What a write took out and can't put back is removed when it is only the caller's old
    /// text or this write's own: nobody's text is lost, so it is `Changed`, not `Kept`.
    #[test]
    fn only_another_programs_text_is_kept() {
        let d = dir("keep");
        let p = d.join("N.md");
        std::fs::write(&p, "now").unwrap();
        for own in ["v1", "ours"] {
            let out = staged(&p, &p, own.as_bytes()).unwrap();
            let refused = keep(&p, &p, &out, &[b"ours".as_slice(), b"v1"]);
            assert!(
                matches!(&refused, WriteError::Changed(at) if *at == p),
                "{own}: {refused:?}"
            );
            assert_eq!(files(&d), ["N.md"], "{own}");
        }
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

    /// A link swapped in at the path after `replace` checked it: it is compared by what it
    /// leads to, then swapped out as a link and removed, and the file it leads to keeps its
    /// bytes.
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

    /// A cold-lava killed during a save leaves its staged file (#443). The next write in that
    /// folder takes it away: removed when it holds what its file holds, kept under a kept name
    /// otherwise (it may be the killed save's text, or another program's it took out). A staged
    /// file of a live cold-lava, one staged moments ago, and a kept text are left alone.
    #[test]
    fn a_staged_file_a_killed_save_left_is_cleared_by_the_next_write() {
        let d = dir("stale");
        let dead = {
            let mut child = std::process::Command::new("true").spawn().unwrap();
            let pid = child.id();
            child.wait().unwrap();
            pid
        };
        let live = std::process::id();
        let long_ago = std::time::SystemTime::now() - Duration::from_secs(600);
        let leave = |name: &str, text: &str, when: Option<std::time::SystemTime>| {
            std::fs::write(d.join(name), text).unwrap();
            if let Some(when) = when {
                let f = std::fs::File::options()
                    .write(true)
                    .open(d.join(name))
                    .unwrap();
                f.set_modified(when).unwrap();
            }
        };
        std::fs::write(d.join("N.md"), "now").unwrap();
        let copy = format!(".N.md.cold-lava-{dead}-0.tmp");
        let other = format!(".N.md.cold-lava-{dead}-1.tmp");
        let alive = format!(".N.md.cold-lava-{live}-900.tmp");
        let young = format!(".N.md.cold-lava-{dead}-2.tmp");
        let kept = ".N.cold-lava-kept-5-0.md";
        leave(&copy, "now", Some(long_ago));
        leave(&other, "the killed save", Some(long_ago));
        leave(&alive, "a save under way", Some(long_ago));
        leave(&young, "staged just now", None);
        leave(kept, "kept", Some(long_ago));
        write_atomic(&d.join("Other.md"), b"x").unwrap();
        let left = files(&d);
        assert!(
            !left.contains(&copy),
            "a copy of the file is removed: {left:?}"
        );
        assert!(!left.contains(&other), "{left:?}");
        let keeps: Vec<_> = left
            .iter()
            .filter(|f| f.starts_with(".N.cold-lava-kept-") && *f != kept)
            .collect();
        assert_eq!(keeps.len(), 1, "{left:?}");
        assert_eq!(std::fs::read(d.join(keeps[0])).unwrap(), b"the killed save");
        for stays in [&alive, &young, &kept.to_string()] {
            assert!(left.contains(stays), "{stays} stays: {left:?}");
        }
        assert_eq!(std::fs::read(d.join("N.md")).unwrap(), b"now");
        let _ = std::fs::remove_dir_all(&d);
    }
}
