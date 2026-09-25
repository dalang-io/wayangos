#!/bin/bash
# Build a WayangOS update bundle (*.wup) per docs/UPDATE-DESIGN.md.
#
# Usage:
#   ./scripts/build-bundle.sh <kernel> <initramfs> <version> <arch> <edition> \
#       [--out DIR] [--channel stable] [--key FILE] [--keyid NAME] [--notes "text"]
#
# The bundle is a tar.gz with these members at the archive root:
#   manifest.json, vmlinuz, initramfs.img, manifest.json.sig (only when signed)
#
# Env:
#   WAYANG_BIN      path to the updater CLI (default: dist/wayang-<host triple>)
#   WAYANG_MIN_FROM minimum source version in the manifest (default: 1.0.0)
#   WAYANG_KEYID    default keyid when --keyid is omitted (default: release)
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

usage() {
    sed -n '2,14s/^# \{0,1\}//p' "$0"
    exit "${1:-0}"
}

PRODUCT="wayangos"
CHANNEL="stable"
NOTES=""
KEY=""
KEYID="${WAYANG_KEYID:-release}"
MIN_FROM="${WAYANG_MIN_FROM:-1.0.0}"
OUT_DIR="$REPO_DIR/dist"

positional=()
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage 0 ;;
        --out)
            [ $# -ge 2 ] || { echo "ERROR: --out needs a directory" >&2; exit 1; }
            OUT_DIR="$2"; shift 2 ;;
        --channel)
            [ $# -ge 2 ] || { echo "ERROR: --channel needs a value" >&2; exit 1; }
            CHANNEL="$2"; shift 2 ;;
        --key)
            [ $# -ge 2 ] || { echo "ERROR: --key needs a file" >&2; exit 1; }
            KEY="$2"; shift 2 ;;
        --keyid)
            [ $# -ge 2 ] || { echo "ERROR: --keyid needs a name" >&2; exit 1; }
            KEYID="$2"; shift 2 ;;
        --notes)
            [ $# -ge 2 ] || { echo "ERROR: --notes needs text" >&2; exit 1; }
            NOTES="$2"; shift 2 ;;
        --*) echo "ERROR: unknown option: $1" >&2; usage 1 ;;
        *) positional+=("$1"); shift ;;
    esac
done

if [ "${#positional[@]}" -ne 5 ]; then
    echo "ERROR: expected 5 positional arguments, got ${#positional[@]}" >&2
    usage 1
fi

KERNEL="${positional[0]}"
INITRAMFS="${positional[1]}"
VERSION="${positional[2]}"
ARCH="${positional[3]}"
EDITION="${positional[4]}"

case "$ARCH" in
    x86_64|arm64) ;;
    *) echo "ERROR: arch must be x86_64 or arm64 (got: $ARCH)" >&2; exit 1 ;;
esac

if [[ ! "$VERSION" =~ ^[0-9]+(\.[0-9]+)*$ ]]; then
    echo "ERROR: version must be numeric semver, e.g. 1.4.1 (got: $VERSION)" >&2
    exit 1
fi
MAJOR="${VERSION%%.*}"

[ -n "$EDITION" ] || { echo "ERROR: edition must be non-empty" >&2; exit 1; }
[ -f "$KERNEL" ] || { echo "ERROR: kernel not found: $KERNEL" >&2; exit 1; }
[ -f "$INITRAMFS" ] || { echo "ERROR: initramfs not found: $INITRAMFS" >&2; exit 1; }
[ -r "$KERNEL" ] || { echo "ERROR: kernel not readable: $KERNEL" >&2; exit 1; }
[ -r "$INITRAMFS" ] || { echo "ERROR: initramfs not readable: $INITRAMFS" >&2; exit 1; }

if [ -n "$KEY" ] && [ ! -f "$KEY" ]; then
    echo "ERROR: signing key not found: $KEY" >&2
    exit 1
fi

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    else
        shasum -a 256 "$1"
    fi | cut -d' ' -f1
}

KERNEL_SHA="$(sha256_of "$KERNEL")"
INITRAMFS_SHA="$(sha256_of "$INITRAMFS")"
TIME="$(date +%s)"

json_escape() {
    local s="$1"
    s="${s//\\/\\\\}"
    s="${s//\"/\\\"}"
    s="${s//$'\n'/\\n}"
    s="${s//$'\r'/\\r}"
    s="${s//$'\t'/\\t}"
    printf '%s' "$s"
}

find_wayang() {
    local arch triple cand
    arch="$(uname -m)"
    case "$arch" in
        arm64|aarch64) triple="aarch64" ;;
        x86_64|amd64)  triple="x86_64" ;;
        *)             triple="$arch" ;;
    esac
    for cand in \
        "${WAYANG_BIN:-}" \
        "$REPO_DIR/dist/wayang-${triple}-apple-darwin" \
        "$REPO_DIR/dist/wayang-${triple}-unknown-linux-musl" \
        "$REPO_DIR/dist/wayang-${triple}-unknown-linux-gnu" \
        "$REPO_DIR/dist/wayang-x86_64-unknown-linux-musl" \
        "$REPO_DIR/dist/wayang"; do
        if [ -n "$cand" ] && [ -x "$cand" ]; then
            printf '%s' "$cand"
            return 0
        fi
    done
    return 1
}

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/wayang-bundle.XXXXXX")"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

cp "$KERNEL" "$STAGE/vmlinuz"
cp "$INITRAMFS" "$STAGE/initramfs.img"

SIGNED=0
EFFECTIVE_KEYID="$KEYID"
[ -n "$KEY" ] || EFFECTIVE_KEYID=""

{
    printf '{\n'
    printf '  "product": "%s",\n' "$(json_escape "$PRODUCT")"
    printf '  "channel": "%s",\n' "$(json_escape "$CHANNEL")"
    printf '  "version": "%s",\n' "$(json_escape "$VERSION")"
    printf '  "major": %s,\n' "$MAJOR"
    printf '  "arch": "%s",\n' "$(json_escape "$ARCH")"
    printf '  "edition": "%s",\n' "$(json_escape "$EDITION")"
    printf '  "kernel_sha256": "%s",\n' "$KERNEL_SHA"
    printf '  "initramfs_sha256": "%s",\n' "$INITRAMFS_SHA"
    printf '  "min_from": "%s",\n' "$(json_escape "$MIN_FROM")"
    printf '  "notes": "%s",\n' "$(json_escape "$NOTES")"
    printf '  "time": %s,\n' "$TIME"
    printf '  "keyid": "%s"\n' "$(json_escape "$EFFECTIVE_KEYID")"
    printf '}\n'
} > "$STAGE/manifest.json"

if [ -n "$KEY" ]; then
    WAYANG="$(find_wayang)" || {
        echo "ERROR: --key given but updater CLI not found." >&2
        echo "       Set WAYANG_BIN or build it with scripts/build-wayang.sh." >&2
        exit 1
    }
    echo "Signing manifest.json with $WAYANG..."
    ( cd "$STAGE" && "$WAYANG" sign --key "$KEY" --keyid "$KEYID" manifest.json ) \
        || { echo "ERROR: signing failed" >&2; exit 1; }
    [ -f "$STAGE/manifest.json.sig" ] || {
        echo "ERROR: signing produced no manifest.json.sig next to manifest.json" >&2
        exit 1
    }
    SIGNED=1
else
    echo "WARNING: no --key given; building an UNSIGNED bundle (updaters may reject it)." >&2
fi

mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"
OUT_FILE="$OUT_DIR/wayang-${VERSION}-${ARCH}.wup"

members=(manifest.json vmlinuz initramfs.img)
if [ "$SIGNED" -eq 1 ]; then
    members+=(manifest.json.sig)
fi

tar -czf "$OUT_FILE" -C "$STAGE" "${members[@]}"

echo ""
echo "=== Bundle Built ==="
echo "  Bundle:    $OUT_FILE"
echo "  Version:   $VERSION (major $MAJOR)"
echo "  Arch:      $ARCH"
echo "  Edition:   $EDITION"
echo "  Channel:   $CHANNEL"
echo "  Signed:    $([ "$SIGNED" -eq 1 ] && echo "yes (keyid $EFFECTIVE_KEYID)" || echo "no")"
echo "  Kernel:    $KERNEL_SHA"
echo "  Initramfs: $INITRAMFS_SHA"
echo "  Size:      $(du -h "$OUT_FILE" | cut -f1)"
echo "  Members:   ${members[*]}"
