#!/usr/bin/env bash
# Run every smoke integration test while bounding temporary disk usage. Pytest
# otherwise retains repositories from the whole suite until process exit.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BATCH_SIZE="${LORE_SMOKE_BATCH_SIZE:-1}"
CLIENT_BINARY="${LORE_CLIENT_BINARY:-release}"
SERVER_BINARY="${LORE_SERVER_BINARY:-release}"
START_FILE="${LORE_SMOKE_START_FILE:-1}"

[[ "$BATCH_SIZE" =~ ^[1-9][0-9]*$ ]] || {
    echo "LORE_SMOKE_BATCH_SIZE must be a positive integer" >&2
    exit 2
}
[[ "$START_FILE" =~ ^[1-9][0-9]*$ ]] || {
    echo "LORE_SMOKE_START_FILE must be a positive integer" >&2
    exit 2
}

cd "$ROOT"
shopt -s nullglob
TEST_FILES=(scripts/test/test_*.py)
[[ "${#TEST_FILES[@]}" -gt 0 ]] || {
    echo "no Lore smoke test files found" >&2
    exit 1
}

TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/lore-smoke.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

for ((index = START_FILE - 1; index < ${#TEST_FILES[@]}; index += BATCH_SIZE)); do
    batch=$((index / BATCH_SIZE + 1))
    batch_root="$TEST_ROOT/batch-$batch"
    batch_files=("${TEST_FILES[@]:index:BATCH_SIZE}")
    echo "Running Lore smoke batch $batch (${#batch_files[@]} files)..."
    set +e
    uv run --locked pytest "${batch_files[@]}" -m smoke \
        --lore-client-binary="$CLIENT_BINARY" \
        --lore-server-binary="$SERVER_BINARY" \
        --basetemp="$batch_root"
    status=$?
    set -e
    rm -rf "$batch_root"
    if [[ "$status" -eq 5 ]]; then
        echo "Batch $batch contains no smoke-marked tests; continuing."
    elif [[ "$status" -ne 0 ]]; then
        exit "$status"
    fi
done
