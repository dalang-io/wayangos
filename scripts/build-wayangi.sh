#!/bin/bash
# Stage the static `wayangi` agent for the rootfs: build-rootfs.sh installs
# $BUILD_DIR/wayangi/wayangi to /usr/sbin/wayangi when present. WayangOS as a
# wayangi EdgeRouter (docs/EDGEROUTER.md) runs this per-device Go agent, which
# enrols with a token (POST /api/v1/bootstrap), brings up wayangi0 and installs
# the delegated IPv6 prefix. **No token or identity is ever baked into the
# image** — the agent reads them from /data/etc/wayangi/ at runtime.
#
# Best-effort, like scripts/fetch-dcheck.sh and build-wg.sh: warns and exits 0
# (leaving any previously staged binary in place) when neither a usable source
# checkout nor a release base URL is provided. A local Go checkout (WAYANGI_SRC)
# is rsynced to the Linux builder and built there — this repo's dev machines
# are not used to cross-build, and the checkout itself is never built in place.
# Alternatively a pinned binary is fetched from the wayangi download channel
# when WAYANGI_BASE_URL is set.
#
# Usage: ./scripts/build-wayangi.sh
# Env:
#   BUILD_DIR         (default: $HOME/wayangos-build)
#   WAYANGI_SRC       Go checkout (default: $HOME/dev/wayangi)
#   WAYANGI_HOST      Linux builder for cross-builds (default: root@10.0.0.251)
#   WAYANGI_BASE_URL  release channel base, e.g. https://wayangi.dalang.io/downloads
#   WAYANGI_VERSION   release version, optional (used with WAYANGI_BASE_URL)
#   WAYANGI_FORCE=1   rebuild even if a binary is already staged
set -uo pipefail

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
OUT="$BUILD/wayangi/wayangi"
SRC="${WAYANGI_SRC:-$HOME/dev/wayangi}"
HOST="${WAYANGI_HOST:-root@10.0.0.251}"
TARGET="${WAYANGI_TARGET:-linux-amd64}"
OS="$(uname -s)"

mkdir -p "$BUILD/wayangi"

if [ -x "$OUT" ] && [ -z "${WAYANGI_FORCE:-}" ]; then
    echo "wayangi already staged: $OUT"
    exit 0
fi

staged=0

# (a) local checkout -> static linux/amd64 binary, built on Linux. On a Linux
# host (the builder, or CI) build directly; otherwise rsync to WAYANGI_HOST.
if [ -d "$SRC/cmd/wayangi" ]; then
    if [ "$OS" = Linux ] && command -v go >/dev/null 2>&1; then
        WORK="$BUILD/wayangi-src"
        echo "=== wayangi: building from $SRC (local go) ==="
        rm -rf "$WORK" && mkdir -p "$WORK"
        rsync -a --delete --exclude .git --exclude dist --exclude target "$SRC/" "$WORK/"
        if ( cd "$WORK" && CGO_ENABLED=0 GOOS=linux GOARCH=amd64 \
                go build -trimpath -ldflags '-s -w' -o "$OUT" ./cmd/wayangi ); then
            staged=1
        else
            echo "build-wayangi: go build failed in $WORK — falling back" >&2
        fi
    elif command -v ssh >/dev/null 2>&1 && command -v rsync >/dev/null 2>&1; then
        WORK="/tmp/wayangi-src-$$"
        echo "=== wayangi: cross-building on $HOST from $SRC ==="
        if ssh -o BatchMode=yes -o ConnectTimeout=5 "$HOST" \
                "mkdir -p '$WORK' && command -v go >/dev/null 2>&1"; then
            if rsync -az --delete --exclude .git --exclude dist --exclude target \
                    "$SRC/" "$HOST:$WORK/" \
               && ssh -o BatchMode=yes "$HOST" \
                    "cd '$WORK' && CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -trimpath -ldflags '-s -w' -o wayangi ./cmd/wayangi" \
               && rsync -az "$HOST:$WORK/wayangi" "$OUT"; then
                staged=1
            else
                echo "build-wayangi: remote build on $HOST failed — falling back" >&2
            fi
            ssh -o BatchMode=yes "$HOST" "rm -rf '$WORK'" 2>/dev/null || true
        else
            echo "build-wayangi: no go on $HOST — falling back" >&2
        fi
    else
        echo "build-wayangi: ssh/rsync unavailable, cannot cross-build — falling back" >&2
    fi
fi

# (b) pinned release from the wayangi download channel (e.g. the hub serves
#     wayangi-linux-amd64). Only attempted when a base URL is provided.
if [ "$staged" = 0 ] && [ -n "${WAYANGI_BASE_URL:-}" ]; then
    BASE="${WAYANGI_BASE_URL%/}"
    TMP="$BUILD/wayangi/.dl"
    rm -rf "$TMP" && mkdir -p "$TMP"
    urls=()
    if [ -n "${WAYANGI_VERSION:-}" ]; then
        v="${WAYANGI_VERSION#v}"
        urls+=("$BASE/v$v/wayangi-$TARGET" "$BASE/wayangi-$v-$TARGET")
    fi
    urls+=("$BASE/wayangi-$TARGET")
    if ! command -v curl >/dev/null 2>&1; then
        echo "build-wayangi: curl not available for $BASE — falling back" >&2
    else
        echo "=== wayangi: fetching pinned release from $BASE ==="
        for u in "${urls[@]}"; do
            if curl -fsSL --retry 3 -o "$TMP/wayangi" "$u"; then
                name="${u##*/}"
                if curl -fsSL --retry 2 -o "$TMP/SHA256SUMS" "${u%/*}/SHA256SUMS" 2>/dev/null; then
                    want="$(grep " $name\$" "$TMP/SHA256SUMS" | awk '{print $1}' | head -1)"
                    if [ -n "$want" ]; then
                        if command -v sha256sum >/dev/null 2>&1; then
                            got="$(sha256sum "$TMP/wayangi" | cut -d' ' -f1)"
                        else
                            got="$(shasum -a 256 "$TMP/wayangi" | cut -d' ' -f1)"
                        fi
                        if [ "$got" != "$want" ]; then
                            echo "build-wayangi: sha256 mismatch for $name — skipping" >&2
                            continue
                        fi
                    fi
                fi
                cp -f "$TMP/wayangi" "$OUT" && chmod 755 "$OUT" && staged=1
                echo "  fetched: $u"
                break
            fi
        done
        [ "$staged" = 0 ] && echo "build-wayangi: no release binary at $BASE — falling back" >&2
    fi
fi

if [ "$staged" = 1 ] && [ -x "$OUT" ]; then
    chmod 755 "$OUT"
    if command -v file >/dev/null 2>&1; then
        file "$OUT" | grep -q 'statically linked' \
            || echo "build-wayangi: WARNING: $OUT does not look statically linked" >&2
    fi
    echo "  wayangi: $(du -h "$OUT" | cut -f1) → $OUT"
    exit 0
fi

echo "build-wayangi: skipped (no usable $SRC checkout and no WAYANGI_BASE_URL)" >&2
exit 0
