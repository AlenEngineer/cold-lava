//! `cold-lava items`, `agenda` and `task toggle` against the built binary, on a copy of the
//! example vault. The agenda goldens in tests/golden were recorded before the agenda read
//! items, so they pin that its output didn't change.

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn sandbox(name: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("items-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let vault = dir.join("example-vault");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-vault"),
        &vault,
    );
    vault.canonicalize().unwrap()
}

/// Runs cold-lava; its stdout with the vault's path written as `VAULT`.
fn run(vault: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .replace(vault.to_str().unwrap(), "VAULT")
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(name),
    )
    .unwrap()
}

#[test]
fn agenda_output_is_unchanged() {
    let vault = sandbox("agenda");
    let v = vault.to_str().unwrap();
    let range = ["--from", "2026-11-01", "--to", "2026-11-30"];
    let args = |json: bool| {
        let mut a = vec!["agenda", v];
        a.extend(range);
        if json {
            a.push("--json");
        }
        a
    };
    assert_eq!(run(&vault, &args(true)), golden("agenda.json"));
    assert_eq!(run(&vault, &args(false)), golden("agenda.txt"));
    let _ = std::fs::remove_dir_all(vault.parent().unwrap());
}

#[test]
fn items_lists_each_kind() {
    let vault = sandbox("kinds");
    let v = vault.to_str().unwrap();
    assert_eq!(
        run(&vault, &["items", v, "task"]),
        "\
Journal/2026-11-05.md:7  [ ] Write a note on lahars ([[Lahar]] is still unresolved)
Journal/2026-11-05.md:8  [x] Add the [[Volcanic Explosivity Index]] table
Journal/2026-11-05.md:9  [ ] Look up the 2022 eruption of [[Mauna Loa]]  due 2026-11-12
Journal/2026-11-05.md:10  [ ] Review the eruption timeline board  scheduled 2026-11-09
"
    );
    let headings = run(&vault, &["items", v, "heading"]);
    assert!(
        headings.contains("Concepts/Lava.md:8  ## Temperature\n"),
        "{headings}"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .args(["items", v, "tasks"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("task, list, heading, block"));
    let _ = std::fs::remove_dir_all(vault.parent().unwrap());
}

#[test]
fn a_task_keeps_its_id_through_a_toggle_by_rev() {
    let vault = sandbox("toggle");
    let v = vault.to_str().unwrap();
    let task_on_line_9 = |vault: &Path| -> serde_json::Value {
        run(vault, &["items", v, "task", "--json"])
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|i| i["line"] == 9)
            .unwrap()
    };
    let before = task_on_line_9(&vault);
    assert_eq!(before["status_type"], "todo");
    let agenda: serde_json::Value = serde_json::from_str(&run(
        &vault,
        &[
            "agenda",
            v,
            "--from",
            "2026-11-01",
            "--to",
            "2026-11-30",
            "--json",
        ],
    ))
    .unwrap();
    let listed = agenda["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["line"] == 9)
        .unwrap();
    assert_eq!(
        listed["id"], before["rev"],
        "the agenda's id is the item's rev"
    );

    let file = format!("{v}/Journal/2026-11-05.md:9");
    let rev = before["rev"].as_str().unwrap();
    let toggled: serde_json::Value = serde_json::from_str(&run(
        &vault,
        &["task", "toggle", &file, "--id", rev, "--json"],
    ))
    .unwrap();
    assert_eq!(toggled["status"], "done");

    let after = task_on_line_9(&vault);
    assert_eq!(after["status_type"], "done");
    assert_eq!(after["id"], before["id"], "ticking keeps the id");
    assert_eq!(after["rev"], toggled["id"], "toggle reports the new rev");
    assert_ne!(after["rev"], before["rev"]);

    let stale = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .args(["task", "toggle", &file, "--id", rev])
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(3), "a stale rev is refused");
    let _ = std::fs::remove_dir_all(vault.parent().unwrap());
}
