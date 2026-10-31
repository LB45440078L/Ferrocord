#!/usr/bin/env bash
# Render GitHub release notes for a tag with git-cliff (config: cliff.toml).
#
#   .github/scripts/release-notes.sh <tag> [output-file]
#
# Final releases (vX.Y.Z) cover everything since the previous final release,
# folding in its pre-releases. Pre-releases (vX.Y.Z-rc.N) cover everything
# since the previous tag of any kind. Needs full history (fetch-depth: 0).
set -euo pipefail

tag=${1:?usage: $0 <tag> [output-file]}
out_args=()
[[ -n ${2:-} ]] && out_args=(--output "$2")

if [[ $tag == *-* ]]; then
  prev=$(git describe --tags --abbrev=0 --match 'v[0-9]*' "$tag^" 2>/dev/null || true)
else
  prev=$(git describe --tags --abbrev=0 --match 'v[0-9]*' --exclude 'v*-*' "$tag^" 2>/dev/null || true)
fi

# --ignore-tags '.*' renders the whole range as a single release named <tag>.
if [[ -n $prev ]]; then
  echo "release notes for $tag: commits $prev..$tag" >&2
  git cliff "$prev..$tag" --tag "$tag" --ignore-tags '.*' --strip header "${out_args[@]}"
else
  # git-cliff has no "everything up to <rev>" range, so render from HEAD.
  if [[ $(git rev-parse HEAD) != $(git rev-parse "$tag^{commit}") ]]; then
    echo "check out $tag first (first release: notes are rendered from HEAD)" >&2
    exit 1
  fi
  echo "release notes for $tag: first release, all history" >&2
  git cliff --tag "$tag" --ignore-tags '.*' --strip header "${out_args[@]}"
fi
