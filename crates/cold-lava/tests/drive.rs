//! `cold-lava drive` against the built binary, on a copy of the example vault.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A fresh vault copy. `drive` gives the program an empty home itself.
fn sandbox(name: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("drive-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-vault"),
        &dir.join("example-vault"),
    );
    dir
}

/// Where the system trash keeps deleted files, under the program's home.
fn system_trash(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join(".Trash")
    } else {
        home.join(".local/share/Trash/files")
    }
}

fn drive(script: &str, dir: &Path) -> Output {
    drive_with(script, dir, &[])
}

fn drive_with(script: &str, dir: &Path, options: &[&std::ffi::OsStr]) -> Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/drive")
        .join(script);
    Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .arg("drive")
        .args(options)
        .arg(&script)
        .arg("--")
        .arg(dir.join("example-vault"))
        .output()
        .unwrap()
}

#[test]
fn deletes_a_note_to_the_system_trash_and_undoes_it() {
    let dir = sandbox("delete");
    let home = dir.join("home");
    let vault = dir.join("example-vault");
    let lava = std::fs::read(vault.join("Concepts/Lava.md")).unwrap();
    let out = drive_with("delete.drive", &dir, &["--home".as_ref(), home.as_os_str()]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!vault.join("Concepts/Lava.md").exists());
    assert!(!vault.join(".trash").exists(), "not the vault's own trash");
    let files = system_trash(&home);
    let mut trashed = files_ending(&files, "");
    trashed.sort();
    assert_eq!(
        trashed,
        [files.join("Lava.md"), files.join("Welcome.canvas")]
    );
    assert!(!vault.join("Welcome.canvas").exists());
    assert_eq!(std::fs::read(files.join("Lava.md")).unwrap(), lava);
    if cfg!(target_os = "macos") {
        let _ = std::fs::remove_dir_all(dir);
        return;
    }
    let trash = home.join(".local/share/Trash");
    let info = std::fs::read_to_string(trash.join("info/Lava.md.trashinfo")).unwrap();
    assert!(
        info.starts_with("[Trash Info]\nPath=/")
            && info.contains("/example-vault/Concepts/Lava.md\n"),
        "{info}"
    );
    assert_eq!(
        files_ending(&trash.join("info"), "").len(),
        2,
        "undo took its info away"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn opens_a_note_follows_a_link_and_quits() {
    let dir = sandbox("follow-link");
    let out = drive("follow-link.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn peeks_at_a_link_without_going_there() {
    let dir = sandbox("peek");
    let out = drive("peek.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Every file under `dir` whose name ends in `suffix`.
fn files_ending(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_ending(&path, suffix));
        } else if path.to_string_lossy().ends_with(suffix) {
            found.push(path);
        }
    }
    found
}

#[test]
fn follows_attachment_links_without_creating_notes_for_them() {
    let dir = sandbox("attachments");
    std::fs::write(
        dir.join("example-vault/Inbox/Field trip.md"),
        "# Field trip\n\nBring [[trail map.pdf]] and the [ticket](tickets/ticket.pdf). \
         Plan it on [[Welcome.canvas]]; read up on [[Mount St. Helens]].\n",
    )
    .unwrap();
    let out = drive("attachments.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut created = files_ending(&dir, ".pdf.md");
    created.extend(files_ending(&dir, ".canvas.md"));
    assert!(created.is_empty(), "created {created:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn which_key_lists_and_runs_each_screens_commands() {
    let dir = sandbox("which-key");
    let out = drive("which-key.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn goes_back_and_forward_through_notes_and_a_search() {
    let dir = sandbox("history");
    let out = drive("history.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn goes_back_and_forward_between_a_note_the_graph_and_a_board() {
    let dir = sandbox("history-screens");
    let out = drive("history-screens.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn follows_links_and_backlinks_by_their_hints() {
    let dir = sandbox("hints");
    let out = drive("hints.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn jumps_through_the_outline_and_heading_by_heading() {
    let dir = sandbox("outline");
    let out = drive("outline.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_false_expect_fails_with_its_line_and_the_screen() {
    let dir = sandbox("fails");
    let out = drive("fails.drive", &dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("fails.drive:3: expected \"Atlantis\" on the screen"),
        "{stdout}"
    );
    assert!(
        stdout.contains("example-vault 33 notes"),
        "the screen: {stdout}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Needs Neovim, which CI doesn't have; runs wherever `nvim` is on PATH.
#[test]
fn restarts_the_embedded_neovim() {
    let has_nvim = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("nvim").is_file()));
    if !has_nvim {
        eprintln!("skipped: no nvim on PATH");
        return;
    }
    let dir = sandbox("nvim-restart");
    let out = drive("nvim-restart.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Needs Neovim, which CI doesn't have; runs wherever `nvim` is on PATH.
#[test]
fn deleting_the_note_open_in_neovim_doesnt_bring_it_back() {
    let has_nvim = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("nvim").is_file()));
    if !has_nvim {
        eprintln!("skipped: no nvim on PATH");
        return;
    }
    let dir = sandbox("delete-nvim");
    let home = dir.join("home");
    let out = drive_with(
        "delete-nvim.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.join("example-vault/Concepts/Lava.md").exists());
    assert!(system_trash(&home).join("Lava.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}
