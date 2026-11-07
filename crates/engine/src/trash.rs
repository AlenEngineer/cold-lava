//! Deleting a file or folder from a vault, the one way cold-lava does it: into a trash, never for good.
//!
//! Where it goes follows the vault's Obsidian setting `trashOption` in `.obsidian/app.json`:
//! `"system"` (the default) is the system trash, `"local"` the vault's `.trash` folder. Obsidian's
//! `"none"` deletes permanently; cold-lava still uses `.trash`. When the system trash can't take
//! the file (none on this platform, or it is on another disk), it goes to `.trash` too.

use crate::write::{WriteError, move_file};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The vault's `trashOption`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrashOption {
    System,
    Local,
    /// Obsidian deletes for good; cold-lava uses `.trash` instead.
    Permanent,
}

impl TrashOption {
    pub fn of_vault(root: &Path) -> TrashOption {
        let app: serde_json::Value = std::fs::read_to_string(root.join(".obsidian/app.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        match app.get("trashOption").and_then(|v| v.as_str()) {
            Some("local") => TrashOption::Local,
            Some("none") => TrashOption::Permanent,
            _ => TrashOption::System,
        }
    }
}

/// A trash folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bin {
    /// A freedesktop.org trash (Linux, BSD): the file goes in `files/`, where it came from in
    /// `info/`, so the desktop's trash can put it back.
    Freedesktop(PathBuf),
    /// A plain folder the file is moved into (macOS's `~/.Trash`, the vault's `.trash`).
    Folder(PathBuf),
}

/// The user's system trash, if this platform has one cold-lava knows.
pub fn system_bin() -> Option<Bin> {
    let home = crate::paths::home_dir();
    if cfg!(target_os = "macos") {
        return home.map(|h| Bin::Folder(h.join(".Trash")));
    }
    if cfg!(all(unix, not(target_os = "macos"))) {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".local/share")))?;
        return Some(Bin::Freedesktop(data.join("Trash")));
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Went {
    System,
    VaultTrash,
}

/// A file cold-lava put in a trash: enough to put it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trashed {
    /// Where it was.
    pub from: PathBuf,
    /// Where it is now.
    pub at: PathBuf,
    pub went: Went,
    /// The freedesktop `.trashinfo` written for it.
    info: Option<PathBuf>,
}

/// What the user was shown of a file or folder, so a delete is refused if it changed since.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Contents {
    File(Vec<u8>),
    /// Everything inside the folder (relative paths, sorted), with each file's size and time.
    Folder(Vec<(PathBuf, u64, Option<SystemTime>)>),
}

impl Contents {
    pub fn read(path: &Path) -> Result<Contents, WriteError> {
        let err = |e: io::Error| WriteError::Io(path.to_path_buf(), e);
        if !path.is_dir() {
            return std::fs::read(path).map(Contents::File).map_err(err);
        }
        let mut entries = Vec::new();
        for e in walkdir::WalkDir::new(path).min_depth(1).sort_by_file_name() {
            let e = e.map_err(|e| err(e.into()))?;
            let meta = e.metadata().map_err(|e| err(e.into()))?;
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

/// Move `path` (a file or a folder) to the trash `trashOption` names, if it still has the
/// `expected` contents (what the user was shown). `system` is the system trash (`system_bin()`;
/// tests pass their own).
pub fn delete(
    root: &Path,
    path: &Path,
    expected: &Contents,
    system: Option<&Bin>,
) -> Result<Trashed, WriteError> {
    if Contents::read(path)? != *expected {
        return Err(WriteError::Changed(path.to_path_buf()));
    }
    let local = Bin::Folder(root.join(".trash"));
    if let (TrashOption::System, Some(bin)) = (TrashOption::of_vault(root), system)
        && let Ok(t) = put(path, bin, Went::System)
    {
        return Ok(t);
    }
    put(path, &local, Went::VaultTrash)
}

fn put(path: &Path, bin: &Bin, went: Went) -> Result<Trashed, WriteError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mk = |d: &Path| std::fs::create_dir_all(d).map_err(|e| WriteError::Io(d.to_path_buf(), e));
    match bin {
        Bin::Folder(dir) => {
            mk(dir)?;
            let at = free_path(&dir.join(&name));
            move_file(path, &at)?;
            Ok(Trashed {
                from: path.to_path_buf(),
                at,
                went,
                info: None,
            })
        }
        Bin::Freedesktop(dir) => {
            let (files, infos) = (dir.join("files"), dir.join("info"));
            mk(&files)?;
            mk(&infos)?;
            let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            // The spec reserves a name by creating its .trashinfo exclusively, then moves.
            for n in 0.. {
                let candidate = suffixed(&name, n);
                let info = infos.join(format!("{candidate}.trashinfo"));
                let at = files.join(&candidate);
                if at.exists() {
                    continue;
                }
                let mut f = match std::fs::File::create_new(&info) {
                    Ok(f) => f,
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(WriteError::Io(info, e)),
                };
                let text = format!(
                    "[Trash Info]\nPath={}\nDeletionDate={}\n",
                    percent_encode(&abs),
                    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S")
                );
                let moved = f
                    .write_all(text.as_bytes())
                    .map_err(|e| WriteError::Io(info.clone(), e))
                    .and_then(|()| move_file(path, &at));
                if let Err(e) = moved {
                    let _ = std::fs::remove_file(&info);
                    return Err(e);
                }
                return Ok(Trashed {
                    from: path.to_path_buf(),
                    at,
                    went,
                    info: Some(info),
                });
            }
            unreachable!()
        }
    }
}

/// Put a trashed file back at `to` (its old path, or another the user chose); refused if
/// something is there now.
pub fn restore(t: &Trashed, to: &Path) -> Result<(), WriteError> {
    move_file(&t.at, to)?;
    if let Some(info) = &t.info {
        let _ = std::fs::remove_file(info);
    }
    Ok(())
}

/// `path`, or the first of `name 1.ext`, `name 2.ext`… that doesn't exist.
pub fn free_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (0..)
        .map(|n| path.with_file_name(suffixed(&name, n)))
        .find(|p| !p.exists())
        .unwrap()
}

/// `Lava.md`, `Lava 1.md`, `Lava 2.md`…: Obsidian's way of making a name free.
fn suffixed(name: &str, n: usize) -> String {
    if n == 0 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} {n}.{ext}"),
        _ => format!("{name} {n}"),
    }
}

fn percent_encode(path: &Path) -> String {
    let mut out = String::new();
    for &b in path.to_string_lossy().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(name: &str, trash_option: Option<&str>) -> (PathBuf, PathBuf) {
        let d = std::env::temp_dir().join(format!("cold-lava-trash-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let vault = d.join("vault");
        std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
        std::fs::create_dir_all(vault.join("Concepts")).unwrap();
        if let Some(o) = trash_option {
            std::fs::write(
                vault.join(".obsidian/app.json"),
                format!(r#"{{"trashOption": "{o}"}}"#),
            )
            .unwrap();
        }
        std::fs::write(vault.join("Concepts/Lava.md"), "hot").unwrap();
        (d, vault)
    }

    #[test]
    fn system_trash_by_default_with_trashinfo() {
        let (d, vault) = sandbox("system", None);
        let bin = Bin::Freedesktop(d.join("data/Trash"));
        let lava = vault.join("Concepts/Lava.md");
        let t = delete(&vault, &lava, &Contents::File(b"hot".to_vec()), Some(&bin)).unwrap();
        assert_eq!(t.went, Went::System);
        assert!(!lava.exists());
        assert_eq!(t.at, d.join("data/Trash/files/Lava.md"));
        assert_eq!(std::fs::read(&t.at).unwrap(), b"hot");
        let info = std::fs::read_to_string(d.join("data/Trash/info/Lava.md.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=/"), "{info}");
        assert!(
            info.contains(&format!("{}\n", percent_encode(&lava))),
            "{info}"
        );
        assert!(info.contains("DeletionDate="), "{info}");

        // A second Lava.md doesn't replace the first.
        std::fs::write(&lava, "again").unwrap();
        let t2 = delete(
            &vault,
            &lava,
            &Contents::File(b"again".to_vec()),
            Some(&bin),
        )
        .unwrap();
        assert_eq!(t2.at, d.join("data/Trash/files/Lava 1.md"));
        assert!(d.join("data/Trash/info/Lava 1.md.trashinfo").exists());
        assert_eq!(std::fs::read(&t.at).unwrap(), b"hot");

        restore(&t2, &lava).unwrap();
        assert_eq!(std::fs::read(&lava).unwrap(), b"again");
        assert!(!d.join("data/Trash/info/Lava 1.md.trashinfo").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn local_and_none_use_the_vaults_trash_folder() {
        for option in ["local", "none"] {
            let (d, vault) = sandbox(option, Some(option));
            let bin = Bin::Freedesktop(d.join("data/Trash"));
            let lava = vault.join("Concepts/Lava.md");
            std::fs::create_dir_all(vault.join(".trash")).unwrap();
            std::fs::write(vault.join(".trash/Lava.md"), "older").unwrap();
            let t = delete(&vault, &lava, &Contents::File(b"hot".to_vec()), Some(&bin)).unwrap();
            assert_eq!(t.went, Went::VaultTrash);
            assert_eq!(t.at, vault.join(".trash/Lava 1.md"), "{option}");
            assert_eq!(std::fs::read(&t.at).unwrap(), b"hot");
            assert_eq!(
                std::fs::read(vault.join(".trash/Lava.md")).unwrap(),
                b"older"
            );
            assert!(!d.join("data").exists(), "{option}: system trash untouched");
            restore(&t, &lava).unwrap();
            assert_eq!(std::fs::read(&lava).unwrap(), b"hot");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn no_system_trash_falls_back_to_the_vaults() {
        let (d, vault) = sandbox("fallback", Some("system"));
        let t = delete(
            &vault,
            &vault.join("Concepts/Lava.md"),
            &Contents::File(b"hot".to_vec()),
            None,
        )
        .unwrap();
        assert_eq!(
            (t.went, t.at),
            (Went::VaultTrash, vault.join(".trash/Lava.md"))
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_a_file_that_changed_since_it_was_read() {
        let (d, vault) = sandbox("changed", None);
        let bin = Bin::Freedesktop(d.join("data/Trash"));
        let lava = vault.join("Concepts/Lava.md");
        assert!(matches!(
            delete(
                &vault,
                &lava,
                &Contents::File(b"what the user saw".to_vec()),
                Some(&bin)
            ),
            Err(WriteError::Changed(_))
        ));
        assert_eq!(std::fs::read(&lava).unwrap(), b"hot");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn restore_refuses_to_overwrite_and_a_free_name_works() {
        let (d, vault) = sandbox("restore", Some("local"));
        let lava = vault.join("Concepts/Lava.md");
        let t = delete(&vault, &lava, &Contents::File(b"hot".to_vec()), None).unwrap();
        std::fs::write(&lava, "a new Lava").unwrap();
        assert!(matches!(restore(&t, &lava), Err(WriteError::Exists(_))));
        let free = free_path(&lava);
        assert_eq!(free, vault.join("Concepts/Lava 1.md"));
        restore(&t, &free).unwrap();
        assert_eq!(std::fs::read(&free).unwrap(), b"hot");
        assert_eq!(std::fs::read(&lava).unwrap(), b"a new Lava");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_goes_to_the_trash_whole_and_comes_back() {
        let (d, vault) = sandbox("folder", None);
        let bin = Bin::Freedesktop(d.join("data/Trash"));
        let concepts = vault.join("Concepts");
        std::fs::create_dir_all(concepts.join("Empty")).unwrap();
        let seen = Contents::read(&concepts).unwrap();
        std::fs::write(concepts.join("New.md"), "added since").unwrap();
        assert!(matches!(
            delete(&vault, &concepts, &seen, Some(&bin)),
            Err(WriteError::Changed(_))
        ));
        std::fs::remove_file(concepts.join("New.md")).unwrap();
        let t = delete(&vault, &concepts, &seen, Some(&bin)).unwrap();
        assert!(!concepts.exists());
        assert_eq!(t.at, d.join("data/Trash/files/Concepts"));
        assert!(t.at.join("Empty").is_dir() && t.at.join("Lava.md").is_file());
        restore(&t, &concepts).unwrap();
        assert_eq!(std::fs::read(concepts.join("Lava.md")).unwrap(), b"hot");
        assert!(!d.join("data/Trash/info/Concepts.trashinfo").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_and_paths() {
        assert_eq!(suffixed("Lava.md", 2), "Lava 2.md");
        assert_eq!(suffixed("Board.canvas", 1), "Board 1.canvas");
        assert_eq!(suffixed(".hidden", 1), ".hidden 1");
        assert_eq!(
            percent_encode(Path::new("/v/Kīlauea 1.md")),
            "/v/K%C4%ABlauea%201.md"
        );
    }
}
