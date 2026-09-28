#!/bin/bash
# Build a static radvd (IPv6 router advertisement daemon) for the rootfs:
# build-rootfs.sh installs $BUILD_DIR/radvd/radvd to /usr/sbin/radvd when
# present. wayang-router renders /etc/radvd.conf from the interfaces with
# `ra = true` and runs `radvd -C /etc/radvd.conf -p /var/run/radvd.pid`, so
# the delegated IPv6 /64 is actually advertised (SLAAC) to the LAN. Without
# radvd the prefix/addresses are still installed but no RAs are sent
# (docs/CONFIG.md in wayang-router).
#
# radvd uses raw netlink (no libnl) and needs flex/bison at build time to
# generate the config parser. Link it fully static so the image needs no libc
# or shared objects. Best-effort: a missing build dependency warns and exits 0
# so the image simply ships without radvd; a checksum mismatch always aborts.
#
# Usage: ./scripts/build-radvd.sh
# Env:   BUILD_DIR    (default: $HOME/wayangos-build)
#        RADVD_VERSION / RADVD_SHA256  override the pin (bump both together)
#        ALLOW_UNVERIFIED=1            skip the sha256 check (explicit opt-out only)
set -euo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together. Latest stable when pinned; the
# sha256 is recorded from the upstream tarball (the release publishes a
# .sha256 next to it: radvd-$RADVD_VERSION.tar.xz.sha256).
RADVD_VERSION="${RADVD_VERSION:-2.21}"
RADVD_SHA256="${RADVD_SHA256:-91df2ed7faca0716bbd726a17d6467ed92fcb2b6e45b57d9e619f9686ab99e1b}"
RADVD_URL="https://github.com/radvd-project/radvd/releases/download/v$RADVD_VERSION/radvd-$RADVD_VERSION.tar.xz"

SRC="$BUILD/radvd-src"
OUTDIR="$BUILD/radvd"
OUT="$OUTDIR/radvd"
JOBS="$(nproc)"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$RADVD_VERSION" ]; then
    echo "radvd $RADVD_VERSION already staged: $OUT"
    exit 0
fi

# Best-effort: without flex/bison (the parser) or a static libc there is
# nothing to build, but the rest of the image is still valid.
for tool in flex bison cc make curl tar; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "WARNING: $tool not found — skipping static radvd (optional)" >&2
        exit 0
    fi
done
if ! echo 'int main(void){return 0;}' | cc -x c - -static -o /dev/null 2>/dev/null; then
    echo "WARNING: no static libc (cc -static) — skipping static radvd (optional)" >&2
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
fetch "$RADVD_URL" "radvd-$RADVD_VERSION.tar.xz" "$RADVD_SHA256"
rm -rf "radvd-$RADVD_VERSION"
tar xf "radvd-$RADVD_VERSION.tar.xz"

echo "=== radvd $RADVD_VERSION (static; flex/bison present) ==="
(
    cd "radvd-$RADVD_VERSION"
    # /etc/radvd.conf + /var/run/radvd.pid are wayang-router's paths; raw
    # netlink is used (no libnl). No systemd unit, no check framework.
    ./configure -q --prefix=/usr \
        --with-configfile=/etc/radvd.conf \
        --with-pidfile=/var/run/radvd.pid \
        --without-systemdsystemunitdir \
        CFLAGS="-Os" LDFLAGS="-static"
    make -s -j"$JOBS" radvd
) >"$SRC/build.log" 2>&1 || { tail -n 40 "$SRC/build.log" >&2; echo "ERROR: radvd build failed (log: $SRC/build.log)" >&2; exit 1; }

[ -f "$SRC/radvd-$RADVD_VERSION/radvd" ] || { echo "ERROR: radvd did not build" >&2; exit 1; }
strip -o "$OUT" "$SRC/radvd-$RADVD_VERSION/radvd"
file "$OUT" | grep -q "statically linked" || { echo "ERROR: $OUT is not static" >&2; exit 1; }
printf '%s\n' "$RADVD_VERSION" > "$OUT.version"
echo "  radvd: $("$OUT" --version 2>&1 | head -n1) · $(du -h "$OUT" | cut -f1) → $OUT"
