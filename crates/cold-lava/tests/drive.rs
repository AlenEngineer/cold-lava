//! `cold-lava drive` against the built binary, on a copy of the example workspace.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A fresh workspace copy. `drive` gives the program an empty home itself.
fn sandbox(name: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("drive-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace"),
        &dir.join("example-workspace"),
    );
    dir
}

/// Where the system trash keeps deleted files, under the program's home.
fn system_trash(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join(".Trash")
    } else {
        home.join(".local/share/Trash/files")
    }
}

fn drive(script: &str, dir: &Path) -> Output {
    drive_with(script, dir, &[])
}

fn drive_with(script: &str, dir: &Path, options: &[&std::ffi::OsStr]) -> Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/drive")
        .join(script);
    Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .arg("drive")
        .args(options)
        .arg(&script)
        .arg("--")
        .arg(dir.join("example-workspace"))
        .output()
        .unwrap()
}

#[test]
fn deletes_a_note_to_the_system_trash_and_undoes_it() {
    let dir = sandbox("delete");
    let home = dir.join("home");
    let workspace = dir.join("example-workspace");
    let lava = std::fs::read(workspace.join("Concepts/Lava.md")).unwrap();
    let out = drive_with("delete.drive", &dir, &["--home".as_ref(), home.as_os_str()]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!workspace.join("Concepts/Lava.md").exists());
    assert!(
        !workspace.join(".trash").exists(),
        "not the workspace's own trash"
    );
    let files = system_trash(&home);
    let mut trashed = files_ending(&files, "");
    trashed.sort();
    assert_eq!(
        trashed,
        [files.join("Lava.md"), files.join("Welcome.canvas")]
    );
    assert!(!workspace.join("Welcome.canvas").exists());
    assert_eq!(std::fs::read(files.join("Lava.md")).unwrap(), lava);
    if cfg!(target_os = "macos") {
        let _ = std::fs::remove_dir_all(dir);
        return;
    }
    let trash = home.join(".local/share/Trash");
    let info = std::fs::read_to_string(trash.join("info/Lava.md.trashinfo")).unwrap();
    assert!(
        info.starts_with("[Trash Info]\nPath=/")
            && info.contains("/example-workspace/Concepts/Lava.md\n"),
        "{info}"
    );
    assert_eq!(
        files_ending(&trash.join("info"), "").len(),
        2,
        "undo took its info away"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Links in notes that lead nowhere, as `note name: target` (names survive folder moves).
fn unresolved(workspace: &Path) -> Vec<String> {
    use cold_lava_engine::{index::Index, resolver::Files, workspace};
    let ix = Index::build(workspace::scan(workspace).unwrap());
    let files = Files::of_workspace(&ix);
    let mut out = Vec::new();
    for n in &ix.workspace.notes {
        let dir = n.rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        for r in &n.refs {
            if files.resolve(dir, &r.target).is_none() {
                out.push(format!("{}: {}", n.name, r.target));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn renames_moves_creates_and_deletes_folders_keeping_links() {
    let dir = sandbox("folders");
    let home = dir.join("home");
    let workspace = dir.join("example-workspace");
    std::fs::write(
        workspace.join("Inbox/Links.md"),
        "---\nrelated: \"[[People/Pliny the Younger]]\"\n---\n# Links\n\
         [[People/David A. Johnston]] and [Krafft](../People/Katia%20and%20Maurice%20Krafft.md) \
         and [[Pliny the Younger]].\n",
    )
    .unwrap();
    let canvas = "{\"nodes\":[\n\
        {\"id\":\"a\",\"type\":\"file\",\"file\":\"People/David A. Johnston.md\",\"x\":0,\"y\":0,\"width\":400,\"height\":400},\n\
        {\"id\":\"b\",\"type\":\"text\",\"text\":\"[[People/Pliny the Younger]]\",\"x\":500,\"y\":0,\"width\":400,\"height\":100}\n\
        ],\"edges\":[]}\n";
    std::fs::write(workspace.join("Inbox/People.canvas"), canvas).unwrap();
    let before = unresolved(&workspace);
    let out = drive_with(
        "folders.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let after = unresolved(&workspace);
    eprintln!(
        "unresolved links: {} before, {} after",
        before.len(),
        after.len()
    );
    assert_eq!(after, before, "every link still leads where it did");
    assert!(!workspace.join("People").exists());
    let moved = workspace.join("Guide/Volcanologists");
    assert!(moved.join("David A. Johnston.md").is_file());
    assert!(
        moved.join("Field Notes").is_dir(),
        "the new folder came back too"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("Inbox/Links.md")).unwrap(),
        "---\nrelated: \"[[Guide/Volcanologists/Pliny the Younger]]\"\n---\n# Links\n\
         [[Guide/Volcanologists/David A. Johnston]] and \
         [Krafft](../Guide/Volcanologists/Katia%20and%20Maurice%20Krafft.md) \
         and [[Pliny the Younger]].\n"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("Inbox/People.canvas")).unwrap(),
        canvas.replace("People/", "Guide/Volcanologists/")
    );
    assert!(
        files_ending(&system_trash(&home), "").is_empty(),
        "undo took the folder out of the trash"
    );
    assert!(!workspace.join(".trash").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn duplicates_a_note_and_merges_one_into_another_keeping_links() {
    let dir = sandbox("merge");
    let home = dir.join("home");
    let workspace = dir.join("example-workspace");
    let source = std::fs::read_to_string(workspace.join("Concepts/Pāhoehoe.md")).unwrap();
    let before = unresolved(&workspace);
    let out = drive_with("merge.drive", &dir, &["--home".as_ref(), home.as_os_str()]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        unresolved(&workspace),
        before,
        "no link was left pointing nowhere"
    );
    let read = |rel: &str| std::fs::read_to_string(workspace.join(rel)).unwrap();
    assert_eq!(read("Volcanoes/Kīlauea 1.md"), read("Volcanoes/Kīlauea.md"));
    assert!(!workspace.join("Concepts/Pāhoehoe.md").exists());
    assert_eq!(
        read("Concepts/Lava.md"),
        "---\ntags: [concept, lava]\naliases: [Pāhoehoe]\n---\n# Lava\n\n\
         Molten rock that reaches the surface. Below ground it's called magma.\n\n\
         ## Temperature\nBasaltic lava, the runny kind at a [[Shield Volcano]], erupts at roughly \
         1,100–1,200 °C.\n\n## Textures\nWhen it cools, basaltic lava usually becomes either \
         smooth, ropy [[Lava]] or rough, jagged\n[[ʻAʻā]]. Both names come from Hawaiian.\n\n\
         # Pāhoehoe\n\nLava with a smooth, billowy or ropy surface, formed when hot, fluid \
         [[Lava]] keeps a thin skin as\nit flows. Common at [[Kīlauea]]. Its rough counterpart \
         is [[ʻAʻā]].\n"
    );
    for e in files_ending(&workspace, "") {
        let text = std::fs::read_to_string(&e).unwrap_or_default();
        assert!(
            !text.contains("[[Pāhoehoe"),
            "{} still links to it",
            e.display()
        );
    }
    let trashed = system_trash(&home).join("Pāhoehoe.md");
    assert_eq!(std::fs::read_to_string(&trashed).unwrap(), source);
    assert_eq!(
        files_ending(&system_trash(&home), "").len(),
        1,
        "the undone merge took its copy back out"
    );
    assert!(!workspace.join(".trash").exists());
    assert!(
        in_real_trash(&dir).is_empty(),
        "the real trash is untouched"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_recipe_renames_a_tag_from_the_palette_and_undoes_it() {
    let dir = sandbox("recipe");
    let home = dir.join("home");
    let workspace = dir.join("example-workspace");
    let before: Vec<_> = (files_ending(&workspace, ".md").into_iter())
        .map(|p| (std::fs::read_to_string(&p).unwrap(), p))
        .collect();
    let out = drive_with("recipe.drive", &dir, &["--home".as_ref(), home.as_os_str()]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Applied, undone, then applied again: each note differs only in its tag.
    let mut renamed = 0;
    for (text, path) in before {
        let now = std::fs::read_to_string(&path).unwrap();
        if now != text {
            renamed += 1;
            assert_eq!(now, text.replacen("tags: [concept", "tags: [idea", 1));
        }
    }
    assert_eq!(renamed, 13);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tidy_lists_what_nothing_uses_and_deletes_it_undoably() {
    let dir = sandbox("tidy");
    let home = dir.join("home");
    let workspace = dir.join("example-workspace");
    std::fs::write(workspace.join("Inbox/Stray.md"), "nothing links here\n").unwrap();
    std::fs::write(workspace.join("Inbox/stray.png"), b"\x89PNG").unwrap();
    std::fs::create_dir_all(workspace.join("Empty/Deeper")).unwrap();
    let out = drive_with("tidy.drive", &dir, &["--home".as_ref(), home.as_os_str()]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!workspace.join("Inbox/Stray.md").exists());
    assert!(!workspace.join("Inbox/stray.png").exists());
    assert!(
        workspace.join("Empty/Deeper").is_dir(),
        "not deleted: never asked"
    );
    assert!(workspace.join("Inbox/Loose thought.md").exists());
    let files = system_trash(&home);
    let mut trashed = files_ending(&files, "");
    trashed.sort();
    assert_eq!(trashed, [files.join("Stray.md"), files.join("stray.png")]);
    assert_eq!(
        std::fs::read_to_string(files.join("Stray.md")).unwrap(),
        "nothing links here\n"
    );
    assert!(
        !workspace.join(".trash").exists(),
        "not the workspace's own trash"
    );
    assert!(
        in_real_trash(&dir).is_empty(),
        "the real trash is untouched"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Trash entries in the user's real system trash that came from `dir`: there must be none.
fn in_real_trash(dir: &Path) -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    let info = Path::new(&home).join(".local/share/Trash/info");
    let Ok(entries) = std::fs::read_dir(&info) else {
        return Vec::new();
    };
    let dir = dir.to_string_lossy().replace(' ', "%20");
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| std::fs::read_to_string(p).is_ok_and(|t| t.contains(&dir)))
        .collect()
}

#[test]
fn moves_tags_and_deletes_marked_notes() {
    let dir = sandbox("multiselect");
    let home = dir.join("home");
    let workspace = dir.join("example-workspace");
    std::fs::write(
        workspace.join("Inbox/Links.md"),
        "# Links\n[[People/Pliny the Younger]], [[Volcanoes/Vesuvius]] and \
         [Hotspot](../Concepts/Hotspot.md).\n",
    )
    .unwrap();
    let vesuvius = workspace.join("Volcanoes/Vesuvius.md");
    let text = std::fs::read_to_string(&vesuvius).unwrap();
    std::fs::write(
        &vesuvius,
        format!(
            "{text}\nSee [Pliny](../People/Pliny%20the%20Younger.md) and [[Concepts/Hotspot]].\n"
        ),
    )
    .unwrap();
    let before = unresolved(&workspace);
    let out = drive_with(
        "multiselect.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        unresolved(&workspace),
        before,
        "every link still leads where it did"
    );
    let archive = workspace.join("Archive");
    for gone in [
        "People/Pliny the Younger.md",
        "Volcanoes/Vesuvius.md",
        "Concepts/Hotspot.md",
    ] {
        assert!(!workspace.join(gone).exists(), "{gone} moved");
    }
    let read = |p: PathBuf| std::fs::read_to_string(p).unwrap();
    assert_eq!(
        read(workspace.join("Inbox/Links.md")),
        "# Links\n[[Archive/Pliny the Younger]], [[Archive/Vesuvius]] and \
         [Hotspot](../Archive/Hotspot.md).\n"
    );
    assert!(
        read(archive.join("Vesuvius.md"))
            .ends_with("\nSee [Pliny](Pliny%20the%20Younger.md) and [[Archive/Hotspot]].\n"),
        "links between moved notes"
    );
    let hotspot = read(archive.join("Hotspot.md"));
    assert!(
        hotspot.starts_with("---\ntags: [concept, review, person]\n---\n# Hotspot\n"),
        "{hotspot}"
    );
    let loose = read(workspace.join("Inbox/Loose thought.md"));
    assert!(
        loose.starts_with("---\ntags: [review, person]\n---\nIs there a volcano on Mars"),
        "a new tags list is written as the workspace's are: {loose}"
    );
    assert!(
        files_ending(&system_trash(&home), "").is_empty(),
        "undo took both out of the trash"
    );
    assert!(!workspace.join(".trash").exists());
    assert_eq!(
        in_real_trash(&dir),
        Vec::<PathBuf>::new(),
        "the real trash is untouched"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn creates_a_note_from_a_template() {
    let dir = sandbox("template");
    let workspace = dir.join("example-workspace");
    let out = drive("template.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let today = today();
    let etna = std::fs::read_to_string(workspace.join("Volcanoes/Etna.md")).unwrap();
    assert!(etna.starts_with("---\ntags: [volcano]\n"), "{etna}");
    assert!(etna.contains(&format!("\nadded: {today}\n")), "{etna}");
    assert!(etna.contains("\n# Etna\n"), "{etna}");
    assert!(!etna.contains("{{"), "{etna}");
    assert_eq!(
        std::fs::read_to_string(workspace.join("Volcanoes/Plain.md")).unwrap(),
        "# Plain\n\n"
    );
    let items = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .args(["items".as_ref(), workspace.as_os_str(), "volcano".as_ref()])
        .output()
        .unwrap();
    let items = String::from_utf8_lossy(&items.stdout);
    assert!(
        items.contains("Volcanoes/Etna.md:1  volcano  name Etna"),
        "{items}"
    );
    assert!(
        !items.contains("Templates/"),
        "a template isn't an item: {items}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Today's date as YYYY-MM-DD, from `date`, the way the program's local clock reads it.
fn today() -> String {
    let out = Command::new("date").arg("+%Y-%m-%d").output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn the_graph_answers_keys_and_the_mouse_and_follows_the_workspace() {
    let dir = sandbox("graph");
    let out = drive("graph.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let workspace = dir.join("example-workspace");
    let kilauea = std::fs::read_to_string(workspace.join("Volcanoes/Kīlauea.md")).unwrap();
    assert!(
        kilauea.ends_with("\n[[Ghostnote]]\n"),
        "the editor got the selected note"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_whiteboard_moves_edits_and_merges_cards() {
    let dir = sandbox("board");
    let workspace = dir.join("example-workspace");
    std::fs::write(
        workspace.join("Inbox/Pin.canvas"),
        "{\"nodes\":[\n\
         {\"id\":\"a\",\"type\":\"text\",\"text\":\"Alpha\",\"x\":0,\"y\":0,\"width\":200,\"height\":100},\n\
         {\"id\":\"b\",\"type\":\"text\",\"text\":\"Bravo\",\"x\":300,\"y\":0,\"width\":200,\"height\":100}\n\
         ],\"edges\":[]}\n",
    )
    .unwrap();
    let out = drive("board.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(workspace.join("Inbox/Pin.canvas")).unwrap();
    let canvas: serde_json::Value = serde_json::from_str(&text).unwrap();
    let nodes = canvas["nodes"].as_array().unwrap();
    let node = |text: &str| {
        (nodes.iter())
            .find(|n| n["text"] == text)
            .unwrap_or_else(|| panic!("no {text:?} card in {text}"))
    };
    assert_eq!(nodes.len(), 4, "{text}");
    assert_eq!(node("Alpha")["x"], 0);
    assert!(node("Alpha")["y"].as_f64().unwrap() > 0.0, "dragged down");
    assert_eq!(node("Bravo, checked")["id"], "b", "edited in place");
    assert_eq!(node("Charlie")["id"], "c", "the outside change merged");
    assert_eq!(node("Delta")["type"], "text");
    assert!(
        text.starts_with("{\n\t\"nodes\": ["),
        "saved as cold-lava saves"
    );
    let _ = std::fs::remove_dir_all(dir);
}

fn placed(nodes: &serde_json::Value, id: &str) -> serde_json::Value {
    (nodes.as_array().unwrap().iter())
        .find(|n| n["id"] == id)
        .unwrap_or_else(|| panic!("no node {id} in {nodes}"))
        .clone()
}

#[test]
fn placed_shapes_show_their_data_and_stretch_turn_and_move() {
    let dir = sandbox("placed-shapes");
    let workspace = dir.join("example-workspace");
    let file = workspace.join("Inbox/Shapes.canvas");
    std::fs::write(&file, include_str!("drive/placed-shapes.canvas")).unwrap();
    let out = drive("placed-shapes.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&file).unwrap();
    let canvas: serde_json::Value = serde_json::from_str(&text).unwrap();
    let venn = placed(&canvas["nodes"], "venn");
    let field = &venn["cold-lava"];
    assert_eq!(field["shape"], "venn2", "{text}");
    assert_eq!(field["rotation"], 105.0, "{text}");
    assert_eq!(
        field["inputs"]["a"]["query"]["filter"]["tag"], "eruption",
        "still bound: {text}"
    );
    assert!(venn["width"].as_i64().unwrap() < 600, "stretched: {text}");
    assert!(venn["y"].as_i64().unwrap() > 0, "moved down: {text}");
    let exact = field["exact"].as_array().expect("the fractions kept");
    assert_eq!(
        exact[2].as_f64().unwrap().round(),
        venn["width"].as_f64().unwrap()
    );
    assert_eq!(
        placed(&canvas["nodes"], "card")["file"],
        "Volcanoes/Krakatoa.md"
    );
    assert_eq!(canvas["cold-lava-version"], 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn placed_shapes_survive_obsidians_saves_and_are_kept_when_a_save_drops_them() {
    let dir = sandbox("placed-guard");
    let workspace = dir.join("example-workspace");
    std::fs::write(
        workspace.join("Inbox/Shapes.canvas"),
        "{\"nodes\":[\n\
         {\"id\":\"venn\",\"type\":\"text\",\"text\":\"Simple Venn diagram\",\"x\":0.4,\"y\":0,\
         \"width\":300.5,\"height\":300,\"cold-lava\":{\"shape\":\"venn2\",\"rotation\":15,\
         \"exact\":[0.4,0,300.5,300]}},\n\
         {\"id\":\"plain\",\"type\":\"text\",\"text\":\"Plain\",\"x\":400,\"y\":0,\"width\":200,\"height\":100}\n\
         ],\"edges\":[],\"cold-lava-version\":1}\n",
    )
    .unwrap();
    let out = drive("placed-guard.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(workspace.join("Inbox/Shapes.canvas")).unwrap();
    let canvas: serde_json::Value = serde_json::from_str(&text).unwrap();
    let venn = (canvas["nodes"].as_array().unwrap().iter())
        .find(|n| n["id"] == "venn")
        .unwrap();
    assert_eq!(venn["cold-lava"]["shape"], "venn2", "{text}");
    assert_eq!(venn["cold-lava"]["rotation"], 15, "{text}");
    assert_eq!(venn["x"], 50, "the outside move is kept: {text}");
    assert_eq!(canvas["cold-lava-version"], 1, "{text}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn opens_a_note_follows_a_link_and_quits() {
    let dir = sandbox("follow-link");
    let out = drive("follow-link.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn follows_a_relative_link_to_the_note_its_path_names() {
    let dir = sandbox("relative-link");
    let workspace = dir.join("example-workspace");
    std::fs::create_dir_all(workspace.join("Volcanoes/Field")).unwrap();
    std::fs::write(
        workspace.join("Volcanoes/Lava.md"),
        "A lava note beside the volcanoes.\n",
    )
    .unwrap();
    std::fs::write(
        workspace.join("Volcanoes/Field/Field notes.md"),
        "See [[../Lava]], not Concepts/Lava.\n",
    )
    .unwrap();
    let out = drive("relative-link.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn new_text_in_the_app_follows_the_workspaces_style() {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("drive-style-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let workspace = dir.join("dataview-notes");
    for (rel, text) in [
        (
            "Lava.md",
            "---\ntitle: Lava\ntags:\n- \"#rock\"\ncreated: 08/11/2026\n---\n\
             * [ ] Sample the flow [due:: 2026-11-12]\n* See [Basalt](Basalt.md)\n",
        ),
        (
            "Basalt.md",
            "---\ntitle: Basalt\ntags:\n- \"#rock\"\ncreated: 31/10/2026\n---\n\
             * [x] Name it [completion:: 2026-11-01]\n* Back to [Lava](Lava.md)\n",
        ),
        ("Inbox.md", "* [ ] Call the lab\n* [ ] Wash the samples\n"),
        (
            "Pumice.md",
            "---\ntitle: Pumice\ncreated: 03/11/2026\n---\nLight.\n",
        ),
    ] {
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join(rel), text).unwrap();
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/drive/style.drive");
    let out = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .args(["drive".as_ref(), script.as_os_str(), "--".as_ref()])
        .arg(&workspace)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let read = |rel: &str| std::fs::read_to_string(workspace.join(rel)).unwrap();
    assert_eq!(
        read("Inbox.md"),
        "* [x] Call the lab [completion:: 2026-11-09]\n\
         * [ ] Wash the samples [due:: 2026-12-01]\n"
    );
    assert_eq!(
        read("Pumice.md"),
        "---\ntitle: Pumice\ntags:\n- \"#rock\"\ncreated: 03/11/2026\n---\nLight.\n"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tables_a_types_items_sorted_and_filtered() {
    let dir = sandbox("table");
    std::fs::write(
        dir.join("example-workspace/Inbox/Today.md"),
        "- [ ] Ring the observatory ⏫\n- [x] Read about [[Lava]] ✅ 2026-11-05\n",
    )
    .unwrap();
    let out = drive("table.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn edits_table_cells_in_their_notes() {
    let dir = sandbox("table-edit");
    let workspace = dir.join("example-workspace");
    let out = drive("table-edit.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let vei =
        std::fs::read_to_string(workspace.join("Concepts/Volcanic Explosivity Index.md")).unwrap();
    assert!(
        vei.contains("\n| 7 | [[Krakatoa]] 1883, Sunda Strait |\n"),
        "{vei}"
    );
    assert!(vei.contains("\n| 8 | [[Yellowstone]] (see [[Supervolcano]]) |\n"));
    let journal = std::fs::read_to_string(workspace.join("Journal/2026-11-05.md")).unwrap();
    assert!(
        journal.contains(
            "\n- [x] Write a note on lahars ([[Lahar]] is still unresolved) 📅 2026-12-01\n"
        ),
        "the workspace writes emoji, so the new date is one: {journal}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_agenda_lists_tasks_by_day_and_ticks_them() {
    let dir = sandbox("agenda");
    let workspace = dir.join("example-workspace");
    std::fs::write(
        workspace.join("Inbox/Errands.md"),
        "- [ ] Pay the observatory bill ⏫ 📅 2026-11-07\n- [ ] Sort the rock photos\n",
    )
    .unwrap();
    let out = drive("agenda.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let journal = std::fs::read_to_string(workspace.join("Journal/2026-11-05.md")).unwrap();
    assert!(
        journal.ends_with(
            "\n- [ ] Write a note on lahars ([[Lahar]] is still unresolved)\n\
             - [x] Add the [[Volcanic Explosivity Index]] table ✅ 2026-11-05\n\
             - [ ] Look up the 2022 eruption of [[Mauna Loa]] 📅 2026-11-12\n\
             - [ ] Review the eruption timeline board 🔁 every week ⏳ 2026-11-16\n\
             - [x] Review the eruption timeline board 🔁 every week ⏳ 2026-11-09 ✅ 2026-11-07\n"
        ),
        "ticked in the agenda, unticked in the reader; the next occurrence above: {journal}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ticks_tasks_in_the_reader() {
    let dir = sandbox("reader-tick");
    let out = drive("reader-tick.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let journal =
        std::fs::read_to_string(dir.join("example-workspace/Journal/2026-11-05.md")).unwrap();
    assert!(
        journal.ends_with(
            "\n- [ ] Write a note on lahars ([[Lahar]] is still unresolved)\n\
             - [x] Add the [[Volcanic Explosivity Index]] table ✅ 2026-11-05\n\
             - [ ] Look up the 2022 eruption of [[Mauna Loa]] 📅 2026-11-12\n\
             - [ ] Review the eruption timeline board 🔁 every week ⏳ 2026-11-16\n\
             - [x] Review the eruption timeline board 🔁 every week ⏳ 2026-11-09 ✅ 2026-11-07\n"
        ),
        "{journal}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn draws_typed_values_with_their_looks() {
    let dir = sandbox("looks");
    std::fs::write(
        dir.join("example-workspace/Inbox/Today.md"),
        "- [ ] Ring the observatory ⏫\n- [x] Read about [[Lava]] ✅ 2026-11-05\n",
    )
    .unwrap();
    let out = drive("looks.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn peeks_at_a_link_without_going_there() {
    let dir = sandbox("peek");
    let out = drive("peek.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Every file under `dir` whose name ends in `suffix`.
fn files_ending(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_ending(&path, suffix));
        } else if path.to_string_lossy().ends_with(suffix) {
            found.push(path);
        }
    }
    found
}

#[test]
fn follows_attachment_links_without_creating_notes_for_them() {
    let dir = sandbox("attachments");
    std::fs::write(
        dir.join("example-workspace/Inbox/Field trip.md"),
        "# Field trip\n\nBring [[trail map.pdf]] and the [ticket](tickets/ticket.pdf). \
         Plan it on [[Welcome.canvas]]; read up on [[Mount St. Helens]].\n",
    )
    .unwrap();
    let out = drive("attachments.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut created = files_ending(&dir, ".pdf.md");
    created.extend(files_ending(&dir, ".canvas.md"));
    assert!(created.is_empty(), "created {created:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn which_key_lists_and_runs_each_screens_commands() {
    let dir = sandbox("which-key");
    let out = drive("which-key.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn goes_back_and_forward_through_notes_and_a_search() {
    let dir = sandbox("history");
    let out = drive("history.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn goes_back_and_forward_between_a_note_the_graph_and_a_board() {
    let dir = sandbox("history-screens");
    let out = drive("history-screens.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn follows_links_and_backlinks_by_their_hints() {
    let dir = sandbox("hints");
    let out = drive("hints.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn jumps_through_the_outline_and_heading_by_heading() {
    let dir = sandbox("outline");
    let out = drive("outline.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bookmarks_are_obsidians_and_follow_a_rename() {
    let dir = sandbox("bookmarks");
    let file = dir.join("example-workspace/.obsidian/bookmarks.json");
    let theirs = std::fs::read_to_string(&file).unwrap().replacen(
        "  \"items\": [\n",
        "  \"items\": [\n    {\n      \"type\": \"graph\",\n      \"ctime\": 1,\n      \"title\": \"Volcano graph\",\n      \"options\": {\n        \"search\": \"path:Volcanoes\",\n        \"showTags\": false\n      }\n    },\n    {\n      \"type\": \"workspace-tab\",\n      \"ctime\": 2,\n      \"zeta\": [\n        1\n      ],\n      \"alpha\": \"kept\"\n    },\n",
        1,
    );
    std::fs::write(&file, &theirs).unwrap();
    let out = drive("bookmarks.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let ours = std::fs::read_to_string(&file).unwrap();
    let unknown = &theirs[..theirs.find("    {\n      \"type\": \"file\"").unwrap()];
    assert!(
        ours.starts_with(unknown),
        "items it doesn't know, kept as they were:\n{ours}"
    );
    let json: serde_json::Value = serde_json::from_str(&ours).unwrap();
    let items = json["items"].as_array().unwrap();
    let file_at = |path: &str, subpath: Option<&str>| {
        items
            .iter()
            .any(|i| i["type"] == "file" && i["path"] == path && i["subpath"].as_str() == subpath)
    };
    assert!(file_at("Volcanoes/Krakatau.md", None), "{ours}");
    assert!(
        !ours.contains("Krakatoa.md"),
        "the bookmark followed the rename"
    );
    assert!(file_at("Concepts/Lava.md", Some("#Temperature")));
    assert!(
        file_at("Concepts/Lava.md", Some("#Textures")),
        "opening it kept it"
    );
    assert!(
        items
            .iter()
            .any(|i| i["type"] == "search" && i["query"] == "magma")
    );
    assert!(items.iter().all(|i| i["ctime"].is_u64()));
    assert_eq!(items.len(), 9, "the group still holds its three");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_false_expect_fails_with_its_line_and_the_screen() {
    let dir = sandbox("fails");
    let out = drive("fails.drive", &dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(
        stdout.contains("fails.drive:3: expected \"Atlantis\" on the screen"),
        "{stdout}"
    );
    assert!(
        stdout.contains("example-workspace 35 notes"),
        "the screen: {stdout}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A key typed right after Esc can arrive with it and read as Alt plus that key on a slow
/// machine, so each script waits for the Esc's effect on the screen before the next key.
#[test]
fn scripts_wait_after_esc_before_the_next_key() {
    let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/drive");
    let mut hurried = Vec::new();
    for entry in std::fs::read_dir(&scripts).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut after_esc = false;
        for (i, line) in text.lines().enumerate() {
            let words: Vec<&str> = line.split_whitespace().collect();
            let hurries = match words.first() {
                Some(&"keys") => {
                    let in_step = words.windows(2).any(|k| k[0] == "esc" && k[1] != "esc");
                    let next = after_esc && words.get(1) != Some(&"esc");
                    after_esc = words.last() == Some(&"esc");
                    in_step || next
                }
                Some(&("type" | "click" | "drag" | "scroll")) => std::mem::take(&mut after_esc),
                Some(&("wait-for" | "wait-gone" | "wait" | "exits")) => {
                    after_esc = false;
                    false
                }
                _ => false,
            };
            if hurries {
                hurried.push(format!("{}:{}: {line}", path.display(), i + 1));
            }
        }
    }
    assert_eq!(hurried, Vec::<String>::new());
}

/// Needs Neovim, which CI doesn't have; runs wherever `nvim` is on PATH.
#[test]
fn restarts_the_embedded_neovim() {
    let has_nvim = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("nvim").is_file()));
    if !has_nvim {
        eprintln!("skipped: no nvim on PATH");
        return;
    }
    let dir = sandbox("nvim-restart");
    let out = drive("nvim-restart.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Needs Neovim, which CI doesn't have; runs wherever `nvim` is on PATH.
#[test]
fn deleting_the_note_open_in_neovim_doesnt_bring_it_back() {
    let has_nvim = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("nvim").is_file()));
    if !has_nvim {
        eprintln!("skipped: no nvim on PATH");
        return;
    }
    let dir = sandbox("delete-nvim");
    let home = dir.join("home");
    let out = drive_with(
        "delete-nvim.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.join("example-workspace/Concepts/Lava.md").exists());
    assert!(system_trash(&home).join("Lava.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}

fn has_python() -> bool {
    let python = Command::new("python3").arg("--version").output();
    python.is_ok_and(|o| o.status.success())
}

/// The example extension installed in `home`, and an overdue task in the workspace for it.
fn install_open_tasks(dir: &Path, home: &Path) {
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/open-tasks"),
        &home.join(".config/cold-lava/extensions/open-tasks"),
    );
    let late = "- [ ] Check the seismograph 📅 2026-11-01\n";
    std::fs::write(dir.join("example-workspace/Inbox/Seismograph.md"), late).unwrap();
}

/// Needs python3, which CI's runners have.
#[test]
fn the_python_example_extension_counts_and_moves_tasks() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("open-tasks");
    let home = dir.join("home");
    install_open_tasks(&dir, &home);
    let out = drive_with(
        "open-tasks.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let moved = std::fs::read_to_string(dir.join("example-workspace/Inbox/Seismograph.md"));
    assert_eq!(
        moved.unwrap(),
        "- [ ] Check the seismograph 📅 2026-11-07\n"
    );
    let log = home.join(".cache/cold-lava/extensions/open-tasks.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Needs python3, which CI's runners have.
#[test]
fn extensions_are_answered_while_the_graph_is_open() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("graph-extensions");
    let home = dir.join("home");
    install_open_tasks(&dir, &home);
    let out = drive_with(
        "graph-extensions.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Needs python3, which CI's runners have.
#[test]
fn the_python_example_extension_lists_tasks_in_a_view_of_its_own() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("open-tasks-list");
    let home = dir.join("home");
    install_open_tasks(&dir, &home);
    let out = drive_with(
        "open-tasks-list.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let ticked = std::fs::read_to_string(dir.join("example-workspace/Inbox/Seismograph.md"));
    assert_eq!(
        ticked.unwrap(),
        "- [x] Check the seismograph 📅 2026-11-01 ✅ 2026-11-07\n",
        "ticked in the extension's list"
    );
    let log = home.join(".cache/cold-lava/extensions/open-tasks.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Needs python3, which CI's runners have.
#[test]
fn the_python_example_extension_opens_edits_and_makes_csv_files() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-table");
    let home = dir.join("home");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/csv-table"),
        &home.join(".config/cold-lava/extensions/csv-table"),
    );
    let out = drive_with(
        "csv-table.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let workspace = dir.join("example-workspace");
    let eruptions = std::fs::read_to_string(workspace.join("Volcanoes/Eruptions.csv"));
    assert_eq!(
        eruptions.unwrap(),
        "Volcano,Year,VEI,Notes\n[[Vesuvius]],79,5,\n\
         [[Krakatoa]],1884,6,- [ ] Read the Royal Society report on [[Krakatoa]] 📅 2026-11-20\n\
         [[Mount St. Helens]],1980,5,\n[[Eyjafjallajökull]],2010,4,\n[[Tambora]],1815,7,\n",
        "both edits saved, on top of the change made outside"
    );
    let made = std::fs::read_to_string(workspace.join("Untitled.csv"));
    assert_eq!(made.unwrap(), "name,notes\n,\n");
    let log = home.join(".cache/cold-lava/extensions/csv-table.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_embedded_png_is_drawn_in_half_blocks_in_the_reader() {
    let dir = sandbox("image-embed");
    let mut lake = tiny_skia::Pixmap::new(8, 4).unwrap();
    lake.fill(tiny_skia::Color::from_rgba8(0, 0, 255, 255));
    let mut red = tiny_skia::Paint::default();
    red.set_color_rgba8(255, 0, 0, 255);
    let top = tiny_skia::Rect::from_xywh(0.0, 0.0, 8.0, 2.0).unwrap();
    lake.fill_rect(top, &red, tiny_skia::Transform::identity(), None);
    let png = dir.join("example-workspace/Volcanoes/lava-lake.png");
    std::fs::write(png, lake.encode_png().unwrap()).unwrap();
    let out = drive("image-embed.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Needs python3, which CI's runners have.
#[test]
fn an_embedded_csv_shows_its_owners_preview_in_the_reader() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-preview");
    let home = dir.join("home");
    install_csv_table(&home, |main| main);
    let out = drive_with(
        "csv-preview.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = home.join(".cache/cold-lava/extensions/csv-table.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Needs python3, which CI's runners have.
#[test]
fn the_kanban_example_shows_a_markdown_board_that_stays_markdown() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("kanban-board");
    let home = dir.join("home");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/kanban-board"),
        &home.join(".config/cold-lava/extensions/kanban-board"),
    );
    let out = drive_with(
        "kanban-board.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let board = std::fs::read_to_string(dir.join("example-workspace/Inbox/Field Season.md"));
    let before = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace/Inbox/Field Season.md"),
    );
    assert_eq!(
        board.unwrap(),
        before.unwrap().replace(
            "- [ ] Pack the gas masks\n",
            "- [x] Pack the gas masks ✅ 2026-11-07\n"
        ),
        "the card ticked by Markdown's own edit, the rest of the board as it was"
    );
    let log = home.join(".cache/cold-lava/extensions/kanban-board.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_csv_files_rows_links_and_tasks_join_the_workspace() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-items");
    let home = dir.join("home");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/csv-table"),
        &home.join(".config/cold-lava/extensions/csv-table"),
    );
    let out = drive_with(
        "csv-items.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let cached = std::fs::read_dir(home.join(".cache/cold-lava/owners"))
        .unwrap()
        .count();
    assert_eq!(cached, 1, "what csv-table gave is kept in the drive's home");
    let log = home.join(".cache/cold-lava/extensions/csv-table.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_owned_files_view_lists_who_links_to_it() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-backlinks");
    let home = dir.join("home");
    install_csv_table(&home, |main| main);
    let out = drive_with(
        "csv-backlinks.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let log = home.join(".cache/cold-lava/extensions/csv-table.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn csv_tables_type_ships_with_it_for_rows_no_other_type_matches() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-types");
    let home = dir.join("home");
    install_csv_table(&home, |main| main);
    let out = drive_with(
        "csv-types.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let csv = dir.join("example-workspace/Volcanoes/Observatories.csv");
    assert_eq!(
        std::fs::read_to_string(csv).unwrap(),
        "Observatory,Volcano,Founded\nOsservatorio Vesuviano,[[Vesuvius]],1841\n\
         Hawaiian Volcano Observatory,[[Kīlauea]],1912\n\
         Cascades Volcano Observatory,[[Mount St. Helens]],1980\n",
        "the row's name set through csv-table"
    );
    let log = home.join(".cache/cold-lava/extensions/csv-table.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// csv-table from the examples, installed in the drive's home; `patch` changes its `main.py`.
fn install_csv_table(home: &Path, patch: impl Fn(String) -> String) {
    let installed = home.join(".config/cold-lava/extensions/csv-table");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/extensions/csv-table"),
        &installed,
    );
    let main = std::fs::read_to_string(installed.join("main.py")).unwrap();
    std::fs::write(installed.join("main.py"), patch(main)).unwrap();
}

#[test]
fn items_and_links_in_a_csv_file_are_edited_through_csv_table() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-edit");
    let home = dir.join("home");
    install_csv_table(&home, |main| main);
    let out = drive_with(
        "csv-edit.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let workspace = dir.join("example-workspace");
    assert_eq!(
        std::fs::read_to_string(workspace.join("Volcanoes/Eruptions.csv")).unwrap(),
        "Volcano,Year,VEI,Notes\n[[Vesuvius]],79,5,\n\
         [[Krakatau]],1883,7,- [x] Read the Royal Society report on [[Krakatau]] 📅 2026-11-20 \
         ✅ 2026-11-07\n[[Mount St. Helens]],1980,5,\n[[Eyjafjallajökull]],2010,4,\n",
        "ticked in the agenda, set in the table, relinked by the rename"
    );
    assert!(workspace.join("Volcanoes/Krakatau.md").is_file());
    let log = home.join(".cache/cold-lava/extensions/csv-table.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_owner_that_refuses_or_doesnt_answer_an_edit_leaves_its_file_alone() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-edit-refused");
    let home = dir.join("home");
    let stubborn = r#"import time


@ext.on("file/edit")
def stubborn(params):
    if any("| 9 |" in c.get("text", "") for c in params["changes"]):
        time.sleep(30)
    raise ValueError("this copy is read-only")


ext.run()"#;
    install_csv_table(&home, |main| main.replace("ext.run()", stubborn));
    let csv = dir.join("example-workspace/Volcanoes/Eruptions.csv");
    let before = std::fs::read(&csv).unwrap();
    let out = drive_with(
        "csv-edit-refused.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(&csv).unwrap(), before, "nothing written");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_merge_relinks_a_csv_file_through_csv_table() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-merge");
    let home = dir.join("home");
    install_csv_table(&home, |main| main);
    let out = drive_with(
        "csv-merge.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let workspace = dir.join("example-workspace");
    assert_eq!(
        std::fs::read_to_string(workspace.join("Volcanoes/Eruptions.csv")).unwrap(),
        "Volcano,Year,VEI,Notes\n[[Mauna Loa]],79,5,\n\
         [[Krakatoa]],1883,6,- [ ] Read the Royal Society report on [[Krakatoa]] 📅 2026-11-20\n\
         [[Mount St. Helens]],1980,5,\n[[Eyjafjallajökull]],2010,4,\n",
        "Vesuvius's row leads to Mauna Loa"
    );
    let observatories = std::fs::read_to_string(workspace.join("Volcanoes/Observatories.csv"));
    assert!(
        observatories
            .unwrap()
            .contains("Vesuvius Observatory,[[Mauna Loa]],1841\n"),
        "and so does the other CSV's"
    );
    assert!(!workspace.join("Volcanoes/Vesuvius.md").exists());
    let log = home.join(".cache/cold-lava/extensions/csv-table.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "",
        "nothing on stderr"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Every file under `dir` with its bytes, by path.
fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        match path.is_dir() {
            true => out.extend(files_under(&path)),
            false => out.push((path.clone(), std::fs::read(&path).unwrap())),
        }
    }
    out.sort();
    out
}

#[test]
fn a_rename_or_merge_its_owner_refuses_or_doesnt_answer_changes_nothing() {
    if !has_python() {
        eprintln!("skipped: python3 isn't on PATH, and the example extension needs it");
        return;
    }
    let dir = sandbox("csv-rename-refused");
    let home = dir.join("home");
    let stubborn = r#"import json
import time


@ext.on("file/edit")
def stubborn(params):
    asked = json.dumps(params["changes"])
    if "Rakata" in asked:
        time.sleep(30)
    if "Krakatau" in asked or "Mount Fuji" in asked:
        raise ValueError("this copy keeps the old names")
    return file_edit(params)


ext.run()"#;
    install_csv_table(&home, |main| {
        assert!(main.contains("def file_edit(params)"));
        main.replace("ext.run()", stubborn)
    });
    let workspace = dir.join("example-workspace");
    let before = files_under(&workspace);
    let out = drive_with(
        "csv-rename-refused.drive",
        &dir,
        &["--home".as_ref(), home.as_os_str()],
    );
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        files_under(&workspace) == before,
        "every file of the workspace is as it was"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ext_list_prints_what_each_extension_may_do() {
    let dir = sandbox("ext-list");
    let home = dir.join("home");
    install_open_tasks(&dir, &home);
    let broken = home.join(".config/cold-lava/extensions/broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("extension.toml"), "name = \"broken\"\n").unwrap();
    let carried = dir.join("example-workspace/.cold-lava/extensions/carried");
    std::fs::create_dir_all(&carried).unwrap();
    let manifest = "name = \"carried\"\nversion = \"1\"\nrun = [\"sh\"]\n";
    std::fs::write(carried.join("extension.toml"), manifest).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .args(["ext", "list"])
        .arg(dir.join("example-workspace"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("COLD_LAVA_EDITOR", "none")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let installed = home.join(".config/cold-lava/extensions/open-tasks");
    let log = home.join(".cache/cold-lava/extensions/open-tasks.log");
    let expected = format!(
        "agenda  built in\n\
         \x20 The workspace's tasks by day\n\
         \x20 permissions: items.read, items.edit\n\
         table  built in\n\
         \x20 A type's items as a table\n\
         \x20 permissions: items.read, items.edit\n\
         graph  built in\n\
         \x20 The workspace's notes and the links between them\n\
         \x20 permissions: items.read\n\
         open-tasks 0.1.0  {}\n\
         \x20 Counts the workspace's open tasks in the status line, moves overdue ones to today, \
         and lists them by note\n\
         \x20 runs: python3 main.py (stdio)\n\
         \x20 permissions: items.read, items.edit\n\
         \x20 command: open-tasks.count  say how many tasks are open, and how many are overdue\n\
         \x20 command: open-tasks.reschedule  move every overdue open task to today\n\
         \x20 command: open-tasks.list  list the open tasks by note\n\
         \x20 log: {}\n\
         carried  {}\n\
         \x20 not loaded: extensions inside a workspace aren't loaded yet; copy it to the \
         extensions folder in ~/.config/cold-lava to use it\n",
        installed.display(),
        log.display(),
        carried.display(),
    );
    assert_eq!(stdout, expected);
    assert!(stderr.starts_with("didn't load:\n"), "{stderr}");
    assert!(
        stderr.contains("broken/extension.toml: missing field `version`"),
        "{stderr}"
    );
    assert_eq!(out.status.code(), Some(1));
    let _ = std::fs::remove_dir_all(dir);
}
