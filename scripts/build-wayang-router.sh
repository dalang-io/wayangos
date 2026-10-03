#!/bin/bash
# Fetch the pinned static wayang-router binary and stage it at
# $BUILD_DIR/wayang-router/wayang-router. build-rootfs.sh installs it to
# /usr/bin/wayang-router, so a WayangOS image carries the router CLI out of the
# box (no separate deploy to /data/bin needed).
#
# The binary is published at the WayangOS tool mirror (WAYANG_ROUTER_BASE_URL),
# so the fetch needs no GitHub auth — the self-hosted runner cannot run the
# box's snap `gh` (snap refuses under a systemd runner cgroup). A checksum
# mismatch always aborts; a missing mirror warns and skips.
#
# Usage: ./scripts/build-wayang-router.sh
# Env:   BUILD_DIR                    (default: $HOME/wayangos-build)
#        WAYANG_ROUTER_VERSION / WAYANG_ROUTER_SHA256  override the pin (bump together)
#        WAYANG_ROUTER_BASE_URL       tool mirror (default https://wayang.dalang.io/edge/tools)
#        ALLOW_UNVERIFIED=1           skip the sha256 check (explicit opt-out only)
set -euo pipefail
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together (sha256 of the published binary).
WAYANG_ROUTER_VERSION="${WAYANG_ROUTER_VERSION:-0.8.0}"
WAYANG_ROUTER_SHA256="${WAYANG_ROUTER_SHA256:-d7a8f1f3aa4d952652da865108e5eda9ad9f19a6a87abc4b66f8313b2ba8e9d2}"
BASE_URL="${WAYANG_ROUTER_BASE_URL:-https://wayang.dalang.io/edge/tools}"
ASSET="wayang-router-v$WAYANG_ROUTER_VERSION-x86_64-unknown-linux-musl"
OUTDIR="$BUILD/wayang-router"
OUT="$OUTDIR/wayang-router"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$WAYANG_ROUTER_VERSION" ]; then
    echo "wayang-router $WAYANG_ROUTER_VERSION already staged: $OUT"
    exit 0
fi
mkdir -p "$OUTDIR"
if ! curl -fsSL -o "$OUTDIR/$ASSET.part" "$BASE_URL/$ASSET"; then
    echo "WARNING: could not fetch $BASE_URL/$ASSET — skipping static wayang-router (optional)" >&2
    rm -f "$OUTDIR/$ASSET.part"
    exit 0
fi
mv -f "$OUTDIR/$ASSET.part" "$OUTDIR/$ASSET"
GOT="$(sha256sum "$OUTDIR/$ASSET" | cut -d' ' -f1)"
if [ "${ALLOW_UNVERIFIED:-0}" = 1 ]; then
    echo "WARNING: ALLOW_UNVERIFIED=1 — skipping sha256 check for $ASSET" >&2
elif [ "$GOT" != "$WAYANG_ROUTER_SHA256" ]; then
    echo "ERROR: wayang-router sha256 mismatch (got $GOT, want $WAYANG_ROUTER_SHA256)" >&2
    rm -f "$OUTDIR/$ASSET"
    exit 1
fi
install -m 755 "$OUTDIR/$ASSET" "$OUT"
rm -f "$OUTDIR/$ASSET"
echo "$WAYANG_ROUTER_VERSION" > "$OUT.version"
echo "wayang-router $WAYANG_ROUTER_VERSION -> $OUT"