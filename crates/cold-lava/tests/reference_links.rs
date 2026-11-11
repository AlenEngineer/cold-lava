//! Links by reference (`[text][label]` with `[label]: Note.md`) through the CLI: a rename
//! rewrites the definition, tidy counts the link, and a task's own tags and links are the ones
//! the syntax tree reads. Each test has its own workspace and an empty home.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("reference-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, text) in files {
        let p = dir.join("ws").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    std::fs::create_dir_all(dir.join("home")).unwrap();
    dir
}

fn cold_lava(dir: &Path, args: &[&str]) -> Output {
    let home = dir.join("home");
    let out = Command::new(env!("CARGO_BIN_EXE_cold-lava"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join("ws").join(rel)).unwrap()
}

const USES: &str = "# Uses\n\nSee [the other note][b] and ![its picture][pic].\n\n- [ ] **#urgent** ask [Bee][b] about [[Uses]]\n\n[b]: Notes/B.md \"The other\"\n[pic]: <Notes/b%20map.png>\n";

#[test]
fn a_rename_rewrites_the_definition_of_a_note_linked_only_by_reference() {
    let files = [
        ("Uses.md", USES),
        ("Notes/B.md", "# B\n"),
        ("Notes/b map.png", ""),
    ];
    let dir = workspace("rename", &files);
    let ws = dir.join("ws");
    let ws = ws.to_str().unwrap();
    let dry = cold_lava(&dir, &["rename", ws, "Notes/B", "C", "--dry-run"]);
    assert_eq!(
        String::from_utf8_lossy(&dry.stdout),
        "would rename Notes/B.md → Notes/C.md\n  Uses.md:7  [b]: Notes/C.md \"The other\"\n1 links in 1 files\n"
    );
    assert_eq!(read(&dir, "Uses.md"), USES, "a dry run writes nothing");

    cold_lava(&dir, &["rename", ws, "Notes/B", "C"]);
    assert_eq!(
        read(&dir, "Uses.md"),
        USES.replace("Notes/B.md", "Notes/C.md"),
        "the definition's destination, and no other byte"
    );
    assert!(dir.join("ws/Notes/C.md").exists() && !dir.join("ws/Notes/B.md").exists());

    // The folder moves: both definitions follow, each as it was written.
    cold_lava(&dir, &["rename", ws, "Notes", "Field Notes"]);
    assert_eq!(
        read(&dir, "Uses.md"),
        USES.replace("Notes/B.md", "Field%20Notes/C.md")
            .replace("Notes/b%20map.png", "Field%20Notes/b%20map.png")
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// #206: an aliased wikilink in a table (`\|`), a link with a title, a destination in `<>`.
const FORMS: &str = "# Forms\n\n| note | part |\n| --- | --- |\n| [[My Note\\|mine]] | ![[My Note#Part\\|part]] |\n\nTitles: [a](My%20Note.md \"double\"), [b](My%20Note.md 'single') and [c](My%20Note.md (parens)).\nBrackets: [d](<My Note.md>), [e](<My Note.md#Part> \"title\") and [f](My%20Note.md).\n";

/// `FORMS` once the note `My Note` is called `name`: a wikilink names it as it is, a Markdown
/// link with its spaces as `%20`, between `<>` too.
fn forms(name: &str) -> String {
    let encoded = name.replace(' ', "%20");
    FORMS
        .replace("[[My Note", &format!("[[{name}"))
        .replace("<My Note.md", &format!("<{encoded}.md"))
        .replace("My%20Note.md", &format!("{encoded}.md"))
}

#[test]
fn a_rename_rewrites_a_link_in_a_table_with_a_title_and_in_angle_brackets() {
    let files = [("Forms.md", FORMS), ("My Note.md", "# Part\n")];
    let dir = workspace("forms", &files);
    let ws = dir.join("ws");
    let ws = ws.to_str().unwrap();
    let dry = cold_lava(&dir, &["rename", ws, "My Note", "Your Note", "--dry-run"]);
    let dry = String::from_utf8_lossy(&dry.stdout);
    assert!(dry.ends_with("\n8 links in 1 files\n"), "{dry}");
    assert_eq!(read(&dir, "Forms.md"), FORMS, "a dry run writes nothing");

    cold_lava(&dir, &["rename", ws, "My Note", "Your Note"]);
    assert_eq!(
        read(&dir, "Forms.md"),
        forms("Your Note"),
        "each target, and no other byte"
    );
    // A name without spaces: the `<>` stay.
    cold_lava(&dir, &["rename", ws, "Your Note", "Plain"]);
    assert_eq!(read(&dir, "Forms.md"), forms("Plain"));
    // And with spaces again: `%20`, as a rename writes every Markdown link.
    cold_lava(&dir, &["rename", ws, "Plain", "Two Words"]);
    assert_eq!(read(&dir, "Forms.md"), forms("Two Words"));
    assert!(dir.join("ws/Two Words.md").exists() && !dir.join("ws/My Note.md").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tidy_counts_a_link_by_reference() {
    let files = [
        ("A.md", "See [the other][b].\n\n[b]: B.md\n[unused]: C.md\n"),
        ("B.md", "Back to [A].\n\n[a]: A.md\n"),
        ("C.md", "alone\n"),
    ];
    let dir = workspace("tidy", &files);
    let ws = dir.join("ws");
    let out = cold_lava(&dir, &["tidy", ws.to_str().unwrap(), "--json"]);
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let orphans: Vec<&str> = (report["orphans"].as_array().unwrap().iter())
        .map(|o| o["note"].as_str().unwrap())
        .collect();
    assert_eq!(
        orphans,
        ["C.md"],
        "A and B link each other by reference; a definition nothing uses is no link"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// The workspace has no `.obsidian`, so its flavour is `github`: a task's wikilinks count all
/// the same, as backlinks do.
#[test]
fn a_tasks_tags_and_links_are_what_the_tree_reads() {
    let dir = workspace("task", &[("Uses.md", USES), ("Notes/B.md", "# B\n")]);
    let ws = dir.join("ws");
    let out = cold_lava(&dir, &["items", ws.to_str().unwrap(), "task", "--json"]);
    let lines = String::from_utf8(out.stdout).unwrap();
    let tasks: Vec<serde_json::Value> = (lines.lines())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(tasks.len(), 1, "{lines}");
    assert_eq!(tasks[0]["tags"], serde_json::json!(["urgent"]));
    assert_eq!(tasks[0]["links"], serde_json::json!(["Uses", "Notes/B"]));
    let _ = std::fs::remove_dir_all(dir);
}
