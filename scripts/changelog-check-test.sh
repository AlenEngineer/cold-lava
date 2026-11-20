#!/usr/bin/env bash
# Runs scripts/changelog-check.sh on scratch git repos, one per case, and says which cases
# gave the wrong answer:
#   scripts/changelog-check-test.sh [CASE...]
# With names (the case_ functions below, without the prefix) it runs only those. Exits 1 if
# any case gave the wrong answer. Needs only git and bash. The repos live under ~/.cache (not
# /tmp, which is RAM) and are removed afterwards.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
check=$here/changelog-check.sh
cache=${XDG_CACHE_HOME:-$HOME/.cache}
mkdir -p "$cache"
scratch=$(mktemp -d "$cache/changelog-check-test.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

failures=0

git_() { local repo=$1; shift; git -C "$repo" -c user.name=test -c user.email=test@example.com "$@"; }

# Replaces a repo's changelog; the arguments after the repo are its lines.
write_changelog() { local repo=$1; shift; printf '%s\n' "$@" > "$repo/CHANGELOG.md"; }
write_version() { printf '[workspace.package]\nversion = "%s"\n' "$2" > "$1/Cargo.toml"; }
commit_all() { git_ "$1" add -A && git_ "$1" commit -q -m "$2"; }
touch_code() { echo "$RANDOM" >> "$1/crates/engine/lib.rs"; }

# A repo on branch pr, one commit past main. Main has a changelog with entry "- a" under
# Unreleased and a released 0.6.0 (Cargo.toml's version, tagged v0.6.0).
new_repo() {
  local repo=$scratch/$1
  mkdir -p "$repo/crates/engine"
  git init -q -b main "$repo"
  write_changelog "$repo" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
  write_version "$repo" 0.6.0
  echo one > "$repo/crates/engine/lib.rs"
  commit_all "$repo" start
  git_ "$repo" tag v0.6.0
  git_ "$repo" checkout -q -b pr
  echo "$repo"
}

# expect REPO NAME WANT_EXIT WANT_TEXT [LABELS]: runs the check on main..HEAD of the repo.
expect() {
  local repo=$1 name=$2 want=$3 text=$4 labels=${5:-} out code
  out=$(cd "$repo" && "$check" main HEAD "$labels" 2>&1)
  code=$?
  if [ "$code" -eq "$want" ] && printf '%s' "$out" | grep -qF -- "$text"; then
    echo "ok    $name"
  else
    echo "FAIL  $name: exit $code (wanted $want), output wanted to hold '$text':"
    printf '%s\n' "$out" | sed 's/^/        /'
    failures=$((failures + 1))
  fi
}

# Each case_ function gets its own repo, made by new_repo, as its argument.

case_entry_under_unreleased() {
  local r=$1
  touch_code "$r"
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '- mine' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
  commit_all "$r" pr
  expect "$r" "an entry under Unreleased passes" 0 "CHANGELOG.md is changed"
}

# The changelog of new_repo with "- mine" added under 0.6.0, as line 14.
under_release() {
  write_changelog "$1" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '- mine'
}

case_entry_under_release() {
  local r=$1
  touch_code "$r"
  under_release "$r"
  commit_all "$r" pr
  expect "$r" "an entry under a released version fails, naming line and section" 1 \
    "line 14, under '## [0.6.0] - 2026-11-11': - mine"
}

case_entry_under_release_docs_only() {
  local r=$1
  under_release "$r"
  commit_all "$r" pr
  expect "$r" "a misplaced line fails even when no code changed" 1 "line 14"
}

case_entry_under_release_no_changelog_label() {
  local r=$1
  touch_code "$r"
  under_release "$r"
  commit_all "$r" pr
  expect "$r" "the no-changelog label doesn't excuse a misplaced line" 1 "line 14" "bug,no-changelog"
}

case_entry_under_release_changelog_edit_label() {
  local r=$1
  touch_code "$r"
  under_release "$r"
  commit_all "$r" pr
  expect "$r" "the changelog-edit label lets a PR edit a released section" 0 \
    "CHANGELOG.md is changed" "bug,changelog-edit"
}

case_release_pr() {
  local r=$1
  touch_code "$r"
  write_version "$r" 0.7.0
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '## [0.7.0] - 2026-11-20' '' '### Fixed' '' '- a' '- mine' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '' \
    '[0.7.0]: https://example.com/compare/v0.6.0...v0.7.0'
  commit_all "$r" release
  expect "$r" "a release PR passes for the lines under its own heading" 0 "CHANGELOG.md is changed"
}

case_release_pr_older_section() {
  local r=$1
  write_version "$r" 0.7.0
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '## [0.7.0] - 2026-11-20' '' '### Fixed' '' '- a' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '- sneaky'
  commit_all "$r" release
  expect "$r" "a release PR still fails for a line under an older released section" 1 \
    "under '## [0.6.0] - 2026-11-11': - sneaky"
}

case_heading_without_version_bump() {
  local r=$1
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '## [0.7.0] - 2026-11-20' '' '### Fixed' '' '- a' '- mine' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
  commit_all "$r" not-a-release
  expect "$r" "a new version heading without a version bump is not a release" 1 \
    "under '## [0.7.0] - 2026-11-20': - mine"
}

case_version_bump_without_section() {
  local r=$1
  write_version "$r" 0.7.0
  under_release "$r"
  commit_all "$r" bump
  expect "$r" "a version bump with no section for it is not a release" 1 "under '## [0.6.0] - 2026-11-11'"
}

case_edited_release_heading() {
  local r=$1
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
    '## [0.6.0] - 2026-11-12' '' '### Added' '' '- old'
  commit_all "$r" redate
  expect "$r" "re-dating a released heading is not a release" 1 "under '## [0.6.0] - 2026-11-12'"
}

case_deleted_only_hunk() {
  local r=$1
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' ''
  commit_all "$r" delete
  expect "$r" "deleting a line from a released section adds none, so it passes" 0 "no entry needed"
}

case_link_definitions_are_no_entries() {
  local r=$1
  touch_code "$r"
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '- mine' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '' \
    '[Unreleased]: https://example.com/compare/v0.6.0...HEAD'
  commit_all "$r" pr
  expect "$r" "a link definition in the footer is no entry, wherever it falls" 0 "CHANGELOG.md is changed"
}

case_label_no_changelog() {
  local r=$1
  touch_code "$r"
  commit_all "$r" refactor
  expect "$r" "no CHANGELOG change with the label passes" 0 "label is set" "no-changelog"
}

case_code_without_entry() {
  local r=$1
  touch_code "$r"
  commit_all "$r" refactor
  expect "$r" "code with no CHANGELOG change and no label still fails" 1 "changes code but not CHANGELOG.md"
}

case_docs_only() {
  local r=$1
  echo hi > "$r/README.md"
  commit_all "$r" docs
  expect "$r" "no code and no CHANGELOG change passes" 0 "no entry needed"
}

# main releases while the PR is open and the PR merges main in: git puts the PR's entry
# where the old Unreleased section's lines now are, which is the 0.7.0 section. The merged
# head's version (0.7.0) is tagged, so it is no release.
case_merge_slides_entry() {
  local r=$1
  touch_code "$r"
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '- mine' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
  commit_all "$r" pr
  git_ "$r" checkout -q main
  write_version "$r" 0.7.0
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '## [0.7.0] - 2026-11-20' '' '### Fixed' '' '- a' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
  commit_all "$r" release
  git_ "$r" tag v0.7.0
  git_ "$r" checkout -q pr
  git_ "$r" merge -q --no-edit main >/dev/null
  expect "$r" "a merge that slides the PR's entry into the released section fails" 1 \
    "under '## [0.7.0] - 2026-11-20': - mine"
}

# main only adds a line to a released section; the PR's entry stays under Unreleased.
case_merge_brings_lines() {
  local r=$1
  touch_code "$r"
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' '### Added' '' '- mine' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
  commit_all "$r" pr
  git_ "$r" checkout -q main
  write_changelog "$r" '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '- from main'
  commit_all "$r" main-entry
  git_ "$r" checkout -q pr
  git_ "$r" merge -q --no-edit main >/dev/null
  expect "$r" "lines a merge from main brings into a released section are not the PR's" 0 "CHANGELOG.md is changed"
}

cases=("$@")
if [ ${#cases[@]} -eq 0 ]; then
  mapfile -t cases < <(declare -F | sed -n 's/^declare -f case_//p')
fi
for name in "${cases[@]}"; do
  "case_$name" "$(new_repo "$name")"
done

echo
if [ "$failures" -eq 0 ]; then
  echo "all cases gave the right answer"
else
  echo "$failures case(s) gave the wrong answer"
  exit 1
fi
