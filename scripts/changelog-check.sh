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
# released. A release PR is exempt: it adds a '## [x.y.z]' heading for a version the base has
# no heading for, which is what scripts/release-check.sh requires. The LABEL doesn't excuse a
# misplaced line. Second, the PR changes no code (CODE below), changes CHANGELOG.md, or
# carries the LABEL.
#
# Compares against the PR's own base, so each PR of a stack needs its own entry, and lines a
# merge of main brings in aren't the PR's. Needs enough history to find the merge base
# (fetch-depth: 0). scripts/changelog-check-test.sh runs the cases.
set -euo pipefail

CODE='crates/ examples/extensions/'
LABEL='no-changelog'

base=${1:?usage: changelog-check.sh BASE HEAD [LABELS]}
head=${2:?usage: changelog-check.sh BASE HEAD [LABELS]}
labels=${3:-}

# Prints "LINE<TAB>SECTION<TAB>TEXT" for each non-blank line the PR adds to CHANGELOG.md that
# is not under '## [Unreleased]'. Reads the added line numbers from a zero-context diff
# ('@@ -a,b +c,d @@': d lines from c, d defaults to 1) and the sections from HEAD's file.
misplaced_lines() {
  local added
  added=$(git diff --unified=0 "$base...$head" -- CHANGELOG.md |
    sed -n 's/^@@ -[0-9,]* +\([0-9]*\),\{0,1\}\([0-9]*\) @@.*/\1 \2/p')
  [ -n "$added" ] || return 0
  git show "$head:CHANGELOG.md" | awk -v added="$added" '
    BEGIN {
      n = split(added, rows, "\n")
      for (i = 1; i <= n; i++) {
        split(rows[i], f, " ")
        count = (f[2] == "") ? 1 : f[2]
        for (j = 0; j < count; j++) new[f[1] + j] = 1
      }
      section = "(before the first version heading)"
    }
    /^## / { section = $0 }
    (NR in new) && $0 !~ /^[ \t]*$/ && section !~ /^## \[Unreleased\]/ {
      printf "%d\t%s\t%s\n", NR, section, $0
    }'
}

# Prints each version whose '## [x.y.z]' heading the PR adds and the base has none for: a
# release PR adds one.
new_releases() {
  local version before
  before=$(git show "$(git merge-base "$base" "$head"):CHANGELOG.md" 2>/dev/null || true)
  git diff --unified=0 "$base...$head" -- CHANGELOG.md |
    sed -n 's/^+## \[\([0-9][^]]*\)\].*/\1/p' |
    while read -r version; do
      printf '%s\n' "$before" | grep -qF "## [$version]" || echo "$version"
    done
}

misplaced=$(misplaced_lines)
if [ -n "$misplaced" ] && [ -z "$(new_releases)" ]; then
  {
    echo "This PR adds lines to CHANGELOG.md outside '## [Unreleased]':"
    printf '%s\n' "$misplaced" | head -n 10 |
      awk -F'\t' '{ printf "  line %s, under \047%s\047: %s\n", $1, $2, $3 }'
    echo
    echo "A released version's notes are cut when it is released, so an entry there is never seen."
    echo "Move each line under '## [Unreleased]' (a merge of main into this branch can slide an entry into the section that was released)."
  } >&2
  printf '%s\n' "$misplaced" | head -n 10 | while IFS=$'\t' read -r line section _; do
    echo "::error file=CHANGELOG.md,line=$line::This line is under '$section', a released version. Move it under '## [Unreleased]'."
  done
  exit 1
fi

if printf '%s\n' "$labels" | tr ',' '\n' | grep -qxF "$LABEL"; then
  echo "The $LABEL label is set: no entry needed."
  exit 0
fi

# -z: names as written (git quotes non-ASCII, quotes and newlines otherwise); --no-renames: a
# file moved out of the code counts as a change there.
changed=$(git diff -z --no-renames --name-only "$base...$head" | tr '\0' '\n')

pattern="^($(printf '%s' "$CODE" | sed 's/ /|/g'))"
code=$(printf '%s\n' "$changed" | grep -E "$pattern" || true)
if [ -z "$code" ]; then
  echo "No change under $CODE: no entry needed."
  exit 0
fi

if printf '%s\n' "$changed" | grep -qx 'CHANGELOG.md'; then
  echo "CHANGELOG.md is changed."
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
  echo "If nothing here is user-visible (a refactor, tests), add the '$LABEL' label to this PR instead; the check runs again when the label is added."
} >&2
echo "::error file=CHANGELOG.md::This PR changes code under $CODE but adds no CHANGELOG.md entry. Add one under '## [Unreleased]', or add the $LABEL label if nothing is user-visible."
exit 1
