#!/usr/bin/env bash
# Build and test a Portals Lore release locally, then optionally publish its
# immutable tag. GitHub builds and signs the distributable artifacts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG="${1:-}"
MODE="${2:-}"

usage() {
    echo "usage: scripts/release-local.sh <vX.Y.Z-portals.N> [--publish]" >&2
    exit 2
}

[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-portals\.[1-9][0-9]*$ ]] || usage
[[ -z "$MODE" || "$MODE" == "--publish" ]] || usage

cd "$ROOT"

for command in cargo git uv; do
    command -v "$command" >/dev/null || {
        echo "$command is required" >&2
        exit 2
    }
done

AVAILABLE_KIB="$(df -Pk . | awk 'NR == 2 {print $4}')"
REQUIRED_KIB=$((12 * 1024 * 1024))
[[ "$AVAILABLE_KIB" -ge "$REQUIRED_KIB" ]] || {
    echo "release requires at least 12 GiB of free disk space" >&2
    exit 1
}

VERSION="${TAG#v}"
WORKSPACE_VERSION="$(sed -n 's/^version = "\([^"]*\)"$/\1/p' Cargo.toml | head -n 1)"
[[ "$VERSION" == "$WORKSPACE_VERSION" ]] || {
    echo "tag $TAG does not match workspace version $WORKSPACE_VERSION" >&2
    exit 1
}

CURRENT_BRANCH="$(git branch --show-current)"
EXPECTED_BRANCH="${VERSION%%-portals.*}"
[[ "$CURRENT_BRANCH" == "$EXPECTED_BRANCH" ]] || {
    echo "release must run from branch $EXPECTED_BRANCH (found ${CURRENT_BRANCH:-detached HEAD})" >&2
    exit 1
}

ORIGIN_URL="$(git remote get-url origin)"
case "$ORIGIN_URL" in
    https://github.com/portalshq/lore|https://github.com/portalshq/lore.git|git@github.com:portalshq/lore|git@github.com:portalshq/lore.git) ;;
    *)
        echo "origin must be portalshq/lore (found $ORIGIN_URL)" >&2
        exit 1
        ;;
esac

git fetch origin "$CURRENT_BRANCH" --tags
git merge-base --is-ancestor "origin/$CURRENT_BRANCH" HEAD || {
    echo "local $CURRENT_BRANCH does not contain the latest origin/$CURRENT_BRANCH" >&2
    exit 1
}

git diff --quiet && git diff --cached --quiet || {
    echo "release requires a clean working tree" >&2
    exit 1
}
[[ -z "$(git status --short)" ]] || {
    echo "release requires no untracked files" >&2
    exit 1
}

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    echo "release tag already exists locally: $TAG" >&2
    exit 1
fi

echo "Building Lore client and server from $(git rev-parse HEAD)..."
CARGO_INCREMENTAL=0 cargo build --locked --release \
    -p lore-client --bin lore \
    -p lore-server --bin loreserver

echo "Running security-sensitive Rust tests..."
CARGO_INCREMENTAL=0 cargo test --locked --release \
    -p lore-credential \
    -p lore-server

echo "Running Lore CLI/server smoke integration tests..."
scripts/run-smoke-tests.sh

./target/release/lore --version
./target/release/loreserver --version

if [[ "$MODE" != "--publish" ]]; then
    echo "Local release gate passed for $TAG. Re-run with --publish after review."
    exit 0
fi

command -v gh >/dev/null || {
    echo "gh is required to verify remote tag availability" >&2
    exit 2
}
gh auth status --hostname github.com >/dev/null

if git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then
    echo "release tag already exists on origin: $TAG" >&2
    exit 1
fi

git push origin "HEAD:refs/heads/$CURRENT_BRANCH"
git tag -a "$TAG" -m "Portals Lore $VERSION"
git push origin "refs/tags/$TAG"

echo "Published $TAG. GitHub will build, sign, and attach release artifacts."
