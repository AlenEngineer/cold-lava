//! `cold-lava drive` against the built binary, on a copy of the example vault.

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

/// A fresh vault copy. `drive` gives the program an empty home itself.
fn sandbox(name: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("drive-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-vault"),
        &dir.join("example-vault"),
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
        .arg(dir.join("example-vault"))
        .output()
        .unwrap()
}

#[test]
fn deletes_a_note_to_the_system_trash_and_undoes_it() {
    let dir = sandbox("delete");
    let home = dir.join("home");
    let vault = dir.join("example-vault");
    let lava = std::fs::read(vault.join("Concepts/Lava.md")).unwrap();
    let out = drive_with("delete.drive", &dir, &["--home".as_ref(), home.as_os_str()]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!vault.join("Concepts/Lava.md").exists());
    assert!(!vault.join(".trash").exists(), "not the vault's own trash");
    let files = system_trash(&home);
    let mut trashed = files_ending(&files, "");
    trashed.sort();
    assert_eq!(
        trashed,
        [files.join("Lava.md"), files.join("Welcome.canvas")]
    );
    assert!(!vault.join("Welcome.canvas").exists());
    assert_eq!(std::fs::read(files.join("Lava.md")).unwrap(), lava);
    if cfg!(target_os = "macos") {
        let _ = std::fs::remove_dir_all(dir);
        return;
    }
    let trash = home.join(".local/share/Trash");
    let info = std::fs::read_to_string(trash.join("info/Lava.md.trashinfo")).unwrap();
    assert!(
        info.starts_with("[Trash Info]\nPath=/")
            && info.contains("/example-vault/Concepts/Lava.md\n"),
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
fn unresolved(vault: &Path) -> Vec<String> {
    use cold_lava_engine::{index::Index, rename::Files, vault};
    let ix = Index::build(vault::scan(vault).unwrap());
    let files = Files::of_vault(&ix);
    let mut out = Vec::new();
    for n in &ix.vault.notes {
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
    let vault = dir.join("example-vault");
    std::fs::write(
        vault.join("Inbox/Links.md"),
        "---\nrelated: \"[[People/Pliny the Younger]]\"\n---\n# Links\n\
         [[People/David A. Johnston]] and [Krafft](../People/Katia%20and%20Maurice%20Krafft.md) \
         and [[Pliny the Younger]].\n",
    )
    .unwrap();
    let canvas = "{\"nodes\":[\n\
        {\"id\":\"a\",\"type\":\"file\",\"file\":\"People/David A. Johnston.md\",\"x\":0,\"y\":0,\"width\":400,\"height\":400},\n\
        {\"id\":\"b\",\"type\":\"text\",\"text\":\"[[People/Pliny the Younger]]\",\"x\":500,\"y\":0,\"width\":400,\"height\":100}\n\
        ],\"edges\":[]}\n";
    std::fs::write(vault.join("Inbox/People.canvas"), canvas).unwrap();
    let before = unresolved(&vault);
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
    let after = unresolved(&vault);
    eprintln!(
        "unresolved links: {} before, {} after",
        before.len(),
        after.len()
    );
    assert_eq!(after, before, "every link still leads where it did");
    assert!(!vault.join("People").exists());
    let moved = vault.join("Guide/Volcanologists");
    assert!(moved.join("David A. Johnston.md").is_file());
    assert!(
        moved.join("Field Notes").is_dir(),
        "the new folder came back too"
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("Inbox/Links.md")).unwrap(),
        "---\nrelated: \"[[Guide/Volcanologists/Pliny the Younger]]\"\n---\n# Links\n\
         [[Guide/Volcanologists/David A. Johnston]] and \
         [Krafft](../Guide/Volcanologists/Katia%20and%20Maurice%20Krafft.md) \
         and [[Pliny the Younger]].\n"
    );
    assert_eq!(
        std::fs::read_to_string(vault.join("Inbox/People.canvas")).unwrap(),
        canvas.replace("People/", "Guide/Volcanologists/")
    );
    assert!(
        files_ending(&system_trash(&home), "").is_empty(),
        "undo took the folder out of the trash"
    );
    assert!(!vault.join(".trash").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn duplicates_a_note_and_merges_one_into_another_keeping_links() {
    let dir = sandbox("merge");
    let home = dir.join("home");
    let vault = dir.join("example-vault");
    let source = std::fs::read_to_string(vault.join("Concepts/Pāhoehoe.md")).unwrap();
    let before = unresolved(&vault);
    let out = drive_with("merge.drive", &dir, &["--home".as_ref(), home.as_os_str()]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        unresolved(&vault),
        before,
        "no link was left pointing nowhere"
    );
    let read = |rel: &str| std::fs::read_to_string(vault.join(rel)).unwrap();
    assert_eq!(read("Volcanoes/Kīlauea 1.md"), read("Volcanoes/Kīlauea.md"));
    assert!(!vault.join("Concepts/Pāhoehoe.md").exists());
    assert_eq!(
        read("Concepts/Lava.md"),
        "---\ntags: [concept, lava]\naliases:\n  - Pāhoehoe\n---\n# Lava\n\n\
         Molten rock that reaches the surface. Below ground it's called magma.\n\n\
         ## Temperature\nBasaltic lava, the runny kind at a [[Shield Volcano]], erupts at roughly \
         1,100–1,200 °C.\n\n## Textures\nWhen it cools, basaltic lava usually becomes either \
         smooth, ropy [[Lava]] or rough, jagged\n[[ʻAʻā]]. Both names come from Hawaiian.\n\n\
         # Pāhoehoe\n\nLava with a smooth, billowy or ropy surface, formed when hot, fluid \
         [[Lava]] keeps a thin skin as\nit flows. Common at [[Kīlauea]]. Its rough counterpart \
         is [[ʻAʻā]].\n"
    );
    for e in files_ending(&vault, "") {
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
    assert!(!vault.join(".trash").exists());
    assert!(
        in_real_trash(&dir).is_empty(),
        "the real trash is untouched"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tidy_lists_what_nothing_uses_and_deletes_it_undoably() {
    let dir = sandbox("tidy");
    let home = dir.join("home");
    let vault = dir.join("example-vault");
    std::fs::write(vault.join("Inbox/Stray.md"), "nothing links here\n").unwrap();
    std::fs::write(vault.join("Inbox/stray.png"), b"\x89PNG").unwrap();
    std::fs::create_dir_all(vault.join("Empty/Deeper")).unwrap();
    let out = drive_with("tidy.drive", &dir, &["--home".as_ref(), home.as_os_str()]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!vault.join("Inbox/Stray.md").exists());
    assert!(!vault.join("Inbox/stray.png").exists());
    assert!(
        vault.join("Empty/Deeper").is_dir(),
        "not deleted: never asked"
    );
    assert!(vault.join("Inbox/Loose thought.md").exists());
    let files = system_trash(&home);
    let mut trashed = files_ending(&files, "");
    trashed.sort();
    assert_eq!(trashed, [files.join("Stray.md"), files.join("stray.png")]);
    assert_eq!(
        std::fs::read_to_string(files.join("Stray.md")).unwrap(),
        "nothing links here\n"
    );
    assert!(!vault.join(".trash").exists(), "not the vault's own trash");
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
    let vault = dir.join("example-vault");
    std::fs::write(
        vault.join("Inbox/Links.md"),
        "# Links\n[[People/Pliny the Younger]], [[Volcanoes/Vesuvius]] and \
         [Hotspot](../Concepts/Hotspot.md).\n",
    )
    .unwrap();
    let vesuvius = vault.join("Volcanoes/Vesuvius.md");
    let text = std::fs::read_to_string(&vesuvius).unwrap();
    std::fs::write(
        &vesuvius,
        format!(
            "{text}\nSee [Pliny](../People/Pliny%20the%20Younger.md) and [[Concepts/Hotspot]].\n"
        ),
    )
    .unwrap();
    let before = unresolved(&vault);
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
        unresolved(&vault),
        before,
        "every link still leads where it did"
    );
    let archive = vault.join("Archive");
    for gone in [
        "People/Pliny the Younger.md",
        "Volcanoes/Vesuvius.md",
        "Concepts/Hotspot.md",
    ] {
        assert!(!vault.join(gone).exists(), "{gone} moved");
    }
    let read = |p: PathBuf| std::fs::read_to_string(p).unwrap();
    assert_eq!(
        read(vault.join("Inbox/Links.md")),
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
    let loose = read(vault.join("Inbox/Loose thought.md"));
    assert!(
        loose.starts_with("---\ntags:\n  - review\n  - person\n---\nIs there a volcano on Mars"),
        "{loose}"
    );
    assert!(
        files_ending(&system_trash(&home), "").is_empty(),
        "undo took both out of the trash"
    );
    assert!(!vault.join(".trash").exists());
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
    let vault = dir.join("example-vault");
    let out = drive("template.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let today = today();
    let etna = std::fs::read_to_string(vault.join("Volcanoes/Etna.md")).unwrap();
    assert!(etna.starts_with("---\ntags: [volcano]\n"), "{etna}");
    assert!(etna.contains(&format!("\nadded: {today}\n")), "{etna}");
    assert!(etna.contains("\n# Etna\n"), "{etna}");
    assert!(!etna.contains("{{"), "{etna}");
    assert_eq!(
        std::fs::read_to_string(vault.join("Volcanoes/Plain.md")).unwrap(),
        "# Plain\n\n"
    );
    let items = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .args(["items".as_ref(), vault.as_os_str(), "volcano".as_ref()])
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
fn tables_a_types_items_sorted_and_filtered() {
    let dir = sandbox("table");
    std::fs::write(
        dir.join("example-vault/Inbox/Today.md"),
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
    let vault = dir.join("example-vault");
    let out = drive("table-edit.drive", &dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let vei =
        std::fs::read_to_string(vault.join("Concepts/Volcanic Explosivity Index.md")).unwrap();
    assert!(
        vei.contains("\n| 7 | [[Krakatoa]] 1883, Sunda Strait |\n"),
        "{vei}"
    );
    assert!(vei.contains("\n| 8 | [[Yellowstone]] (see [[Supervolcano]]) |\n"));
    let journal = std::fs::read_to_string(vault.join("Journal/2026-11-05.md")).unwrap();
    assert!(
        journal.contains(
            "\n- [x] Write a note on lahars ([[Lahar]] is still unresolved) 📅 2026-12-01\n"
        ),
        "the vault writes emoji, so the new date is one: {journal}"
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
    let journal = std::fs::read_to_string(dir.join("example-vault/Journal/2026-11-05.md")).unwrap();
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
        dir.join("example-vault/Inbox/Today.md"),
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
        dir.join("example-vault/Inbox/Field trip.md"),
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
    let file = dir.join("example-vault/.obsidian/bookmarks.json");
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
        stdout.contains("example-vault 34 notes"),
        "the screen: {stdout}"
    );
    let _ = std::fs::remove_dir_all(dir);
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
    assert!(!dir.join("example-vault/Concepts/Lava.md").exists());
    assert!(system_trash(&home).join("Lava.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}
