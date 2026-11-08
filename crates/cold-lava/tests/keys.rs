//! `cold-lava keys` against the built binary, with a config.toml of its own.

use std::path::Path;
use std::process::Output;

fn keys(name: &str, config_toml: &str) -> Output {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("keys-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("cold-lava")).unwrap();
    std::fs::write(dir.join("cold-lava/config.toml"), config_toml).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .arg("keys")
        .env("XDG_CONFIG_HOME", &dir)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    out
}

#[test]
fn an_old_command_name_is_a_warning_not_a_failure() {
    let out = keys("old-name", "[keys]\n\"vault.add\" = \"V\"\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("[keys] \"vault.add\" is now called \"workspace.add\""),
        "{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find(|l| l.contains("workspace.add"))
        .unwrap();
    assert!(line.split_whitespace().nth(1) == Some("V"), "{line}");
}

#[test]
fn an_unknown_command_still_fails() {
    let out = keys("unknown", "[keys]\n\"graph.opne\" = \"G\"\n");
    assert_eq!(out.status.code(), Some(1));
}
