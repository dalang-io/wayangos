#!/bin/sh
# dcheck installer (Linux and macOS, x86_64 / aarch64).
#
#   curl -fsSL https://wayang.dalang.io/dcheck/install.sh | sh
#   curl -fsSL https://wayang.dalang.io/dcheck/install.sh | sh -s -- --with-smartmontools
#
# Options:
#   --with-smartmontools  also install smartmontools with the system package
#                         manager (optional second SMART source; dcheck reads
#                         SATA/SAS/NVMe natively without it)
#   --dry-run             print what would be done, change nothing
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

WITH_SMARTMONTOOLS="${DCHECK_WITH_SMARTMONTOOLS:-}"
DRY_RUN=""
for arg in "$@"; do
    case "$arg" in
        --with-smartmontools) WITH_SMARTMONTOOLS=1 ;;
        --dry-run) DRY_RUN=1 ;;
        -h | --help)
            echo "usage: curl -fsSL https://wayang.dalang.io/dcheck/install.sh | sh -s -- [--with-smartmontools] [--dry-run]"
            echo "env:   DCHECK_VERSION, DCHECK_INSTALL_DIR, DCHECK_BASE_URL, DCHECK_WITH_SMARTMONTOOLS=1"
            exit 0 ;;
        *) printf 'unknown option: %s\n' "$arg" >&2; exit 2 ;;
    esac
done

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

OS="$(uname -s)"
case "$OS" in
    Linux) OS_SUFFIX=unknown-linux-musl ;;
    Darwin) OS_SUFFIX=apple-darwin ;;
    *) die "no prebuilt binary for $OS (Linux and macOS only); build from source (cargo build --release)" ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) TARGET="x86_64-$OS_SUFFIX" ;;
    aarch64 | arm64) TARGET="aarch64-$OS_SUFFIX" ;;
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

if [ -n "$DRY_RUN" ]; then
    say "dry run: would install $PKG ($TARGET) to ${DCHECK_INSTALL_DIR:-/usr/local/bin}"
fi

EXPECTED="$(awk -v f="$PKG" '{ n=$2; sub(/^\*/, "", n); if (n == f) print $1 }' "$TMP/SHA256SUMS")"
[ -n "$EXPECTED" ] || die "$PKG is not listed in SHA256SUMS"
ACTUAL="$(sha256 "$TMP/$PKG")"
[ "$EXPECTED" = "$ACTUAL" ] || die "checksum mismatch for $PKG"
say "checksum ok"

tar xzf "$TMP/$PKG" -C "$TMP" dcheck
chmod 0755 "$TMP/dcheck"
"$TMP/dcheck" --version >/dev/null 2>&1 || die "downloaded binary does not run on this system"

# Optional: smartmontools through the system package manager.
install_smartmontools() {
    if command -v smartctl >/dev/null 2>&1; then
        say "smartmontools already installed ($(command -v smartctl))"
        return 0
    fi
    if command -v apt-get >/dev/null 2>&1; then cmd="apt-get install -y smartmontools"
    elif command -v dnf >/dev/null 2>&1; then cmd="dnf install -y smartmontools"
    elif command -v yum >/dev/null 2>&1; then cmd="yum install -y smartmontools"
    elif command -v zypper >/dev/null 2>&1; then cmd="zypper --non-interactive install smartmontools"
    elif command -v apk >/dev/null 2>&1; then cmd="apk add smartmontools"
    elif command -v pacman >/dev/null 2>&1; then cmd="pacman -S --noconfirm smartmontools"
    elif command -v brew >/dev/null 2>&1; then cmd="brew install smartmontools"
    else
        say "no supported package manager found; install smartmontools manually"
        return 0
    fi
    pre=""
    case "$cmd" in
        brew*) ;;
        *) if [ "$(id -u)" -ne 0 ]; then
               command -v sudo >/dev/null 2>&1 || { say "need root or sudo to run: $cmd"; return 0; }
               pre="sudo "
           fi ;;
    esac
    if [ -n "$DRY_RUN" ]; then
        say "dry run: would run: $pre$cmd"
        return 0
    fi
    say "installing smartmontools: $pre$cmd"
    # shellcheck disable=SC2086
    $pre$cmd || say "smartmontools install failed (dcheck works without it)"
}

if [ -n "$DRY_RUN" ]; then
    [ -n "$WITH_SMARTMONTOOLS" ] && install_smartmontools
    say "dry run: nothing changed"
    exit 0
fi

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
if [ -n "$WITH_SMARTMONTOOLS" ]; then
    install_smartmontools
elif [ "$OS" = "Darwin" ] && ! command -v smartctl >/dev/null 2>&1; then
    say "macOS: disks show SMART status via diskutil; for full attributes: brew install smartmontools"
fi
cat <<EOF

  sudo dcheck            # terminal UI (SMART needs root)
  dcheck storage         # list disks
  dcheck update          # upgrade to the latest release

EOF
