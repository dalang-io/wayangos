#!/bin/bash
# Stage a small, curated set of firmware blobs into $BUILD/wifi/firmware, which
# build-rootfs.sh bundles at /lib/firmware in the initramfs.
#
# The kernel has no CONFIG_FW_LOADER_COMPRESS, so .zst sources are decompressed
# here. Best-effort: a missing source file is a warning, not an error (CI
# runners often lack linux-firmware).
#
# Source tree: $FIRMWARE_DIR (default: /lib/firmware, e.g. Ubuntu linux-firmware).
# Usage: [FIRMWARE_DIR=/lib/firmware] ./scripts/stage-firmware.sh
set -euo pipefail

BUILD_DIR="${BUILD_DIR:-$HOME/wayangos-build}"
FIRMWARE_DIR="${FIRMWARE_DIR:-/lib/firmware}"
DEST="$BUILD_DIR/wifi/firmware"

# Curated blobs. Add entries when new hardware shows up (find the exact name in
# `dmesg | grep -i firmware` or with `wayang wifi detect`).
FILES="
iwlwifi-3160-17.ucode
iwlwifi-3168-29.ucode
iwlwifi-7265D-29.ucode
iwlwifi-8265-36.ucode
iwlwifi-9000-pu-b0-jf-b0-46.ucode
iwlwifi-9260-th-b0-jf-b0-46.ucode
i915/kbl_dmc_ver1_04.bin
regulatory.db
regulatory.db.p7s
intel/ibt-hw-37.8.10-fw-22.50.19.14.f.bseq
intel/ibt-hw-37.8.bseq
intel/ibt-12-16.sfi
intel/ibt-12-16.ddc
"

echo "=== staging firmware ($FIRMWARE_DIR -> $DEST) ==="
mkdir -p "$DEST"
missing=0
for f in $FILES; do
    src=""
    for cand in "$FIRMWARE_DIR/$f" "$FIRMWARE_DIR/$f.zst"; do
        [ -f "$cand" ] && { src="$cand"; break; }
    done
    if [ -z "$src" ]; then
        echo "  WARNING: $f not found under $FIRMWARE_DIR" >&2
        missing=$((missing + 1))
        continue
    fi
    mkdir -p "$DEST/$(dirname "$f")"
    case "$src" in
        *.zst) zstd -q -d -c "$src" > "$DEST/$f" ;;
        *)     cp -f "$src" "$DEST/$f" ;;
    esac
    echo "  $f"
done
echo "  firmware in $DEST ($(du -sh "$DEST" | cut -f1))"
[ "$missing" -eq 0 ] || echo "  ($missing file(s) missing — continuing)" >&2
