#!/bin/bash
# Build a static conntrackd (conntrack-tools) for the rootfs: build-rootfs.sh
# installs $BUILD_DIR/conntrackd/conntrackd to /usr/sbin/conntrackd when
# present. wayang-fw renders /data/etc/fw/conntrackd.conf from an `[ha]` block
# with `state_sync = "conntrackd"` and runs the daemon itself for HA
# connection-state replication (docs/HA.md / wayang-fw/docs/HA.md); no init
# script is involved. Without this binary that feature is warn-and-noop, so the
# firewall is never affected.
#
# It follows scripts/build-nft.sh (several netfilter libraries, each pinned and
# statically linked into a private prefix) plus the header/optional shape of
# scripts/build-dnsmasq.sh:
#
#   * libmnl, libnfnetlink and libnetfilter_conntrack are built `--enable-static
#     --disable-shared` into a private prefix; conntrack-tools then links them
#     with libtool's `-all-static`, so the result carries no shared objects.
#   * conntrack-tools' optional halves are turned off on purpose: no userspace
#     conntrack helper support (`--disable-cthelper`; the product kernel ships
#     no conntrack helpers, see wayang-fw/docs/HA.md), no `nfct` timeout
#     extensions (`--disable-cttimeout`) and no systemd unit. Only `conntrackd`
#     is built (`make -C src conntrackd`), not the `conntrack`/`nfct` CLIs.
#   * `ac_cv_header_rpc_rpc_msg_h=yes`: conntrack-tools' configure unconditionally
#     checks for the pre-2.26 glibc `rpc/rpc_msg.h` (or pkg-config `libtirpc`) even
#     though the only user is `src/helpers/rpc.c`, which `--disable-cthelper`
#     excludes from the build. Answering the check directly drops a build-host
#     dependency (libtirpc-dev) that would otherwise be installed to satisfy a
#     check whose feature we do not compile. This is the sole consumer of the
#     libtirpc result; nothing else in conntrackd uses RPC.
#   * flex/bison generate the config parser and pkg-config locates the three
#     libraries: both are required, and their absence is a hard error. Only a
#     missing compiler (or static libc) warns and exits 0, the optional-tool
#     convention dnsmasq/radvd use, so a non-build host still yields a valid
#     image shipped without conntrackd.
#
# Usage: ./scripts/build-conntrackd.sh
# Env:   BUILD_DIR    (default: $HOME/wayangos-build)
#        CONNTRACK_TOOLS_VERSION / CONNTRACK_TOOLS_SHA256
#        LIBNETFILTER_CONNTRACK_VERSION / LIBNETFILTER_CONNTRACK_SHA256
#        LIBNFNETLINK_VERSION / LIBNFNETLINK_SHA256
#        LIBMNL_VERSION / LIBMNL_SHA256          override the pins (bump each
#        version + sha256 together)
#        ALLOW_UNVERIFIED=1                      skip the sha256 check
set -euo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump each version + sha256 together. All sums are the ones
# netfilter.org publishes in the `.sha256sum` next to each tarball (verified
# against the upstream file when pinning). conntrack-tools 1.4.9 requires
# libnetfilter_conntrack >= 1.1.1, so the two are bumped as a pair; 1.1.1 is
# also the newest release. libmnl 1.0.5 is the same pin build-nft.sh uses.
CONNTRACK_TOOLS_VERSION="${CONNTRACK_TOOLS_VERSION:-1.4.9}"
CONNTRACK_TOOLS_SHA256="${CONNTRACK_TOOLS_SHA256:-c15afe488a8d408c9d6d61e97dbd19f3c591942f62c13df6453a961ca4231cae}"
LIBNETFILTER_CONNTRACK_VERSION="${LIBNETFILTER_CONNTRACK_VERSION:-1.1.1}"
LIBNETFILTER_CONNTRACK_SHA256="${LIBNETFILTER_CONNTRACK_SHA256:-769d3eaf57fa4fbdb05dd12873b6cb9a5be7844d8937e222b647381d44284820}"
LIBNFNETLINK_VERSION="${LIBNFNETLINK_VERSION:-1.0.2}"
LIBNFNETLINK_SHA256="${LIBNFNETLINK_SHA256:-b064c7c3d426efb4786e60a8e6859b82ee2f2c5e49ffeea640cfe4fe33cbc376}"
LIBMNL_VERSION="${LIBMNL_VERSION:-1.0.5}"
LIBMNL_SHA256="${LIBMNL_SHA256:-274b9b919ef3152bfb3da3a13c950dd60d6e2bcd54230ffeca298d03b40d0525}"
BASE="https://www.netfilter.org/pub"

SRC="$BUILD/conntrackd-src"
PREFIX="$SRC/prefix"
OUTDIR="$BUILD/conntrackd"
OUT="$OUTDIR/conntrackd"
JOBS="$(nproc)"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$CONNTRACK_TOOLS_VERSION" ]; then
    echo "conntrackd $CONNTRACK_TOOLS_VERSION already staged: $OUT"
    exit 0
fi

# A missing compiler (or a libc that cannot link statically) means no static
# tool can be built at all: warn and skip, as dnsmasq/radvd do, so the image
# simply ships without conntrackd. Everything else is a hard error.
for tool in cc make curl tar; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "WARNING: $tool not found — skipping static conntrackd (optional)" >&2
        exit 0
    fi
done
if ! echo 'int main(void){return 0;}' | cc -x c - -static -o /dev/null 2>/dev/null; then
    echo "WARNING: no static libc (cc -static) — skipping static conntrackd (optional)" >&2
    exit 0
fi
# flex/bison generate conntrackd's config parser; pkg-config locates the three
# libraries. These are required: fail loudly rather than ship without the tool.
for tool in flex bison pkg-config; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "ERROR: $tool not found — cannot build conntrackd" >&2
        exit 1
    fi
done

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
MNL="libmnl-$LIBMNL_VERSION"
NFNL="libnfnetlink-$LIBNFNETLINK_VERSION"
NFCT="libnetfilter_conntrack-$LIBNETFILTER_CONNTRACK_VERSION"
CT="conntrack-tools-$CONNTRACK_TOOLS_VERSION"
fetch "$BASE/libmnl/$MNL.tar.bz2" "$MNL.tar.bz2" "$LIBMNL_SHA256"
fetch "$BASE/libnfnetlink/$NFNL.tar.bz2" "$NFNL.tar.bz2" "$LIBNFNETLINK_SHA256"
fetch "$BASE/libnetfilter_conntrack/$NFCT.tar.xz" "$NFCT.tar.xz" "$LIBNETFILTER_CONNTRACK_SHA256"
fetch "$BASE/conntrack-tools/$CT.tar.xz" "$CT.tar.xz" "$CONNTRACK_TOOLS_SHA256"
rm -rf "$MNL" "$NFNL" "$NFCT" "$CT"
tar xf "$MNL.tar.bz2"
tar xf "$NFNL.tar.bz2"
tar xf "$NFCT.tar.xz"
tar xf "$CT.tar.xz"

export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"

echo "=== libmnl $LIBMNL_VERSION ==="
(cd "$MNL" && ./configure -q --prefix="$PREFIX" --enable-static --disable-shared \
    && make -s -j"$JOBS" && make -s install) >/dev/null
echo "=== libnfnetlink $LIBNFNETLINK_VERSION ==="
(cd "$NFNL" && ./configure -q --prefix="$PREFIX" --enable-static --disable-shared \
    && make -s -j"$JOBS" && make -s install) >/dev/null
echo "=== libnetfilter_conntrack $LIBNETFILTER_CONNTRACK_VERSION ==="
(cd "$NFCT" && ./configure -q --prefix="$PREFIX" --enable-static --disable-shared \
    && make -s -j"$JOBS" && make -s install) >/dev/null

echo "=== conntrack-tools $CONNTRACK_TOOLS_VERSION (static; conntrackd only, no cthelper) ==="
(
    cd "$CT"
    # `ac_cv_header_rpc_rpc_msg_h` answers the pre-2.26-glibc RPC header check;
    # its only consumer, src/helpers/rpc.c, is excluded by --disable-cthelper.
    ./configure -q --prefix="$PREFIX" --enable-static --disable-shared \
        --disable-cthelper --disable-cttimeout --disable-systemd \
        ac_cv_header_rpc_rpc_msg_h=yes
    # libtool drops a plain -static; -all-static links libmnl/libnfnetlink/
    # libnetfilter_conntrack/libc in. Only conntrackd is built, not the CLIs.
    make -s -j"$JOBS" LDFLAGS=-all-static -C src conntrackd
) >"$SRC/build.log" 2>&1 || { tail -n 40 "$SRC/build.log" >&2; echo "ERROR: conntrackd build failed (log: $SRC/build.log)" >&2; exit 1; }

BIN="$SRC/$CT/src/conntrackd"
[ -f "$BIN" ] || { echo "ERROR: conntrackd did not build" >&2; exit 1; }
strip -o "$OUT" "$BIN"
file "$OUT" | grep -q "statically linked" || { echo "ERROR: $OUT is not static" >&2; exit 1; }
printf '%s\n' "$CONNTRACK_TOOLS_VERSION" > "$OUT.version"
echo "  conntrackd: $("$OUT" -v 2>&1 | head -n1) · $(du -h "$OUT" | cut -f1) → $OUT"
