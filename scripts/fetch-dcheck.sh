#!/bin/bash
# Fetch the dcheck static binary from its release channel and stage it at
# $BUILD_DIR/dcheck/dcheck; build-rootfs.sh installs it to /usr/bin/dcheck so
# every WayangOS image ships it by default.
#
# Release layout (see https://wayang.dalang.io/dcheck/install.sh):
#   <base>/LATEST
#   <base>/v<ver>/dcheck-<ver>-<target>.tar.gz
#   <base>/v<ver>/SHA256SUMS
#
# Best-effort: warns and leaves any existing binary in place on failure.
#
# Env: BUILD_DIR, DCHECK_VERSION (default: <base>/LATEST), DCHECK_BASE_URL,
#      DCHECK_TARGET (default x86_64-unknown-linux-musl).
set -uo pipefail

BUILD_DIR="${BUILD_DIR:-$HOME/wayangos-build}"
BASE="${DCHECK_BASE_URL:-https://wayang.dalang.io/dcheck}"
BASE="${BASE%/}"
TARGET="${DCHECK_TARGET:-x86_64-unknown-linux-musl}"
OUT="$BUILD_DIR/dcheck"

mkdir -p "$OUT"
if [ -x "$OUT/dcheck" ]; then
    echo "dcheck already staged: $OUT/dcheck"
    exit 0
fi
for t in curl tar; do
    command -v "$t" >/dev/null 2>&1 || { echo "fetch-dcheck: missing '$t' — skipping" >&2; exit 0; }
done

VER="${DCHECK_VERSION:-}"
if [ -z "$VER" ]; then
    VER="$(curl -fsSL "$BASE/LATEST" | tr -d ' \r\n')" || true
fi
VER="${VER#v}"
if [ -z "$VER" ]; then
    echo "fetch-dcheck: cannot read $BASE/LATEST — skipping" >&2
    exit 0
fi

NAME="dcheck-$VER-$TARGET.tar.gz"
TMP="$OUT/.dl"
mkdir -p "$TMP"
if ! curl -fsSL --retry 3 -o "$TMP/$NAME" "$BASE/v$VER/$NAME"; then
    echo "fetch-dcheck: download failed ($BASE/v$VER/$NAME) — skipping" >&2
    exit 0
fi

# Verify against the published checksums when available.
curl -fsSL --retry 3 -o "$TMP/SHA256SUMS" "$BASE/v$VER/SHA256SUMS" 2>/dev/null || true
if [ -f "$TMP/SHA256SUMS" ]; then
    want="$(grep " $NAME\$" "$TMP/SHA256SUMS" | awk '{print $1}' | head -1)"
    if [ -n "$want" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            got="$(sha256sum "$TMP/$NAME" | cut -d' ' -f1)"
        else
            got="$(shasum -a 256 "$TMP/$NAME" | cut -d' ' -f1)"
        fi
        if [ "$got" != "$want" ]; then
            echo "fetch-dcheck: sha256 mismatch for $NAME — skipping" >&2
            exit 0
        fi
    fi
fi

tar -xzf "$TMP/$NAME" -C "$TMP" 2>/dev/null || { echo "fetch-dcheck: extract failed — skipping" >&2; exit 0; }
if [ -f "$TMP/dcheck" ]; then
    cp -f "$TMP/dcheck" "$OUT/dcheck"
    chmod 755 "$OUT/dcheck"
    echo "dcheck $VER -> $OUT/dcheck"
else
    echo "fetch-dcheck: no dcheck binary in tarball — skipping" >&2
fi
