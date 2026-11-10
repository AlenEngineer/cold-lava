//! `cold-lava recipe`: the plan as a diff or JSON, applying it, and what it refuses. Each test
//! has its own workspace and an empty home, so your extensions and cache take no part.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("recipe-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, text) in files {
        let p = dir.join("ws").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    std::fs::create_dir_all(dir.join("home")).unwrap();
    dir
}

fn cold_lava(dir: &Path, args: &[&str]) -> Output {
    let home = dir.join("home");
    Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .args(args)
        .output()
        .unwrap()
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join("ws").join(rel)).unwrap()
}

const A: &str = "---\ntags: [lava, magma]\n---\n# A\n\n#lava/flow and `#lava`\n";
const B: &str = "```\n#lava\n```\n#Lava\n";
const C: &str = "#lavafield\n";

#[test]
fn dry_run_json_is_pinned_and_writes_nothing() {
    let dir = workspace("dry", &[("A.md", A), ("Sub/B.md", B), ("C.md", C)]);
    let ws = dir.join("ws");
    let out = cold_lava(
        &dir,
        &[
            "recipe",
            ws.to_str().unwrap(),
            "rename-tag",
            "--from",
            "lava",
            "--to",
            "magma",
            "--dry-run",
            "--json",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "recipe": "rename-tag",
            "options": {"from": "lava", "to": "magma"},
            "applied": false,
            "changes": 3,
            "files": [
                {
                    "file": "A.md",
                    "changes": 2,
                    "owner": null,
                    "diff": "@@ line 1 @@\n ---\n-tags: [lava, magma]\n+tags: [magma]\n ---\n@@ line 5 @@\n \n-#lava/flow and `#lava`\n+#magma/flow and `#lava`\n",
                },
                {
                    "file": "Sub/B.md",
                    "changes": 1,
                    "owner": null,
                    "diff": "@@ line 3 @@\n ```\n-#Lava\n+#magma\n",
                },
            ],
            "warnings": [],
        })
    );
    assert_eq!(read(&dir, "A.md"), A);
    assert_eq!(read(&dir, "Sub/B.md"), B);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn apply_writes_every_file_and_the_tags_read_renamed() {
    let dir = workspace("apply", &[("A.md", A), ("Sub/B.md", B), ("C.md", C)]);
    let ws = dir.join("ws");
    let ws = ws.to_str().unwrap();
    let args = [
        "recipe",
        ws,
        "rename-tag",
        "--from",
        "#lava",
        "--to",
        "magma",
    ];
    let out = cold_lava(&dir, &args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.starts_with("--- A.md\n@@ line 1 @@\n ---\n-tags: [lava, magma]\n"));
    assert!(
        stdout.ends_with("rename-tag from=#lava to=magma: changed 3 tags in 2 files\n"),
        "{stdout}"
    );
    assert_eq!(
        read(&dir, "A.md"),
        "---\ntags: [magma]\n---\n# A\n\n#magma/flow and `#lava`\n"
    );
    assert_eq!(read(&dir, "Sub/B.md"), "```\n#lava\n```\n#magma\n");
    assert_eq!(read(&dir, "C.md"), C);

    // Search reads the renamed tags as written; nothing is left to rename.
    let out = cold_lava(&dir, &["search", ws, "#magma/flow"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("A.md:6"));
    let again = cold_lava(&dir, &args);
    assert!(
        String::from_utf8_lossy(&again.stdout)
            .contains("rename-tag from=#lava to=magma: nothing to change")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_options_and_unknown_recipes_are_refused() {
    let dir = workspace("refused", &[("A.md", A)]);
    let ws = dir.join("ws");
    let ws = ws.to_str().unwrap();
    for (args, says) in [
        (
            vec!["recipe", ws, "rename-tag", "--from", "lava"],
            "rename-tag needs --to",
        ),
        (
            vec![
                "recipe",
                ws,
                "rename-tag",
                "--from",
                "lava",
                "--to",
                "two words",
            ],
            "to: \"two words\" isn't a tag",
        ),
        (vec!["recipe", ws, "no-such"], "no recipe called no-such"),
    ] {
        let out = cold_lava(&dir, &args);
        assert!(!out.status.success());
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(says), "{err}");
    }
    assert_eq!(read(&dir, "A.md"), A);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn list_names_the_recipes_and_their_options() {
    let dir = workspace("list", &[]);
    let out = cold_lava(&dir, &["recipe", "--list", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["id"], "rename-tag");
    assert_eq!(json[0]["options"][0]["name"], "from");
    assert_eq!(json[0]["options"][1]["name"], "to");
    let out = cold_lava(&dir, &["recipe", "--list"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("--from VALUE"));
    let _ = std::fs::remove_dir_all(&dir);
}
