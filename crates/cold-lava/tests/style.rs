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

/// `OTHER` with notes to write into, and a type with a link and a property.
fn writes() -> Vec<(&'static str, &'static str)> {
    let mut files = OTHER.to_vec();
    files.extend([
        ("Inbox.md", "* [ ] Call the lab\n* [ ] Wash the samples\n"),
        (
            "Scoria.md",
            "---\ntags:\n- \"#rock\"\ncreated: 02/11/2026\n---\n# Scoria\n",
        ),
        (
            ".cold-lava/types.toml",
            "version = 1\n\n[types.rock]\non = \"note\"\nwhere.tag = [\"rock\"]\n\
             fields.site = { inline = \"site\", type = \"link\" }\n\
             fields.title = { property = \"title\" }\n",
        ),
    ]);
    files
}

/// The id of the item of `kind` on `note`'s `line`.
fn id(dir: &Path, kind: &str, note: &str, line: u64) -> String {
    let ws = dir.join("ws");
    let out = stdout(&cold_lava(
        dir,
        &["items", ws.to_str().unwrap(), kind, "--json"],
    ));
    let items: Vec<serde_json::Value> = (out.lines())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let item = items
        .iter()
        .find(|i| i["note"] == note && i["line"] == line);
    let item = item.unwrap_or_else(|| panic!("no {kind} on {note}:{line} in {out}"));
    item["id"].as_str().unwrap().to_string()
}

fn set(dir: &Path, note: &str, id: &str, field: &str, value: &str) {
    let file = dir.join("ws").join(note);
    let args = ["item", "set", file.to_str().unwrap(), id, field, value];
    stdout(&cold_lava(dir, &args));
}

fn read(dir: &Path, note: &str) -> String {
    std::fs::read_to_string(dir.join("ws").join(note)).unwrap()
}

#[test]
fn each_insertion_writes_in_the_workspaces_style() {
    let dir = workspace("writes", &writes());

    let call = id(&dir, "task", "Inbox.md", 1);
    set(&dir, "Inbox.md", &call, "due", "2026-12-01");
    let line = format!("{}:2", dir.join("ws/Inbox.md").to_str().unwrap());
    stdout(&cold_lava(&dir, &["task", "toggle", &line]));
    assert_eq!(
        read(&dir, "Inbox.md"),
        "* [ ] Call the lab [due:: 2026-12-01]\n\
         * [x] Wash the samples [completion:: 2026-11-09]\n",
        "a new task field and a done date are Dataview fields"
    );

    let basalt = id(&dir, "rock", "Basalt.md", 1);
    set(&dir, "Basalt.md", &basalt, "site", "Projects/Lava");
    let text = read(&dir, "Basalt.md");
    assert!(
        text.contains("---\nsite:: [Lava](Projects/Lava.md)\n* [x] Name it"),
        "a new link is a Markdown link: {text}"
    );

    let scoria = id(&dir, "rock", "Scoria.md", 1);
    set(&dir, "Scoria.md", &scoria, "title", "Scoria");
    assert_eq!(
        read(&dir, "Scoria.md"),
        "---\ntitle: Scoria\ntags:\n- \"#rock\"\ncreated: 02/11/2026\n---\n# Scoria\n",
        "a new key goes where the workspace writes it"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
