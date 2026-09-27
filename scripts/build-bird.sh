#!/bin/bash
# Build a static BIRD 2 routing daemon for the rootfs: build-rootfs.sh installs
# $BUILD_DIR/bird/bird to /usr/sbin/bird when present (and birdc to
# /usr/sbin/birdc if it was built). wayang-router renders the config to
# /data/etc/router/bird.conf and talks to the control socket
# /var/run/bird.ctl directly; /etc/init.d/router starts bird only when that
# config exists.
#
# birdc needs a static libreadline; it is built only when one is available
# (BIRD_CLIENT=auto, the default) or forced with BIRD_CLIENT=yes. wayang-router
# does not need it. RPKI-over-SSH (libssh) is disabled; plain-TCP RPKI works.
#
# Usage: ./scripts/build-bird.sh
# Env:   BUILD_DIR    (default: $HOME/wayangos-build)
#        BIRD_CLIENT  auto | yes | no   (default: auto)
#        BIRD_VERSION / BIRD_SHA256  override the pin (bump both together)
#        ALLOW_UNVERIFIED=1          skip the sha256 check (explicit opt-out only)
set -euo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
# Pinned: bump version + sha256 together. Latest BIRD 2.x stable when pinned
# (upstream publishes no checksum file; sha256 recorded from the release
# tarball at https://bird.nic.cz/download/).
BIRD_VERSION="${BIRD_VERSION:-2.19.2}"
BIRD_SHA256="${BIRD_SHA256:-aff89abba3b92b7637bd57e0168b8d7ae887747f160ada4973378ad72f5f3660}"
BIRD_URL="https://bird.nic.cz/download/bird-$BIRD_VERSION.tar.gz"
BIRD_CLIENT="${BIRD_CLIENT:-auto}"

SRC="$BUILD/bird-src"
OUTDIR="$BUILD/bird"
OUT="$OUTDIR/bird"
JOBS="$(nproc)"

if [ -x "$OUT" ] && [ -f "$OUT.version" ] && [ "$(cat "$OUT.version")" = "$BIRD_VERSION" ]; then
    echo "bird $BIRD_VERSION already staged: $OUT"
    exit 0
fi

mkdir -p "$SRC" "$OUTDIR"
cd "$SRC"

# fetch URL FILE SHA256 — download once, always verify
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
fetch "$BIRD_URL" "bird-$BIRD_VERSION.tar.gz" "$BIRD_SHA256"
rm -rf "bird-$BIRD_VERSION"
tar xzf "bird-$BIRD_VERSION.tar.gz"

client=no
case "$BIRD_CLIENT" in
    yes) client=yes ;;
    no) ;;
    auto)
        # a static birdc needs libreadline.a (+ a curses .a)
        if echo 'int main(void){return 0;}' | cc -x c - -static -o /dev/null -lreadline -ltinfo 2>/dev/null; then
            client=yes
        fi
        ;;
    *) echo "ERROR: BIRD_CLIENT must be auto|yes|no" >&2; exit 1 ;;
esac

echo "=== bird $BIRD_VERSION (static, client=$client) ==="
(
    cd "bird-$BIRD_VERSION"
    # sysconfdir -> default config /data/etc/router/bird.conf
    # runstatedir -> default control socket /var/run/bird.ctl
    ./configure -q --prefix=/usr --sysconfdir=/data/etc/router \
        --runstatedir=/var/run --localstatedir=/var \
        --enable-client="$client" --disable-libssh \
        CFLAGS="-Os" LDFLAGS="-static"
    make -j"$JOBS"
) >"$SRC/build.log" 2>&1 || { tail -n 40 "$SRC/build.log" >&2; echo "ERROR: bird build failed (log: $SRC/build.log)" >&2; exit 1; }

strip -o "$OUT" "$SRC/bird-$BIRD_VERSION/bird"
file "$OUT" | grep -q "statically linked" || { echo "ERROR: $OUT is not static" >&2; exit 1; }
rm -f "$OUTDIR/birdc"
if [ "$client" = yes ] && [ -f "$SRC/bird-$BIRD_VERSION/birdc" ]; then
    strip -o "$OUTDIR/birdc" "$SRC/bird-$BIRD_VERSION/birdc"
    file "$OUTDIR/birdc" | grep -q "statically linked" || rm -f "$OUTDIR/birdc"
fi
printf '%s\n' "$BIRD_VERSION" > "$OUT.version"
echo "  bird: $("$OUT" --version 2>&1 | head -n1) · $(du -h "$OUT" | cut -f1) → $OUT"
if [ -f "$OUTDIR/birdc" ]; then
    echo "  birdc: $(du -h "$OUTDIR/birdc" | cut -f1) → $OUTDIR/birdc"
else
    echo "  birdc: not built (no static libreadline; optional)"
fi
