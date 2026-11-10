//! `cold-lava items`, `agenda` and `task toggle` against the built binary, on a copy of the
//! example workspace. The agenda goldens in tests/golden were recorded before the agenda read
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
    let workspace = dir.join("example-workspace");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace"),
        &workspace,
    );
    workspace.canonicalize().unwrap()
}

/// cold-lava without your own config.toml, so only the workspace's types apply.
fn cold_lava(workspace: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cold-lava"));
    c.env("XDG_CONFIG_HOME", workspace.parent().unwrap());
    c
}

/// Runs cold-lava; its stdout with the workspace's path written as `WORKSPACE`.
fn run(workspace: &Path, args: &[&str]) -> String {
    let out = cold_lava(workspace).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .replace(workspace.to_str().unwrap(), "WORKSPACE")
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
    let workspace = sandbox("agenda");
    let v = workspace.to_str().unwrap();
    let range = ["--from", "2026-11-01", "--to", "2026-11-30"];
    let args = |json: bool| {
        let mut a = vec!["agenda", v];
        a.extend(range);
        if json {
            a.push("--json");
        }
        a
    };
    assert_eq!(run(&workspace, &args(true)), golden("agenda.json"));
    assert_eq!(run(&workspace, &args(false)), golden("agenda.txt"));
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

#[test]
fn items_lists_each_kind() {
    let workspace = sandbox("kinds");
    let v = workspace.to_str().unwrap();
    assert_eq!(
        run(&workspace, &["items", v, "task"]),
        "\
Inbox/Field Season.md:9  [ ] Book the flight to Kīlauea
Inbox/Field Season.md:10  [ ] Pack the gas masks
Inbox/Field Season.md:14  [ ] Read up on pāhoehoe
Inbox/Field Season.md:18  [x] Renew the park permit
Journal/2026-11-05.md:7  [ ] Write a note on lahars ([[Lahar]] is still unresolved)
Journal/2026-11-05.md:8  [x] Add the [[Volcanic Explosivity Index]] table
Journal/2026-11-05.md:9  [ ] Look up the 2022 eruption of [[Mauna Loa]]  due 2026-11-12
Journal/2026-11-05.md:10  [ ] Review the eruption timeline board  scheduled 2026-11-09
Volcano Venn.canvas:2  [ ] Two per person, for the trip
Welcome.canvas:2  [ ] `Tab` to select a card
Welcome.canvas:3  [ ] `Shift+arrows` to move it
Welcome.canvas:4  [ ] `a` to add a linked card
Welcome.canvas:5  [ ] `c` to connect two cards
Welcome.canvas:6  [ ] `1`–`6` to colour a card
Welcome.canvas:7  [ ] `Enter` to edit text
Welcome.canvas:8  [ ] `u` to undo
Welcome.canvas:9  [ ] `n` or double-click for a new card
"
    );
    let journal: String = (run(&workspace, &["items", v, "task", "--json"]).lines())
        .filter(|l| l.contains(r#""note":"Journal/"#))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(
        journal,
        golden("items-task.json"),
        "the tasks' ids and JSON, as the native parser had them before the built-in definition"
    );
    let headings = run(&workspace, &["items", v, "heading"]);
    assert!(
        headings.contains("Concepts/Lava.md:8  ## Temperature\n"),
        "{headings}"
    );
    let out = cold_lava(&workspace)
        .args(["items", v, "tasks"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("task, list, heading, block, volcano, eruption, vei"),
        "the workspace's types are kinds too"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

#[test]
fn types_find_their_items() {
    let workspace = sandbox("types");
    let v = workspace.to_str().unwrap();
    assert_eq!(
        run(&workspace, &["items", v, "volcano"]),
        golden("items-volcano.txt")
    );
    assert_eq!(
        run(&workspace, &["items", v, "eruption", "--json"]),
        golden("items-eruption.json")
    );
    assert_eq!(
        run(&workspace, &["items", v, "vei", "--json"]),
        golden("items-vei.json"),
        "an inline type, found in the prose of three notes"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

#[test]
fn a_task_keeps_its_id_through_a_toggle_by_rev() {
    let workspace = sandbox("toggle");
    let v = workspace.to_str().unwrap();
    let task_on_line_9 = |workspace: &Path| -> serde_json::Value {
        run(workspace, &["items", v, "task", "--json"])
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|i| i["line"] == 9 && i["note"] == "Journal/2026-11-05.md")
            .unwrap()
    };
    let before = task_on_line_9(&workspace);
    assert_eq!(before["status_type"], "todo");
    let agenda: serde_json::Value = serde_json::from_str(&run(
        &workspace,
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
        .find(|t| t["line"] == 9 && t["file"] == "Journal/2026-11-05.md")
        .unwrap();
    assert_eq!(
        listed["id"], before["rev"],
        "the agenda's id is the item's rev"
    );

    let file = format!("{v}/Journal/2026-11-05.md:9");
    let rev = before["rev"].as_str().unwrap();
    let toggled: serde_json::Value = serde_json::from_str(&run(
        &workspace,
        &["task", "toggle", &file, "--id", rev, "--json"],
    ))
    .unwrap();
    assert_eq!(toggled["status"], "done");

    let after = task_on_line_9(&workspace);
    assert_eq!(after["status_type"], "done");
    assert_eq!(after["id"], before["id"], "ticking keeps the id");
    assert_eq!(after["rev"], toggled["id"], "toggle reports the new rev");
    assert_ne!(after["rev"], before["rev"]);

    let stale = cold_lava(&workspace)
        .args(["task", "toggle", &file, "--id", rev])
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(3), "a stale rev is refused");
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

#[test]
fn item_set_writes_one_cell_and_refuses_a_stale_rev() {
    let workspace = sandbox("set");
    let v = workspace.to_str().unwrap();
    let note = "Concepts/Volcanic Explosivity Index.md";
    let file = format!("{v}/{note}");
    let original = std::fs::read_to_string(&file).unwrap();
    let row = |workspace: &Path, vei: i64| -> serde_json::Value {
        run(workspace, &["items", v, "eruption", "--json"])
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|i| i["vei"] == vei)
            .unwrap()
    };
    let before = row(&workspace, 6);
    let id = before["id"].as_str().unwrap();
    let rev = before["rev"].as_str().unwrap();
    let value = "[[Krakatoa]] 1883, 416";

    let dry = run(
        &workspace,
        &["item", "set", &file, id, "example", value, "--dry-run"],
    );
    assert!(dry.starts_with(&format!("would set {note}:13")), "{dry}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), original);

    let set: serde_json::Value = serde_json::from_str(&run(
        &workspace,
        &[
            "item", "set", &file, id, "example", value, "--rev", rev, "--json",
        ],
    ))
    .unwrap();
    assert_eq!(set["applied"], true);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        original.replace(
            "| 6 | [[Krakatoa]] 1883 |",
            "| 6 | [[Krakatoa]] 1883, 416 |"
        ),
        "only the cell changed"
    );
    let after = row(&workspace, 6);
    assert_eq!(after["id"], before["id"], "example is not the row's key");
    assert_eq!(set["item"], after, "set reports the item as it reads now");

    let stale = cold_lava(&workspace)
        .args(["item", "set", &file, id, "example", "x", "--rev", rev])
        .output()
        .unwrap();
    assert_eq!(stale.status.code(), Some(3), "a stale rev is refused");
    let read_only = cold_lava(&workspace)
        .args(["item", "set", &file, id, "volcanoes", "Etna"])
        .output()
        .unwrap();
    assert_eq!(read_only.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&read_only.stderr).contains("can't be written"),
        "links in text are read-only"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

#[test]
fn table_prints_a_types_rows_sorted() {
    let workspace = sandbox("table");
    let v = workspace.to_str().unwrap();
    let text = run(&workspace, &["table", v, "volcano", "--sort", "-elevation"]);
    let names: Vec<&str> = (text.lines().skip(1))
        .map(|l| l.split("  ").next().unwrap())
        .collect();
    assert_eq!(
        names[..3],
        ["Mauna Loa", "Mount Fuji", "Parícutin"],
        "{text}"
    );
    assert_eq!(
        names[7..],
        ["Krakatoa", "Yellowstone"],
        "no elevation: last"
    );
    assert!(text.starts_with("name  "), "{text}");
    let json: serde_json::Value =
        serde_json::from_str(&run(&workspace, &["table", v, "eruption", "--json"])).unwrap();
    assert_eq!(json["columns"][0]["name"], "vei");
    assert_eq!(json["rows"].as_array().unwrap().len(), 4);
    assert_eq!(json["rows"][2]["cells"][0], 6);
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

#[test]
fn item_set_finds_a_workspace_marked_by_cold_lava_and_explains_a_guess() {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("items-locate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for ws in ["marked", "loose"] {
        std::fs::create_dir_all(dir.join(ws).join("Notes")).unwrap();
        std::fs::write(dir.join(ws).join("Notes/A.md"), "- [ ] Plain one\n").unwrap();
    }
    std::fs::create_dir_all(dir.join("marked/.cold-lava")).unwrap();
    let id = |ws: &Path| -> String {
        let out = cold_lava(ws)
            .args(["items", ws.to_str().unwrap(), "task", "--json"])
            .output()
            .unwrap();
        let line = String::from_utf8(out.stdout).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        v["id"].as_str().unwrap().to_string()
    };
    let set = |ws: &Path, id: &str| {
        cold_lava(ws)
            .args([
                "item",
                "set",
                ws.join("Notes/A.md").to_str().unwrap(),
                id,
                "due",
                "2026-12-24",
            ])
            .output()
            .unwrap()
    };
    // A .cold-lava folder marks the workspace, so the id from `items` on it matches.
    let marked = dir.join("marked").canonicalize().unwrap();
    let out = set(&marked, &id(&marked));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        std::fs::read_to_string(marked.join("Notes/A.md"))
            .unwrap()
            .contains("2026-12-24")
    );
    // Without a marker, the note's folder is guessed, and the refusal says why.
    let loose = dir.join("loose").canonicalize().unwrap();
    let out = set(&loose, &id(&loose));
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no .obsidian or .cold-lava folder"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
