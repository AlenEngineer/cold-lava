//! The core is a workspace's only file watcher (CLAUDE.md, `core.rs`): screens follow its
//! events instead of watching files themselves.

#[test]
fn only_the_engine_can_watch_files() {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let manifest: toml::Table =
        toml::from_str(&std::fs::read_to_string(manifest).unwrap()).unwrap();
    let depends = manifest["dependencies"].as_table().unwrap();
    assert!(
        !depends.contains_key("notify"),
        "the binary depends on a file watcher: follow `Core::poll`'s events instead"
    );
}
