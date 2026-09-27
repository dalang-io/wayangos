#!/bin/bash
# Build a static iproute2 `tc` for the rootfs: build-rootfs.sh installs
# $BUILD_DIR/iproute2/tc to /usr/sbin/tc when present. wayang-router uses it
# for QoS / rate limits (htb, fq_codel, cake, ingress + ifb, u32/fw filters,
# police/mirred actions). Only `tc`: BusyBox keeps providing `ip` (its own tc
# applet is disabled).
#
# The QoS *kernel* options (NET_SCH_*, NET_CLS_*, NET_ACT_*, IFB) are still
# under hardware bisect (docs/ROUTER-KERNEL-BISECT.md); until they ship, `tc`
# reports "Specified qdisc kind is unknown" on the device.
#
# libmnl is built static like scripts/build-nft.sh (extended netlink ACK
# messages). libelf/libbpf/libcap/selinux/xtables are left out on purpose
# (no `tc ... bpf`, no `action xt`), so the binary links fully static.
#
# Usage: ./scripts/build-iproute2.sh
# Env:   BUILD_DIR   (default: $HOME/wayangos-build)
set -euo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together (sha256sums.asc next to each tarball
# on kernel.org / netfilter.org).
IPROUTE2_VERSION="7.2.0"
IPROUTE2_SHA256="4c2fa124c2cf0afd7ca34d1eeacba6ba048a56f6374e2aab93dafbdbd4eea9c0"
LIBMNL_VERSION="1.0.5"
LIBMNL_SHA256="274b9b919ef3152bfb3da3a13c950dd60d6e2bcd54230ffeca298d03b40d0525"

SRC="$BUILD/iproute2-src"
PREFIX="$SRC/prefix"
OUT="$BUILD/iproute2/tc"
JOBS="$(nproc)"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$IPROUTE2_VERSION" ]; then
    echo "tc (iproute2 $IPROUTE2_VERSION) already staged: $OUT"
    exit 0
fi

mkdir -p "$SRC" "$(dirname "$OUT")"
cd "$SRC"

# fetch URL FILE SHA256 — download once, always verify
fetch() {
    if [ ! -f "$2" ]; then
        curl -fsSL -o "$2.part" "$1"
        mv -f "$2.part" "$2"
    fi
    if ! echo "$3  $2" | sha256sum -c --quiet -; then
        echo "ERROR: sha256 mismatch for $2 (expected $3)" >&2
        rm -f "$2"
        exit 1
    fi
}
fetch "https://www.netfilter.org/pub/libmnl/libmnl-$LIBMNL_VERSION.tar.bz2" \
    "libmnl-$LIBMNL_VERSION.tar.bz2" "$LIBMNL_SHA256"
fetch "https://mirrors.edge.kernel.org/pub/linux/utils/net/iproute2/iproute2-$IPROUTE2_VERSION.tar.xz" \
    "iproute2-$IPROUTE2_VERSION.tar.xz" "$IPROUTE2_SHA256"
rm -rf "libmnl-$LIBMNL_VERSION" "iproute2-$IPROUTE2_VERSION"
tar xf "libmnl-$LIBMNL_VERSION.tar.bz2"
tar xf "iproute2-$IPROUTE2_VERSION.tar.xz"

echo "=== libmnl $LIBMNL_VERSION ==="
(cd "libmnl-$LIBMNL_VERSION" && ./configure -q --prefix="$PREFIX" --enable-static --disable-shared \
    && make -s -j"$JOBS" && make -s install) >/dev/null

echo "=== iproute2 $IPROUTE2_VERSION (tc, static) ==="
(
    cd "iproute2-$IPROUTE2_VERSION"
    # Only our prefix is visible to pkg-config: the host's libelf/libbpf/libcap/
    # selinux/xtables (shared-only on most builders) are not picked up.
    export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"
    export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
    ./configure --libbpf_force off >/dev/null
    # belt and braces: anything configure still found that can't link static
    sed -i -e 's/^HAVE_ELF:=y/HAVE_ELF:=n/' -e 's/^HAVE_CAP:=y/HAVE_CAP:=n/' \
        -e 's/^HAVE_SELINUX:=y/HAVE_SELINUX:=n/' -e '/^TC_CONFIG_XT/d' \
        -e '/^HAVE_LIBBPF/d' config.mk
    # top-level make (it sets the include paths); only lib + tc
    make -s -j"$JOBS" SUBDIRS="lib tc" SHARED_LIBS=n CCOPTS="-Os" LDFLAGS="-static" >/dev/null
) 2>&1 | grep -v -e 'statically linked applications' -e 'dlopen' -e 'in function' || true
[ -f "$SRC/iproute2-$IPROUTE2_VERSION/tc/tc" ] || { echo "ERROR: tc did not build" >&2; exit 1; }
strip -o "$OUT" "$SRC/iproute2-$IPROUTE2_VERSION/tc/tc"
file "$OUT" | grep -q "statically linked" || { echo "ERROR: $OUT is not static" >&2; exit 1; }
# the qdiscs/filters/actions wayang-router relies on must be compiled in
for m in q_cake q_htb q_fq_codel q_ingress f_u32 f_fw m_police m_mirred; do
    [ -f "$SRC/iproute2-$IPROUTE2_VERSION/tc/$m.o" ] || { echo "ERROR: tc lacks $m" >&2; exit 1; }
done
printf '%s\n' "$IPROUTE2_VERSION" > "$OUT.version"
echo "  tc: $("$OUT" -V) · $(du -h "$OUT" | cut -f1) → $OUT"
