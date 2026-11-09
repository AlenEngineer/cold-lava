//! Records the commit the binary is built from, so `cold-lava --version` says exactly which
//! code it is (a CI build of a pull request is GitHub's merge commit for it). Outside a git
//! checkout (a source tarball) there is no commit, and the version stands alone.

use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

fn main() {
    let commit = git(&["rev-parse", "--short=12", "HEAD"]);
    let suffix = commit.map(|c| format!(" ({c})")).unwrap_or_default();
    println!("cargo:rustc-env=COLD_LAVA_COMMIT_SUFFIX={suffix}");

    // Build again when HEAD moves: HEAD itself (a checkout), the branch it points at (a commit),
    // and packed-refs (where that branch may live). Paths come from git, so worktrees work.
    let mut watch = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(branch);
    }
    for name in watch {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &name])
            && PathBuf::from(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
