//! The one finder of texts outside the files they were taken from (#427): kept under a kept
//! name (`kept::kept_for`), or left under a staged name by a killed save that no sweep has
//! cleared (`stale::left`). The core lists what it finds, so the user sees them. A caller of
//! those two.

use std::path::{Path, PathBuf};

use super::kept::kept_for;
use super::stale::{self, Left};

/// A text outside the file it was taken from, found in a folder: `file` is the file's path,
/// `kept` where the text is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outside {
    pub file: PathBuf,
    pub kept: PathBuf,
}

/// Every text in `dir` that is outside its file, sorted by where it is: the ones kept under a
/// kept name (`kept_for`), and the staged files dead cold-lavas left that no sweep has cleared
/// (`stale::left`, older than `stale::AGE`), as on a file system without the folder's lock,
/// where a save killed after moving a note aside leaves the note's only text under its
/// staged name (#427). Only regular files are found, and no empty leftover: it holds no
/// text, and the sweep removes it.
pub fn find(dir: &Path) -> Vec<Outside> {
    find_left(dir, stale::AGE)
}

/// `find`, with the staged files left older than `age`.
fn find_left(dir: &Path, age: std::time::Duration) -> Vec<Outside> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let file = match name.to_str().and_then(kept_for) {
            Some(file) if entry.file_type().is_ok_and(|t| t.is_file()) => file,
            Some(_) => continue,
            None => match stale::left(&entry, age) {
                Left::Old { file, .. } if !is_empty(&entry) => file,
                Left::Old { .. } => continue,
                Left::Not | Left::Young => continue,
            },
        };
        found.push(Outside {
            file: dir.join(file),
            kept: entry.path(),
        });
    }
    found.sort_by(|a, b| a.kept.cmp(&b.kept));
    found
}

fn is_empty(entry: &std::fs::DirEntry) -> bool {
    entry.metadata().is_ok_and(|m| m.len() == 0)
}

#[cfg(test)]
mod tests {
    use super::{Outside, find, find_left};

    /// The finder lists kept texts and the staged files dead cold-lavas left that are old
    /// enough to clear, with the file each is for; never a live save's staged file, a young
    /// one, an empty one, another hidden file or a folder under a kept name.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn kept_texts_and_what_killed_saves_left_are_found() {
        use std::time::{Duration, SystemTime};
        let d = std::env::temp_dir().join(format!("cold-lava-kept-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let dead = i32::MAX as u32;
        let live = std::process::id();
        let leave = |name: &str, ago: Duration| {
            std::fs::write(d.join(name), "a text").unwrap();
            let f = std::fs::File::options()
                .write(true)
                .open(d.join(name))
                .unwrap();
            f.set_modified(SystemTime::now() - ago).unwrap();
        };
        let long = Duration::from_secs(600);
        std::fs::write(d.join("N.md"), "now").unwrap();
        leave(".N.cold-lava-kept-5-0.md", long);
        leave(&format!(".M.md.cold-lava-{dead}-0.tmp"), long);
        leave(&format!(".N.md.cold-lava-{live}-1.tmp"), long);
        leave(".hidden.md", long);
        std::fs::write(d.join(format!(".E.md.cold-lava-{dead}-3.tmp")), "").unwrap();
        std::fs::create_dir(d.join(".F.cold-lava-kept-5-1.md")).unwrap();
        assert_eq!(
            find(&d).len(),
            1,
            "what was left was renamed just now (ctime)"
        );
        let found = find_left(&d, Duration::ZERO);
        let staged = d.join(format!(".M.md.cold-lava-{dead}-0.tmp"));
        assert_eq!(
            found,
            [
                Outside {
                    file: d.join("M.md"),
                    kept: staged,
                },
                Outside {
                    file: d.join("N.md"),
                    kept: d.join(".N.cold-lava-kept-5-0.md"),
                },
            ]
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
