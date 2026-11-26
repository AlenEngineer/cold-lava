# How a change gets merged

One request is one PR, squash-merged. `main` takes no direct pushes: a ruleset requires a PR
with the `ubuntu-latest` and `release check` checks passing (`.github/workflows/ci.yml`).
Agents run the whole sequence below with the `ship` skill (`.claude/skills/ship/SKILL.md`).

1. **Order.** Find the work's line on roadmap issue #100. If it says "after track X" and that
   step isn't merged, do the step first, as its own PR: a behaviour-identical refactor,
   checked against `main` and merged before the feature that needs it is opened. One track
   step per refactor PR.
2. **Check locally what you touched**: `cargo fmt --all --check`, then `cargo clippy -p CRATE
   --all-targets -- -D warnings` and `cargo test -p CRATE` for each crate the change touches.
   CI runs the whole workspace.
3. **Open the PR** from the template (`.github/pull_request_template.md`). The title is one
   line a user would understand ("The agenda shows tasks due today first"), because the
   release notes are written from the titles. A user-visible change updates the README in
   the same PR.
4. **Two reviews by two different agents, neither the PR's author**, posted on the PR:
   behaviour (CI's binary against `main`'s, trying to break what the PR claims) and design
   (the `design-reviewer` agent, against `docs/foundations.md`). A must-fix from either
   blocks the merge. Fixes are new commits on the same PR, and both reviews run again. A
   pushed branch is never rebased or force-pushed; bring `main` in with a merge commit.
   Should-fix items become issues on #100.
5. **Merge** with a squash once CI is green and both reviews have no must-fix. Ask the owner
   first when the PR:
   - changes the shared-files code: `crates/engine/src/write.rs`, `trash.rs`, `read.rs`,
     `files.rs` or `gateway.rs`;
   - is a release (it sets a new `version` in `Cargo.toml`);
   - changes `.github/`, this file, or the ship skill.

## Releases

A release PR sets a new `version` under `[workspace.package]` in `Cargo.toml` and adds a
`## [x.y.z] - date` section to `CHANGELOG.md`. Draft it with `scripts/changelog-draft.sh`,
which lists the PRs merged since the last tag, and rewrite the list for users: a short
paragraph on what matters, then Added, Changed, Fixed, and anything breaking for extensions
in bold. `scripts/release-check.sh` says whether a commit is a release and whether it is
ready. Merging it makes `.github/workflows/release.yml` tag the commit, build the Linux
binaries (x86-64 and ARM) and publish the release with that section as its notes. Before
1.0, a minor version may break; a patch only fixes.
