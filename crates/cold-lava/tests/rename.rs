//! `cold-lava rename` and canvases as other programs write them: a node without a type, nodes
//! that share an id. Obsidian opens these, so a rename must rewrite every link in them. The
//! test has its own workspace and an empty home.

use std::path::Path;
use std::process::Command;

#[test]
fn a_rename_rewrites_every_canvas_node_whatever_its_type_or_id() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("rename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (ws, home) = (dir.join("ws"), dir.join("home"));
    let size = "\"x\":0,\"y\":0,\"width\":1,\"height\":1";
    let text = |id: &str, text: &str| {
        format!("{{\"id\":\"{id}\",\"type\":\"text\",\"text\":\"{text}\",{size}}}")
    };
    let canvas = |nodes: &[String]| format!("{{\"nodes\":[{}],\"edges\":[]}}", nodes.join(","));
    let card = format!("{{\"id\":\"a\",\"type\":\"file\",\"file\":\"Notes/Lava.md\",{size}}}");
    let good = canvas(&[card, text("b", "about [[Lava]]")]);
    let canvases = [
        ("Good.canvas", good.clone()),
        ("NoType.canvas", good.replace("\"type\":\"file\",", "")),
        (
            "DupId.canvas",
            canvas(&[text("a", "first [[Lava]]"), text("a", "second [[Lava]]")]),
        ),
        (
            "DupOne.canvas",
            canvas(&[text("a", "first"), text("a", "second [[Lava]]")]),
        ),
    ];
    let broken = "{\"nodes\":[{\"id\":\"a\",\"type\":\"file\",\"file\":\"Notes/Lava.md\"";
    std::fs::create_dir_all(ws.join("Notes")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(ws.join("Notes/Lava.md"), "# Lava\n").unwrap();
    std::fs::write(ws.join("Other.md"), "See [[Lava]].\n").unwrap();
    std::fs::write(ws.join("Broken.canvas"), broken).unwrap();
    for (rel, text) in &canvases {
        std::fs::write(ws.join(rel), text).unwrap();
    }

    let out = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .args(["rename", ws.to_str().unwrap()])
        .args(["Notes/Lava.md", "Notes/Magma.md"])
        .output()
        .unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.ends_with("8 links in 5 files\n"), "{stdout}");
    assert!(
        stderr.contains("warning: Broken.canvas can't be read"),
        "a canvas that can't be searched is named: {stderr}"
    );
    let read = |rel: &str| std::fs::read_to_string(ws.join(rel)).unwrap();
    for (rel, before) in &canvases {
        let after = before
            .replace("Notes/Lava.md", "Notes/Magma.md")
            .replace("[[Lava]]", "[[Magma]]");
        assert_eq!(read(rel), after, "{rel}");
    }
    assert_eq!(read("Other.md"), "See [[Magma]].\n");
    assert_eq!(read("Broken.canvas"), broken);
    assert!(ws.join("Notes/Magma.md").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}
