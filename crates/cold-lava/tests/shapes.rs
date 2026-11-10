//! `cold-lava shapes` against the built binary: the example workspace's listing and JSON, shapes
//! drawn with `--render`, and broken definitions. None of them reads your own config.toml.
//! `UPDATE_GOLDEN=1` rewrites the golden files instead of comparing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shapes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `cold-lava shapes ARGS` from the Cargo workspace root, with `config` as the config folder.
fn shapes(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .arg("shapes")
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .env("XDG_CONFIG_HOME", config)
        .env("HOME", config)
        .output()
        .unwrap()
}

fn golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        expected == actual,
        "{name} differs (UPDATE_GOLDEN=1 rewrites it)\n--- expected\n{expected}--- actual\n{actual}"
    );
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn example_workspace_listing_and_json() {
    let dir = scratch("example");
    golden("shapes.txt", &stdout(&shapes(&dir, &["example-workspace"])));
    golden(
        "shapes.json",
        &stdout(&shapes(&dir, &["example-workspace", "--json"])),
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn shapes_render_as_text() {
    let dir = scratch("render");
    for (id, size) in [
        ("venn2", "40x12"),
        ("venn2", "60x18"),
        ("venn3", "60x18"),
        ("timeline", "60x8"),
        ("card", "40x10"),
        ("table", "40x10"),
    ] {
        let out = stdout(&shapes(
            &dir,
            &["example-workspace", "--render", id, "--size", size],
        ));
        golden(&format!("shape-{id}-{size}.txt"), &out);
    }
    let unknown = shapes(&dir, &["example-workspace", "--render", "hexagon"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("no shape is called hexagon"));
    let _ = std::fs::remove_dir_all(dir);
}

/// The two-circle Venn bound to two queries over the example workspace: volcanoes with an
/// `#eruption` against stratovolcanoes, matched on their names.
const ERUPTED: &str = "a=type=volcano tag=eruption title=Erupted";
const STRATO: &str = "b=type=volcano tag=volcano/strato title=Stratovolcanoes";

#[test]
fn bound_shapes_show_their_data() {
    let dir = scratch("bound");
    let ws = "example-workspace";
    let venn = [
        "--render", "venn2", "--size", "80x24", "--bind", ERUPTED, "--bind", STRATO,
    ];
    let out = stdout(&shapes(&dir, &[&[ws][..], &venn].concat()));
    golden("shape-venn2-bound-80x24.txt", &out);
    let json = stdout(&shapes(&dir, &[&[ws][..], &venn, &["--json"]].concat()));
    golden("shape-venn2-bound.json", &json);
    let shown: serde_json::Value = serde_json::from_str(&json).unwrap();
    let labels = |region: usize| -> Vec<String> {
        (shown["regions"][region]["items"].as_array().unwrap().iter())
            .map(|e| e["label"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(labels(0), ["Krakatoa"], "erupted, not a stratovolcano");
    assert_eq!(
        labels(1),
        ["Eyjafjallajökull", "Mount St. Helens", "Vesuvius"],
        "both"
    );
    assert_eq!(
        labels(2),
        ["Mount Fuji"],
        "a stratovolcano with no #eruption"
    );

    let card = [
        "--render",
        "card",
        "--size",
        "50x10",
        "--bind",
        "note=note:Volcanoes/Krakatoa",
    ];
    golden(
        "shape-card-bound-50x10.txt",
        &stdout(&shapes(&dir, &[&[ws][..], &card].concat())),
    );

    for (bind, error) in [
        ("c=items:x", "venn2 has no input c (its inputs: a, b)"),
        (
            "a=type=vulcano",
            "input a: no item kind or type called vulcano",
        ),
        (
            "a=type=volcano colour=red",
            "--bind a: colour is not part of a query",
        ),
        ("a", "--bind \"a\": write it INPUT=SOURCE"),
    ] {
        let out = shapes(&dir, &[ws, "--render", "venn2", "--bind", bind]);
        assert!(!out.status.success(), "{bind}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(error), "{bind}: {stderr}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// The region language over the example workspace: an input's `where`, and a region that
/// filters, sorts and limits.
#[test]
fn bound_shapes_filter_sort_and_limit() {
    let dir = scratch("language");
    let ws = "example-workspace";
    let tall = "b=type=volcano title=\"Over 2500 m\" where elevation >= 2500";
    let venn = [
        "--render", "venn2", "--size", "80x24", "--bind", ERUPTED, "--bind", tall,
    ];
    let json = stdout(&shapes(&dir, &[&[ws][..], &venn, &["--json"]].concat()));
    let shown: serde_json::Value = serde_json::from_str(&json).unwrap();
    let labels = |shown: &serde_json::Value, region: usize| -> Vec<String> {
        (shown["regions"][region]["items"].as_array().unwrap().iter())
            .map(|e| e["label"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        labels(&shown, 0),
        ["Eyjafjallajökull", "Krakatoa", "Vesuvius"]
    );
    assert_eq!(labels(&shown, 1), ["Mount St. Helens"]);
    assert_eq!(labels(&shown, 2), ["Mauna Loa", "Mount Fuji", "Parícutin"]);
    golden(
        "shape-venn2-tall-80x24.txt",
        &stdout(&shapes(&dir, &[&[ws][..], &venn].concat())),
    );

    let tallest = [ws, "--render", "tallest", "--size", "40x14"];
    let bind = [
        "--bind",
        "volcanoes=type=volcano title=\"Tallest volcanoes\"",
    ];
    let json = stdout(&shapes(&dir, &[&tallest[..], &bind, &["--json"]].concat()));
    let shown: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        labels(&shown, 0),
        [
            "Mauna Loa",
            "Mount Fuji",
            "Parícutin",
            "Mount St. Helens",
            "Eyjafjallajökull"
        ],
        "where elevation, sort by elevation desc, limit 5"
    );
    golden(
        "shape-tallest-40x14.txt",
        &stdout(&shapes(&dir, &[&tallest[..], &bind].concat())),
    );

    let out = shapes(
        &dir,
        &[
            ws,
            "--render",
            "venn2",
            "--bind",
            "a=type=volcano where >= 3",
        ],
    );
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--bind a: where >= 3: expected a field, found `>=`"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_venn_titles_never_touch_at_40x12() {
    let dir = scratch("titles");
    let out = stdout(&shapes(
        &dir,
        &["example-workspace", "--render", "venn2", "--size", "40x12"],
    ));
    let titles = out.lines().find(|l| l.contains("a.title")).unwrap();
    assert!(titles.contains("a.title  b.title"), "{titles}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn broken_definitions_exit_1_and_the_rest_load() {
    let dir = scratch("broken");
    let workspace = dir.join("workspace");
    std::fs::create_dir_all(workspace.join(".cold-lava")).unwrap();
    std::fs::write(
        workspace.join(".cold-lava/shapes.toml"),
        "version = 1\n\
         [[shapes]]\nid = \"lane\"\nparts = [{ rect = { id = \"box\", from = [0, 0], to = [1, 1] } }]\n\
         regions = [{ id = \"in\", area = \"box & nothing\" }]\n\
         [[shapes]]\nid = \"blob\"\nparts = [{ ellipse = { center = [0.5, 0.5], radii = [0.5, 0.3] } }]\n",
    )
    .unwrap();
    let config = dir.join("config");
    std::fs::create_dir_all(config.join("cold-lava")).unwrap();
    std::fs::write(
        config.join("cold-lava/config.toml"),
        "[[shapes]]\nid = \"card\"\ntitle = \"My card\"\nparts = [{ rect = { from = [0, 0], to = [1, 1] } }]\n\
         [[shapes]]\nid = \"spiral\"\nparts = [{ spiral = {} }]\n",
    )
    .unwrap();
    let out = shapes(&config, &[workspace.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let listing = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    let file = workspace.join(".cold-lava/shapes.toml");
    let config_file = config.join("cold-lava/config.toml");
    assert!(listing.contains("\nblob  workspace "), "{listing}");
    assert!(listing.contains("\ncard  config "), "{listing}");
    assert!(listing.contains("My card"), "{listing}");
    assert!(!listing.contains("\nlane "), "{listing}");
    assert!(
        stderr.contains(&format!(
            "{}: shape lane: region `in`: no part is called `nothing`",
            file.display()
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{}: shape spiral: unknown variant `spiral`",
            config_file.display()
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "{}: shape card replaces the one from built in",
            config_file.display()
        )),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
