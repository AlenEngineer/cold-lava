//! Text from outside (a note's words, a card's, a file's name) never reaches the terminal as
//! written when it holds characters a terminal acts on: the built binary on a copy of the
//! example workspace with such text planted in it. What a listing shows and what a message
//! shows differ in one thing, the characters that turn the direction of text, and that is
//! pinned here per command.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A card's task with an escape sequence that would retitle the terminal, and a character
/// that turns the text after it right to left.
const CARD: &str = r#"{"nodes":[{"id":"n1","type":"text","x":0,"y":0,"width":300,"height":100,"text":"- [ ] card \u001b]0;PWNED-card\u0007 task \u202egnp.exe"}],"edges":[]}"#;

/// A note with the same in a task, a heading and beside a link that leads nowhere, and the
/// other characters that are never shown as written: DEL, one of C1, a line separator (which
/// an item's text holds as a space, and a line of the note as it is).
const NOTE: &str = "- [ ] note \u{1b}]0;PWNED-note\u{7} task \u{202e}gnp.exe 📅 2026-11-20\n\
    - [ ] csi \u{1b}[31mred\u{1b}[0m del\u{7f} c1\u{9b}2J sep\u{2028}arated\n\
    # Head \u{1b}]0;PWNED-head\u{7}\n\
    \n\
    A link to [[Nowhere]] \u{1b}]0;PWNED-link\u{7} here.\n";

/// A note that links to the planted one, on a line that holds both kinds of character.
const LINKER: &str = "See [[EscNote]] \u{1b}]0;PWNED-linker\u{7} and \u{202e}back.\n";

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

/// A copy of the example workspace with the planted files, and an empty home.
fn sandbox(name: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("outside-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let workspace = dir.join("example-workspace");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example-workspace"),
        &workspace,
    );
    std::fs::write(workspace.join("Evil.canvas"), CARD).unwrap();
    std::fs::write(workspace.join("EscNote.md"), NOTE).unwrap();
    std::fs::write(workspace.join("Linker.md"), LINKER).unwrap();
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    (workspace, home)
}

fn cold_lava(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cold-lava"));
    c.env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("COLD_LAVA_TODAY", "2026-11-07");
    c
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8(out.stdout.clone()).unwrap(),
        String::from_utf8(out.stderr.clone()).unwrap(),
    )
}

/// The characters of `text` that are never shown as written, whoever wrote them: the
/// control characters but the tab and the end of a line, and the two separators that end one.
fn acting(text: &str) -> Vec<char> {
    let acts = |c: &char| {
        (c.is_control() && !matches!(c, '\n' | '\t')) || matches!(c, '\u{2028}' | '\u{2029}')
    };
    text.chars().filter(acts).collect()
}

/// The characters of `text` that turn the direction of the text after them.
fn turning(text: &str) -> Vec<char> {
    let turns = |c: &char| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}');
    text.chars().filter(turns).collect()
}

/// Runs a command that succeeds; its stdout and stderr.
fn run(home: &Path, args: &[&str]) -> (String, String) {
    let out = cold_lava(home).args(args).output().unwrap();
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{args:?}\n{stdout}{stderr}");
    (stdout, stderr)
}

/// The commands whose lines are the user's own content. What a terminal would act on is
/// shown as `\u{…}`; a character that turns the text stays, as the user wrote it.
#[test]
fn a_listing_shows_what_a_terminal_would_act_on_and_keeps_the_rest() {
    let (workspace, home) = sandbox("listing");
    let ws = workspace.to_str().unwrap();
    let listings: [(&[&str], &[&str]); 7] = [
        (
            &["items", ws, "task"],
            &[
                "Evil.canvas:1  [ ] card \\u{1b}]0;PWNED-card\\u{7} task \u{202e}gnp.exe",
                "EscNote.md:1  [ ] note \\u{1b}]0;PWNED-note\\u{7} task \u{202e}gnp.exe  due 2026-11-20",
                "EscNote.md:2  [ ] csi \\u{1b}[31mred\\u{1b}[0m del\\u{7f} c1\\u{9b}2J sep arated",
            ],
        ),
        (
            &["items", ws, "heading"],
            &["EscNote.md:3  # Head \\u{1b}]0;PWNED-head\\u{7}"],
        ),
        (
            &["agenda", ws, "--from", "2026-11-01", "--to", "2026-11-30"],
            &["  [ ] note \\u{1b}]0;PWNED-note\\u{7} task \u{202e}gnp.exe   EscNote.md:1"],
        ),
        (
            &["table", ws, "task"],
            &["card \\u{1b}]0;PWNED-card\\u{7} task \u{202e}gnp.exe"],
        ),
        (
            &["search", ws, "csi"],
            &[
                "EscNote.md:2: - [ ] csi \\u{1b}[31mred\\u{1b}[0m del\\u{7f} c1\\u{9b}2J sep\\u{2028}arated",
            ],
        ),
        (
            &["tidy", ws, "--no-backlinks"],
            &["    EscNote.md:5  A link to [[Nowhere]] \\u{1b}]0;PWNED-link\\u{7} here."],
        ),
        (
            &[
                "shapes",
                ws,
                "--render",
                "venn2",
                "--size",
                "100x30",
                "--bind",
                "a=type=task",
            ],
            &["\\u{1b}]0"],
        ),
    ];
    for (args, expected) in listings {
        let (stdout, stderr) = run(&home, args);
        for shown in [&stdout, &stderr] {
            let acting = acting(shown);
            assert!(acting.is_empty(), "{args:?}: {acting:?} in\n{shown:?}");
        }
        for line in expected {
            assert!(stdout.contains(line), "{args:?}: no {line:?} in\n{stdout}");
        }
    }
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// A line that says what a command did or why it didn't is a message: there a character
/// that turns the text is shown as `\u{…}` too, on stdout and on stderr.
#[test]
fn a_message_shows_the_characters_that_turn_text_as_well() {
    let (workspace, home) = sandbox("message");
    let ws = workspace.to_str().unwrap();
    let task = format!("{}:1", workspace.join("EscNote.md").display());
    let messages: [(&[&str], &str); 3] = [
        (
            &["rename", ws, "EscNote", "Other", "--dry-run"],
            "  Linker.md:1  See [[Other]] \\u{1b}]0;PWNED-linker\\u{7} and \\u{202e}back.",
        ),
        (
            &["task", "toggle", &task],
            "\\u{1b}]0;PWNED-note\\u{7} task \\u{202e}gnp.exe",
        ),
        (
            &["rename", ws, "EscNote", "Other"],
            "  Linker.md:1  See [[Other]] \\u{1b}]0;PWNED-linker\\u{7} and \\u{202e}back.",
        ),
    ];
    for (args, expected) in messages {
        let (stdout, stderr) = run(&home, args);
        for shown in [&stdout, &stderr] {
            let (acting, turning) = (acting(shown), turning(shown));
            assert!(acting.is_empty(), "{args:?}: {acting:?} in\n{shown:?}");
            assert!(turning.is_empty(), "{args:?}: {turning:?} in\n{shown:?}");
        }
        assert!(
            stdout.contains(expected),
            "{args:?}: no {expected:?} in\n{stdout}"
        );
    }
    // What is refused is said on stderr, and the same holds there.
    let out = cold_lava(&home)
        .args(["items", ws, "no\u{1b}]0;PWNED-kind\u{7}\u{202e}kind"])
        .output()
        .unwrap();
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
    assert_eq!(stdout, "");
    assert!(acting(&stderr).is_empty(), "{stderr:?}");
    assert!(turning(&stderr).is_empty(), "{stderr:?}");
    assert!(
        stderr.starts_with(
            "cold-lava: no item kind called no\\u{1b}]0;PWNED-kind\\u{7}\\u{202e}kind; KIND is one of: "
        ),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// What is shown is never what is kept: `--json` gives the text as it is written, escaped
/// the way JSON escapes, and the files are not touched by a listing.
#[test]
fn json_and_the_files_keep_the_text_as_written() {
    let (workspace, home) = sandbox("json");
    let ws = workspace.to_str().unwrap();
    let (stdout, _) = run(&home, &["items", ws, "task", "--json"]);
    let card = (stdout.lines())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|item| item["note"] == "Evil.canvas")
        .expect("the card's task");
    assert_eq!(
        card["text"],
        "card \u{1b}]0;PWNED-card\u{7} task \u{202e}gnp.exe"
    );
    assert!(stdout.contains(r"card \u001b]0;PWNED-card\u0007 task"));
    run(&home, &["items", ws, "task"]);
    assert_eq!(
        std::fs::read_to_string(workspace.join("Evil.canvas")).unwrap(),
        CARD
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("EscNote.md")).unwrap(),
        NOTE
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// Text with none of those characters prints as it is written: a tab, a backslash, an
/// accent, an emoji joined of several, a note whose lines end in CR LF.
#[test]
fn ordinary_text_prints_as_it_is_written() {
    let (workspace, home) = sandbox("ordinary");
    let ws = workspace.to_str().unwrap();
    std::fs::write(
        workspace.join("Plain.md"),
        "- [ ] tab\there \\n and \\u{1b} as typed, Parícutin 👨\u{200d}👩\u{200d}👧 \u{200e}ok\r\n",
    )
    .unwrap();
    // An item's text holds a tab as a space; a line of the note holds it as it is.
    let (stdout, _) = run(&home, &["items", ws, "task"]);
    assert!(
        stdout.contains(
            "Plain.md:1  [ ] tab here \\n and \\u{1b} as typed, Parícutin 👨\u{200d}👩\u{200d}👧 \u{200e}ok\n"
        ),
        "{stdout:?}"
    );
    let (stdout, _) = run(&home, &["search", ws, "typed"]);
    assert_eq!(
        stdout,
        "Plain.md:1: - [ ] tab\there \\n and \\u{1b} as typed, Parícutin 👨\u{200d}👩\u{200d}👧 \u{200e}ok\n"
    );
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}

/// A reader that stops early (`cold-lava items … | head`) is not an error: the command ends
/// quietly, with nothing on stderr.
#[test]
fn a_listing_cut_short_by_its_reader_ends_quietly() {
    use std::io::Read;
    let (workspace, home) = sandbox("pipe");
    let ws = workspace.to_str().unwrap();
    let line = format!(
        "- [ ] a task long enough to fill a pipe {}\n",
        "x".repeat(80)
    );
    std::fs::write(workspace.join("Long.md"), line.repeat(4000)).unwrap();
    for args in [&["items", ws, "task"][..], &["table", ws, "task"][..]] {
        let mut child = cold_lava(&home)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut first = [0u8; 16];
        let mut stdout = child.stdout.take().unwrap();
        stdout.read_exact(&mut first).unwrap();
        drop(stdout);
        let out = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{args:?}: {stderr}");
        assert_eq!(stderr, "", "{args:?}");
    }
    let _ = std::fs::remove_dir_all(workspace.parent().unwrap());
}
