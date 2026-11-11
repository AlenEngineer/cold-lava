//! The extension host has no user interface (CLAUDE.md, `crates/host`): a headless frontend,
//! such as a language server, runs extensions through it without a terminal. This walks the
//! workspace's Cargo.lock, so it sees what `cargo tree -p cold-lava-host` would.

use std::collections::{BTreeSet, HashMap};

const UI: &[&str] = &["ratatui", "crossterm", "portable-pty", "vt100"];

/// Every package `root` depends on, itself included, by name.
fn closure(root: &str) -> BTreeSet<String> {
    let lock = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
    let lock: toml::Table = toml::from_str(&std::fs::read_to_string(lock).unwrap()).unwrap();
    let mut depends: HashMap<&str, Vec<&str>> = HashMap::new();
    for package in lock["package"].as_array().unwrap() {
        let name = package["name"].as_str().unwrap();
        let on = package.get("dependencies").and_then(|d| d.as_array());
        let on = on.into_iter().flatten().filter_map(|d| d.as_str());
        // A dependency is written "name", or "name version" when two versions are locked.
        let names = on.map(|d| d.split(' ').next().unwrap_or(d));
        depends.entry(name).or_default().extend(names);
    }
    let mut seen = BTreeSet::new();
    let mut todo = vec![root];
    while let Some(name) = todo.pop() {
        if seen.insert(name.to_string()) {
            todo.extend(depends.get(name).into_iter().flatten());
        }
    }
    seen
}

#[test]
fn the_host_and_the_view_vocabulary_depend_on_no_ui_crate() {
    assert!(
        closure("cold-lava").contains("ratatui"),
        "the walk finds what the terminal UI depends on"
    );
    for krate in ["cold-lava-host", "cold-lava-views"] {
        let all = closure(krate);
        assert!(all.contains("cold-lava-engine"), "{krate}: {all:?}");
        for ui in UI {
            assert!(
                !all.contains(*ui),
                "{krate} depends on {ui}: what draws or reads a terminal stays in the frontend"
            );
        }
    }
}

#[test]
fn the_engine_depends_on_neither_the_host_nor_the_views() {
    let all = closure("cold-lava-engine");
    assert!(all.contains("serde"), "{all:?}");
    for above in ["cold-lava-host", "cold-lava-views", "cold-lava"] {
        assert!(
            !all.contains(above),
            "the engine depends on {above}: the direction is frontends, host, views, engine"
        );
    }
}
