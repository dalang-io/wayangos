#!/bin/bash
# Fetch the pinned static wayang-fw binary and stage it at
# $BUILD_DIR/wayang-fw/wayang-fw. build-rootfs.sh installs it to
# /usr/bin/wayang-fw, so a WayangOS image carries the firewall CLI out of the
# box (no separate deploy to /data/bin needed). The repo is private, so the
# fetch uses `gh` (which must be authenticated on the builder). Best-effort: a
# missing gh skips the tool so the image still builds; a checksum mismatch
# always aborts.
#
# Usage: ./scripts/build-wayang-fw.sh
# Env:   BUILD_DIR                (default: $HOME/wayangos-build)
#        WAYANG_FW_VERSION / WAYANG_FW_SHA256  override the pin (bump together)
#        ALLOW_UNVERIFIED=1       skip the sha256 check (explicit opt-out only)
set -euo pipefail
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together (sha256 of the release asset).
WAYANG_FW_VERSION="${WAYANG_FW_VERSION:-0.3.1}"
WAYANG_FW_SHA256="${WAYANG_FW_SHA256:-9c434fd798775a5aab4475eaa9143c36437cf56868b8fcb60f333e420ec03eee}"
ASSET="wayang-fw-v$WAYANG_FW_VERSION-x86_64-unknown-linux-musl"
OUTDIR="$BUILD/wayang-fw"
OUT="$OUTDIR/wayang-fw"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$WAYANG_FW_VERSION" ]; then
    echo "wayang-fw $WAYANG_FW_VERSION already staged: $OUT"
    exit 0
fi
if ! command -v gh >/dev/null 2>&1; then
    echo "WARNING: gh not found — skipping static wayang-fw (optional)" >&2
    exit 0
fi
mkdir -p "$OUTDIR"
# `gh release download` on the builder's gh (2.74 dev) reports success but
# writes nothing; fetch the asset bytes through the API instead (authenticated,
# works for a private repo).
AID="$(gh api "repos/dalang-io/wayang-fw/releases/tags/v$WAYANG_FW_VERSION" \
    --jq ".assets[] | select(.name == \"$ASSET\") | .id")"
if [ -z "$AID" ]; then
    echo "WARNING: wayang-fw $WAYANG_FW_VERSION has no asset $ASSET — skipping" >&2
    exit 0
fi
gh api "repos/dalang-io/wayang-fw/releases/assets/$AID" \
    -H 'Accept: application/octet-stream' > "$OUTDIR/$ASSET.part"
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
