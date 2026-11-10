//! `cold-lava style`: what the example workspace is detected to write like (pinned), a workspace
//! written another way, and the overrides. Each test has its own workspace and an empty home.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("style-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, text) in files {
        let p = dir.join("ws").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    std::fs::create_dir_all(dir.join("home/.config/cold-lava")).unwrap();
    dir
}

fn cold_lava(dir: &Path, args: &[&str]) -> Output {
    let home = dir.join("home");
    Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("COLD_LAVA_TODAY", "2026-11-09")
        .args(args)
        .output()
        .unwrap()
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(name),
    )
    .unwrap()
}

fn style(dir: &Path) -> serde_json::Value {
    let ws = dir.join("ws");
    let out = cold_lava(dir, &["style", ws.to_str().unwrap(), "--json"]);
    serde_json::from_str(&stdout(&out)).unwrap()
}

#[test]
fn the_example_workspaces_style_is_pinned() {
    let dir = workspace("example", &[]);
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace");
    let example = example.to_str().unwrap();
    let json = stdout(&cold_lava(&dir, &["style", example, "--json"]));
    assert_eq!(json, golden("style.json"));
    let text = stdout(&cold_lava(&dir, &["style", example]));
    assert_eq!(text, golden("style.txt"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A workspace written with `*` lists, Markdown links and Dataview fields, and frontmatter with
/// `#` tags in `- ` lines at the start of the line and its keys in one order.
const OTHER: &[(&str, &str)] = &[
    (
        "Projects/Lava.md",
        "---\ntitle: Lava\ntags:\n- \"#rock\"\ncreated: 08/11/2026\n---\n\
         * [ ] Sample the flow [due:: 2026-11-12]\n* See [Basalt](Basalt.md)\n",
    ),
    (
        "Basalt.md",
        "---\ntitle: Basalt\ntags:\n- \"#rock\"\n- \"#mafic\"\ncreated: 31/10/2026\n---\n\
         * [x] Name it [completion:: 2026-11-01]\n* Back to [Lava](Projects/Lava.md)\n",
    ),
];

#[test]
fn a_workspace_written_another_way_is_detected() {
    let dir = workspace("other", OTHER);
    let s = style(&dir);
    let value = |k: &str| s[k]["value"].as_str().unwrap().to_string();
    assert_eq!(value("list-marker"), "*");
    assert_eq!(value("links"), "markdown");
    assert_eq!(
        value("link-path"),
        "shortest",
        "one shortest, one path: a tie"
    );
    assert_eq!(s["link-path"]["from"], "default");
    assert_eq!(value("task-fields"), "dataview");
    assert_eq!(value("frontmatter-lists"), "flush");
    assert_eq!(value("tags"), "hash");
    assert_eq!(
        s["key-order"]["value"],
        serde_json::json!(["title", "tags", "created"])
    );
    assert_eq!(s["dates"]["created"]["value"], "DD/MM/YYYY");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn obsidian_settings_then_config_then_the_workspace_file_win() {
    let mut files = OTHER.to_vec();
    files.push((".obsidian/app.json", r#"{"useMarkdownLinks": false}"#));
    let dir = workspace("overrides", &files);
    let s = style(&dir);
    assert_eq!(s["links"]["value"], "wikilink");
    assert_eq!(s["links"]["from"], "obsidian useMarkdownLinks");
    assert_eq!(s["links"]["counts"]["markdown"], 2, "the counts stay");

    let config = dir.join("home/.config/cold-lava/config.toml");
    std::fs::write(
        &config,
        "[style]\nlinks = \"markdown\"\nlist-marker = \"+\"\n",
    )
    .unwrap();
    let s = style(&dir);
    assert_eq!(s["links"]["value"], "markdown");
    assert_eq!(s["links"]["from"], "config.toml [style]");

    let file = dir.join("ws/.cold-lava/style.toml");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        "list-marker = \"-\"\n[dates]\ncreated = \"YYYY-MM-DD\"\n",
    )
    .unwrap();
    let s = style(&dir);
    assert_eq!(s["list-marker"]["value"], "-");
    assert_eq!(s["list-marker"]["from"], ".cold-lava/style.toml");
    assert_eq!(s["dates"]["created"]["value"], "YYYY-MM-DD");

    std::fs::write(&file, "list-markers = \"-\"\n").unwrap();
    let ws = dir.join("ws");
    let out = cold_lava(&dir, &["style", ws.to_str().unwrap()]);
    assert!(!out.status.success(), "a mistake fails the check");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(".cold-lava/style.toml") && err.contains("list-markers"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
