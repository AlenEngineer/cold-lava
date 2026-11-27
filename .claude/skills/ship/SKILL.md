---
name: ship
description: Take a cold-lava change from a finished branch to merged on main - order check, local checks, PR, CI, two independent reviews, fixes, squash merge. Use for /ship, "ship this", "open a PR and merge it", or when a change is ready to land.
---

# Ship

The steps are `CONTRIBUTING.md`'s; read it first. This skill is how an agent runs them. You
are the PR's author, so you never review it yourself: two other agents do.

## 1. Before the PR

- Find the work's line on roadmap #100 (`gh issue view 100`). If it waits on a track step
  that isn't merged, stop and say so.
- `free -h`. Under 3 GB available, or swap mostly full: wait or tell the owner. Then
  `cargo fmt --all --check`, and `cargo clippy -p CRATE --all-targets -- -D warnings` and
  `cargo test -p CRATE` for each crate the diff touches, one at a time.
- Commit with a short summary line, then what changed and why. Push the branch.

## 2. The PR

`gh pr create` with the template's sections filled in. The title is one line a user would
understand. A user-visible change updates the README in this PR. Link the PR in the issue.

## 3. CI

Wait for `ubuntu-latest` and `release check` (`gh pr checks PR --watch` in a background
shell, or the host's PR watch). A failure: read its log, fix, push, wait again.

## 4. Two reviews, in parallel

Spawn both with the Agent tool in one message, `model: "opus"`, and wait for both.

- **Behaviour** (`general-purpose`). The brief gives it:
  - the PR number and its claim, from the PR's Problem and Change sections;
  - main's and the PR's CI binaries: `gh run download RUN_ID -n cold-lava-linux -D DIR`,
    then `chmod +x`. It checks `--version`: a PR's binary is built from GitHub's merge
    commit, whose second parent is the PR's head (`git log -1 --format=%P COMMIT`);
  - the task: compare main and the PR byte for byte (stdout, stderr, exit codes, whole-tree
    diffs after edits) on copies of `example-workspace/` at the same absolute path, with
    empty HOME and XDG folders, then attack the new behaviour with hostile input (symlinks,
    files changed by another program meanwhile, non-UTF-8, huge or empty files, slow or
    out-of-order news). For a refactor, any difference is a must-fix;
  - the rules: build nothing (it has the binaries), put scratch copies under `~/.cache/`,
    not `/tmp`, and delete them; report must-fix and should-fix, each with the command
    that shows it.
- **Design** (`design-reviewer`). The brief is the PR number; the agent file has the rest.

Post each report as a PR comment (`gh pr comment`) and link both in the Reviews section.

## 5. Fixes

Fix each must-fix as a new commit on the branch (never rebase or force-push; bring `main`
in with a merge commit). Push, wait for CI, and run both reviews again on the new head,
each as a new agent. Open an issue for each should-fix and add it to #100.

## 6. Merge

When CI is green and neither review has a must-fix, merge:
`gh pr merge PR --squash --delete-branch`. Don't ask first, unless the owner said to hold
this PR or only to open it.

Then `git switch main && git pull --ff-only`, comment on the issue with what merged and what
is left, tick its line on #100, and remove any worktree the work used.
