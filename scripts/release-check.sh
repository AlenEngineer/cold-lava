#!/usr/bin/env bash
# Is this commit a release, and is it ready to be one?
#
# A commit is a release when the version in Cargo.toml has no vX.Y.Z tag yet (or when the
# workflow runs for a pushed vX.Y.Z tag). A release must:
#   - have a "## [X.Y.Z]" section in CHANGELOG.md,
#   - be a higher version than the latest release,
#   - match the tag, when run for a tag.
#
# Prints version=… and release=true|false (also to $GITHUB_OUTPUT on GitHub Actions).
# Needs the repository's tags (fetch-depth: 0). Run it locally before opening a release PR.
set -euo pipefail

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
if [ -z "$version" ]; then
  echo "::error file=Cargo.toml::No version found in Cargo.toml"
  exit 1
fi
tag="v$version"
on_tag=false
if [ "${GITHUB_REF_TYPE:-}" = "tag" ]; then
  on_tag=true
  if [ "${GITHUB_REF_NAME:-}" != "$tag" ]; then
    echo "::error::Tag ${GITHUB_REF_NAME:-} doesn't match the version in Cargo.toml ($version)"
    exit 1
  fi
fi

release=false
if $on_tag || ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  release=true
  if ! grep -q "^## \[$version\]" CHANGELOG.md; then
    echo "::error file=CHANGELOG.md::Version $version is new but CHANGELOG.md has no '## [$version] - date' section for it"
    exit 1
  fi
  latest=$(git tag --list 'v[0-9]*' --sort=-v:refname | grep -vx "$tag" | head -n 1 || true)
  if [ -n "$latest" ]; then
    highest=$(printf '%s\n%s\n' "${latest#v}" "$version" | sort -V | tail -n 1)
    if [ "$highest" != "$version" ]; then
      echo "::error file=Cargo.toml::Version $version is lower than the latest release $latest"
      exit 1
    fi
  fi
fi

echo "version=$version"
echo "release=$release"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "version=$version"
    echo "release=$release"
  } >> "$GITHUB_OUTPUT"
fi
