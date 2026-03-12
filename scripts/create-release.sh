#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null && pwd)"
cd "$SCRIPT_DIR/.."

readonly repo=git@github.com:Mic92/envfs
readonly branch=main

version="${1:-}"
if [[ -z $version ]]; then
  echo "USAGE: $0 version" >&2
  exit 1
fi

if [[ "$(git symbolic-ref --short HEAD)" != "$branch" ]]; then
  echo "must be on main branch" >&2
  exit 1
fi

waitForPr() {
  local pr=$1
  while true; do
    if gh pr view "$pr" | grep -q 'MERGED'; then
      break
    fi
    echo "Waiting for PR to be merged..."
    sleep 5
  done
}

# ensure we are up-to-date
uncommitted_changes=$(git diff --compact-summary)
if [[ -n $uncommitted_changes ]]; then
  echo -e "There are uncommitted changes, exiting:\n${uncommitted_changes}" >&2
  exit 1
fi
git pull "$repo" "$branch"
unpushed_commits=$(git log --format=oneline "origin/$branch..$branch")
if [[ $unpushed_commits != "" ]]; then
  echo -e "\nThere are unpushed changes, exiting:\n$unpushed_commits" >&2
  exit 1
fi
# make sure tag does not exist
if git tag -l | grep -q "^${version}\$"; then
  echo "Tag ${version} already exists, exiting" >&2
  exit 1
fi
cargo set-version "$version"
cargo update --package envfs
git add Cargo.lock Cargo.toml
git branch -D "release-${version}" || true
git checkout -b "release-${version}"
git commit -m "bump version ${version}"
git push origin "release-${version}"
pr_url=$(gh pr create \
  --base "$branch" \
  --head "release-${version}" \
  --title "Release ${version}" \
  --body "Release ${version} of envfs")

# Extract PR number from URL
pr_number=$(echo "$pr_url" | grep -oE '[0-9]+$')

# Enable auto-merge with specific merge method and delete branch
gh pr merge "$pr_number" --auto --merge --delete-branch
git checkout "$branch"

waitForPr "release-${version}"
git pull "$repo" "$branch"
gh release create "${version}" --draft --title "${version}" --notes ""
