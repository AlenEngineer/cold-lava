//! A file, folder or link as it was seen, so what is done to it later is refused if it changed
//! since: `Seen` says whether it is `unchanged`, and `replace` writes over a file only then.
//!
//! A file is read through a symlink, as every write reads it (`Seen::file`), or is known by
//! bytes read earlier or just written (`Seen::kept`). Whatever is at a path, a folder or the
//! link itself, is read without following one (`Seen::entry`). Each is read again the way it
//! was seen. A delete checks the entry itself, whatever way it was seen (`unchanged_entry`).
//!
//! It is the one home of a version, as the gateway names it: `rev` of a file's bytes, and
//! `Seen::rev` of what is at a path.

use crate::identity;
use crate::write::{self, WriteError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What was at `path` when it was seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    path: PathBuf,
    was: Contents,
    way: Way,
}

/// How a path is read, the first time and every time after.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    /// A file's bytes, through a symlink.
    File,
    /// What is at the path, never following a symlink.
    Entry,
}

/// What was read isn't UTF-8 text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotText;

impl NotText {
    /// The refusal, in its one wording, naming what isn't text.
    pub fn about(self, name: &str) -> String {
        format!("{name} isn't UTF-8 text")
    }
}

impl Seen {
    /// The file at `path`, through a symlink.
    pub fn file(path: &Path) -> io::Result<Seen> {
        Seen::by(Way::File, path)
    }

    /// Whatever is at `path`: a file, a folder by its listing, or a symlink by where it points.
    pub fn entry(path: &Path) -> io::Result<Seen> {
        Seen::by(Way::Entry, path)
    }

    /// The file at `path` as holding `bytes`, read earlier or just written. It is checked and
    /// written over as a file seen now (`file`) is.
    pub fn kept(path: &Path, bytes: impl Into<Vec<u8>>) -> Seen {
        Seen {
            path: path.to_path_buf(),
            was: Contents::File(bytes.into()),
            way: Way::File,
        }
    }

    fn by(way: Way, path: &Path) -> io::Result<Seen> {
        Ok(Seen {
            path: path.to_path_buf(),
            was: way.read(path)?,
            way,
        })
    }

    /// Where it was seen.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The bytes read, when it was a file.
    pub fn bytes(&self) -> Option<&[u8]> {
        match &self.was {
            Contents::File(bytes) => Some(bytes.as_slice()),
            _ => None,
        }
    }

    /// What a folder held when it was seen, each path relative to it, sorted; `None` for a file
    /// or a link.
    pub fn inside(&self) -> Option<impl Iterator<Item = &Path>> {
        match &self.was {
            Contents::Folder(entries) => Some(entries.iter().map(|(path, _, _)| path.as_path())),
            _ => None,
        }
    }

    /// The text read, when it was a file of UTF-8 text.
    pub fn text(&self) -> Result<&str, NotText> {
        let bytes = self.bytes().ok_or(NotText)?;
        std::str::from_utf8(bytes).map_err(|_| NotText)
    }

    /// The version read, as the gateway names it (`workspace/read`, `workspace/stat`).
    pub fn rev(&self) -> String {
        rev_of(&self.was)
    }

    /// Refused with `WriteError::Changed` unless the path still holds what was seen, read again
    /// the way it was seen.
    pub fn unchanged(&self) -> Result<(), WriteError> {
        self.unchanged_as(self.way)
    }

    /// Refused with `WriteError::Changed` unless the entry at the path, a symlink never
    /// followed, still holds what was seen, whatever way it was seen: the check before a delete,
    /// which moves the entry itself. A file seen through a link is not the link.
    pub fn unchanged_entry(&self) -> Result<(), WriteError> {
        self.unchanged_as(Way::Entry)
    }

    fn unchanged_as(&self, way: Way) -> Result<(), WriteError> {
        let now = way.read(&self.path);
        if now.map_err(|e| WriteError::Io(self.path.clone(), e))? != self.was {
            return Err(WriteError::Changed(self.path.clone()));
        }
        Ok(())
    }

    /// Write `bytes` over the file seen, through `write::replace`. Refused with
    /// `WriteError::Changed` if it changed since, or if it wasn't seen as a file (`file`,
    /// `kept`), since a write reads through a symlink and an entry never follows one.
    pub fn replace(&self, bytes: &[u8]) -> Result<(), WriteError> {
        match (self.way, &self.was) {
            (Way::File, Contents::File(was)) => write::replace(&self.path, was, bytes),
            _ => Err(WriteError::Changed(self.path.clone())),
        }
    }

    /// As `replace`, but over the path itself, through `write::replace_entry`: a symlink found
    /// there now is replaced, never written through. The gateway's write, which follows no
    /// link, so one swapped in after its check can't lead the write to another file.
    pub fn replace_entry(&self, bytes: &[u8]) -> Result<(), WriteError> {
        match (self.way, &self.was) {
            (Way::File, Contents::File(was)) => write::replace_entry(&self.path, was, bytes),
            _ => Err(WriteError::Changed(self.path.clone())),
        }
    }
}

impl Way {
    fn read(self, path: &Path) -> io::Result<Contents> {
        match self {
            Way::File => std::fs::read(path).map(Contents::File),
            Way::Entry => Contents::at(path),
        }
    }
}

/// What the user was shown of a file or folder, so a delete is refused if it changed since.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Contents {
    File(Vec<u8>),
    /// Everything inside the folder (relative paths, sorted), with each file's size and time.
    Folder(Vec<(PathBuf, u64, Option<SystemTime>)>),
    /// A symlink, by where it points: the link is what goes, and it is never followed.
    Link(PathBuf),
}

impl Contents {
    /// What is at `path` now, never following a symlink.
    fn at(path: &Path) -> io::Result<Contents> {
        let meta = std::fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return std::fs::read_link(path).map(Contents::Link);
        }
        if !meta.is_dir() {
            return std::fs::read(path).map(Contents::File);
        }
        let mut entries = Vec::new();
        for e in walkdir::WalkDir::new(path).min_depth(1).sort_by_file_name() {
            let e = e.map_err(io::Error::from)?;
            let meta = e.metadata().map_err(io::Error::from)?;
            let rel = e
                .path()
                .strip_prefix(path)
                .unwrap_or(e.path())
                .to_path_buf();
            let size = if meta.is_file() { meta.len() } else { 0 };
            entries.push((rel, size, meta.modified().ok()));
        }
        Ok(Contents::Folder(entries))
    }
}

/// A file's version: a hash of its bytes, as 16 hex digits.
pub fn rev(bytes: &[u8]) -> String {
    identity::hash(bytes)
}

/// The version of the file at `rel` now, `None` when it is gone or can't be read.
pub fn rev_at(root: &Path, rel: &str) -> Option<String> {
    Seen::file(&root.join(rel)).ok().map(|seen| seen.rev())
}

/// The version of what is at a path, as a delete checks it. A folder's is a hash of its
/// listing: every path in it with its size and modified time.
fn rev_of(contents: &Contents) -> String {
    match contents {
        Contents::File(bytes) => rev(bytes),
        Contents::Folder(entries) => {
            let mut listing = b"folder\n".to_vec();
            for (path, size, modified) in entries {
                let since = modified.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
                let nanos = since.map_or(0, |d| d.as_nanos());
                listing.extend(path.to_string_lossy().as_bytes());
                listing.extend(format!("\0{size}\0{nanos}\n").as_bytes());
            }
            rev(&listing)
        }
        Contents::Link(target) => {
            let mut link = b"link\n".to_vec();
            link.extend(target.to_string_lossy().as_bytes());
            rev(&link)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cold-lava-read-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_file_says_its_bytes_its_text_and_its_rev() {
        let d = dir("file");
        let p = d.join("Lava.md");
        std::fs::write(&p, "hot").unwrap();
        let seen = Seen::file(&p).unwrap();
        assert_eq!(seen.bytes(), Some(&b"hot"[..]));
        assert_eq!(seen.text(), Ok("hot"));
        assert_eq!(seen.rev(), rev(b"hot"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Clients compare revs, so each is pinned as the engine gave it before. A file's is the one
    /// `files.rs`'s wire tests pin; the folder's and the link's are FNV-1a of their listing,
    /// worked out outside the engine.
    #[test]
    fn revs_given_before_are_given_again() {
        let d = dir("revs");
        std::fs::write(d.join("a.md"), "a").unwrap();
        assert_eq!(rev(b"a"), "af63dc4c8601ec8c");
        assert_eq!(rev_at(&d, "a.md").as_deref(), Some("af63dc4c8601ec8c"));
        assert_eq!(rev_at(&d, "gone.md"), None);
        assert_eq!(
            Seen::file(&d.join("a.md")).unwrap().rev(),
            "af63dc4c8601ec8c"
        );
        let f = d.join("Volcanoes");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("Etna.md"), "hot").unwrap();
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let etna = std::fs::File::options().write(true).open(f.join("Etna.md"));
        etna.unwrap().set_modified(when).unwrap();
        assert_eq!(Seen::entry(&f).unwrap().rev(), "3923f92ec6909451");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("Lava.md", d.join("Link.md")).unwrap();
            let link = Seen::entry(&d.join("Link.md")).unwrap();
            assert_eq!(link.rev(), "af545e6e22634d3a");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bytes_that_arent_utf8_are_not_text() {
        let d = dir("not-text");
        let p = d.join("Lava.md");
        std::fs::write(&p, [b'h', 0xff]).unwrap();
        let seen = Seen::file(&p).unwrap();
        assert_eq!(seen.bytes(), Some(&[b'h', 0xff][..]));
        assert_eq!(seen.text(), Err(NotText));
        assert_eq!(NotText.about("Lava.md"), "Lava.md isn't UTF-8 text");
        assert_eq!(NotText.about("it"), "it isn't UTF-8 text");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_change_made_since_it_was_read_is_never_written_over() {
        let d = dir("replace");
        let p = d.join("Lava.md");
        std::fs::write(&p, "v1").unwrap();
        let seen = Seen::file(&p).unwrap();
        assert!(seen.unchanged().is_ok());
        std::fs::write(&p, "v2").unwrap();
        assert!(matches!(seen.unchanged(), Err(WriteError::Changed(at)) if at == p));
        assert!(matches!(seen.replace(b"v3"), Err(WriteError::Changed(at)) if at == p));
        assert_eq!(
            std::fs::read(&p).unwrap(),
            b"v2",
            "another program's change is kept"
        );
        let again = Seen::file(&p).unwrap();
        again.replace(b"v3").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"v3");
        std::fs::remove_file(&p).unwrap();
        let gone = again.unchanged();
        assert!(
            matches!(gone, Err(WriteError::Io(at, e)) if at == p && e.kind() == io::ErrorKind::NotFound)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nothing_there_is_an_io_error() {
        let d = dir("missing");
        let p = d.join("Nothing.md");
        assert_eq!(Seen::file(&p).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(Seen::entry(&p).unwrap_err().kind(), io::ErrorKind::NotFound);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_is_read_as_its_listing() {
        let d = dir("folder");
        let f = d.join("Volcanoes");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("Etna.md"), "hot").unwrap();
        let seen = Seen::entry(&f).unwrap();
        assert_eq!(seen.bytes(), None);
        assert_eq!(seen.text(), Err(NotText));
        assert_eq!(
            seen.rev(),
            Seen::entry(&f).unwrap().rev(),
            "the same listing"
        );
        assert!(seen.unchanged().is_ok());
        assert!(matches!(seen.replace(b"x"), Err(WriteError::Changed(at)) if at == f));
        assert!(f.is_dir(), "a folder is never written over");
        std::fs::write(f.join("Hekla.md"), "hot").unwrap();
        assert_ne!(seen.rev(), Seen::entry(&f).unwrap().rev());
        assert!(matches!(seen.unchanged(), Err(WriteError::Changed(at)) if at == f));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_a_file_seen_as_a_file_is_written_over() {
        let d = dir("entry-file");
        let p = d.join("Lava.md");
        std::fs::write(&p, "hot").unwrap();
        let entry = Seen::entry(&p).unwrap();
        assert_eq!(entry.bytes(), Some(&b"hot"[..]));
        assert!(entry.unchanged().is_ok());
        assert!(matches!(entry.replace(b"cold"), Err(WriteError::Changed(at)) if at == p));
        assert_eq!(
            std::fs::read(&p).unwrap(),
            b"hot",
            "a write reads through a link, which an entry never follows"
        );
        Seen::file(&p).unwrap().replace(b"cold").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"cold");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bytes_read_earlier_or_just_written_are_checked_as_a_file_seen_now() {
        let d = dir("kept");
        let p = d.join("Lava.md");
        std::fs::write(&p, "v1").unwrap();
        let earlier = Seen::kept(&p, "v1");
        assert_eq!(earlier, Seen::file(&p).unwrap());
        earlier.replace(b"v2").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"v2");
        assert!(matches!(earlier.unchanged(), Err(WriteError::Changed(at)) if at == p));
        let written = Seen::kept(&p, b"v2".to_vec());
        assert!(
            written.unchanged().is_ok(),
            "what was just written is there"
        );
        std::fs::write(&p, "v3").unwrap();
        assert!(matches!(written.replace(b"v4"), Err(WriteError::Changed(at)) if at == p));
        assert_eq!(
            std::fs::read(&p).unwrap(),
            b"v3",
            "another program's change is kept"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_delete_checks_the_entry_itself_whatever_way_it_was_seen() {
        let d = dir("entry-check");
        let (target, link) = (d.join("Lava.md"), d.join("Link.md"));
        std::fs::write(&target, "hot").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let kept = Seen::kept(&target, "hot");
        assert!(
            kept.unchanged_entry().is_ok(),
            "a file seen as one is that entry"
        );
        let through = Seen::file(&link).unwrap();
        assert!(through.unchanged().is_ok());
        assert!(
            matches!(through.unchanged_entry(), Err(WriteError::Changed(at)) if at == link),
            "a file seen through a link is not the link"
        );
        let entry = Seen::entry(&link).unwrap();
        std::fs::write(&target, "cold").unwrap();
        assert!(matches!(kept.unchanged_entry(), Err(WriteError::Changed(at)) if at == target));
        assert!(
            entry.unchanged_entry().is_ok(),
            "the link still points where it did"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_seen_through_a_link_is_written_through_it() {
        let d = dir("write-through");
        let (target, link) = (d.join("Lava.md"), d.join("Link.md"));
        std::fs::write(&target, "hot").unwrap();
        std::os::unix::fs::symlink("Lava.md", &link).unwrap();
        let seen = Seen::file(&link).unwrap();
        seen.replace(b"cooling").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"cooling");
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("Lava.md"));
        assert!(
            matches!(seen.replace(b"lost"), Err(WriteError::Changed(at)) if at == link),
            "a stale read is still refused"
        );
        Seen::kept(&link, "cooling").replace(b"cold").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"cold");
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("Lava.md"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_is_read_through_a_link_and_an_entry_is_the_link() {
        let d = dir("link");
        let (target, link) = (d.join("Lava.md"), d.join("Link.md"));
        std::fs::write(&target, "hot").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let file = Seen::file(&link).unwrap();
        assert_eq!(file.text(), Ok("hot"));
        assert_eq!(file.rev(), rev(b"hot"));
        let entry = Seen::entry(&link).unwrap();
        assert_eq!(entry.bytes(), None);
        assert_ne!(entry.rev(), rev(b"hot"));
        assert!(entry.unchanged().is_ok());
        std::fs::remove_file(&link).unwrap();
        std::fs::write(&link, "hot").unwrap();
        assert!(file.unchanged().is_ok(), "the same bytes");
        assert!(
            matches!(entry.unchanged(), Err(WriteError::Changed(at)) if at == link),
            "a file where the link was is not the link"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
