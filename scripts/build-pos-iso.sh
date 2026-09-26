#!/bin/bash
# Build complete WayangOS POS ISO: rootfs + POS binary + kernel → ISO
# Usage: ./scripts/build-pos-iso.sh [config-name] [output.iso]
# Example: ./scripts/build-pos-iso.sh defconfig-qemu wayangos-pos-qemu.iso
set -e

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPTS_DIR="$REPO_DIR/scripts"
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
POS_BINARY="${POS_BINARY:-$BUILD/wayang-pos-static}"

CONFIG_NAME="${1:-defconfig-qemu}"
OUTPUT="${2:-$BUILD/wayangos-pos-${CONFIG_NAME}.iso}"
KERNEL="$BUILD/bzImage-$CONFIG_NAME"
INITRAMFS="$BUILD/wayangos-pos-initramfs.img"

echo "=== WayangOS POS ISO Pipeline ==="
echo "  Config: $CONFIG_NAME"
echo "  POS binary: $POS_BINARY"
echo ""

# ============================================
# 1. Build kernel (if not present)
# ============================================
if [ ! -f "$KERNEL" ]; then
    echo "--- Building kernel ---"
    bash "$SCRIPTS_DIR/build-kernel.sh" "$CONFIG_NAME" "bzImage-$CONFIG_NAME"
fi

# ============================================
# 2. Build base rootfs (if not present)
# ============================================
BASE_INITRAMFS="$BUILD/wayangos-initramfs.img"
if [ ! -f "$BASE_INITRAMFS" ]; then
    echo "--- Building rootfs ---"
    bash "$SCRIPTS_DIR/build-rootfs.sh"
fi

# ============================================
# 3. Add POS binary to rootfs
# ============================================
echo "--- Adding POS to rootfs ---"

if [ ! -f "$POS_BINARY" ]; then
    echo "--- POS binary not found, building it ---"
    bash "$SCRIPTS_DIR/build-pos.sh"
    POS_BINARY="$BUILD/wayang-pos-static"
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

cd "$WORK"
gunzip -c "$BASE_INITRAMFS" | cpio -id 2>/dev/null

# Install POS binary
cp "$POS_BINARY" usr/bin/wayang-pos
chmod +x usr/bin/wayang-pos
echo "  POS binary: $(du -h usr/bin/wayang-pos | cut -f1)"

# /etc/init.d/pos (from build-rootfs.sh) starts /usr/bin/wayang-pos at boot,
# restarts it if it crashes and is controlled with `wayang pos`.

# Rebuild initramfs with POS
find . -print0 | cpio -0 -o -H newc -R 0:0 2>/dev/null | gzip -9 > "$INITRAMFS"
echo "  POS initramfs: $(du -h "$INITRAMFS" | cut -f1)"

# ============================================
# 4. Build ISO
# ============================================
echo "--- Building ISO ---"
VERSION="POS" bash "$SCRIPTS_DIR/build-iso.sh" "$KERNEL" "$INITRAMFS" "$OUTPUT"

echo ""
echo "=== POS ISO Complete ==="
echo "  $OUTPUT ($(du -h "$OUTPUT" | cut -f1))"
echo ""
echo "Test with QEMU:"
echo "  qemu-system-x86_64 -cdrom $OUTPUT -m 256M -vga std -display gtk \\"
echo "    -nic user,hostfwd=tcp::2222-:22"
