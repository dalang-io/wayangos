#!/bin/bash
# One-shot WayangOS build pipeline (kernel -> rootfs -> installer ISO + bundle).
# Shared by CI and remote builders. Run from the repo root.
#
# Env:
#   BUILD_DIR       build dir (default: $HOME/wayangos-build)
#   WAYANG_VERSION  WayangOS version (default: 1.0.13)
#   KERNEL_VERSION  kernel version for the ISO name/manifest (default: 7.2.7)
#   KERNEL_CONFIG   kernel config name (default: defconfig-intel)
#   CHANNEL         release channel (default: stable)
#   WAYANG_EDITION  edition string (default: installer)
#   WAYANG_KEY      signing key file (optional) -> signed .wup
#   WAYANG_KEYID    signing key id (default: release)
set -e

export PATH="$HOME/.cargo/bin:$PATH"
BUILD_DIR="${BUILD_DIR:-$HOME/wayangos-build}"
WAYANG_VERSION="${WAYANG_VERSION:-1.0.13}"
KERNEL_VERSION="${KERNEL_VERSION:-7.2.7}"
KERNEL_CONFIG="${KERNEL_CONFIG:-defconfig-intel}"
CHANNEL="${CHANNEL:-stable}"
WAYANG_EDITION="${WAYANG_EDITION:-installer}"
# Bake the release signing key(s) so the installed system can verify updates.
WAYANG_TRUSTED_KEYS="${WAYANG_TRUSTED_KEYS:-$PWD/wayang/trusted_keys}"
[ -f "$WAYANG_TRUSTED_KEYS" ] || WAYANG_TRUSTED_KEYS=""
export BUILD_DIR WAYANG_VERSION KERNEL_VERSION KERNEL_CONFIG CHANNEL WAYANG_EDITION WAYANG_TRUSTED_KEYS

ISO="$BUILD_DIR/wayangos-$WAYANG_VERSION-linux-$KERNEL_VERSION-installer-x86_64.iso"

echo "=== ci-build: v$WAYANG_VERSION / linux $KERNEL_VERSION / $KERNEL_CONFIG ==="
./scripts/fetch-sources.sh
./scripts/build-wayang.sh
./scripts/build-installer.sh
./scripts/build-kernel.sh "$KERNEL_CONFIG" bzImage-installer
if [ -d "${FIRMWARE_DIR:-/lib/firmware}" ]; then
    ./scripts/stage-firmware.sh || true
fi
./scripts/build-wifi-tools.sh || true
./scripts/fetch-dcheck.sh || true
./scripts/build-nft.sh
./scripts/build-rootfs.sh
./scripts/build-installer-iso.sh "$BUILD_DIR/bzImage-installer" "$BUILD_DIR/wayangos-initramfs.img" "$ISO"
cp "$ISO" "$BUILD_DIR/wayangos-installer.iso"

if [ -n "${WAYANG_KEY:-}" ] && [ -f "$WAYANG_KEY" ]; then
    ./scripts/build-bundle.sh "$BUILD_DIR/bzImage-installer" "$BUILD_DIR/wayangos-initramfs.img" \
        "$WAYANG_VERSION" x86_64 "$WAYANG_EDITION" \
        --out "$BUILD_DIR" --channel "$CHANNEL" --key "$WAYANG_KEY" --keyid "${WAYANG_KEYID:-release}"
    WUP="$BUILD_DIR/wayang-$WAYANG_VERSION-x86_64.wup"
    DEST="$BUILD_DIR/channel/$CHANNEL/x86_64"
    mkdir -p "$DEST"
    cp "$WUP" "$DEST/"
    tar -xzf "$WUP" -C "$DEST" manifest.json
fi

(
    cd "$BUILD_DIR" || exit 0
    {
        sha256sum wayangos-installer.iso 2>/dev/null
        sha256sum "$(basename "$ISO")" 2>/dev/null
        [ -f "wayang-$WAYANG_VERSION-x86_64.wup" ] && sha256sum "wayang-$WAYANG_VERSION-x86_64.wup"
        [ -f "channel/$CHANNEL/x86_64/manifest.json" ] && sha256sum "channel/$CHANNEL/x86_64/manifest.json"
    } > SHA256SUMS 2>/dev/null || true
)

echo ""
echo "=== done ==="
ls -lh "$BUILD_DIR"/*.iso "$BUILD_DIR"/*.wup 2>/dev/null || true
