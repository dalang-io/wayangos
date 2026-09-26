#!/bin/bash
# Build the WayangPOS framebuffer app into a static binary.
# Source lives in the private repo dalang-io/wayang-pos, fetched at a pinned ref.
# Usage: ./scripts/build-pos.sh
# Env:   BUILD_DIR    build output directory (default: $HOME/wayangos-build)
#        POS_SRC_DIR  use this local wayang-pos checkout instead of fetching
#        POS_REPO     git URL (default: https://github.com/dalang-io/wayang-pos.git)
#        POS_REF      tag/branch/commit to build (default: v3.1.0)
#        POS_TOKEN    GitHub token for the private repo (CI); locally git's
#                     credential helper (e.g. `gh auth setup-git`) is used
set -e

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
POS_REPO="${POS_REPO:-https://github.com/dalang-io/wayang-pos.git}"
POS_REF="${POS_REF:-v3.1.0}"
OUT="$BUILD/wayang-pos-static"
SQLITE="$BUILD/sqlite3.c"

mkdir -p "$BUILD"

if [ -n "${POS_SRC_DIR:-}" ]; then
    SRC_DIR="$POS_SRC_DIR"
else
    SRC_DIR="$BUILD/wayang-pos"
    url="$POS_REPO"
    if [ -n "${POS_TOKEN:-}" ]; then
        url="${POS_REPO/https:\/\//https://x-access-token:${POS_TOKEN}@}"
    fi
    echo "=== Fetching WayangPOS ($POS_REF) ==="
    rm -rf "$SRC_DIR"
    git init -q "$SRC_DIR"
    if ! git -C "$SRC_DIR" fetch -q --depth 1 "$url" "$POS_REF"; then
        echo "ERROR: cannot fetch $POS_REPO@$POS_REF (private repo: set POS_TOKEN," >&2
        echo "       run 'gh auth setup-git', or point POS_SRC_DIR at a checkout)" >&2
        exit 1
    fi
    git -C "$SRC_DIR" checkout -q FETCH_HEAD
fi

SRC="$SRC_DIR/fbpos-v3.c"
if [ ! -f "$SRC" ]; then
    echo "ERROR: POS source not found at $SRC" >&2
    exit 1
fi

echo "=== Building WayangPOS ==="
echo "  Source: $SRC"
echo "  Output: $OUT"

if [ -f "$SQLITE" ]; then
    echo "  SQLite: $SQLITE (persistence enabled)"
    gcc -static -O2 -I"$BUILD" -o "$OUT" "$SRC" "$SQLITE" \
        -DSQLITE_INTEGRATION -lpthread -lm
else
    echo "  SQLite: not found (building without persistence)"
    gcc -static -O2 -o "$OUT" "$SRC" -lm
fi

echo "  Built: $OUT ($(du -h "$OUT" | cut -f1))"
