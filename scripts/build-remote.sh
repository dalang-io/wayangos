#!/bin/bash
# Fast WayangOS build on a remote Linux builder (many cores/RAM), then fetch
# the artifacts back to dist/remote/. The builder just needs apt + internet.
#
# Usage: ./scripts/build-remote.sh
# Env:
#   HOST            ssh target (default: root@10.0.0.251)
#   REMOTE_SRC      repo copy on the builder (default: /root/wayangos-src)
#   BUILD_DIR       build dir on the builder (default: /root/wayangos-build)
#   WAYANG_VERSION  (default: 1.0.11)
#   KERNEL_VERSION  (default: 7.2.7)
#   KERNEL_CONFIG   (default: defconfig-intel)
#   WAYANG_KEY      local signing key file (optional)
#   SKIP_DEPS=1     do not install build dependencies on the builder
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${HOST:-root@10.0.0.251}"
REMOTE_SRC="${REMOTE_SRC:-/root/wayangos-src}"
BUILD_DIR="${BUILD_DIR:-/root/wayangos-build}"
WAYANG_VERSION="${WAYANG_VERSION:-1.0.11}"
KERNEL_VERSION="${KERNEL_VERSION:-7.2.7}"
KERNEL_CONFIG="${KERNEL_CONFIG:-defconfig-intel}"
WAYANG_KEY="${WAYANG_KEY:-}"

echo "=== syncing repo -> $HOST:$REMOTE_SRC ==="
# shellcheck disable=SC2029
ssh "$HOST" "mkdir -p '$REMOTE_SRC' '$BUILD_DIR'"
rsync -az --delete \
    --exclude '.git' --exclude 'dist' --exclude 'target' --exclude 'node_modules' \
    --exclude 'landing-page' \
    "$REPO_DIR/" "$HOST:$REMOTE_SRC/"

if [ "${SKIP_DEPS:-}" != 1 ]; then
    echo "=== ensuring build deps on $HOST (apt + rustup) ==="
    # shellcheck disable=SC2087
    ssh "$HOST" 'bash -s' <<'DEPS'
set -e
export DEBIAN_FRONTEND=noninteractive
need=""
for t in gcc make flex bison bc cpio xz unzip grub-mkrescue grub-mkstandalone xorriso mtools curl wget git; do
    command -v "$t" >/dev/null 2>&1 || need="$need $t"
done
if [ -n "$need" ]; then
    apt-get update -qq
    apt-get install -y -qq build-essential flex bison bc libelf-dev libssl-dev \
        libnl-3-dev libnl-genl-3-dev libnl-route-3-dev \
        cpio xz-utils unzip grub-pc-bin grub-efi-amd64-bin xorriso mtools curl wget git >/dev/null
fi
if ! command -v cargo >/dev/null 2>&1; then
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
fi
"$HOME/.cargo/bin/rustup" target add x86_64-unknown-linux-musl >/dev/null 2>&1 || true
DEPS
fi

if [ -n "$WAYANG_KEY" ]; then
    scp -q "$WAYANG_KEY" "$HOST:$BUILD_DIR/release.key"
fi

echo "=== building on $HOST ==="
# shellcheck disable=SC2029
ssh "$HOST" "bash -lc '
    set -e
    export PATH=\$HOME/.cargo/bin:\$PATH
    cd \"$REMOTE_SRC\"
    BUILD_DIR=\"$BUILD_DIR\" WAYANG_VERSION=\"$WAYANG_VERSION\" KERNEL_VERSION=\"$KERNEL_VERSION\" \
    KERNEL_CONFIG=\"$KERNEL_CONFIG\" CHANNEL=stable WAYANG_EDITION=installer \
    WAYANG_KEY=\"$BUILD_DIR/release.key\" WAYANG_KEYID=release \
    ./scripts/ci-build.sh
'"

echo "=== fetching artifacts -> dist/remote/ ==="
mkdir -p "$REPO_DIR/dist/remote"
rsync -az \
    "$HOST:$BUILD_DIR/wayangos-installer.iso" \
    "$HOST:$BUILD_DIR/wayangos-${WAYANG_VERSION}-linux-${KERNEL_VERSION}-installer-x86_64.iso" \
    "$REPO_DIR/dist/remote/"
scp -q "$HOST:$BUILD_DIR/wayang-${WAYANG_VERSION}-x86_64.wup" "$REPO_DIR/dist/remote/" 2>/dev/null || true
scp -q "$HOST:$BUILD_DIR/SHA256SUMS" "$REPO_DIR/dist/remote/" 2>/dev/null || true
rsync -az "$HOST:$BUILD_DIR/channel/" "$REPO_DIR/dist/remote/channel/" 2>/dev/null || true
ls -lh "$REPO_DIR/dist/remote/" 2>/dev/null || true
echo "done."
