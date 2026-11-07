//! `cold-lava types` against the built binary: the example vault's listing, and a broken
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

/// `cold-lava types ARGS` from the workspace root, with an empty config folder.
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
fn example_vault_listing() {
    let dir = scratch("example");
    let out = types(&dir, &["example-vault"]);
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

    let out = types(&dir, &["example-vault", "--json"]);
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        json["types"][1]["from"][0]["where"]["header"],
        serde_json::json!(["VEI", "Example"])
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_broken_types_file_exits_1_and_keeps_the_good_types() {
    let dir = scratch("broken");
    let vault = dir.join("vault");
    std::fs::create_dir_all(vault.join(".cold-lava")).unwrap();
    std::fs::write(
        vault.join(".cold-lava/types.toml"),
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
    let out = types(&config, &[vault.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let file = vault.join(".cold-lava/types.toml");
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
    assert!(stdout.contains("\nperson  vault "), "{stdout}");
    assert!(
        stdout.contains(&format!(
            "{file}: [types.person] replaces the one from config {config_file}"
        )),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn shadow_compares_the_task_definition_with_the_native_parser() {
    let dir = scratch("shadow");
    let out = types(&dir, &["example-vault", "--shadow"]);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.starts_with("the task as a definition, beside the native parser: ")
            && stdout.trim_end().ends_with(" · 0 differences"),
        "{stdout}"
    );

    let vault = dir.join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(
        vault.join("Todo.md"),
        "- [ ] Pay rent 📅 2026-11-12 ⏫\n  - [x] Transfer ✅ 2026-11-10\n- [ ] Tagged #2026\n",
    )
    .unwrap();
    let out = types(&dir, &[vault.to_str().unwrap(), "--shadow"]);
    assert_eq!(out.status.code(), Some(1), "a difference exits 1");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "the task as a definition, beside the native parser: 1 note · 3 tasks · 1 difference\n\
         by field: tags 1\n\
         Todo.md:3 tags: native [\"2026\"] · definition []\n"
    );
    let out = types(&dir, &[vault.to_str().unwrap(), "--shadow", "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["tasks"], 3);
    assert_eq!(json["differences"][0]["field"], "tags");
    let _ = std::fs::remove_dir_all(dir);
}
