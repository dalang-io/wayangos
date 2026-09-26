#!/bin/bash
# Download and prepare external sources for the WayangOS build.
# Kernel, BusyBox, Dropbear, and SQLite are too large to track in git.
#
# Usage: ./scripts/fetch-sources.sh
# Env:
#   BUILD_DIR       target dir (default: ~/wayangos-build)
#   ARCH            x86_64 (default) or arm64 (cross-builds BusyBox)
#   CROSS_COMPILE   cross prefix (default: aarch64-linux-gnu- for arm64)
#   KERNEL_FLAVOR   main (default) or rt (PREEMPT_RT patched tree)
#   KERNEL_VERSION  kernel version (default: 7.2.7, or 6.19.3 for rt)
#   WIFI_TOOLS_URL  optional tar with static wpa_supplicant/wpa_cli/iw
#   FIRMWARE_URL    optional tar of WiFi firmware -> $BUILD/wifi/firmware/
#
# Versions can be overridden, e.g.:
#   KERNEL_VERSION=7.2.7 BUSYBOX_VERSION=1.37.0 ./scripts/fetch-sources.sh
#   KERNEL_FLAVOR=rt ARCH=arm64 ./scripts/fetch-sources.sh
#   WIFI_TOOLS_URL=https://…/wifi-tools.tar.gz FIRMWARE_URL=https://…/fw.tar.xz \
#       ./scripts/fetch-sources.sh
set -e

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"

ARCH="${ARCH:-x86_64}"
case "$ARCH" in
    x86_64) CROSS_COMPILE="${CROSS_COMPILE:-}" ;;
    arm64)  CROSS_COMPILE="${CROSS_COMPILE:-aarch64-linux-gnu-}" ;;
    *)
        echo "ERROR: unsupported ARCH=$ARCH (allowed: x86_64, arm64)" >&2
        exit 1
        ;;
esac

KERNEL_FLAVOR="${KERNEL_FLAVOR:-main}"
case "$KERNEL_FLAVOR" in
    main) KERNEL_VERSION="${KERNEL_VERSION:-7.2.7}" ;;
    rt)   KERNEL_VERSION="${KERNEL_VERSION:-6.19.3}" ;;
    *)
        echo "ERROR: unsupported KERNEL_FLAVOR=$KERNEL_FLAVOR (allowed: main, rt)" >&2
        exit 1
        ;;
esac

if [ "$KERNEL_FLAVOR" = "rt" ]; then
    KERNEL_DIR="linux-$KERNEL_VERSION-rt1"
else
    KERNEL_DIR="linux-$KERNEL_VERSION"
fi

BUSYBOX_VERSION="${BUSYBOX_VERSION:-1.37.0}"
DROPBEAR_VERSION="${DROPBEAR_VERSION:-2024.86}"
SQLITE_AMALGAMATION_URL="${SQLITE_AMALGAMATION_URL:-https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip}"

mkdir -p "$BUILD"
cd "$BUILD"

have() { command -v "$1" >/dev/null 2>&1; }
if have curl; then
    fetch() { curl -fL --retry 3 -o "$2" "$1"; }
elif have wget; then
    fetch() { wget -q -O "$2" "$1"; }
else
    echo "ERROR: need curl or wget" >&2
    exit 1
fi

echo "=== Fetching WayangOS sources into $BUILD ==="
echo "  Arch:   $ARCH"
echo "  Kernel: $KERNEL_DIR ($KERNEL_FLAVOR)"
echo ""

# ============================================
# 1. Linux kernel
# ============================================
if [ -d "$KERNEL_DIR" ]; then
    echo "[1/5] Kernel $KERNEL_DIR already present"
elif [ "$KERNEL_FLAVOR" = "rt" ]; then
    echo "[1/5] Downloading Linux $KERNEL_VERSION + PREEMPT_RT patch..."
    KERNEL_TARBALL="linux-$KERNEL_VERSION.tar.xz"
    [ -f "$KERNEL_TARBALL" ] || fetch \
        "https://cdn.kernel.org/pub/linux/kernel/v${KERNEL_VERSION%%.*}.x/$KERNEL_TARBALL" "$KERNEL_TARBALL"
    [ -d "linux-$KERNEL_VERSION" ] || tar -xf "$KERNEL_TARBALL"

    RT_PATCH="patch-$KERNEL_VERSION-rt1.patch.xz"
    RT_SERIES="${KERNEL_VERSION%.*}"
    [ -f "$RT_PATCH" ] || fetch \
        "https://cdn.kernel.org/pub/linux/kernel/projects/rt/$RT_SERIES/$RT_PATCH" "$RT_PATCH"

    echo "  Applying $RT_PATCH..."
    (
        cd "linux-$KERNEL_VERSION"
        xz -dc "../$RT_PATCH" | patch -p1
    )
    mv "linux-$KERNEL_VERSION" "$KERNEL_DIR"
else
    echo "[1/5] Downloading Linux $KERNEL_VERSION..."
    KERNEL_TARBALL="linux-$KERNEL_VERSION.tar.xz"
    [ -f "$KERNEL_TARBALL" ] || fetch \
        "https://cdn.kernel.org/pub/linux/kernel/v${KERNEL_VERSION%%.*}.x/$KERNEL_TARBALL" "$KERNEL_TARBALL"
    echo "  Extracting (this takes a moment)..."
    tar -xf "$KERNEL_TARBALL"
fi

# ============================================
# 2. BusyBox (built static)
# ============================================
if [ -x "busybox-$BUSYBOX_VERSION/busybox" ] && [ -f "busybox-$BUSYBOX_VERSION/busybox.links" ]; then
    echo "[2/5] BusyBox $BUSYBOX_VERSION already built"
else
    echo "[2/5] Building BusyBox $BUSYBOX_VERSION (static, $ARCH)..."
    BB_TARBALL="busybox-$BUSYBOX_VERSION.tar.bz2"
    [ -f "$BB_TARBALL" ] || fetch \
        "https://busybox.net/downloads/$BB_TARBALL" "$BB_TARBALL"
    [ -d "busybox-$BUSYBOX_VERSION" ] || tar -xf "$BB_TARBALL"
    (
        cd "busybox-$BUSYBOX_VERSION"
        BB_MAKE=(ARCH="$ARCH" CROSS_COMPILE="$CROSS_COMPILE")
        make "${BB_MAKE[@]}" distclean >/dev/null 2>&1 || true
        make "${BB_MAKE[@]}" defconfig >/dev/null
        sed -i 's/# CONFIG_STATIC is not set/CONFIG_STATIC=y/' .config
        # tc uses the CBQ qdisc, removed from kernel headers in 6.8
        sed -i 's/^CONFIG_TC=y/# CONFIG_TC is not set/' .config
        make "${BB_MAKE[@]}" -j"$(nproc)" LDFLAGS=-static >/dev/null
        # applet list for build-rootfs.sh (a cross-built binary can't list them)
        make "${BB_MAKE[@]}" busybox.links >/dev/null
    )
    echo "  BusyBox: $(du -h "busybox-$BUSYBOX_VERSION/busybox" | cut -f1)"
fi

# ============================================
# 3. Dropbear SSH (source; built by build-rootfs.sh)
# ============================================
if [ -d "dropbear-$DROPBEAR_VERSION" ]; then
    echo "[3/5] Dropbear $DROPBEAR_VERSION already present"
else
    echo "[3/5] Downloading Dropbear $DROPBEAR_VERSION..."
    DB_TARBALL="dropbear-$DROPBEAR_VERSION.tar.bz2"
    [ -f "$DB_TARBALL" ] || fetch \
        "https://matt.ucc.asn.au/dropbear/releases/$DB_TARBALL" "$DB_TARBALL"
    tar -xf "$DB_TARBALL"
fi

# ============================================
# 4. SQLite amalgamation
# ============================================
if [ -f sqlite3.c ] && [ -f sqlite3.h ]; then
    echo "[4/5] SQLite amalgamation already present"
else
    echo "[4/5] Downloading SQLite amalgamation..."
    have unzip || { echo "ERROR: unzip required for SQLite" >&2; exit 1; }
    SQLITE_ZIP="$(basename "$SQLITE_AMALGAMATION_URL")"
    [ -f "$SQLITE_ZIP" ] || fetch "$SQLITE_AMALGAMATION_URL" "$SQLITE_ZIP"
    unzip -o "$SQLITE_ZIP" >/dev/null
    SQLITE_DIR="$(unzip -Z1 "$SQLITE_ZIP" | head -1 | cut -d/ -f1)"
    cp "$SQLITE_DIR/sqlite3.c" "$SQLITE_DIR/sqlite3.h" .
fi

# ============================================
# 5. WiFi tools + firmware (optional, best-effort)
# ============================================
# wpa_supplicant/wpa_cli/iw need libnl/openssl and there is no package manager,
# so they are static binaries built elsewhere. Provide a tar whose top level is
# wpa_supplicant, wpa_cli and iw:
#   WIFI_TOOLS_URL=https://…/wifi-tools.tar.gz ./scripts/fetch-sources.sh
# and/or a tar of firmware blobs (rtlwifi/, mt76/, ath9k_htc/, brcm/, …):
#   FIRMWARE_URL=https://…/firmware.tar.xz ./scripts/fetch-sources.sh
WIFI_DIR="$BUILD/wifi"
if [ -n "${WIFI_TOOLS_URL:-}" ]; then
    echo "[5/5] Fetching WiFi tools..."
    mkdir -p "$WIFI_DIR"
    WIFI_TARBALL="$WIFI_DIR/$(basename "$WIFI_TOOLS_URL")"
    if [ ! -f "$WIFI_TARBALL" ]; then
        fetch "$WIFI_TOOLS_URL" "$WIFI_TARBALL" || \
            echo "  WARNING: could not fetch $WIFI_TOOLS_URL (continuing)" >&2
    fi
    [ -f "$WIFI_TARBALL" ] && tar -xf "$WIFI_TARBALL" -C "$WIFI_DIR" || \
        echo "  WARNING: could not extract WiFi tools (continuing)" >&2
    echo "  WiFi tools in $WIFI_DIR"
else
    echo "[5/5] WiFi tools not fetched (optional)"
    echo "  Set WIFI_TOOLS_URL to a tar containing static wpa_supplicant,"
    echo "  wpa_cli and iw (top level); build-rootfs.sh installs them."
fi

if [ -n "${FIRMWARE_URL:-}" ]; then
    echo "  Fetching WiFi firmware..."
    mkdir -p "$WIFI_DIR/firmware"
    FW_TARBALL="$WIFI_DIR/$(basename "$FIRMWARE_URL")"
    if [ ! -f "$FW_TARBALL" ]; then
        fetch "$FIRMWARE_URL" "$FW_TARBALL" || \
            echo "  WARNING: could not fetch $FIRMWARE_URL (continuing)" >&2
    fi
    [ -f "$FW_TARBALL" ] && tar -xf "$FW_TARBALL" -C "$WIFI_DIR/firmware" || \
        echo "  WARNING: could not extract firmware (continuing)" >&2
    echo "  firmware in $WIFI_DIR/firmware"
elif [ -z "${WIFI_TOOLS_URL:-}" ]; then
    echo "  Set FIRMWARE_URL to a tar of firmware blobs (rtlwifi/, mt76/,"
    echo "  ath9k_htc/, brcm/, …); build-rootfs.sh copies it to /lib/firmware."
fi

echo ""
echo "=== Sources ready in $BUILD ==="
echo "  Kernel:   $KERNEL_DIR"
echo "  BusyBox:  busybox-$BUSYBOX_VERSION/busybox"
echo "  Dropbear: dropbear-$DROPBEAR_VERSION"
echo "  SQLite:   sqlite3.c / sqlite3.h"
echo "  WiFi:     $WIFI_DIR (optional)"
