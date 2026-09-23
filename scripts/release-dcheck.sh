#!/bin/bash
# Build dcheck release artifacts for the supported targets and package them.
#
# Usage: ./scripts/release-dcheck.sh
# Env:
#   TARGETS   space-separated rust targets (default: linux amd64 + arm64)
#   OUT_DIR   output dir (default: <repo>/dist)
#   FREEBSD   1 to also try x86_64-unknown-freebsd (needs a cross linker)
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DCHECK_DIR="$REPO_DIR/dcheck"
OUT_DIR="${OUT_DIR:-$REPO_DIR/dist}"
TARGETS="${TARGETS:-x86_64-unknown-linux-musl aarch64-unknown-linux-musl}"

VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$DCHECK_DIR/Cargo.toml" | head -1)"
[ -n "$VERSION" ] || { echo "ERROR: could not read version from Cargo.toml" >&2; exit 1; }

if [ -n "${FREEBSD:-}" ]; then
    TARGETS="$TARGETS x86_64-unknown-freebsd"
fi

mkdir -p "$OUT_DIR"
echo "=== dcheck $VERSION release ==="

for target in $TARGETS; do
    echo "--- $target ---"
    if ! TARGET="$target" "$REPO_DIR/scripts/build-dcheck.sh" >/dev/null; then
        echo "  skipped (build failed — needs a linker for $target)"
        continue
    fi

    bin="$OUT_DIR/dcheck-$target"
    pkg="dcheck-$VERSION-$target.tar.gz"
    tmp="$(mktemp -d)"
    cp "$bin" "$tmp/dcheck"
    cp "$DCHECK_DIR/README.md" "$tmp/README.md"
    (cd "$tmp" && tar czf "$OUT_DIR/$pkg" dcheck README.md)
    rm -rf "$tmp"
    echo "  $pkg ($(du -h "$OUT_DIR/$pkg" | cut -f1))"
done

echo ""
echo "=== artifacts ==="
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$OUT_DIR" && sha256sum dcheck-*.tar.gz > SHA256SUMS && cat SHA256SUMS)
elif command -v shasum >/dev/null 2>&1; then
    (cd "$OUT_DIR" && shasum -a 256 dcheck-*.tar.gz > SHA256SUMS && cat SHA256SUMS)
fi

# Optional GPG signing of the checksums (set GPG_KEY to the signing key id).
if [ -n "${GPG_KEY:-}" ] && command -v gpg >/dev/null 2>&1; then
    gpg --batch --yes --local-user "$GPG_KEY" --armor --detach-sign "$OUT_DIR/SHA256SUMS"
    echo "  signed SHA256SUMS with key $GPG_KEY"
fi

ls -1 "$OUT_DIR"/dcheck-*.tar.gz 2>/dev/null || true
