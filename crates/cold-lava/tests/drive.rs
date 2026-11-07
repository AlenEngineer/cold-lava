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

fn drive(script: &str, dir: &Path) -> Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/drive")
        .join(script);
    Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .arg("drive")
        .arg(&script)
        .arg("--")
        .arg(dir.join("example-vault"))
        .output()
        .unwrap()
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
