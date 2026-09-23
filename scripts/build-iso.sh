#!/bin/bash
# Build a WayangOS bootable ISO from kernel + initramfs
# Usage: ./scripts/build-iso.sh [kernel] [initramfs] [output.iso]
set -e

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
KERNEL="${1:-$BUILD/bzImage-qemu}"
INITRAMFS="${2:-$BUILD/wayangos-initramfs.img}"
OUTPUT="${3:-$BUILD/wayangos.iso}"
VERSION="${VERSION:-dev}"

for f in "$KERNEL" "$INITRAMFS"; do
    [ -f "$f" ] || { echo "ERROR: Missing $f"; exit 1; }
done

echo "=== Building WayangOS ISO ==="
echo "  Kernel:    $KERNEL ($(du -h "$KERNEL" | cut -f1))"
echo "  Initramfs: $INITRAMFS ($(du -h "$INITRAMFS" | cut -f1))"
echo "  Output:    $OUTPUT"

ISO_DIR=$(mktemp -d)
trap 'rm -rf "$ISO_DIR"' EXIT

mkdir -p "$ISO_DIR/boot/grub"
cp "$KERNEL" "$ISO_DIR/boot/vmlinuz"
cp "$INITRAMFS" "$ISO_DIR/boot/initramfs.img"

cat > "$ISO_DIR/boot/grub/grub.cfg" << EOF
set timeout=3
set default=0

set menu_color_normal=light-gray/black
set menu_color_highlight=yellow/black

menuentry "WayangOS $VERSION" {
    linux /boot/vmlinuz console=tty0 loglevel=1 quiet
    initrd /boot/initramfs.img
}

menuentry "WayangOS $VERSION (Verbose)" {
    linux /boot/vmlinuz console=tty0
    initrd /boot/initramfs.img
}

menuentry "WayangOS $VERSION (Serial Console)" {
    linux /boot/vmlinuz console=ttyS0,115200
    initrd /boot/initramfs.img
}
EOF

grub-mkrescue -o "$OUTPUT" "$ISO_DIR" -- -volid WAYANGOS 2>&1

echo ""
echo "=== ISO Built ==="
echo "  Output: $OUTPUT"
echo "  Size: $(du -h "$OUTPUT" | cut -f1)"
