#!/bin/bash
# Build a static nft (nftables CLI) for the rootfs: build-rootfs.sh installs
# $BUILD_DIR/nft/nft to /usr/sbin/nft when present. wayang-fw drives the
# kernel's nftables through it.
#
# Usage: ./scripts/build-nft.sh
# Env:   BUILD_DIR   (default: $HOME/wayangos-build)
#        LIBMNL_VERSION / LIBNFTNL_VERSION / NFTABLES_VERSION
#        LIBMNL_SHA256 / LIBNFTNL_SHA256 / NFTABLES_SHA256 (override the pins)
#        ALLOW_UNVERIFIED=1  skip the sha256 check (explicit opt-out only)
set -euo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump each version + sha256 together. Recorded from the upstream
# tarballs (netfilter.org publishes a .sha256sum next to each one).
LIBMNL_VERSION="${LIBMNL_VERSION:-1.0.5}"
LIBMNL_SHA256="${LIBMNL_SHA256:-274b9b919ef3152bfb3da3a13c950dd60d6e2bcd54230ffeca298d03b40d0525}"
LIBNFTNL_VERSION="${LIBNFTNL_VERSION:-1.2.8}"
LIBNFTNL_SHA256="${LIBNFTNL_SHA256:-37fea5d6b5c9b08de7920d298de3cdc942e7ae64b1a3e8b880b2d390ae67ad95}"
NFTABLES_VERSION="${NFTABLES_VERSION:-1.1.1}"
NFTABLES_SHA256="${NFTABLES_SHA256:-6358830f3a64f31e39b0ad421d7dadcd240b72343ded48d8ef13b8faf204865a}"
SRC="$BUILD/nft-src"
PREFIX="$SRC/prefix"
OUT="$BUILD/nft/nft"
JOBS="$(nproc)"

if [ -x "$OUT" ] && "$OUT" --version 2>/dev/null | grep -q "v$NFTABLES_VERSION"; then
    echo "nft $NFTABLES_VERSION already staged: $OUT"
    exit 0
fi

mkdir -p "$SRC" "$(dirname "$OUT")"
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
fetch "https://www.netfilter.org/pub/libmnl/libmnl-$LIBMNL_VERSION.tar.bz2" \
    "libmnl-$LIBMNL_VERSION.tar.bz2" "$LIBMNL_SHA256"
fetch "https://www.netfilter.org/pub/libnftnl/libnftnl-$LIBNFTNL_VERSION.tar.xz" \
    "libnftnl-$LIBNFTNL_VERSION.tar.xz" "$LIBNFTNL_SHA256"
fetch "https://www.netfilter.org/pub/nftables/nftables-$NFTABLES_VERSION.tar.xz" \
    "nftables-$NFTABLES_VERSION.tar.xz" "$NFTABLES_SHA256"
rm -rf "libmnl-$LIBMNL_VERSION" "libnftnl-$LIBNFTNL_VERSION" "nftables-$NFTABLES_VERSION"
for t in libmnl-*.tar.bz2 libnftnl-*.tar.xz nftables-*.tar.xz; do tar xf "$t"; done

export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
echo "=== libmnl $LIBMNL_VERSION ==="
(cd "libmnl-$LIBMNL_VERSION" && ./configure -q --prefix="$PREFIX" --enable-static --disable-shared && make -s -j"$JOBS" && make -s install) >/dev/null
echo "=== libnftnl $LIBNFTNL_VERSION ==="
(cd "libnftnl-$LIBNFTNL_VERSION" && ./configure -q --prefix="$PREFIX" --enable-static --disable-shared && make -s -j"$JOBS" && make -s install) >/dev/null
echo "=== nftables $NFTABLES_VERSION ==="
(
    cd "nftables-$NFTABLES_VERSION"
    ./configure -q --prefix="$PREFIX" --enable-static --disable-shared \
        --with-mini-gmp --without-cli --without-json --disable-man-doc
    # libtool drops a plain -static; -all-static links libmnl/libnftnl/libc in
    make -s -j"$JOBS" LDFLAGS=-all-static
) >/dev/null 2>&1
strip -o "$OUT" "$SRC/nftables-$NFTABLES_VERSION/src/nft"
file "$OUT" | grep -q "statically linked" || { echo "ERROR: $OUT is not static" >&2; exit 1; }
echo "  nft: $("$OUT" --version) · $(du -h "$OUT" | cut -f1) → $OUT"
