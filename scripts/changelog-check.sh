#!/usr/bin/env bash
# Does a pull request that changes code also add a CHANGELOG entry?
#
#   scripts/changelog-check.sh BASE HEAD [LABELS]
#
# BASE and HEAD are commits (the PR's base branch tip and head); LABELS is the PR's labels,
# one per line or comma separated. Run it locally the same way:
#   scripts/changelog-check.sh origin/main HEAD
#
# Passes when the PR changes no code (CODE below), changes CHANGELOG.md, or carries the LABEL.
# A release PR changes CHANGELOG.md (scripts/release-check.sh requires its section), so it
# needs nothing of its own here. Compares against the PR's own base, so each PR of a stack
# needs its own entry. Needs enough history to find the merge base (fetch-depth: 0).
set -euo pipefail

CODE='crates/ examples/extensions/'
LABEL='no-changelog'

base=${1:?usage: changelog-check.sh BASE HEAD [LABELS]}
head=${2:?usage: changelog-check.sh BASE HEAD [LABELS]}
labels=${3:-}

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
