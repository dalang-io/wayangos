#!/bin/bash
# Fetch the pinned static wayang-fw binary and stage it at
# $BUILD_DIR/wayang-fw/wayang-fw. build-rootfs.sh installs it to
# /usr/bin/wayang-fw, so a WayangOS image carries the firewall CLI out of the
# box (no separate deploy to /data/bin needed).
#
# The binary is published at the WayangOS tool mirror (WAYANG_FW_BASE_URL), so
# the fetch needs no GitHub auth — the self-hosted runner cannot run the box's
# snap `gh` (snap refuses under a systemd runner cgroup). A checksum mismatch
# always aborts; a missing mirror warns and skips.
#
# Usage: ./scripts/build-wayang-fw.sh
# Env:   BUILD_DIR                (default: $HOME/wayangos-build)
#        WAYANG_FW_VERSION / WAYANG_FW_SHA256  override the pin (bump together)
#        WAYANG_FW_BASE_URL       tool mirror (default https://wayang.dalang.io/edge/tools)
#        ALLOW_UNVERIFIED=1       skip the sha256 check (explicit opt-out only)
set -euo pipefail
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together (sha256 of the published binary).
WAYANG_FW_VERSION="${WAYANG_FW_VERSION:-0.4.0}"
WAYANG_FW_SHA256="${WAYANG_FW_SHA256:-3399195adee54531c81e9c179c846872ed71e51aeed287ba91573572f8d5012e}"
BASE_URL="${WAYANG_FW_BASE_URL:-https://wayang.dalang.io/edge/tools}"
ASSET="wayang-fw-v$WAYANG_FW_VERSION-x86_64-unknown-linux-musl"
OUTDIR="$BUILD/wayang-fw"
OUT="$OUTDIR/wayang-fw"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$WAYANG_FW_VERSION" ]; then
    echo "wayang-fw $WAYANG_FW_VERSION already staged: $OUT"
    exit 0
fi
mkdir -p "$OUTDIR"
if ! curl -fsSL -o "$OUTDIR/$ASSET.part" "$BASE_URL/$ASSET"; then
    echo "WARNING: could not fetch $BASE_URL/$ASSET — skipping static wayang-fw (optional)" >&2
    rm -f "$OUTDIR/$ASSET.part"
    exit 0
fi
mv -f "$OUTDIR/$ASSET.part" "$OUTDIR/$ASSET"
GOT="$(sha256sum "$OUTDIR/$ASSET" | cut -d' ' -f1)"
if [ "${ALLOW_UNVERIFIED:-0}" = 1 ]; then
    echo "WARNING: ALLOW_UNVERIFIED=1 — skipping sha256 check for $ASSET" >&2
elif [ "$GOT" != "$WAYANG_FW_SHA256" ]; then
    echo "ERROR: wayang-fw sha256 mismatch (got $GOT, want $WAYANG_FW_SHA256)" >&2
    rm -f "$OUTDIR/$ASSET"
    exit 1
fi
install -m 755 "$OUTDIR/$ASSET" "$OUT"
rm -f "$OUTDIR/$ASSET"
echo "$WAYANG_FW_VERSION" > "$OUT.version"
echo "wayang-fw $WAYANG_FW_VERSION -> $OUT"