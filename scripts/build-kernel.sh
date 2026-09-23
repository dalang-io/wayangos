#!/bin/bash
# Build a WayangOS kernel from a named config
# Usage: ./scripts/build-kernel.sh <config-name> [output-name]
# Example: ./scripts/build-kernel.sh defconfig-qemu bzImage-qemu
#          ARCH=arm64 ./scripts/build-kernel.sh defconfig-arm64-rpi3
#
# Env:
#   ARCH           target arch: x86_64 (default) or arm64
#   CROSS_COMPILE  cross prefix (default: aarch64-linux-gnu- for arm64)
#   BUILD          build root (default: ~/wayangos-build)
#   KDIR           kernel source (default: $BUILD/linux-6.19.7, or
#                  $BUILD/linux-6.19.3-rt1 for configs whose name contains "rt")
#
# Configs may be either a full .config (e.g. configs/defconfig-qemu) or a
# small fragment whose first line is "# WAYANG_BASE: <name>". For a fragment,
# <name> is either "defconfig" (run `make defconfig`) or another file in
# configs/ used verbatim as the base; the remaining fragment lines are then
# appended and `make olddefconfig` resolves the result.
set -e

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

ARCH="${ARCH:-x86_64}"
case "$ARCH" in
    x86_64)
        CROSS_COMPILE="${CROSS_COMPILE:-}"
        ;;
    arm64)
        CROSS_COMPILE="${CROSS_COMPILE:-aarch64-linux-gnu-}"
        ;;
    *)
        echo "ERROR: unsupported ARCH=$ARCH (allowed: x86_64, arm64)" >&2
        exit 1
        ;;
esac

BUILD="${BUILD:-${BUILD_DIR:-$HOME/wayangos-build}}"

CONFIG_NAME="${1:?Usage: $0 <config-name> [output-name]}"

case "$CONFIG_NAME" in
    *rt*) DEFAULT_KDIR="$BUILD/linux-6.19.3-rt1" ;;
    *)    DEFAULT_KDIR="$BUILD/linux-6.19.7" ;;
esac
KDIR="${KDIR:-$DEFAULT_KDIR}"

if [ "$ARCH" = "arm64" ]; then
    IMAGE_PATH="arch/arm64/boot/Image"
    IMAGE_TARGET="Image"
    DEFAULT_OUTPUT="Image-$CONFIG_NAME"
else
    IMAGE_PATH="arch/x86/boot/bzImage"
    IMAGE_TARGET="bzImage"
    DEFAULT_OUTPUT="bzImage-$CONFIG_NAME"
fi
OUTPUT_NAME="${2:-$DEFAULT_OUTPUT}"

CONFIG_FILE="$REPO_DIR/configs/$CONFIG_NAME"

if [ ! -f "$CONFIG_FILE" ]; then
    echo "ERROR: Config not found: $CONFIG_FILE"
    echo "Available configs:"
    ls "$REPO_DIR/configs/"
    exit 1
fi

if [ ! -d "$KDIR" ]; then
    echo "ERROR: Kernel source not found at $KDIR"
    echo "Run scripts/fetch-sources.sh (KERNEL_FLAVOR / KERNEL_VERSION) first."
    exit 1
fi

MAKE_ARGS=(ARCH="$ARCH" CROSS_COMPILE="$CROSS_COMPILE")

echo "=== Building WayangOS Kernel ==="
echo "  Config:  $CONFIG_NAME"
echo "  Arch:    $ARCH"
echo "  Output:  $OUTPUT_NAME"
echo "  Source:  $KDIR"

cd "$KDIR"

# Backup current .config if it exists
if [ -f .config ]; then
    cp .config .config.bak
    echo "  Backed up existing .config → .config.bak"
fi

# Resolve base + fragment config
FIRST_LINE="$(head -n 1 "$CONFIG_FILE")"
if [[ "$FIRST_LINE" =~ ^#[[:space:]]+WAYANG_BASE:[[:space:]]+([^[:space:]]+) ]]; then
    BASE_NAME="${BASH_REMATCH[1]}"
    if [ "$BASE_NAME" = "defconfig" ]; then
        echo "  Base: defconfig (make defconfig)"
        make "${MAKE_ARGS[@]}" defconfig >/dev/null
    else
        BASE_FILE="$REPO_DIR/configs/$BASE_NAME"
        if [ ! -f "$BASE_FILE" ]; then
            echo "ERROR: Base config not found: $BASE_FILE" >&2
            exit 1
        fi
        echo "  Base: $BASE_NAME"
        cp "$BASE_FILE" .config
    fi
    echo "  Fragment: $CONFIG_NAME"
    tail -n +2 "$CONFIG_FILE" >> .config
else
    echo "  Config: full .config"
    cp "$CONFIG_FILE" .config
fi

make "${MAKE_ARGS[@]}" olddefconfig 2>&1 | tail -3

echo "Building kernel ($(nproc) jobs)..."
make "${MAKE_ARGS[@]}" -j"$(nproc)" "$IMAGE_TARGET" 2>&1 | tail -10

# Copy output
cp "$IMAGE_PATH" "$BUILD/$OUTPUT_NAME"
echo ""
echo "=== Kernel Built ==="
echo "  Output: $BUILD/$OUTPUT_NAME"
echo "  Size: $(du -h "$BUILD/$OUTPUT_NAME" | cut -f1)"
