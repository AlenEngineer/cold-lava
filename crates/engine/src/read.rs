//! A file, folder or link as it was read, so what is done to it later is refused if it changed
//! since: `Read` says whether it is `unchanged`, and `replace` writes over a file only then.
//!
//! A file is read through a symlink, as every write reads it (`Read::file`); whatever is at a
//! path, a folder or the link itself, is read without following one, as a delete checks it
//! (`Read::entry`). Either is read again the same way it was read first.

use crate::files::rev;
use crate::trash::Contents;
use crate::write::{self, WriteError};
use std::io;
use std::path::{Path, PathBuf};

/// What was at `path` when it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
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

impl Read {
    /// The file at `path`, through a symlink.
    pub fn file(path: &Path) -> io::Result<Read> {
        Read::by(Way::File, path)
    }

    /// Whatever is at `path`: a file, a folder by its listing, or a symlink by where it points.
    pub fn entry(path: &Path) -> io::Result<Read> {
        Read::by(Way::Entry, path)
    }

    fn by(way: Way, path: &Path) -> io::Result<Read> {
        Ok(Read {
            path: path.to_path_buf(),
            was: way.read(path)?,
            way,
        })
    }

    /// The bytes read, when it was a file.
    pub fn bytes(&self) -> Option<&[u8]> {
        match &self.was {
            Contents::File(bytes) => Some(bytes.as_slice()),
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

    /// Refused with `WriteError::Changed` unless the path still holds what was read.
    pub fn unchanged(&self) -> Result<(), WriteError> {
        let now = self.way.read(&self.path);
        if now.map_err(|e| WriteError::Io(self.path.clone(), e))? != self.was {
            return Err(WriteError::Changed(self.path.clone()));
        }
        Ok(())
    }

    /// Write `bytes` over the file read, through `write::replace`: refused with
    /// `WriteError::Changed` if it changed since, or if what was read wasn't a file.
    pub fn replace(&self, bytes: &[u8]) -> Result<(), WriteError> {
        match self.bytes() {
            Some(was) => write::replace(&self.path, was, bytes),
            None => Err(WriteError::Changed(self.path.clone())),
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

/// The version of what is at a path, as a delete checks it. A folder's is a hash of its
/// listing: every path in it with its size and modified time.
pub(crate) fn rev_of(contents: &Contents) -> String {
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
        let read = Read::file(&p).unwrap();
        assert_eq!(read.bytes(), Some(&b"hot"[..]));
        assert_eq!(read.text(), Ok("hot"));
        assert_eq!(read.rev(), rev(b"hot"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bytes_that_arent_utf8_are_not_text() {
        let d = dir("not-text");
        let p = d.join("Lava.md");
        std::fs::write(&p, [b'h', 0xff]).unwrap();
        let read = Read::file(&p).unwrap();
        assert_eq!(read.bytes(), Some(&[b'h', 0xff][..]));
        assert_eq!(read.text(), Err(NotText));
        assert_eq!(NotText.about("Lava.md"), "Lava.md isn't UTF-8 text");
        assert_eq!(NotText.about("it"), "it isn't UTF-8 text");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_change_made_since_it_was_read_is_never_written_over() {
        let d = dir("replace");
        let p = d.join("Lava.md");
        std::fs::write(&p, "v1").unwrap();
        let read = Read::file(&p).unwrap();
        assert!(read.unchanged().is_ok());
        std::fs::write(&p, "v2").unwrap();
        assert!(matches!(read.unchanged(), Err(WriteError::Changed(at)) if at == p));
        assert!(matches!(read.replace(b"v3"), Err(WriteError::Changed(at)) if at == p));
        assert_eq!(
            std::fs::read(&p).unwrap(),
            b"v2",
            "another program's change is kept"
        );
        let again = Read::file(&p).unwrap();
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
        assert_eq!(Read::file(&p).unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(Read::entry(&p).unwrap_err().kind(), io::ErrorKind::NotFound);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_is_read_as_its_listing() {
        let d = dir("folder");
        let f = d.join("Volcanoes");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("Etna.md"), "hot").unwrap();
        let read = Read::entry(&f).unwrap();
        assert_eq!(read.bytes(), None);
        assert_eq!(read.text(), Err(NotText));
        assert_eq!(
            read.rev(),
            Read::entry(&f).unwrap().rev(),
            "the same listing"
        );
        assert!(read.unchanged().is_ok());
        assert!(matches!(read.replace(b"x"), Err(WriteError::Changed(at)) if at == f));
        assert!(f.is_dir(), "a folder is never written over");
        std::fs::write(f.join("Hekla.md"), "hot").unwrap();
        assert_ne!(read.rev(), Read::entry(&f).unwrap().rev());
        assert!(matches!(read.unchanged(), Err(WriteError::Changed(at)) if at == f));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_is_read_through_a_link_and_an_entry_is_the_link() {
        let d = dir("link");
        let (target, link) = (d.join("Lava.md"), d.join("Link.md"));
        std::fs::write(&target, "hot").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let file = Read::file(&link).unwrap();
        assert_eq!(file.text(), Ok("hot"));
        assert_eq!(file.rev(), rev(b"hot"));
        let entry = Read::entry(&link).unwrap();
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
