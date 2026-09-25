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

# Create POS init script
cat > etc/init.d/pos-app << 'POS_INIT'
#!/bin/sh
case "$1" in
    start)
        echo "  Starting WayangOS POS..."
        sleep 1
        mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
        mkdir -p /dev/input
        mdev -s 2>/dev/null || true

        # Setup framebuffer
        if [ -e /dev/fb0 ]; then
            chmod 666 /dev/fb0
            chmod 666 /dev/input/event* /dev/input/mice 2>/dev/null
            # Disable kernel console on fb
            echo 0 > /sys/class/vtconsole/vtcon1/bind 2>/dev/null
            echo 0 > /proc/sys/kernel/printk 2>/dev/null
            # Clear screen
            dd if=/dev/zero of=/dev/fb0 bs=4096 count=1000 2>/dev/null
        fi

        mkdir -p /data
        /usr/bin/wayang-pos &
        ;;
    stop)
        killall wayang-pos 2>/dev/null
        ;;
esac
POS_INIT
chmod +x etc/init.d/pos-app

# Add POS startup to init
if ! grep -q "pos-app" etc/init.d/rcS 2>/dev/null; then
    sed -i '/WayangOS ready/a\\n# Start POS application\n/etc/init.d/pos-app start' etc/init.d/rcS
fi

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
