//! A reader that stops early (`cold-lava --help | head -15`) skips the rest of the output
//! quietly: nothing is said about it on stderr, and the command still does everything else
//! it was asked and exits as it would have (#339).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A workspace made of `files`, and an empty home beside it.
fn scratch(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("pipe-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, text) in files {
        let p = dir.join("ws").join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    std::fs::create_dir_all(dir.join("home/.config")).unwrap();
    dir
}

fn cold_lava(dir: &Path, args: &[&str]) -> Command {
    let home = dir.join("home");
    let mut c = Command::new(env!("CARGO_BIN_EXE_cold-lava"));
    c.args(args)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    c
}

/// `cold-lava ARGS` with stdout's reader gone before the first line is written, so every
/// line meets a closed pipe.
fn closed_stdout(dir: &Path, args: &[&str]) -> Output {
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    cold_lava(dir, args)
        .stdout(writer)
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap()
}

fn assert_quiet(args: &[&str], out: &Output, code: i32) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
        "cold-lava {args:?} said on stderr:\n{stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(code),
        "cold-lava {args:?}:\n{stderr}"
    );
}

#[test]
fn a_closed_stdout_ends_the_output_quietly() {
    let dir = scratch("closed", &[("Note.md", "# Note\n- [ ] A task\n")]);
    let ws = dir.join("ws");
    let ws = ws.to_str().unwrap();
    for args in [
        &["--help"][..],
        &["keys"][..],
        &["types", ws][..],
        &["style", ws][..],
    ] {
        assert_quiet(args, &closed_stdout(&dir, args), 0);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `style` and `types` print their listing first and their errors after it, on stderr. A
/// closed stdout must not cut the errors or the exit code that says there were some.
#[test]
fn a_closed_stdout_still_reports_what_comes_after_the_listing() {
    let dir = scratch(
        "after",
        &[
            ("Note.md", "# Note\n"),
            (".cold-lava/style.toml", "list-markers = \"-\"\n"),
            (".cold-lava/types.toml", "version = 1\n[types.x\n"),
        ],
    );
    let ws = dir.join("ws");
    let ws = ws.to_str().unwrap();
    for (args, said) in [(["style", ws], "list-markers"), (["types", ws], "errors:")] {
        let out = closed_stdout(&dir, &args);
        assert_quiet(&args, &out, 1);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(said),
            "cold-lava {args:?} lost its errors:\n{stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The reader takes one line and leaves, as `head` does, while the rest is still being
/// written. The output is far longer than a pipe holds, so the writer is still writing
/// when the reader leaves, whichever way the two are scheduled: `agenda` prints a line per
/// call, `items` holds stdout for its listing.
#[test]
fn a_reader_that_leaves_after_one_line_ends_the_output_quietly() {
    let tasks: String = (0..20_000)
        .map(|n| format!("- [ ] Task number {n} 📅 2026-11-12\n"))
        .collect();
    let dir = scratch("head", &[("Tasks.md", &tasks)]);
    let ws = dir.join("ws");
    let ws = ws.to_str().unwrap();
    for args in [
        &["agenda", ws, "--from", "2026-11-01", "--to", "2026-11-30"][..],
        &["items", ws, "task"][..],
    ] {
        let mut child = cold_lava(&dir, args)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut first = String::new();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        stdout.read_line(&mut first).unwrap();
        assert!(!first.is_empty(), "cold-lava {args:?} printed nothing");
        drop(stdout);
        assert_quiet(args, &child.wait_with_output().unwrap(), 0);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
