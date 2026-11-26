#!/usr/bin/env bash
# Lists what was merged since the last release, to draft a release's CHANGELOG.md section from.
# PRs are squash-merged, so each line is one PR's title and number. Rewrite the list for
# users before it goes in (CONTRIBUTING.md, "Releases").
#
#   scripts/changelog-draft.sh [REF]     (REF defaults to origin/main)
#
# Needs the repository's tags (git fetch --tags).
set -euo pipefail

ref=${1:-origin/main}
last=$(git describe --tags --abbrev=0 --match 'v[0-9]*' "$ref")
echo "Merged since $last:"
git log --first-parent --format='- %s' "$last..$ref"
