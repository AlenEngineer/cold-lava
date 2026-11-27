//! Deleting a file or folder from a workspace, the one way cold-lava does it: into a trash, never for good.
//!
//! Where it goes follows the workspace's Obsidian setting `trashOption` in `.obsidian/app.json`:
//! `"system"` (the default) is the system trash, `"local"` the workspace's `.trash` folder. Obsidian's
//! `"none"` deletes permanently; cold-lava still uses `.trash`. When the system trash can't take
//! the file (none on this platform, or it is on another disk), it goes to `.trash` too.

use crate::read::Seen;
use crate::write::{WriteError, move_file, taken};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// The workspace's `trashOption`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrashOption {
    System,
    Local,
    /// Obsidian deletes for good; cold-lava uses `.trash` instead.
    Permanent,
}

impl TrashOption {
    pub fn of_workspace(root: &Path) -> TrashOption {
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
    /// A plain folder the file is moved into (macOS's `~/.Trash`, the workspace's `.trash`).
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

/// Which trash took it. On the wire (`workspace/delete`): `system` or `workspace`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Went {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "workspace")]
    WorkspaceTrash,
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

/// Move what was `seen` (a file or a folder) to the trash `trashOption` names, if the entry at
/// its path still holds it (what the user was shown). `system` is the system trash
/// (`system_bin()`; tests pass their own).
pub fn delete(root: &Path, seen: &Seen, system: Option<&Bin>) -> Result<Trashed, WriteError> {
    seen.unchanged_entry()?;
    let path = seen.path();
    let local = Bin::Folder(root.join(".trash"));
    if let (TrashOption::System, Some(bin)) = (TrashOption::of_workspace(root), system)
        && let Ok(t) = put(path, bin, Went::System)
    {
        return Ok(t);
    }
    put(path, &local, Went::WorkspaceTrash)
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
                if taken(&at) {
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

/// `path`, or the first of `name 1.ext`, `name 2.ext`… that nothing is at (`write::taken`).
pub fn free_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (0..)
        .map(|n| path.with_file_name(suffixed(&name, n)))
        .find(|p| !taken(p))
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
        let workspace = d.join("workspace");
        std::fs::create_dir_all(workspace.join(".obsidian")).unwrap();
        std::fs::create_dir_all(workspace.join("Concepts")).unwrap();
        if let Some(o) = trash_option {
            std::fs::write(
                workspace.join(".obsidian/app.json"),
                format!(r#"{{"trashOption": "{o}"}}"#),
            )
            .unwrap();
        }
        std::fs::write(workspace.join("Concepts/Lava.md"), "hot").unwrap();
        (d, workspace)
    }

    #[test]
    fn system_trash_by_default_with_trashinfo() {
        let (d, workspace) = sandbox("system", None);
        let bin = Bin::Freedesktop(d.join("data/Trash"));
        let lava = workspace.join("Concepts/Lava.md");
        let t = delete(&workspace, &Seen::kept(&lava, "hot"), Some(&bin)).unwrap();
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
        let t2 = delete(&workspace, &Seen::kept(&lava, "again"), Some(&bin)).unwrap();
        assert_eq!(t2.at, d.join("data/Trash/files/Lava 1.md"));
        assert!(d.join("data/Trash/info/Lava 1.md.trashinfo").exists());
        assert_eq!(std::fs::read(&t.at).unwrap(), b"hot");

        restore(&t2, &lava).unwrap();
        assert_eq!(std::fs::read(&lava).unwrap(), b"again");
        assert!(!d.join("data/Trash/info/Lava 1.md.trashinfo").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn local_and_none_use_the_workspaces_trash_folder() {
        for option in ["local", "none"] {
            let (d, workspace) = sandbox(option, Some(option));
            let bin = Bin::Freedesktop(d.join("data/Trash"));
            let lava = workspace.join("Concepts/Lava.md");
            std::fs::create_dir_all(workspace.join(".trash")).unwrap();
            std::fs::write(workspace.join(".trash/Lava.md"), "older").unwrap();
            let t = delete(&workspace, &Seen::kept(&lava, "hot"), Some(&bin)).unwrap();
            assert_eq!(t.went, Went::WorkspaceTrash);
            assert_eq!(t.at, workspace.join(".trash/Lava 1.md"), "{option}");
            assert_eq!(std::fs::read(&t.at).unwrap(), b"hot");
            assert_eq!(
                std::fs::read(workspace.join(".trash/Lava.md")).unwrap(),
                b"older"
            );
            assert!(!d.join("data").exists(), "{option}: system trash untouched");
            restore(&t, &lava).unwrap();
            assert_eq!(std::fs::read(&lava).unwrap(), b"hot");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn no_system_trash_falls_back_to_the_workspaces() {
        let (d, workspace) = sandbox("fallback", Some("system"));
        let lava = workspace.join("Concepts/Lava.md");
        let t = delete(&workspace, &Seen::kept(&lava, "hot"), None).unwrap();
        assert_eq!(
            (t.went, t.at),
            (Went::WorkspaceTrash, workspace.join(".trash/Lava.md"))
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_a_file_that_changed_since_it_was_read() {
        let (d, workspace) = sandbox("changed", None);
        let bin = Bin::Freedesktop(d.join("data/Trash"));
        let lava = workspace.join("Concepts/Lava.md");
        assert!(matches!(
            delete(
                &workspace,
                &Seen::kept(&lava, "what the user saw"),
                Some(&bin)
            ),
            Err(WriteError::Changed(_))
        ));
        assert_eq!(std::fs::read(&lava).unwrap(), b"hot");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn restore_refuses_to_overwrite_and_a_free_name_works() {
        let (d, workspace) = sandbox("restore", Some("local"));
        let lava = workspace.join("Concepts/Lava.md");
        let t = delete(&workspace, &Seen::kept(&lava, "hot"), None).unwrap();
        std::fs::write(&lava, "a new Lava").unwrap();
        assert!(matches!(restore(&t, &lava), Err(WriteError::Exists(_))));
        let free = free_path(&lava);
        assert_eq!(free, workspace.join("Concepts/Lava 1.md"));
        restore(&t, &free).unwrap();
        assert_eq!(std::fs::read(&free).unwrap(), b"hot");
        assert_eq!(std::fs::read(&lava).unwrap(), b"a new Lava");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A name a symlink to nothing holds is taken, in either trash and where a file comes
    /// back: the delete takes the next name, the restore is refused, and the link stays
    /// (#356).
    #[cfg(unix)]
    #[test]
    fn a_symlink_to_nothing_holds_its_name() {
        let dangling = |at: &Path| {
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink("Gone.md", at).unwrap();
        };
        let stays = |at: &Path| {
            let led = std::fs::read_link(at).unwrap();
            assert_eq!(
                led,
                Path::new("Gone.md"),
                "{} is still a link",
                at.display()
            );
        };

        let (d, workspace) = sandbox("dangling-local", Some("local"));
        let lava = workspace.join("Concepts/Lava.md");
        let held = workspace.join(".trash/Lava.md");
        dangling(&held);
        let t = delete(&workspace, &Seen::kept(&lava, "hot"), None).unwrap();
        assert_eq!(t.at, workspace.join(".trash/Lava 1.md"));
        assert_eq!(std::fs::read(&t.at).unwrap(), b"hot");
        stays(&held);
        dangling(&lava);
        assert!(matches!(restore(&t, &lava), Err(WriteError::Exists(_))));
        stays(&lava);
        assert_eq!(std::fs::read(&t.at).unwrap(), b"hot", "still in the trash");
        let _ = std::fs::remove_dir_all(&d);

        let (d, workspace) = sandbox("dangling-system", None);
        let bin = Bin::Freedesktop(d.join("data/Trash"));
        let lava = workspace.join("Concepts/Lava.md");
        let held = d.join("data/Trash/files/Lava.md");
        dangling(&held);
        let t = delete(&workspace, &Seen::kept(&lava, "hot"), Some(&bin)).unwrap();
        assert_eq!(t.went, Went::System);
        assert_eq!(t.at, d.join("data/Trash/files/Lava 1.md"));
        assert!(d.join("data/Trash/info/Lava 1.md.trashinfo").exists());
        assert!(!d.join("data/Trash/info/Lava.md.trashinfo").exists());
        stays(&held);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A `.trash` that is a symlink is never followed: a delete into it is refused and the
    /// note stays, and a file trashed before `.trash` became a link isn't fetched back
    /// through it (#435).
    #[cfg(unix)]
    #[test]
    fn a_workspace_trash_that_is_a_link_is_refused() {
        for option in ["local", "none", "system"] {
            let (d, workspace) = sandbox(&format!("linked-{option}"), Some(option));
            let outside = d.join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, workspace.join(".trash")).unwrap();
            let lava = workspace.join("Concepts/Lava.md");
            assert!(
                matches!(
                    delete(&workspace, &Seen::kept(&lava, "hot"), None),
                    Err(WriteError::Link(p)) if p == workspace.join(".trash")
                ),
                "{option}"
            );
            assert_eq!(std::fs::read(&lava).unwrap(), b"hot", "{option}");
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "{option}");
            let _ = std::fs::remove_dir_all(&d);
        }

        let (d, workspace) = sandbox("linked-after", Some("local"));
        let lava = workspace.join("Concepts/Lava.md");
        let t = delete(&workspace, &Seen::kept(&lava, "hot"), None).unwrap();
        let outside = d.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("Lava.md"), "not ours").unwrap();
        std::fs::rename(workspace.join(".trash"), d.join("moved-trash")).unwrap();
        std::os::unix::fs::symlink(&outside, workspace.join(".trash")).unwrap();
        assert!(matches!(
            restore(&t, &lava),
            Err(WriteError::Link(p)) if p == workspace.join(".trash")
        ));
        assert!(!lava.exists());
        assert_eq!(std::fs::read(outside.join("Lava.md")).unwrap(), b"not ours");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_goes_to_the_trash_whole_and_comes_back() {
        let (d, workspace) = sandbox("folder", None);
        let bin = Bin::Freedesktop(d.join("data/Trash"));
        let concepts = workspace.join("Concepts");
        std::fs::create_dir_all(concepts.join("Empty")).unwrap();
        let seen = Seen::entry(&concepts).unwrap();
        std::fs::write(concepts.join("New.md"), "added since").unwrap();
        assert!(matches!(
            delete(&workspace, &seen, Some(&bin)),
            Err(WriteError::Changed(_))
        ));
        std::fs::remove_file(concepts.join("New.md")).unwrap();
        let t = delete(&workspace, &seen, Some(&bin)).unwrap();
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
