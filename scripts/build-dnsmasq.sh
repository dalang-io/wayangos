#!/bin/bash
# Build a static dnsmasq for the WayangOS rootfs: wayang-fw renders
# /data/etc/fw/dnsmasq.conf from the [[dhcp]] / [[dns]] blocks and runs this
# binary for per-zone DHCP + DNS (docs/DHCP-DNS.md). Without dnsmasq the
# firewall still works; DHCP/DNS is simply off (warn-and-noop).
#
# Moved here from wayang-fw/scripts/ (that repo renders the config but does not
# build the image). Staged by build-rootfs.sh via
# `install_tool dnsmasq/dnsmasq /usr/sbin/dnsmasq build-dnsmasq.sh` and built in
# ci-build.sh alongside nft/wg/iproute2/bird/radvd.
#
# It follows scripts/build-radvd.sh: pinned version + sha256, fully static,
# best-effort (a missing build tool warns and exits 0; a checksum mismatch
# always aborts). dnsmasq is GPL-2.0 and is built unmodified upstream.
#
# Usage: ./scripts/build-dnsmasq.sh
# Env:   BUILD_DIR    (default: $HOME/wayangos-build)
#        DNSMASQ_VERSION / DNSMASQ_SHA256  override the pin (bump both together)
#        ALLOW_UNVERIFIED=1                skip the sha256 check (explicit opt-out)
set -euo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together (the sha256 is from the upstream
# tarball at https://thekelleys.org.uk/dnsmasq/).
DNSMASQ_VERSION="${DNSMASQ_VERSION:-2.93}"
DNSMASQ_SHA256="${DNSMASQ_SHA256:-0c00d4e5c97c8306e5fb932b348b34269c9c29a0e7df0e8e82958b407092bc19}"
DNSMASQ_URL="https://thekelleys.org.uk/dnsmasq/dnsmasq-$DNSMASQ_VERSION.tar.xz"

SRC="$BUILD/dnsmasq-src"
OUTDIR="$BUILD/dnsmasq"
OUT="$OUTDIR/dnsmasq"
JOBS="$(nproc)"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$DNSMASQ_VERSION" ]; then
    echo "dnsmasq $DNSMASQ_VERSION already staged: $OUT"
    exit 0
fi

# Best-effort: the rest of the image is still valid without dnsmasq.
for tool in cc make curl tar; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "WARNING: $tool not found — skipping static dnsmasq (optional)" >&2
        exit 0
    fi
done
if ! echo 'int main(void){return 0;}' | cc -x c - -static -o /dev/null 2>/dev/null; then
    echo "WARNING: no static libc (cc -static) — skipping static dnsmasq (optional)" >&2
    exit 0
fi

mkdir -p "$SRC" "$OUTDIR"
cd "$SRC"

# fetch URL FILE SHA256 — download once, always verify before extracting
fetch() {
    if [ ! -f "$2" ]; then
        curl -fsSL -o "$2.part" "$1"
        mv -f "$2.part" "$2"
    fi
    if [ "${ALLOW_UNVERIFIED:-0}" = 1 ]; then
        echo "WARNING: ALLOW_UNVERIFIED=1 — skipping sha256 check for $2" >&2
    elif ! echo "$3  $2" | sha256sum -c --quiet -; then
        echo "ERROR: sha256 mismatch for $2 (expected $3)" >&2
        rm -f "$2"
        exit 1
    fi
}
fetch "$DNSMASQ_URL" "dnsmasq-$DNSMASQ_VERSION.tar.xz" "$DNSMASQ_SHA256"
rm -rf "dnsmasq-$DNSMASQ_VERSION"
tar xf "dnsmasq-$DNSMASQ_VERSION.tar.xz"

echo "=== dnsmasq $DNSMASQ_VERSION (static; no DNSSEC/Lua) ==="
(
    cd "dnsmasq-$DNSMASQ_VERSION"
    # Default target (`all`) sets VERSION/build_cflags for the sub-make; build
    # the target directly and VERSION ends up undefined. NO_DNSSEC/
    # NO_LUASCRIPT keep it dependency-free (no nettle, no Lua); -Os shrinks it.
    # dnsmasq's default install paths are not used: wayang-fw passes
    # --conf-file/--pid-file explicitly.
    make -s -j"$JOBS" \
        COPTS="-DNO_DNSSEC -DNO_LUASCRIPT -Os" \
        LDFLAGS="-static"
) >"$SRC/build.log" 2>&1 || { tail -n 40 "$SRC/build.log" >&2; echo "ERROR: dnsmasq build failed (log: $SRC/build.log)" >&2; exit 1; }

BIN="$SRC/dnsmasq-$DNSMASQ_VERSION/src/dnsmasq"
[ -f "$BIN" ] || { echo "ERROR: dnsmasq did not build" >&2; exit 1; }
strip -o "$OUT" "$BIN"
file "$OUT" | grep -q "statically linked" || { echo "ERROR: $OUT is not static" >&2; exit 1; }
printf '%s\n' "$DNSMASQ_VERSION" > "$OUT.version"
echo "  dnsmasq: $("$OUT" --version 2>&1 | head -n1) · $(du -h "$OUT" | cut -f1) → $OUT"
