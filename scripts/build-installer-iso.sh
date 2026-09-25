#!/bin/bash
# Build a WayangOS USB installer ISO (hybrid: dd it to a USB stick).
#
# The installer boots the normal WayangOS rootfs plus the wayang-installer TUI
# (installer/, built by scripts/build-installer.sh), which partitions an internal disk (GPT: ESP + ext4 /data) and copies a UEFI
# GRUB, the kernel and the rootfs initramfs onto it. The installed system
# boots UEFI-only (Secure Boot off) and still runs from RAM.
#
# Usage: ./scripts/build-installer-iso.sh [kernel] [initramfs] [output.iso]
#   kernel     default: $BUILD/bzImage-intel  (scripts/build-kernel.sh defconfig-intel bzImage-intel)
#   initramfs  default: $BUILD/wayangos-initramfs.img  (scripts/build-rootfs.sh)
#
# Env:
#   BUILD_DIR       build root (default: ~/wayangos-build)
#   CROSS_COMPILE   toolchain prefix for the static sfdisk/mke2fs (x86_64 target)
#   INSTALLER_BIN   wayang-installer binary
#                   (default: dist/wayang-installer-x86_64-unknown-linux-musl)
#
# Needs: grub-mkrescue + grub-mkstandalone with the i386-pc and x86_64-efi
# platforms, xorriso, mtools, and a C toolchain for x86_64.
set -e

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INSTALLER_BIN="${INSTALLER_BIN:-$REPO_DIR/dist/wayang-installer-x86_64-unknown-linux-musl}"
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
KERNEL="${1:-$BUILD/bzImage-intel}"
BASE_INITRAMFS="${2:-$BUILD/wayangos-initramfs.img}"
OUTPUT="${3:-$BUILD/wayangos-installer.iso}"
CROSS_COMPILE="${CROSS_COMPILE:-}"

UTIL_LINUX_VERSION="${UTIL_LINUX_VERSION:-2.41.6}"
E2FSPROGS_VERSION="${E2FSPROGS_VERSION:-1.47.4}"

for f in "$KERNEL" "$BASE_INITRAMFS"; do
    [ -f "$f" ] || { echo "ERROR: Missing $f" >&2; exit 1; }
done
[ -f "$INSTALLER_BIN" ] || { echo "ERROR: Missing $INSTALLER_BIN — run scripts/build-installer.sh" >&2; exit 1; }
for tool in grub-mkrescue grub-mkstandalone xorriso mformat cpio; do
    command -v "$tool" >/dev/null 2>&1 || { echo "ERROR: $tool not found" >&2; exit 1; }
done
for platform in i386-pc x86_64-efi; do
    [ -d "/usr/lib/grub/$platform" ] || { echo "ERROR: GRUB $platform modules missing (/usr/lib/grub/$platform)" >&2; exit 1; }
done

echo "=== Building WayangOS installer ISO ==="
echo "  Kernel:    $KERNEL"
echo "  Initramfs: $BASE_INITRAMFS"
echo "  Output:    $OUTPUT"

cd "$BUILD"
fetch() { [ -f "$2" ] || curl -fL --retry 3 -o "$2" "$1"; }
HOST_ARGS=(${CROSS_COMPILE:+--host="${CROSS_COMPILE%-}"})

# ============================================
# 1. Static sfdisk (GPT) and mke2fs (ext4) — BusyBox has neither
# ============================================
if [ ! -x "util-linux-$UTIL_LINUX_VERSION/sfdisk.static" ]; then
    echo "[1/4] Building static sfdisk (util-linux $UTIL_LINUX_VERSION)..."
    fetch "https://www.kernel.org/pub/linux/utils/util-linux/v${UTIL_LINUX_VERSION%.*}/util-linux-$UTIL_LINUX_VERSION.tar.xz" \
        "util-linux-$UTIL_LINUX_VERSION.tar.xz"
    [ -d "util-linux-$UTIL_LINUX_VERSION" ] || tar -xf "util-linux-$UTIL_LINUX_VERSION.tar.xz"
    (
        cd "util-linux-$UTIL_LINUX_VERSION"
        CC="${CROSS_COMPILE}gcc" ./configure "${HOST_ARGS[@]}" \
            --disable-all-programs --enable-fdisks=check --enable-libfdisk \
            --enable-libuuid --enable-libblkid --enable-libsmartcols \
            --enable-static-programs=sfdisk --disable-shared --disable-nls \
            --without-ncurses --without-ncursesw --without-readline \
            --without-systemd --without-udev --without-python >/dev/null
        make -j"$(nproc)" sfdisk.static >/dev/null
        "${CROSS_COMPILE}strip" sfdisk.static
    )
else
    echo "[1/4] sfdisk.static already built"
fi

if [ ! -x "e2fsprogs-$E2FSPROGS_VERSION/misc/mke2fs" ]; then
    echo "      Building static mke2fs (e2fsprogs $E2FSPROGS_VERSION)..."
    fetch "https://www.kernel.org/pub/linux/kernel/people/tytso/e2fsprogs/v$E2FSPROGS_VERSION/e2fsprogs-$E2FSPROGS_VERSION.tar.xz" \
        "e2fsprogs-$E2FSPROGS_VERSION.tar.xz"
    [ -d "e2fsprogs-$E2FSPROGS_VERSION" ] || tar -xf "e2fsprogs-$E2FSPROGS_VERSION.tar.xz"
    (
        cd "e2fsprogs-$E2FSPROGS_VERSION"
        CC="${CROSS_COMPILE}gcc" ./configure "${HOST_ARGS[@]}" \
            --disable-nls --disable-fuse2fs --disable-e2initrd-helper \
            --disable-defrag --disable-imager --disable-debugfs \
            LDFLAGS=-static >/dev/null
        make -j"$(nproc)" libs >/dev/null
        make -j"$(nproc)" -C misc mke2fs >/dev/null
        "${CROSS_COMPILE}strip" misc/mke2fs
    )
fi

# ============================================
# 2. GRUB for the installed disk (UEFI fallback path, finds its ESP by label)
# ============================================
echo "[2/4] Building GRUB EFI image for the target disk..."
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# partition/filesystem modules aren't autoloaded for `search`
cat > "$WORK/embedded.cfg" << 'EOF'
insmod part_gpt
insmod part_msdos
insmod fat
search --no-floppy --label --set=root WAYANGBOOT
configfile ($root)/boot/grub/grub.cfg
EOF
grub-mkstandalone -O x86_64-efi -o "$WORK/BOOTX64.EFI" \
    --locales="" --fonts="" --themes="" \
    "boot/grub/grub.cfg=$WORK/embedded.cfg"

cat > "$WORK/grub-disk.cfg" << 'EOF'
set timeout=3
set default=0

menuentry "WayangOS" {
    linux /boot/vmlinuz loglevel=3 wayang.data=LABEL=WAYANGDATA
    initrd /boot/initramfs.img
}
menuentry "WayangOS (verbose)" {
    linux /boot/vmlinuz wayang.data=LABEL=WAYANGDATA
    initrd /boot/initramfs.img
}
menuentry "WayangOS (serial console)" {
    linux /boot/vmlinuz console=ttyS0,115200 wayang.data=LABEL=WAYANGDATA
    initrd /boot/initramfs.img
}
menuentry "UEFI firmware settings" {
    fwsetup
}
EOF

# ============================================
# 3. Installer initramfs = rootfs + installer + tools + payload
# ============================================
echo "[3/4] Building installer initramfs..."
mkdir "$WORK/root"
(cd "$WORK/root" && gzip -dc "$BASE_INITRAMFS" | cpio -id --quiet)

PAYLOAD="$WORK/root/usr/share/wayang-install"
mkdir -p "$PAYLOAD"
cp "$WORK/BOOTX64.EFI" "$PAYLOAD/BOOTX64.EFI"
cp "$WORK/grub-disk.cfg" "$PAYLOAD/grub.cfg"
cp "$KERNEL" "$PAYLOAD/vmlinuz"
cp "$BASE_INITRAMFS" "$PAYLOAD/initramfs.img"

install -m 755 "util-linux-$UTIL_LINUX_VERSION/sfdisk.static" "$WORK/root/usr/sbin/sfdisk"
install -m 755 "e2fsprogs-$E2FSPROGS_VERSION/misc/mke2fs" "$WORK/root/usr/sbin/mke2fs"
install -m 755 "$INSTALLER_BIN" "$WORK/root/usr/sbin/wayang-installer"

# The console runs the installer once (when booted with wayang.install), then
# a normal shell. Not from rcS: init starts the tty2/tty3 shells only after
# rcS returns, and they're handy while installing.
cat > "$WORK/root/usr/sbin/wayang-console" << 'EOF'
#!/bin/sh
if grep -qw wayang.install /proc/cmdline && [ ! -e /tmp/.wayang-install-ran ]; then
    : > /tmp/.wayang-install-ran
    /usr/sbin/wayang-installer
    echo "Run wayang-installer to open the installer again."
fi
exec /bin/sh -l
EOF
chmod 755 "$WORK/root/usr/sbin/wayang-console"
sed -i 's|^::respawn:/bin/cttyhack /bin/sh -l$|::respawn:/bin/cttyhack /usr/sbin/wayang-console|' "$WORK/root/etc/inittab"
grep -q wayang-console "$WORK/root/etc/inittab" || { echo "ERROR: console entry not found in /etc/inittab" >&2; exit 1; }

(cd "$WORK/root" && find . | cpio -o -H newc -R 0:0 --quiet | gzip -9 > "$WORK/installer.img")

# ============================================
# 4. Hybrid ISO (BIOS + UEFI, bootable from USB)
# ============================================
echo "[4/4] Building ISO..."
ISO="$WORK/iso"
mkdir -p "$ISO/boot/grub"
cp "$KERNEL" "$ISO/boot/vmlinuz"
cp "$WORK/installer.img" "$ISO/boot/installer.img"
cat > "$ISO/boot/grub/grub.cfg" << 'EOF'
set timeout=10
set default=0

menuentry "Install WayangOS to disk" {
    linux /boot/vmlinuz loglevel=3 wayang.install
    initrd /boot/installer.img
}
menuentry "Try WayangOS (live, nothing is written)" {
    linux /boot/vmlinuz loglevel=3
    initrd /boot/installer.img
}
menuentry "Install WayangOS (serial console)" {
    linux /boot/vmlinuz console=ttyS0,115200 wayang.install
    initrd /boot/installer.img
}
EOF

# the volume label lets wayang-installer recognise (and skip) its own USB stick
grub-mkrescue -o "$OUTPUT" "$ISO" -- -volid WAYANG_INSTALL 2>&1 | grep -v '^xorriso : UPDATE' || true
[ -s "$OUTPUT" ] || { echo "ERROR: grub-mkrescue produced no ISO" >&2; exit 1; }

echo ""
echo "=== Installer ISO Built ==="
echo "  Output: $OUTPUT ($(du -h "$OUTPUT" | cut -f1))"
echo "  Write it to a USB stick, e.g. on macOS: sudo dd if=$(basename "$OUTPUT") of=/dev/rdiskN bs=4m"
