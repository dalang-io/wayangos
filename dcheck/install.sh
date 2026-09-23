#!/bin/sh
# dcheck installer.
#
#   curl -fsSL https://wayang.dalang.io/dcheck/install.sh | sh
#
# Env:
#   DCHECK_VERSION      version to install (default: <base>/LATEST)
#   DCHECK_INSTALL_DIR  target directory (default: /usr/local/bin, falls back
#                       to ~/.local/bin when it is not writable and sudo is absent)
#   DCHECK_BASE_URL     release base URL (default: https://wayang.dalang.io/dcheck)
#
# Release layout: <base>/LATEST, <base>/v<ver>/dcheck-<ver>-<target>.tar.gz,
# <base>/v<ver>/SHA256SUMS. Upgrade later with `dcheck update`.
set -eu

BASE="${DCHECK_BASE_URL:-https://wayang.dalang.io/dcheck}"
BASE="${BASE%/}"

say() { printf '\033[36m::\033[0m %s\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

fetch() { # fetch URL [OUTFILE]
    if command -v curl >/dev/null 2>&1; then
        if [ $# -ge 2 ]; then curl -fsSL --retry 2 -o "$2" "$1"; else curl -fsSL --retry 2 "$1"; fi
    elif command -v wget >/dev/null 2>&1; then
        if [ $# -ge 2 ]; then wget -q -O "$2" "$1"; else wget -q -O - "$1"; fi
    else
        die "need curl or wget"
    fi
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
    else die "need sha256sum or shasum to verify the download"
    fi
}

[ "$(uname -s)" = "Linux" ] || die "prebuilt binaries are Linux-only; on $(uname -s) build from source (cargo build --release)"
case "$(uname -m)" in
    x86_64 | amd64) TARGET=x86_64-unknown-linux-musl ;;
    aarch64 | arm64) TARGET=aarch64-unknown-linux-musl ;;
    *) die "unsupported architecture $(uname -m) (x86_64 and aarch64 only)" ;;
esac

VERSION="${DCHECK_VERSION:-}"
if [ -z "$VERSION" ]; then
    VERSION="$(fetch "$BASE/LATEST" | tr -d ' \r\n')" || die "could not read $BASE/LATEST"
fi
VERSION="${VERSION#v}"
case "$VERSION" in
    [0-9]*.[0-9]*) ;;
    *) die "unexpected version '$VERSION' from $BASE/LATEST" ;;
esac

PKG="dcheck-$VERSION-$TARGET.tar.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT INT TERM

say "downloading dcheck $VERSION ($TARGET)"
fetch "$BASE/v$VERSION/$PKG" "$TMP/$PKG" || die "download failed: $BASE/v$VERSION/$PKG"
fetch "$BASE/v$VERSION/SHA256SUMS" "$TMP/SHA256SUMS" || die "download failed: SHA256SUMS"

EXPECTED="$(awk -v f="$PKG" '{ n=$2; sub(/^\*/, "", n); if (n == f) print $1 }' "$TMP/SHA256SUMS")"
[ -n "$EXPECTED" ] || die "$PKG is not listed in SHA256SUMS"
ACTUAL="$(sha256 "$TMP/$PKG")"
[ "$EXPECTED" = "$ACTUAL" ] || die "checksum mismatch for $PKG"
say "checksum ok"

tar xzf "$TMP/$PKG" -C "$TMP" dcheck
chmod 0755 "$TMP/dcheck"
"$TMP/dcheck" --version >/dev/null 2>&1 || die "downloaded binary does not run on this system"

DIR="${DCHECK_INSTALL_DIR:-/usr/local/bin}"
SUDO=""
if [ ! -d "$DIR" ] || [ ! -w "$DIR" ]; then
    if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then
        SUDO="sudo"
        say "installing to $DIR (sudo)"
    elif [ -z "${DCHECK_INSTALL_DIR:-}" ]; then
        DIR="$HOME/.local/bin"
    fi
fi
$SUDO mkdir -p "$DIR"
$SUDO install -m 0755 "$TMP/dcheck" "$DIR/dcheck" 2>/dev/null \
    || { $SUDO cp "$TMP/dcheck" "$DIR/dcheck" && $SUDO chmod 0755 "$DIR/dcheck"; }

say "installed $("$DIR/dcheck" --version) to $DIR/dcheck"
case ":$PATH:" in
    *":$DIR:"*) ;;
    *) say "note: $DIR is not on your PATH" ;;
esac
cat <<EOF

  sudo dcheck            # terminal UI (SMART needs root)
  dcheck storage         # list disks
  dcheck update          # upgrade to the latest release

EOF
