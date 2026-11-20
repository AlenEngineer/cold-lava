#!/usr/bin/env bash
# Does a pull request that changes code also add a CHANGELOG entry?
#
#   scripts/changelog-check.sh BASE HEAD [LABELS]
#
# BASE and HEAD are commits (the PR's base branch tip and head); LABELS is the PR's labels,
# one per line or comma separated. Run it locally the same way:
#   scripts/changelog-check.sh origin/main HEAD
#
# Passes when the PR changes nothing under crates/ or examples/extensions/, changes
# CHANGELOG.md, bumps the version in Cargo.toml (a release PR: scripts/release-check.sh covers
# it), or carries the no-changelog label. Compares against the PR's own base, so each PR of a
# stack needs its own entry. Needs enough history to find the merge base (fetch-depth: 0).
set -euo pipefail

base=${1:?usage: changelog-check.sh BASE HEAD [LABELS]}
head=${2:?usage: changelog-check.sh BASE HEAD [LABELS]}
labels=${3:-}

if printf '%s\n' "$labels" | tr ',' '\n' | grep -qx 'no-changelog'; then
  echo "The no-changelog label is set: no entry needed."
  exit 0
fi

changed=$(git diff --name-only "$base...$head")

code=$(printf '%s\n' "$changed" | grep -E '^(crates/|examples/extensions/)' || true)
if [ -z "$code" ]; then
  echo "No change under crates/ or examples/extensions/: no entry needed."
  exit 0
fi

if printf '%s\n' "$changed" | grep -qx 'CHANGELOG.md'; then
  echo "CHANGELOG.md is changed."
  exit 0
fi

version_of() {
  git show "$1:Cargo.toml" | sed -n 's/^version = "\(.*\)"$/\1/p' | head -n 1
}
base_version=$(version_of "$(git merge-base "$base" "$head")")
head_version=$(version_of "$head")
if [ "$base_version" != "$head_version" ]; then
  echo "The version goes from $base_version to $head_version: a release PR, checked by release-check."
  exit 0
fi

shown=$(printf '%s\n' "$code" | head -n 10)
more=$(( $(printf '%s\n' "$code" | wc -l) - 10 ))
{
  echo "This PR changes code but not CHANGELOG.md. Changed code:"
  printf '%s\n' "$shown" | sed 's/^/  /'
  if [ "$more" -gt 0 ]; then echo "  ... and $more more"; fi
  echo
  echo "Add an entry under '## [Unreleased]' in CHANGELOG.md for each user-visible change (CLAUDE.md, Workflow)."
  echo "If nothing here is user-visible (a refactor, tests, CI), add the 'no-changelog' label to this PR instead; the check runs again when the label is added."
} >&2
echo "::error file=CHANGELOG.md::This PR changes code under crates/ or examples/extensions/ but adds no CHANGELOG.md entry. Add one under '## [Unreleased]', or add the no-changelog label if nothing is user-visible."
exit 1
