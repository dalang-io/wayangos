#!/bin/bash
# Build the WayangOS release matrix, bundle every edition into dist/, and emit
# the online update channel tree per docs/UPDATE-DESIGN.md
# ("Channel publishing layout").
#
# Usage:
#   WAYANG_VERSION=1.4.1 [WAYANG_KEY=key.sec] ./scripts/release-wayang.sh
#   ./scripts/release-wayang.sh --version 1.4.1 --key key.sec --channel stable
#
# Outputs:
#   dist/<edition>/wayang-<version>-<arch>.wup        (one per built edition)
#   dist/channel/<channel>/<arch>/manifest.json       (channel manifest)
#   dist/channel/<channel>/<arch>/wayang-<version>-<arch>.wup
#   dist/SHA256SUMS                                   (all of the above)
#
# Env / args:
#   WAYANG_VERSION / --version   required, numeric semver (e.g. 1.4.1)
#   WAYANG_KEY     / --key       ed25519 signing key (unsigned when unset)
#   WAYANG_KEYID   / --keyid     keyid embedded in the manifest (default: release)
#   CHANNEL        / --channel   channel for the tree/manifest (default: stable)
#   WAYANG_CHANNEL               legacy alias for CHANNEL
#   WAYANG_NOTES   / --notes     release notes text
#   CHANNEL_EDITION / --channel-edition
#                                x86_64 edition placed in the channel tree
#                                (default: generic; "generic" uses the qemu
#                                kernel). intel|amd|nvidia|qemu also accepted.
#   CHANNEL_EDITION_ARM          arm64 target placed in the tree (default: rpi3)
#   TARGETS        / --targets   space-separated subset, e.g. "qemu intel"|"rpi3"
#   BUILD_DIR      / --build-dir build root (default: ~/wayangos-build)
#   OUT_DIR        / --out       output dir (default: <repo>/dist)
#   ARCH                         x86_64 | arm64 (default: both)
#   WAYANG_PREBUILT              auto (default) | 1 | 0: reuse existing artifacts
#                                in BUILD_DIR instead of rebuilding. In `auto`,
#                                a prebuilt kernel is reused and a prebuilt
#                                wayangos-initramfs.img is used for the first
#                                arch only (per-arch copies are cached).
#
# Best effort: a missing source tree or toolchain skips that target with a
# message instead of aborting the release. Nothing is uploaded; the script
# prints the `gh release create` / `gh release upload` commands to run.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

usage() {
    sed -n '2,45s/^# \{0,1\}//p' "$0"
    exit "${1:-0}"
}

VERSION="${WAYANG_VERSION:-}"
KEY="${WAYANG_KEY:-}"
KEYID="${WAYANG_KEYID:-release}"
CHANNEL="${CHANNEL:-${WAYANG_CHANNEL:-stable}}"
NOTES="${WAYANG_NOTES:-}"
BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
OUT="${OUT_DIR:-$REPO_DIR/dist}"
ARCH_FILTER="${ARCH:-}"
TARGETS="${TARGETS:-}"
CHANNEL_EDITION="${CHANNEL_EDITION:-generic}"
CHANNEL_EDITION_ARM="${CHANNEL_EDITION_ARM:-rpi3}"
PREBUILT="${WAYANG_PREBUILT:-auto}"

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage 0 ;;
        --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
        --version=*) VERSION="${1#*=}"; shift ;;
        --key) KEY="${2:?--key needs a file}"; shift 2 ;;
        --key=*) KEY="${1#*=}"; shift ;;
        --keyid) KEYID="${2:?--keyid needs a name}"; shift 2 ;;
        --keyid=*) KEYID="${1#*=}"; shift ;;
        --channel) CHANNEL="${2:?--channel needs a value}"; shift 2 ;;
        --channel=*) CHANNEL="${1#*=}"; shift ;;
        --channel-edition) CHANNEL_EDITION="${2:?--channel-edition needs a value}"; shift 2 ;;
        --channel-edition=*) CHANNEL_EDITION="${1#*=}"; shift ;;
        --notes) NOTES="${2:?--notes needs text}"; shift 2 ;;
        --notes=*) NOTES="${1#*=}"; shift ;;
        --targets) TARGETS="${2:?--targets needs a value}"; shift 2 ;;
        --targets=*) TARGETS="${1#*=}"; shift ;;
        --build-dir) BUILD="${2:?--build-dir needs a value}"; shift 2 ;;
        --build-dir=*) BUILD="${1#*=}"; shift ;;
        --out) OUT="${2:?--out needs a value}"; shift 2 ;;
        --out=*) OUT="${1#*=}"; shift ;;
        --arch) ARCH_FILTER="${2:?--arch needs a value}"; shift 2 ;;
        --arch=*) ARCH_FILTER="${1#*=}"; shift ;;
        --*) echo "ERROR: unknown option: $1" >&2; usage 1 ;;
        *)   echo "ERROR: unexpected argument: $1" >&2; usage 1 ;;
    esac
done

NOTES="${NOTES:-WayangOS $VERSION}"

[ -n "$VERSION" ] || { echo "ERROR: WAYANG_VERSION is required (e.g. WAYANG_VERSION=1.4.1 $0)" >&2; exit 1; }
if [[ ! "$VERSION" =~ ^[0-9]+(\.[0-9]+)*$ ]]; then
    echo "ERROR: WAYANG_VERSION must be numeric semver, e.g. 1.4.1 (got: $VERSION)" >&2
    exit 1
fi

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
produced_arch=()
produced_edition=()
channel_files=()

# In `auto` mode a single prebuilt initramfs may be used for one arch; after
# that, other arches are built from source (avoid reusing x86_64 for arm64).
shared_initramfs_used=0

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

prebuilt_enabled() {
    case "$PREBUILT" in
        1|yes|true|auto) return 0 ;;
        *)               return 1 ;;
    esac
}

# Reuse an existing per-edition kernel in BUILD_DIR when prebuilt is enabled.
need_kernel_build() {
    [ -f "$BUILD/$1" ] && prebuilt_enabled && return 1
    return 0
}

build_rootfs() {
    local arch="$1"
    local cached="$WORK/initramfs-$arch.img"
    local shared="$BUILD/wayangos-initramfs.img"

    if [ -f "$cached" ]; then
        echo "  reusing cached initramfs: $cached"
        return 0
    fi
    if prebuilt_enabled && [ -f "$shared" ] && [ "$shared_initramfs_used" -eq 0 ]; then
        echo "  using prebuilt initramfs: $shared"
        cp "$shared" "$cached"
        shared_initramfs_used=1
        return 0
    fi

    if ! ARCH="$arch" BUILD_DIR="$BUILD" "$REPO_DIR/scripts/build-rootfs.sh"; then
        warn "rootfs build failed for $arch — skipping its bundles"
        return 1
    fi
    cp "$shared" "$cached"
    shared_initramfs_used=1
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
        produced_arch+=("$arch")
        produced_edition+=("$edition")
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
            if need_kernel_build "bzImage-$e"; then
                if ! ARCH=x86_64 BUILD_DIR="$BUILD" \
                    "$REPO_DIR/scripts/build-kernel.sh" "defconfig-$e" "bzImage-$e"; then
                    warn "kernel build failed for $e — skipping"
                    continue
                fi
            else
                echo "  using prebuilt kernel: $BUILD/bzImage-$e"
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
            if need_kernel_build "Image-$t"; then
                if ! ARCH=arm64 BUILD_DIR="$BUILD" \
                    "$REPO_DIR/scripts/build-kernel.sh" "$config" "Image-$t"; then
                    warn "kernel build failed for $t — skipping"
                    continue
                fi
            else
                echo "  using prebuilt kernel: $BUILD/Image-$t"
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

# ------------------------------------------------------- channel tree
# `emit_channel <arch> <edition>...` copies the first matching bundle into
# dist/channel/<channel>/<arch>/ and extracts its manifest.json.
emit_channel() {
    local arch="$1"; shift
    local ed i src="" picked=""

    for ed in "$@"; do
        for i in "${!produced[@]}"; do
            if [ "${produced_arch[$i]}" = "$arch" ] && [ "${produced_edition[$i]}" = "$ed" ]; then
                src="${produced[$i]}"
                picked="$ed"
                break 2
            fi
        done
    done

    if [ -z "$src" ]; then
        warn "no produced bundle for arch $arch (editions: $*) — channel entry skipped"
        return 0
    fi

    local dir wup manifest
    dir="$CHAN_ROOT/$arch"
    mkdir -p "$dir"
    wup="$dir/wayang-$VERSION-$arch.wup"
    manifest="$dir/manifest.json"

    cp "$src" "$wup"
    if ! tar -xzf "$src" -O manifest.json > "$manifest"; then
        warn "failed to extract manifest.json from $src — channel entry skipped"
        rm -f "$wup" "$manifest"
        return 0
    fi
    channel_files+=("$wup" "$manifest")
    echo "  channel: ${dir#"$OUT"/} (edition: $picked)"
}

CHAN_ROOT="$OUT/channel/$CHANNEL"
echo ""
echo "=== channel /$CHANNEL ==="
warn "channel layout is <base>/$CHANNEL/<arch>/… with an arch-only bundle name"
warn "  (wayang-<version>-<arch>.wup), so only ONE edition can live in a given"
warn "  <arch>. x86_64 uses '$CHANNEL_EDITION', arm64 uses '$CHANNEL_EDITION_ARM'."
echo "  TODO: per-edition channel directories (<channel>/<edition>/<arch>/…) are"
echo "        a future change; the frozen v0 layout is arch-only."

if [ "${#selected_x86[@]}" -gt 0 ]; then
    if [ "$CHANNEL_EDITION" = "generic" ]; then
        emit_channel x86_64 generic qemu
    else
        emit_channel x86_64 "$CHANNEL_EDITION"
    fi
fi
if [ "${#selected_arm[@]}" -gt 0 ] && [ -n "$CHANNEL_EDITION_ARM" ]; then
    emit_channel arm64 "$CHANNEL_EDITION_ARM"
fi

if [ "${#channel_files[@]}" -eq 0 ]; then
    warn "no channel entries were produced (no bundle matched the chosen editions)"
fi

# --------------------------------------------------------------- sums
SUMS="$OUT/SHA256SUMS"
: > "$SUMS"
sums_files=("${produced[@]}" "${channel_files[@]}")
for f in "${sums_files[@]}"; do
    rel="${f#"$OUT"/}"
    printf '%s  %s\n' "$(sha256_of "$f")" "$rel" >> "$SUMS"
done

echo ""
echo "=== Release $VERSION ready ==="
echo "  Bundles: $OUT"
for f in "${produced[@]}"; do
    echo "    ${f#"$OUT"/}"
done
echo "  Channel: ${CHAN_ROOT#"$OUT"/}/"
for f in "${channel_files[@]}"; do
    echo "    ${f#"$OUT"/}"
done
echo "  Checksums: ${SUMS#"$REPO_DIR"/}"
echo ""
echo "Nothing was uploaded. Review, then run from $REPO_DIR:"
files=()
for f in "${produced[@]}"; do
    files+=("${f#"$REPO_DIR"/}")
done
channel_rel=()
for f in "${channel_files[@]}"; do
    channel_rel+=("${f#"$REPO_DIR"/}")
done
channel_rel_str=""
if [ "${#channel_rel[@]}" -gt 0 ]; then
    channel_rel_str="${channel_rel[*]}"
fi

echo "  gh release create \"v$VERSION\" --title \"WayangOS v$VERSION\" --notes \"$NOTES\" \\"
echo "      ${files[*]} $channel_rel_str ${SUMS#"$REPO_DIR"/}"
echo ""
echo "  # if the release already exists, upload the channel tree instead:"
echo "  gh release upload \"v$VERSION\" \\"
echo "      $channel_rel_str ${SUMS#"$REPO_DIR"/} --clobber"
