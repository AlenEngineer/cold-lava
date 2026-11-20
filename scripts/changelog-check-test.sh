#!/usr/bin/env bash
# Runs scripts/changelog-check.sh on scratch git repos, one per case, and says which cases
# gave the wrong answer:
#   scripts/changelog-check-test.sh
# Exits 1 if any did. Needs only git and bash. The repos live under ~/.cache (not /tmp, which
# is RAM) and are removed afterwards.
set -uo pipefail

check=$(cd "$(dirname "$0")" && pwd)/changelog-check.sh
cache=${XDG_CACHE_HOME:-$HOME/.cache}
mkdir -p "$cache"
scratch=$(mktemp -d "$cache/changelog-check-test.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

failures=0
repo=""

git_() { git -C "$repo" -c user.name=test -c user.email=test@example.com "$@"; }

# A repo whose main has a changelog with entry "- a" under Unreleased and a released 0.6.0.
new_repo() {
  repo="$scratch/$1"
  mkdir -p "$repo/crates/engine"
  git init -q -b main "$repo"
  printf '%s\n' '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
    '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' > "$repo/CHANGELOG.md"
  echo one > "$repo/crates/engine/lib.rs"
  git_ add -A && git_ commit -q -m start
}

# Replaces the changelog; the arguments are its lines.
write_changelog() { printf '%s\n' "$@" > "$repo/CHANGELOG.md"; }
commit_all() { git_ add -A && git_ commit -q -m "$1"; }
touch_code() { echo "$RANDOM" >> "$repo/crates/engine/lib.rs"; }

# expect NAME WANT_EXIT WANT_TEXT [LABELS]: runs the check on main..HEAD of the current repo.
expect() {
  local name=$1 want=$2 text=$3 labels=${4:-} out code
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

new_repo entry-under-unreleased
git_ checkout -q -b pr
touch_code
write_changelog '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '- mine' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
commit_all pr
expect "an entry under Unreleased passes" 0 "CHANGELOG.md is changed"

new_repo entry-under-release
git_ checkout -q -b pr
touch_code
write_changelog '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '- mine'
commit_all pr
expect "an entry under a released version fails, naming line and section" 1 \
  "line 14, under '## [0.6.0] - 2026-11-11': - mine"

new_repo entry-under-release-docs-only
git_ checkout -q -b pr
write_changelog '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '- mine'
commit_all pr
expect "a misplaced line fails even when no code changed" 1 "line 14"

new_repo entry-under-release-labelled
git_ checkout -q -b pr
touch_code
write_changelog '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '- mine'
commit_all pr
expect "the no-changelog label doesn't excuse a misplaced line" 1 "line 14" "bug,no-changelog"

new_repo release-pr
git_ checkout -q -b pr
touch_code
write_changelog '# Changelog' '' '## [Unreleased]' '' '## [0.7.0] - 2026-11-20' '' '### Fixed' '' '- a' '- mine' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
commit_all release
expect "a release PR passes" 0 "CHANGELOG.md is changed"

new_repo edited-release-heading
git_ checkout -q -b pr
write_changelog '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
  '## [0.6.0] - 2026-11-12' '' '### Added' '' '- old'
commit_all redate
expect "re-dating a released heading is not a release" 1 "under '## [0.6.0] - 2026-11-12'"

new_repo label-no-changelog
git_ checkout -q -b pr
touch_code
commit_all refactor
expect "no CHANGELOG change with the label passes" 0 "label is set" "no-changelog"

new_repo code-without-entry
git_ checkout -q -b pr
touch_code
commit_all refactor
expect "code with no CHANGELOG change and no label still fails" 1 "changes code but not CHANGELOG.md"

new_repo docs-only
git_ checkout -q -b pr
echo hi > "$repo/README.md"
commit_all docs
expect "no code and no CHANGELOG change passes" 0 "no entry needed"

# main releases while the PR is open and the PR merges main in: git puts the PR's entry
# where the old Unreleased section's lines now are, which is the 0.7.0 section.
new_repo merge-slides-entry
git_ checkout -q -b pr
touch_code
write_changelog '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '- mine' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
commit_all pr
git_ checkout -q main
write_changelog '# Changelog' '' '## [Unreleased]' '' '## [0.7.0] - 2026-11-20' '' '### Fixed' '' '- a' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
commit_all release
git_ checkout -q pr
git_ merge -q --no-edit main >/dev/null
echo "      (merged text: $(sed -n '3,12p' "$repo/CHANGELOG.md" | tr '\n' '|'))"
expect "a merge that slides the PR's entry into the released section fails" 1 \
  "under '## [0.7.0] - 2026-11-20': - mine"

# main only adds a line to a released section; the PR's entry stays under Unreleased.
new_repo merge-brings-lines
git_ checkout -q -b pr
touch_code
write_changelog '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' '### Added' '' '- mine' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old'
commit_all pr
git_ checkout -q main
write_changelog '# Changelog' '' '## [Unreleased]' '' '### Fixed' '' '- a' '' \
  '## [0.6.0] - 2026-11-11' '' '### Added' '' '- old' '- from main'
commit_all main-entry
git_ checkout -q pr
git_ merge -q --no-edit main >/dev/null
expect "lines a merge from main brings into a released section are not the PR's" 0 "CHANGELOG.md is changed"

echo
if [ "$failures" -eq 0 ]; then
  echo "all cases gave the right answer"
else
  echo "$failures case(s) gave the wrong answer"
  exit 1
fi
