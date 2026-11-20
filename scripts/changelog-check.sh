#!/usr/bin/env bash
# Does a pull request that changes code also add a CHANGELOG entry?
#
#   scripts/changelog-check.sh BASE HEAD [LABELS]
#
# BASE and HEAD are commits (the PR's base branch tip and head); LABELS is the PR's labels,
# one per line or comma separated. Run it locally the same way:
#   scripts/changelog-check.sh origin/main HEAD
#
# Two checks. First, every line the PR adds to CHANGELOG.md lies under '## [Unreleased]'; a
# line under a released version's heading is never seen (a release's notes are cut from its
# section), which is what a merge of main into an open PR does to an entry once main has
# released. Two things lift that. When the PR's head is a release (scripts/release-check.sh
# says so: Cargo.toml's version has no tag yet), the lines under that version's own heading
# are allowed, for a release moves the Unreleased entries there; a line under any older
# heading still fails. And the CHANGELOG_EDIT label lets a PR edit a released section on
# purpose (a typo, a correction). The no-changelog label never lifts it. Second, the PR
# changes no code (CODE below), changes CHANGELOG.md, or carries the LABEL.
#
# Compares against the PR's own base, so each PR of a stack needs its own entry, and lines a
# merge of main brings in aren't the PR's. Needs enough history to find the merge base and the
# tags (fetch-depth: 0). Run from inside the repository. scripts/changelog-check-test.sh runs
# the cases.
set -euo pipefail

CODE='crates/ examples/extensions/'
LABEL='no-changelog'
CHANGELOG_EDIT='changelog-edit'

base=${1:?usage: changelog-check.sh BASE HEAD [LABELS]}
head=${2:?usage: changelog-check.sh BASE HEAD [LABELS]}
labels=${3:-}

has_label() { printf '%s\n' "$labels" | tr ',' '\n' | grep -qxF "$1"; }

# Prints the number of each line the PR adds to HEAD's CHANGELOG.md, from a zero-context diff
# ('@@ -a,b +c,d @@': d lines from c, d defaults to 1, 0 is a deletion).
added_lines() {
  git diff --unified=0 "$base...$head" -- CHANGELOG.md |
    sed -n 's/^@@ -[0-9,]* +\([0-9]*\),\{0,1\}\([0-9]*\) @@.*/\1 \2/p' |
    awk '{ n = ($2 == "") ? 1 : $2; for (i = 0; i < n; i++) print $1 + i }'
}

# Prints the version when HEAD is a release, by release-check.sh's definition. One that fails
# its own checks (no section for the version) is not one here; its own CI job reports that.
release_version() {
  local out
  out=$(env -u GITHUB_REF_TYPE -u GITHUB_OUTPUT "$(dirname "$0")/release-check.sh" "$head" 2>/dev/null) || return 0
  if printf '%s\n' "$out" | grep -qx 'release=true'; then
    printf '%s\n' "$out" | sed -n 's/^version=//p'
  fi
}

# Prints "LINE<TAB>SECTION<TAB>TEXT" for each non-blank added line that is not under
# '## [Unreleased]' or under the heading that starts with $1 (none when empty). A link
# definition ('[0.7.0]: https://...', the footer the file ends in) is no entry and is skipped.
misplaced_lines() {
  git show "$head:CHANGELOG.md" | awk -v allowed="$1" -v added="$(added_lines)" '
    BEGIN {
      n = split(added, rows, "\n")
      for (i = 1; i <= n; i++) new[rows[i]] = 1
      section = "(before the first version heading)"
    }
    /^## / { section = $0 }
    (NR in new) && $0 !~ /^[ \t]*$/ && $0 !~ /^\[[^]]*\]: / && section !~ /^## \[Unreleased\]/ \
      && !(allowed != "" && index(section, allowed) == 1) {
      printf "%d\t%s\t%s\n", NR, section, $0
    }'
}

report_misplaced() {
  local rows=$1
  {
    echo "This PR adds lines to CHANGELOG.md outside '## [Unreleased]':"
    printf '%s\n' "$rows" | head -n 10 |
      awk -F'\t' '{ printf "  line %s, under \047%s\047: %s\n", $1, $2, $3 }'
    echo
    echo "A released version's notes are cut when it is released, so an entry there is never seen."
    echo "Move each line under '## [Unreleased]' (a merge of main into this branch can slide an entry into the section that was released)."
    echo "To edit a released section on purpose, add the '$CHANGELOG_EDIT' label to this PR; the check runs again."
  } >&2
  printf '%s\n' "$rows" | head -n 10 | while IFS=$'\t' read -r line section _; do
    echo "::error file=CHANGELOG.md,line=$line::This line is under '$section', a released version. Move it under '## [Unreleased]'."
  done
}

# Exits 0 when the PR needs no entry or has one, 1 (saying why) when it changes code and has none.
needs_entry() {
  if has_label "$LABEL"; then
    echo "The $LABEL label is set: no entry needed."
    return 0
  fi

  # -z: names as written (git quotes non-ASCII, quotes and newlines otherwise); --no-renames: a
  # file moved out of the code counts as a change there.
  local changed code pattern shown more
  changed=$(git diff -z --no-renames --name-only "$base...$head" | tr '\0' '\n')

  pattern="^($(printf '%s' "$CODE" | sed 's/ /|/g'))"
  code=$(printf '%s\n' "$changed" | grep -E "$pattern" || true)
  if [ -z "$code" ]; then
    echo "No change under $CODE: no entry needed."
    return 0
  fi

  if printf '%s\n' "$changed" | grep -qx 'CHANGELOG.md'; then
    echo "CHANGELOG.md is changed."
    return 0
  fi

  shown=$(printf '%s\n' "$code" | head -n 10)
  more=$(( $(printf '%s\n' "$code" | wc -l) - 10 ))
  {
    echo "This PR changes code but not CHANGELOG.md. Changed code:"
    printf '%s\n' "$shown" | sed 's/^/  /'
    if [ "$more" -gt 0 ]; then echo "  ... and $more more"; fi
    echo
    echo "Add an entry under '## [Unreleased]' in CHANGELOG.md for each user-visible change (CLAUDE.md, Workflow)."
    echo "If nothing here is user-visible (a refactor, tests), add the '$LABEL' label to this PR instead; the check runs again when the label is added."
  } >&2
  echo "::error file=CHANGELOG.md::This PR changes code under $CODE but adds no CHANGELOG.md entry. Add one under '## [Unreleased]', or add the $LABEL label if nothing is user-visible."
  return 1
}

if ! has_label "$CHANGELOG_EDIT"; then
  version=$(release_version)
  misplaced=$(misplaced_lines "${version:+## [$version]}")
  if [ -n "$misplaced" ]; then
    report_misplaced "$misplaced"
    exit 1
  fi
fi
needs_entry
