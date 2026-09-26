#!/bin/bash
# Build a static `wg` (wireguard-tools) for the rootfs: build-rootfs.sh installs
# $BUILD_DIR/wg/wg to /usr/bin/wg when present. wayang-router drives WireGuard
# over generic netlink itself; `wg` is for operators and debugging
# (`wg show`, `wg genkey`, `wg pubkey`). No wg-quick (it needs bash).
#
# The WireGuard *kernel* side (CONFIG_WIREGUARD) is still under hardware
# bisect (docs/ROUTER-KERNEL-BISECT.md); without it `wg show` just reports no
# interfaces, and genkey/pubkey still work.
#
# Usage: ./scripts/build-wg.sh
# Env:   BUILD_DIR   (default: $HOME/wayangos-build)
set -euo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together. The sha256 is of the upstream
# snapshot tarball whose detached signature (…/wireguard-tools-$V.tar.asc, over
# the uncompressed tar) was verified against Jason A. Donenfeld's key
# AB9942E6D4A4CFC3412620A749FC7012A5DE03AE when the pin was set.
WG_VERSION="1.0.20260223"
WG_SHA256="af459827b80bfd31b83b08077f4b5843acb7d18ad9a33a2ef532d3090f291fbf"
WG_URL="https://git.zx2c4.com/wireguard-tools/snapshot/wireguard-tools-$WG_VERSION.tar.xz"

SRC="$BUILD/wg-src"
OUT="$BUILD/wg/wg"
JOBS="$(nproc)"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$WG_VERSION" ]; then
    echo "wg $WG_VERSION already staged: $OUT"
    exit 0
fi

mkdir -p "$SRC" "$(dirname "$OUT")"
cd "$SRC"

# fetch URL FILE SHA256 — download once, always verify
fetch() {
    if [ ! -f "$2" ]; then
        curl -fsSL -A "Mozilla/5.0" -o "$2.part" "$1"
        mv -f "$2.part" "$2"
    fi
    if ! echo "$3  $2" | sha256sum -c --quiet -; then
        echo "ERROR: sha256 mismatch for $2 (expected $3)" >&2
        rm -f "$2"
        exit 1
    fi
}
fetch "$WG_URL" "wireguard-tools-$WG_VERSION.tar.xz" "$WG_SHA256"
rm -rf "wireguard-tools-$WG_VERSION"
tar xf "wireguard-tools-$WG_VERSION.tar.xz"

echo "=== wireguard-tools $WG_VERSION (wg, static) ==="
# CFLAGS/LDFLAGS via the environment: the Makefile appends to them (on the
# command line they would replace its `-isystem uapi/linux`, and the builder's
# older <linux/wireguard.h> would be used instead of the bundled one).
CFLAGS="-Os" LDFLAGS="-static" make -s -C "wireguard-tools-$WG_VERSION/src" -j"$JOBS" wg >/dev/null
strip -o "$OUT" "wireguard-tools-$WG_VERSION/src/wg"
file "$OUT" | grep -q "statically linked" || { echo "ERROR: $OUT is not static" >&2; exit 1; }
printf '%s\n' "$WG_VERSION" > "$OUT.version"
echo "  wg: $("$OUT" --version) · $(du -h "$OUT" | cut -f1) → $OUT"
