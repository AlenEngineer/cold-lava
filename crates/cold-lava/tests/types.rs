//! `cold-lava types` against the built binary: the example workspace's listing, and a broken
//! types.toml. Neither reads your own config.toml.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("types-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `cold-lava types ARGS` from the Cargo workspace root, with an empty config folder.
fn types(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .arg("types")
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env("XDG_CONFIG_HOME", config)
        .output()
        .unwrap()
}

#[test]
fn example_workspace_listing() {
    let dir = scratch("example");
    let out = types(&dir, &["example-workspace"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let golden = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/types.txt"),
    )
    .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), golden);

    let out = types(&dir, &["example-workspace", "--json"]);
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json["types"][2]["from"][0]["where"]["header"],
        serde_json::json!(["VEI", "Example"])
    );
    assert_eq!(
        json["types"][0]["name"], "task",
        "the built-in task comes first"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_broken_types_file_exits_1_and_keeps_the_good_types() {
    let dir = scratch("broken");
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(workspace.join(".cold-lava")).unwrap();
    std::fs::write(
        workspace.join(".cold-lava/types.toml"),
        "version = 1\n\
         [types.estimate]\non = \"inline\"\nmatch = '~(?<v>{duration})'\n\
         fields.minutes = { capture = \"m\", type = \"duration\" }\n\
         [types.book]\non = \"note\"\nfields.title = { column = \"Title\" }\n\
         [types.person]\non = \"note\"\nfields.name = { note = \"name\" }\n",
    )
    .unwrap();
    let config = dir.join("config");
    std::fs::create_dir_all(config.join("cold-lava")).unwrap();
    std::fs::write(
        config.join("cold-lava/config.toml"),
        "[types.person]\non = \"note\"\n[types.task]\non = \"list-item\"\n",
    )
    .unwrap();
    let out = types(&config, &[workspace.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let file = workspace.join(".cold-lava/types.toml");
    let config_file = config.join("cold-lava/config.toml");
    let (file, config_file) = (file.display(), config_file.display());
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        format!(
            "errors:\n  \
             {config_file}: [types.task] task is built in: it can be extended (extends = \"task\") but not redefined\n  \
             {file}: [types.estimate] fields.minutes: the match has no group named \"m\"\n  \
             {file}: [types.book] fields.title: column can't be read on note (only on table-row)\n"
        )
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("\nperson  workspace "), "{stdout}");
    assert!(
        stdout.contains(&format!(
            "{file}: [types.person] replaces the one from config {config_file}"
        )),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
