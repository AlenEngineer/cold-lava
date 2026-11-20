//! A reader that stops early (`cold-lava --help | head -15`) ends the output quietly: the
//! command exits 0 and nothing is said on stderr, as it does for `items | head` (#339).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn cold_lava(name: &str, args: &[&str]) -> Command {
    let home: PathBuf = [env!("CARGO_TARGET_TMPDIR"), &format!("pipe-{name}")]
        .iter()
        .collect();
    std::fs::create_dir_all(&home).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_cold-lava"));
    c.args(args)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .stdin(Stdio::null())
        .stderr(Stdio::piped());
    c
}

fn assert_quiet(args: &[&str], out: &std::process::Output) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
        "cold-lava {args:?} said on stderr:\n{stderr}"
    );
    assert_eq!(out.status.code(), Some(0), "cold-lava {args:?}:\n{stderr}");
}

/// Stdout's reader is gone before the first line is written, so every line meets a closed
/// pipe.
#[test]
fn a_closed_stdout_ends_the_output_quietly() {
    for args in [&["--help"][..], &["keys"][..]] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let out = cold_lava("closed", args)
            .stdout(writer)
            .spawn()
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert_quiet(args, &out);
    }
}

/// The reader takes one line and leaves, as `head` does, while the rest is still being
/// written. Run again and again, since where the writer is when the reader leaves varies.
#[test]
fn a_reader_that_leaves_after_one_line_ends_the_output_quietly() {
    for args in [&["--help"][..], &["keys"][..]] {
        for _ in 0..30 {
            let mut child = cold_lava("head", args)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut first = String::new();
            let mut stdout = BufReader::new(child.stdout.take().unwrap());
            stdout.read_line(&mut first).unwrap();
            assert!(!first.is_empty(), "cold-lava {args:?} printed nothing");
            drop(stdout);
            let out = child.wait_with_output().unwrap();
            assert_quiet(args, &out);
        }
    }
}
