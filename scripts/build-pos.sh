#!/bin/bash
# Build the WayangPOS framebuffer app (wayangos-pos/fbpos-v3.c) into a static binary.
# Usage: ./scripts/build-pos.sh
# Env:   BUILD_DIR   build output directory (default: $HOME/wayangos-build)
set -e

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
SRC="$REPO_DIR/wayangos-pos/fbpos-v3.c"
OUT="$BUILD/wayang-pos-static"
SQLITE="$BUILD/sqlite3.c"

if [ ! -f "$SRC" ]; then
    echo "ERROR: POS source not found at $SRC" >&2
    exit 1
fi

mkdir -p "$BUILD"

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
