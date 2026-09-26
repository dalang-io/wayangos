#!/bin/bash
# Build static WiFi userspace tools (iw, wpa_supplicant, wpa_cli) for the
# WayangOS initramfs. The rootfs has no package manager and no dynamic loader
# libs, so these must be statically linked.
#
# Best-effort: on a host without the build deps (or without network) it warns
# and leaves any existing binaries in place.
#
# Output: $BUILD_DIR/wifi/{iw,wpa_supplicant,wpa_cli}; build-rootfs.sh installs
# them into /usr/sbin. Needs libnl-3-dev, libnl-genl-3-dev and libssl-dev.
#
# Env: BUILD_DIR (default $HOME/wayangos-build), IW_VERSION, HOSTAP_VERSION.
set -uo pipefail

BUILD_DIR="${BUILD_DIR:-$HOME/wayangos-build}"
IW_VERSION="${IW_VERSION:-6.9}"
HOSTAP_VERSION="${HOSTAP_VERSION:-2.11}"
SRC="$BUILD_DIR/wifi-tools-src"
OUT="$BUILD_DIR/wifi"

for t in gcc make pkg-config curl tar xz; do
    command -v "$t" >/dev/null 2>&1 || { echo "build-wifi-tools: missing '$t' — skipping" >&2; exit 0; }
done
# libnl is needed by iw and by wpa_supplicant's nl80211 driver.
if ! pkg-config --exists libnl-3.0 libnl-genl-3.0 libnl-route-3.0; then
    if command -v apt-get >/dev/null 2>&1 && [ "$(id -u)" = 0 ]; then
        DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
            libnl-3-dev libnl-genl-3-dev libnl-route-3-dev >/dev/null 2>&1 || true
    fi
fi
if ! pkg-config --exists libnl-3.0 libnl-genl-3.0 libnl-route-3.0; then
    echo "build-wifi-tools: libnl-3-dev/libnl-genl-3-dev/libnl-route-3-dev missing — skipping" >&2
    exit 0
fi

mkdir -p "$SRC" "$OUT"

fetch() { [ -f "$2" ] || curl -fL --retry 3 -o "$2" "$1"; }

# --- iw -------------------------------------------------------------------
if [ ! -x "$OUT/iw" ]; then
    echo "=== building iw $IW_VERSION (static) ==="
    fetch "https://kernel.org/pub/software/network/iw/iw-$IW_VERSION.tar.xz" \
        "$SRC/iw-$IW_VERSION.tar.xz" || echo "  iw fetch failed" >&2
    if [ -f "$SRC/iw-$IW_VERSION.tar.xz" ]; then
        tar -xf "$SRC/iw-$IW_VERSION.tar.xz" -C "$SRC"
        if ( cd "$SRC/iw-$IW_VERSION" && make clean >/dev/null 2>&1; \
             make CC=gcc CFLAGS="-O2 -static" LDFLAGS="-static" -j"$(nproc)" >/dev/null 2>&1 ); then
            cp "$SRC/iw-$IW_VERSION/iw" "$OUT/iw" && echo "  iw -> $OUT/iw"
        else
            echo "  iw build failed" >&2
        fi
    fi
fi

# --- wpa_supplicant -------------------------------------------------------
if [ ! -x "$OUT/wpa_supplicant" ]; then
    echo "=== building wpa_supplicant $HOSTAP_VERSION (static) ==="
    fetch "https://w1.fi/releases/wpa_supplicant-$HOSTAP_VERSION.tar.gz" \
        "$SRC/wpa_supplicant-$HOSTAP_VERSION.tar.gz" || echo "  wpa_supplicant fetch failed" >&2
    if [ -f "$SRC/wpa_supplicant-$HOSTAP_VERSION.tar.gz" ]; then
        tar -xf "$SRC/wpa_supplicant-$HOSTAP_VERSION.tar.gz" -C "$SRC"
        if ( cd "$SRC/wpa_supplicant-$HOSTAP_VERSION/wpa_supplicant" && \
             make clean >/dev/null 2>&1
             cp defconfig .config
             # no D-Bus or readline in the initramfs; drop those from defconfig
             sed -i -E 's/^(CONFIG_[A-Z0-9_]*DBUS[A-Z0-9_]*)=y/# \1=y/' .config
             sed -i -E 's/^(CONFIG_READLINE)=y/# \1=y/' .config
             cat >> .config <<'EOF'
CONFIG_DRIVER_NL80211=y
CONFIG_LIBNL32=y
CONFIG_TLS=openssl
CONFIG_CTRL_IFACE=y
CONFIG_BACKEND=file
CONFIG_NO_RANDOM_POOL=y
EOF
             make CC=gcc EXTRA_CFLAGS="-static" LDFLAGS="-static" -j"$(nproc)" >/dev/null 2>&1 ); then
            cp "$SRC/wpa_supplicant-$HOSTAP_VERSION/wpa_supplicant/wpa_supplicant" "$OUT/wpa_supplicant"
            [ -f "$SRC/wpa_supplicant-$HOSTAP_VERSION/wpa_supplicant/wpa_cli" ] && \
                cp "$SRC/wpa_supplicant-$HOSTAP_VERSION/wpa_supplicant/wpa_cli" "$OUT/wpa_cli"
            echo "  wpa_supplicant -> $OUT/"
        else
            echo "  wpa_supplicant build failed" >&2
        fi
    fi
fi

ls -lh "$OUT"/iw "$OUT"/wpa_supplicant "$OUT"/wpa_cli 2>/dev/null || true
