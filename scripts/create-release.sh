#!/usr/bin/env bash

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null && pwd)"
readonly SCRIPT_DIR
readonly repo=git@github.com:Mic92/envfs
readonly branch=main
cd "$SCRIPT_DIR/.."

version=${1:-}
if [[ -z "$version" ]]; then
  echo "USAGE: $0 version" >&2
  exit 1
fi

if [[ "$(git symbolic-ref --short HEAD)" != "$branch" ]]; then
  echo "must be on main branch" >&2
  exit 1
fi

# ensure we are up-to-date
uncommitted_changes=$(git diff --compact-summary)
if [[ -n "$uncommitted_changes" ]]; then
  echo -e "There are uncommitted changes, exiting:\n${uncommitted_changes}" >&2
  exit 1
fi
git pull "$repo" "$branch"
unpushed_commits=$(git log --format=oneline "origin/$branch..$branch")
if [[ "$unpushed_commits" != "" ]]; then
  echo -e "\nThere are unpushed changes, exiting:\n$unpushed_commits" >&2
  exit 1
fi
cargo set-version "$version"
cargo build
git add Cargo.lock Cargo.toml
git commit -m "bump version ${version}"
git tag -e "${version}"

echo "now run 'git push origin $version'"
