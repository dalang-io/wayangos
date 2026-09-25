#!/bin/bash
# Build the WayangOS release matrix and bundle every edition into dist/.
#
# Usage:
#   WAYANG_VERSION=1.4.1 [WAYANG_KEY=key.sec] ./scripts/release-wayang.sh
#
# Env:
#   WAYANG_VERSION  required, semver (e.g. 1.4.1)
#   WAYANG_KEY      ed25519 signing key (if unset bundles are unsigned)
#   WAYANG_KEYID    keyid embedded in the manifest (default: release)
#   WAYANG_CHANNEL  channel in the manifest (default: stable)
#   WAYANG_NOTES    release notes text
#   BUILD_DIR       build root (default: ~/wayangos-build)
#   OUT_DIR         output dir (default: <repo>/dist)
#   ARCH            x86_64 | arm64 (default: both)
#   TARGETS         space-separated subset, e.g. "qemu intel" or "rpi3"
#
# Best effort: a missing source tree or toolchain skips that target with a
# message instead of aborting the release. Nothing is uploaded; the script
# prints the `gh release create` command to run.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

VERSION="${WAYANG_VERSION:-}"
[ -n "$VERSION" ] || { echo "ERROR: WAYANG_VERSION is required (e.g. WAYANG_VERSION=1.4.1 $0)" >&2; exit 1; }
if [[ ! "$VERSION" =~ ^[0-9]+(\.[0-9]+)*$ ]]; then
    echo "ERROR: WAYANG_VERSION must be numeric semver, e.g. 1.4.1 (got: $VERSION)" >&2
    exit 1
fi

KEY="${WAYANG_KEY:-}"
KEYID="${WAYANG_KEYID:-release}"
CHANNEL="${WAYANG_CHANNEL:-stable}"
NOTES="${WAYANG_NOTES:-WayangOS $VERSION}"
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
OUT="${OUT_DIR:-$REPO_DIR/dist}"
ARCH_FILTER="${ARCH:-}"
TARGETS="${TARGETS:-}"

X86_EDITIONS=(qemu intel amd nvidia)
ARM_TARGETS=(rpi3 orangepi)

arm_config() {
    case "$1" in
        rpi3)     printf 'defconfig-arm64-rpi3' ;;
        orangepi) printf 'defconfig-arm64-orangepi-zero2w' ;;
        *)        return 1 ;;
    esac
}

die()  { echo "ERROR: $*" >&2; exit 1; }
warn() { echo "WARNING: $*" >&2; }

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    else
        shasum -a 256 "$1"
    fi | cut -d' ' -f1
}

WORK="$BUILD/release-$VERSION"
mkdir -p "$WORK" "$OUT"

produced=()

selected_x86=()
selected_arm=()
if [ -n "$TARGETS" ]; then
    for t in $TARGETS; do
        case "$t" in
            qemu|intel|amd|nvidia) selected_x86+=("$t") ;;
            rpi3|orangepi)         selected_arm+=("$t") ;;
            *)                     warn "unknown target ignored: $t" ;;
        esac
    done
else
    case "${ARCH_FILTER:-all}" in
        all|"") selected_x86=("${X86_EDITIONS[@]}"); selected_arm=("${ARM_TARGETS[@]}") ;;
        x86_64) selected_x86=("${X86_EDITIONS[@]}") ;;
        arm64)  selected_arm=("${ARM_TARGETS[@]}") ;;
        *)      die "ARCH must be x86_64 or arm64 (got: $ARCH_FILTER)" ;;
    esac
fi

build_rootfs() {
    local arch="$1"
    if ! ARCH="$arch" BUILD_DIR="$BUILD" "$REPO_DIR/scripts/build-rootfs.sh"; then
        warn "rootfs build failed for $arch — skipping its bundles"
        return 1
    fi
    cp "$BUILD/wayangos-initramfs.img" "$WORK/initramfs-$arch.img"
    return 0
}

bundle() {
    local kernel="$1" initramfs="$2" arch="$3" edition="$4"
    local outdir="$OUT/$edition" outfile
    outfile="$outdir/wayang-$VERSION-$arch.wup"
    local args=("$kernel" "$initramfs" "$VERSION" "$arch" "$edition"
        --out "$outdir" --channel "$CHANNEL" --notes "$NOTES")
    if [ -n "$KEY" ]; then
        args+=(--key "$KEY" --keyid "$KEYID")
    fi
    if "$REPO_DIR/scripts/build-bundle.sh" "${args[@]}"; then
        produced+=("$outfile")
    else
        warn "bundle for $edition failed — skipping"
    fi
}

# ---------------------------------------------------------------- x86_64
if [ "${#selected_x86[@]}" -gt 0 ]; then
    echo "=== x86_64 ==="
    if build_rootfs x86_64; then
        initramfs="$WORK/initramfs-x86_64.img"
        for e in "${selected_x86[@]}"; do
            echo "--- x86_64/$e ---"
            if ! ARCH=x86_64 BUILD_DIR="$BUILD" \
                "$REPO_DIR/scripts/build-kernel.sh" "defconfig-$e" "bzImage-$e"; then
                warn "kernel build failed for $e — skipping"
                continue
            fi
            [ -f "$BUILD/bzImage-$e" ] || { warn "missing $BUILD/bzImage-$e — skipping"; continue; }
            bundle "$BUILD/bzImage-$e" "$initramfs" x86_64 "$e"
        done
    fi
fi

# ----------------------------------------------------------------- arm64
if [ "${#selected_arm[@]}" -gt 0 ]; then
    echo "=== arm64 ==="
    if ! command -v aarch64-linux-gnu-gcc >/dev/null 2>&1; then
        warn "aarch64-linux-gnu-gcc not found — skipping ARM64 targets"
    elif build_rootfs arm64; then
        initramfs="$WORK/initramfs-arm64.img"
        for t in "${selected_arm[@]}"; do
            config="$(arm_config "$t")" || { warn "no config for $t"; continue; }
            echo "--- arm64/$t ---"
            if ! ARCH=arm64 BUILD_DIR="$BUILD" \
                "$REPO_DIR/scripts/build-kernel.sh" "$config" "Image-$t"; then
                warn "kernel build failed for $t — skipping"
                continue
            fi
            [ -f "$BUILD/Image-$t" ] || { warn "missing $BUILD/Image-$t — skipping"; continue; }
            bundle "$BUILD/Image-$t" "$initramfs" arm64 "$t"
        done
    fi
fi

# --------------------------------------------------------------- sums
if [ "${#produced[@]}" -eq 0 ]; then
    warn "no bundles were produced (missing sources/toolchain?) — nothing to release"
    exit 1
fi

SUMS="$OUT/SHA256SUMS"
: > "$SUMS"
for f in "${produced[@]}"; do
    rel="${f#"$OUT"/}"
    printf '%s  %s\n' "$(sha256_of "$f")" "$rel" >> "$SUMS"
done

echo ""
echo "=== Release $VERSION ready ==="
echo "  Bundles: $OUT"
for f in "${produced[@]}"; do
    echo "    ${f#"$OUT"/}"
done
echo "  Checksums: ${SUMS#"$REPO_DIR"/}"
echo ""
echo "Nothing was uploaded. Review, then run from $REPO_DIR:"
files=()
for f in "${produced[@]}"; do
    files+=("${f#"$REPO_DIR"/}")
done
echo "  gh release create \"v$VERSION\" --title \"WayangOS v$VERSION\" --notes \"$NOTES\" \\"
echo "      ${files[*]} ${SUMS#"$REPO_DIR"/}"
