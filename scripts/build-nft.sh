#!/bin/bash
# Build a static nft (nftables CLI) for the rootfs: build-rootfs.sh installs
# $BUILD_DIR/nft/nft to /usr/sbin/nft when present. wayang-fw drives the
# kernel's nftables through it.
#
# Usage: ./scripts/build-nft.sh
# Env:   BUILD_DIR   (default: $HOME/wayangos-build)
#        LIBMNL_VERSION / LIBNFTNL_VERSION / NFTABLES_VERSION
set -euo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
LIBMNL_VERSION="${LIBMNL_VERSION:-1.0.5}"
LIBNFTNL_VERSION="${LIBNFTNL_VERSION:-1.2.8}"
NFTABLES_VERSION="${NFTABLES_VERSION:-1.1.1}"
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
fetch() {
    [ -f "$2" ] && return 0
    curl -fsSL -o "$2.part" "$1"
    mv -f "$2.part" "$2"
}
fetch "https://www.netfilter.org/pub/libmnl/libmnl-$LIBMNL_VERSION.tar.bz2" "libmnl-$LIBMNL_VERSION.tar.bz2"
fetch "https://www.netfilter.org/pub/libnftnl/libnftnl-$LIBNFTNL_VERSION.tar.xz" "libnftnl-$LIBNFTNL_VERSION.tar.xz"
fetch "https://www.netfilter.org/pub/nftables/nftables-$NFTABLES_VERSION.tar.xz" "nftables-$NFTABLES_VERSION.tar.xz"
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
