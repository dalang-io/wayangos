#!/bin/sh
# build-arm64-image.sh -- assemble a WayangOS A/B boot-partition tree for ARM64
# (milestone M7: Raspberry Pi 3, Orange Pi Zero 2W). See docs/UPDATE-ARM.md and
# the frozen interfaces in docs/UPDATE-DESIGN.md ("ARM64 slots (M7)").
#
# This produces a *directory tree* that is the content of the FAT boot partition
# (the ESP-equivalent on ARM). It does not install firmware blobs, U-Boot or a
# rootfs; it lays out the slot files and the board boot configuration so that
# `wayang` can stage updates into the idle slot.
#
# The per-slot kernel filename is always `vmlinuz` on both boards, matching what
# the `wayang` updater writes (see wayang/src/staging.rs). `--kernel` names the
# *input* artifact (on ARM usually `Image`).
#
# Usage:
#   ./scripts/build-arm64-image.sh --board rpi3|orangepi \
#       --kernel Image --initramfs initramfs.img \
#       [--dtb X.dtb] [--slot A|B] --out DIR [--img FILE] [--size MiB]
#
# Options:
#   --board    rpi3      Raspberry Pi 3 (config.txt + os_prefix + tryboot.txt)
#              orangepi  Orange Pi Zero 2W (extlinux + U-Boot boot.cmd)
#   --kernel   kernel image to install into <slot>/vmlinuz (required)
#   --initramfs initramfs image to install into <slot>/initramfs.img (required)
#   --dtb      optional device tree blob, copied to <slot>/dtb/
#   --slot     slot to populate, A or B (default: A)
#   --out      output directory (boot-partition root) (required)
#   --img      best-effort: also build a FAT32 image at FILE
#   --size     FAT image size in MiB (default: 512)
#   -h|--help  show this help
#
# Env:
#   IMG_SIZE   same as --size
#
# Exit codes: 0 ok, 1 usage/input error, 2 unsupported/missing tool requested.
#
# shellcheck disable=SC2016  # generated U-Boot scripts contain literal ${vars}
set -eu

PROG=$(basename "$0")

usage() {
    cat <<EOF
Usage: $PROG --board rpi3|orangepi --kernel IMAGE --initramfs INITRAMFS
              [--dtb DTB] [--slot A|B] --out DIR [--img FILE] [--size MiB]

Assemble a WayangOS A/B boot-partition tree for ARM64 boards (M7).
See docs/UPDATE-ARM.md for the design and runbook.
EOF
}

note() { printf '%s\n' "$*"; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
die()  { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

BOARD=""
KERNEL=""
INITRAMFS=""
DTB=""
SLOT="A"
OUT_DIR=""
IMG=""
IMG_SIZE="${IMG_SIZE:-512}"

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage; exit 0 ;;
        --board)
            [ $# -ge 2 ] || die "--board needs a value"
            BOARD=$2; shift 2 ;;
        --kernel)
            [ $# -ge 2 ] || die "--kernel needs a file"
            KERNEL=$2; shift 2 ;;
        --initramfs)
            [ $# -ge 2 ] || die "--initramfs needs a file"
            INITRAMFS=$2; shift 2 ;;
        --dtb)
            [ $# -ge 2 ] || die "--dtb needs a file"
            DTB=$2; shift 2 ;;
        --slot)
            [ $# -ge 2 ] || die "--slot needs A or B"
            SLOT=$2; shift 2 ;;
        --out)
            [ $# -ge 2 ] || die "--out needs a directory"
            OUT_DIR=$2; shift 2 ;;
        --img)
            [ $# -ge 2 ] || die "--img needs a file"
            IMG=$2; shift 2 ;;
        --size)
            [ $# -ge 2 ] || die "--size needs MiB"
            IMG_SIZE=$2; shift 2 ;;
        --*)
            die "unknown option: $1" ;;
        *)
            die "unexpected positional argument: $1" ;;
    esac
done

[ -n "$BOARD" ] || die "missing --board (rpi3|orangepi)"
[ -n "$KERNEL" ] || die "missing --kernel"
[ -n "$INITRAMFS" ] || die "missing --initramfs"
[ -n "$OUT_DIR" ] || die "missing --out"

case "$BOARD" in
    rpi3|orangepi) ;;
    *) die "--board must be rpi3 or orangepi (got: $BOARD)" ;;
esac

case "$SLOT" in
    A|B) ;;
    *) die "--slot must be A or B (got: $SLOT)" ;;
esac

[ -f "$KERNEL" ] || die "kernel not found: $KERNEL"
[ -r "$KERNEL" ] || die "kernel not readable: $KERNEL"
[ -f "$INITRAMFS" ] || die "initramfs not found: $INITRAMFS"
[ -r "$INITRAMFS" ] || die "initramfs not readable: $INITRAMFS"
if [ -n "$DTB" ]; then
    [ -f "$DTB" ] || die "dtb not found: $DTB"
    [ -r "$DTB" ] || die "dtb not readable: $DTB"
fi

case "$IMG_SIZE" in
    ''|*[!0-9]*) die "--size must be a whole number of MiB (got: $IMG_SIZE)" ;;
esac
[ "$IMG_SIZE" -gt 0 ] || die "--size must be > 0"

other_slot() {
    if [ "$1" = "A" ]; then printf 'B'; else printf 'A'; fi
}

# ---------------------------------------------------------------- layout ----

mkdir -p "$OUT_DIR/$SLOT"

# Canonical slot filenames (match wayang/src/staging.rs).
cp "$KERNEL" "$OUT_DIR/$SLOT/vmlinuz"
cp "$INITRAMFS" "$OUT_DIR/$SLOT/initramfs.img"

DTB_NAME=""
if [ -n "$DTB" ]; then
    DTB_NAME=$(basename "$DTB")
    mkdir -p "$OUT_DIR/$SLOT/dtb"
    cp "$DTB" "$OUT_DIR/$SLOT/dtb/$DTB_NAME"
fi

# --------------------------------------------------------- RPi 3 boot cfg ---

rpi_config() {
    # $1 = slot, $2 = output file
    _slot=$1
    _file=$2
    _dtb="$OUT_DIR/$_slot/dtb/$DTB_NAME"
    {
        printf '# WayangOS / Raspberry Pi 3 -- generated by %s\n' "$PROG"
        printf '# Slot: %s\n' "$_slot"
        printf 'arm_64bit=1\n'
        printf 'enable_uart=1\n'
        printf 'os_prefix=%s/\n' "$_slot"
        printf 'kernel=vmlinuz\n'
        printf 'initramfs initramfs.img followkernel\n'
        if [ -n "$DTB_NAME" ] && [ -f "$_dtb" ]; then
            printf 'device_tree=dtb/%s\n' "$DTB_NAME"
        fi
    } > "$_file"
}

write_rpi() {
    # config.txt = default / known-good; tryboot.txt = one-shot trial slot.
    rpi_config "$SLOT" "$OUT_DIR/config.txt"
    note "Wrote $OUT_DIR/config.txt (os_prefix=$SLOT/)"

    OTHER=$(other_slot "$SLOT")
    if [ -d "$OUT_DIR/$OTHER" ]; then
        rpi_config "$OTHER" "$OUT_DIR/tryboot.txt"
        note "Wrote $OUT_DIR/tryboot.txt (os_prefix=$OTHER/, one-shot tryboot)"
    else
        note "Slot $OTHER/ not present; skipping tryboot.txt"
    fi

    cat > "$OUT_DIR/cmdline.txt" <<'EOF'
console=serial0,115200 console=tty1 root=/dev/ram0 rw quiet
EOF
    note "Wrote $OUT_DIR/cmdline.txt"
}

# --------------------------------------------------- Orange Pi (extlinux) ---

extlinux_label() {
    _slot=$1
    printf 'LABEL wayang-%s\n' "$_slot"
    printf '    LINUX ../%s/vmlinuz\n' "$_slot"
    printf '    INITRD ../%s/initramfs.img\n' "$_slot"
    if [ -n "$DTB_NAME" ] && [ -f "$OUT_DIR/$_slot/dtb/$DTB_NAME" ]; then
        printf '    FDT ../%s/dtb/%s\n' "$_slot" "$DTB_NAME"
    fi
    printf '    APPEND console=ttyS0,115200 root=/dev/ram0 rw quiet\n'
}

write_extlinux() {
    mkdir -p "$OUT_DIR/extlinux"
    _cfg="$OUT_DIR/extlinux/extlinux.cfg"
    {
        printf '# WayangOS / Orange Pi Zero 2W -- generated by %s\n' "$PROG"
        printf '# Default is switched by `wayang` staging (see wayang/vars).\n'
        printf 'DEFAULT wayang-%s\n' "$SLOT"
        printf 'TIMEOUT 1\n'
        printf 'PROMPT 0\n'
        printf 'MENU TITLE WayangOS A/B\n'
        printf '\n'
        extlinux_label "$SLOT"
        OTHER=$(other_slot "$SLOT")
        if [ -d "$OUT_DIR/$OTHER" ]; then
            printf '\n'
            extlinux_label "$OTHER"
        fi
    } > "$_cfg"
    # Stock U-Boot `sysboot` reads extlinux.conf; keep a copy under that name
    # while also emitting the .cfg requested by the M7 spec.
    cp "$_cfg" "$OUT_DIR/extlinux/extlinux.conf"
    note "Wrote $OUT_DIR/extlinux/extlinux.cfg and extlinux.conf"
}

write_boot_cmd() {
    # U-Boot boot script that consumes <boot>/wayang/vars to pick the slot.
    # Best effort: compile with mkimage when available.
    _cmd="$OUT_DIR/boot.cmd"
    _dtb_name=${DTB_NAME:-sun50i-h618-orangepi-zero2w.dtb}
    {
        printf '# WayangOS A/B selector for Orange Pi Zero 2W (U-Boot script).\n'
        printf '# Generated by %s\n' "$PROG"
        printf '# Compile: mkimage -T script -C none -n "WayangOS A/B" -d boot.cmd boot.scr\n'
        printf '# Place boot.scr (or source boot.cmd) at the root of FAT mmc 0:1.\n'
        printf '\n'
        printf 'setenv bootpart 1\n'
        printf 'setenv scriptaddr 0x42000000\n'
        printf 'setenv wayang_slot A\n'
        printf 'setenv wayang_good A\n'
        printf 'setenv wayang_attempts 0\n'
        printf 'setenv wayang_dtb %s\n' "$_dtb_name"
        printf '\n'
        printf 'if load mmc 0:${bootpart} ${scriptaddr} wayang/vars; then\n'
        printf '    env import -t ${scriptaddr} ${filesize}\n'
        printf '    if test -n "${wayang_attempts}" && test ${wayang_attempts} -ge 3; then\n'
        printf '        setenv wayang_slot ${wayang_good}\n'
        printf '    fi\n'
        printf 'fi\n'
        printf '\n'
        printf 'load mmc 0:${bootpart} ${kernel_addr_r} ${wayang_slot}/vmlinuz\n'
        printf 'load mmc 0:${bootpart} ${ramdisk_addr_r} ${wayang_slot}/initramfs.img\n'
        printf 'setenv ramdisk_size ${filesize}\n'
        printf 'load mmc 0:${bootpart} ${fdt_addr_r} ${wayang_slot}/dtb/${wayang_dtb}\n'
        printf 'booti ${kernel_addr_r} ${ramdisk_addr_r}:${ramdisk_size} ${fdt_addr_r}\n'
    } > "$_cmd"
    note "Wrote $OUT_DIR/boot.cmd"

    if command -v mkimage >/dev/null 2>&1; then
        if mkimage -T script -C none -n 'WayangOS A/B' -d "$_cmd" "$OUT_DIR/boot.scr" >/dev/null 2>&1; then
            note "Compiled $OUT_DIR/boot.scr (mkimage)"
        else
            warn "mkimage failed; boot.cmd left for manual compilation"
        fi
    else
        note "mkimage not found; boot.cmd left for manual compilation"
    fi
}

# ------------------------------------------------------------ state vars ----

ensure_vars() {
    _vars="$OUT_DIR/wayang/vars"
    mkdir -p "$OUT_DIR/wayang"
    if [ ! -f "$_vars" ]; then
        printf 'wayang_slot=A\nwayang_good=A\nwayang_attempts=0\n' > "$_vars"
        note "Created $_vars (slot A, attempts 0)"
        return
    fi
    for _kv in wayang_slot=A wayang_good=A wayang_attempts=0; do
        _k=${_kv%%=*}
        if ! grep -q "^${_k}=" "$_vars" 2>/dev/null; then
            printf '%s\n' "$_kv" >> "$_vars"
            note "Merged missing key $_k into $_vars"
        fi
    done
}

# ------------------------------------------------------------ FAT image ----

make_fat_image() {
    _img=$1
    _mkfs=""
    for _c in mkfs.vfat mkfs.fat; do
        if command -v "$_c" >/dev/null 2>&1; then
            _mkfs=$_c
            break
        fi
    done
    if [ -z "$_mkfs" ]; then
        warn "--img requested but no mkfs.vfat/mkfs.fat found; skipping image"
        return 1
    fi

    note "Creating FAT32 image $_img (${IMG_SIZE} MiB)..."
    rm -f "$_img"
    dd if=/dev/zero of="$_img" bs=1048576 count="$IMG_SIZE" >/dev/null 2>&1
    "$_mkfs" -F 32 -n WAYANGBOOT "$_img" >/dev/null

    if command -v mcopy >/dev/null 2>&1; then
        for _entry in "$OUT_DIR"/*; do
            [ -e "$_entry" ] || continue
            mcopy -s -i "$_img" "$_entry" ::/
        done
        note "FAT image ready (mtools): $_img"
        return 0
    fi

    if command -v losetup >/dev/null 2>&1 && [ "$(id -u)" -eq 0 ]; then
        _loop=$(losetup -f --show "$_img")
        _mnt=$(mktemp -d)
        mount "$_loop" "$_mnt"
        cp -a "$OUT_DIR"/. "$_mnt"/
        sync
        umount "$_mnt"
        rmdir "$_mnt"
        losetup -d "$_loop"
        note "FAT image ready (loop mount): $_img"
        return 0
    fi

    warn "formatted $_img but found neither mcopy nor (root + losetup); mount it manually"
    return 1
}

# ------------------------------------------------------------------ main ----

case "$BOARD" in
    rpi3)     write_rpi ;;
    orangepi) write_extlinux; write_boot_cmd ;;
esac

ensure_vars

echo ""
echo "=== ARM64 boot tree ==="
echo "  Board:  $BOARD"
echo "  Slot:   $SLOT"
echo "  Out:    $OUT_DIR"
echo "  Kernel: $KERNEL -> $SLOT/vmlinuz"
echo "  Initr:  $INITRAMFS -> $SLOT/initramfs.img"
[ -n "$DTB_NAME" ] && echo "  DTB:    $DTB -> $SLOT/dtb/$DTB_NAME"

if [ -n "$IMG" ]; then
    echo ""
    make_fat_image "$IMG" || true
fi

echo ""
echo "Contents:"
for _f in $(cd "$OUT_DIR" && find . -type f 2>/dev/null | sort); do
    printf '  %s\n' "$_f"
done

echo ""
echo "Next steps:"
echo "  1. Add board firmware/U-Boot and a rootfs; see docs/UPDATE-ARM.md."
echo "  2. Flash $OUT_DIR to the boot partition (FAT, label WAYANGBOOT)."
echo "  3. On the board: wayang update --from <bundle>.wup   # stages the idle slot"