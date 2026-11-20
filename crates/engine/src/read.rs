#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::rev;
    use std::io;

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
