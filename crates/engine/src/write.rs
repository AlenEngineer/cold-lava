//! The one way cold-lava writes to a workspace.
//!
//! Every write is atomic (a hidden temporary file in the same folder, then a rename), so Obsidian,
//! Neovim or a sync tool never sees a half-written note. Writes that replace existing content
//! can say what they expect to be there, and are refused if the file changed in the meantime, so
//! an edit made elsewhere is never overwritten.

use std::fmt;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum WriteError {
    /// The file's content isn't what the caller read earlier: someone else changed it.
    Changed(PathBuf),
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
/// `replace_through_link` land here; `replace` and `create` rename over the path itself, so a
/// link swapped in at the path after its check is replaced, not followed. A folder on the way
/// swapped for a link is still followed (#366).
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
    let dir = file
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Hidden, so file watchers (ours and Obsidian's) ignore it.
    let tmp = dir.join(format!(".{name}.cold-lava-{}.tmp", std::process::id()));
    let write = || -> io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        // Keep the original file's permissions.
        if let Ok(meta) = std::fs::metadata(file) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, file)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        WriteError::Io(path.to_path_buf(), e)
    })
}

/// Replace the file at `path` only if it still contains `expected` (what the caller read
/// before). Outside the engine, `read::Seen::replace` is the way. It never follows a symlink: a
/// link at `path` is refused (`Link`), and one swapped in after that check is replaced by the
/// rename, never written through. Every path the engine found by its scan, or the gateway let
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
    let now = std::fs::read(file).map_err(io_err(path))?;
    if now != expected {
        return Err(WriteError::Changed(path.to_path_buf()));
    }
    write_at(path, file, bytes)
}

/// Whether something is at `path`: a file, a folder or a symlink, a link to nothing included.
/// A new file never goes where something is.
pub fn taken(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Create a new file; refused if something is already there (`taken`). The rename puts it at
/// `path` itself, so a link made there after the check is replaced, never written through.
pub fn create(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    if taken(path) {
        return Err(WriteError::Exists(path.to_path_buf()));
    }
    write_at(path, path, bytes)
}

/// Create a new file unless it already exists (then leave it alone). Returns whether it was made.
pub fn create_if_missing(path: &Path, bytes: &[u8]) -> Result<bool, WriteError> {
    match create(path, bytes) {
        Ok(()) => Ok(true),
        Err(WriteError::Exists(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Create a new, empty folder (and any missing parents); refused if it already exists.
pub fn create_dir(path: &Path) -> Result<(), WriteError> {
    if path.exists() {
        return Err(WriteError::Exists(path.to_path_buf()));
    }
    std::fs::create_dir_all(path).map_err(io_err(path))
}

/// Move a file or folder, refusing to overwrite an existing one (except a case-only rename of itself).
pub fn move_file(from: &Path, to: &Path) -> Result<(), WriteError> {
    let same_file_case_change =
        to.exists() && from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
    if to.exists() && !same_file_case_change {
        return Err(WriteError::Exists(to.to_path_buf()));
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    }
    std::fs::rename(from, to).map_err(io_err(from))
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

    #[cfg(unix)]
    #[test]
    fn nothing_is_written_over_a_symlink_to_nothing_or_to_a_folder() {
        let d = dir("not-through");
        std::fs::create_dir(d.join("Folder")).unwrap();
        std::os::unix::fs::symlink("Gone.md", d.join("Dangling.md")).unwrap();
        std::os::unix::fs::symlink("Folder", d.join("Folder.md")).unwrap();
        for name in ["Dangling.md", "Folder.md"] {
            let at = d.join(name);
            let refused =
                |e: Result<(), WriteError>| matches!(e, Err(WriteError::NotAFile(p)) if p == at);
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
            assert!(
                matches!(create(&at, b"x"), Err(WriteError::Exists(_))),
                "create {name}"
            );
            assert!(!create_if_missing(&at, b"x").unwrap(), "{name}");
            assert!(is_link(&at), "{name} is still a link");
        }
        let said = write_atomic(&d.join("Dangling.md"), b"x").unwrap_err();
        let words = format!(
            "{} is a symbolic link that doesn't lead to a file",
            d.join("Dangling.md").display()
        );
        assert_eq!(said.to_string(), words);
        assert_eq!(
            files(&d),
            ["Dangling.md", "Folder", "Folder.md"],
            "nothing made through a link to nothing"
        );
        assert!(files(&d.join("Folder")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
