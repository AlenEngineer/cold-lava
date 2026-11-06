//! The one way cold-lava writes to a vault.
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
    /// Creating a file that already exists.
    Exists(PathBuf),
    Io(PathBuf, io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WriteError::Changed(p) => write!(f, "{} changed since it was read", p.display()),
            WriteError::Exists(p) => write!(f, "{} already exists", p.display()),
            WriteError::Io(p, e) => write!(f, "{}: {e}", p.display()),
        }
    }
}

impl std::error::Error for WriteError {}

fn io_err(path: &Path) -> impl Fn(io::Error) -> WriteError + '_ {
    move |e| WriteError::Io(path.to_path_buf(), e)
}

/// Replace (or create) `path` with `bytes`, atomically. Creates missing folders.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io_err(dir))?;
    let name = path
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
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        WriteError::Io(path.to_path_buf(), e)
    })
}

/// Replace `path` only if it still contains `expected` (what the caller read before).
pub fn replace(path: &Path, expected: &[u8], bytes: &[u8]) -> Result<(), WriteError> {
    let now = std::fs::read(path).map_err(io_err(path))?;
    if now != expected {
        return Err(WriteError::Changed(path.to_path_buf()));
    }
    write_atomic(path, bytes)
}

/// Create a new file; refused if it already exists.
pub fn create(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    if path.exists() {
        return Err(WriteError::Exists(path.to_path_buf()));
    }
    write_atomic(path, bytes)
}

/// Create a new file unless it already exists (then leave it alone). Returns whether it was made.
pub fn create_if_missing(path: &Path, bytes: &[u8]) -> Result<bool, WriteError> {
    match create(path, bytes) {
        Ok(()) => Ok(true),
        Err(WriteError::Exists(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Move a file, refusing to overwrite an existing one (except a case-only rename of itself).
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
        let _ = std::fs::remove_dir_all(&d);
    }
}
