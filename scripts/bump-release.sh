#!/usr/bin/env bash
# Bump the Portals Lore version in every place it is pinned, including
# Cargo.lock, so the release gate's --locked builds always pass. Run this
# instead of hand-editing Cargo.toml.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="${1:-}"

[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-portals\.[1-9][0-9]*$ ]] || {
    echo "usage: scripts/bump-release.sh <vX.Y.Z-portals.N>" >&2
    exit 2
}

cd "$ROOT"
VERSION="${TAG#v}"

sed -i '' "s/^version = \"[^\"]*\"/version = \"$VERSION\"/" Cargo.toml
sed -i '' "s/LORE_INTERFACE_VERSION \"[^\"]*\"/LORE_INTERFACE_VERSION \"$VERSION\"/" lore-capi/lore.h
cargo update --workspace

git diff --stat
echo "Review, commit, then: scripts/release-local.sh $TAG"