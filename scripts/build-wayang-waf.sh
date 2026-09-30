#!/bin/bash
# Fetch the pinned static wayang-waf binary and stage it at
# $BUILD_DIR/wayang-waf/wayang-waf. build-rootfs.sh installs it to
# /usr/bin/wayang-waf, so an image can run the WAF (mode A reverse proxy,
# Coraza + OWASP CRS) that wayang-fw's `[waf]` block configures.
#
# Source: the Go module `waf/` in dalang-io/wayang-fw. Build it there with
#
#     cd wayang-fw/waf
#     CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -trimpath \
#         -ldflags="-s -w" -o wayang-waf-linux-amd64 .
#
# and publish the result at the tool mirror. The asset name is Go-style
# (`-linux-amd64`) rather than the Rust triple used by build-wayang-{fw,router}.sh:
# a Go static binary links no libc, so a "musl" suffix would be a lie.
#
# The binary is optional: wayang-fw warns and no-ops without it (a firewall that
# does not use `[waf]` is unaffected), and a missing mirror must not fail a build.
#
# Usage: ./scripts/build-wayang-waf.sh
# Env:   BUILD_DIR                (default: $HOME/wayangos-build)
#        WAYANG_WAF_VERSION / WAYANG_WAF_SHA256  override the pin (bump together)
#        WAYANG_WAF_BASE_URL      tool mirror (default https://wayang.dalang.io/edge/tools)
#        ALLOW_UNVERIFIED=1       skip the sha256 check (explicit opt-out only)
set -euo pipefail
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together (sha256 of the published binary).
WAYANG_WAF_VERSION="${WAYANG_WAF_VERSION:-0.1.0}"
WAYANG_WAF_SHA256="${WAYANG_WAF_SHA256:-a384270a7e7eb686bfe027bae7010662bc20fd3199fd4d8daddc1311198345ad}"
BASE_URL="${WAYANG_WAF_BASE_URL:-https://wayang.dalang.io/edge/tools}"
ASSET="wayang-waf-v$WAYANG_WAF_VERSION-linux-amd64"
OUTDIR="$BUILD/wayang-waf"
OUT="$OUTDIR/wayang-waf"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$WAYANG_WAF_VERSION" ]; then
    echo "wayang-waf $WAYANG_WAF_VERSION already staged: $OUT"
    exit 0
fi
mkdir -p "$OUTDIR"
if ! curl -fsSL -o "$OUTDIR/$ASSET.part" "$BASE_URL/$ASSET"; then
    echo "WARNING: could not fetch $BASE_URL/$ASSET — skipping static wayang-waf (optional)" >&2
    rm -f "$OUTDIR/$ASSET.part"
    exit 0
fi
mv -f "$OUTDIR/$ASSET.part" "$OUTDIR/$ASSET"
GOT="$(sha256sum "$OUTDIR/$ASSET" | cut -d' ' -f1)"
if [ "${ALLOW_UNVERIFIED:-0}" = 1 ]; then
    echo "WARNING: ALLOW_UNVERIFIED=1 — skipping sha256 check for $ASSET" >&2
elif [ "$GOT" != "$WAYANG_WAF_SHA256" ]; then
    echo "ERROR: wayang-waf sha256 mismatch (got $GOT, want $WAYANG_WAF_SHA256)" >&2
    rm -f "$OUTDIR/$ASSET"
    exit 1
fi
install -m 755 "$OUTDIR/$ASSET" "$OUT"
rm -f "$OUTDIR/$ASSET"
echo "$WAYANG_WAF_VERSION" > "$OUT.version"
echo "wayang-waf $WAYANG_WAF_VERSION -> $OUT"
