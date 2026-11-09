//! `cold-lava --version` names the commit the binary was built from (`build.rs`).

use std::process::Command;

#[test]
fn the_version_names_the_commit_it_was_built_from() {
    let out = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    let printed = String::from_utf8(out.stdout).unwrap();
    let version = format!("cold-lava {}", env!("CARGO_PKG_VERSION"));
    let head = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8(o.stdout).unwrap().trim().to_string());
    match head {
        Some(head) => assert_eq!(printed.trim(), format!("{version} ({head})")),
        None => assert_eq!(printed.trim(), version),
    }
}
