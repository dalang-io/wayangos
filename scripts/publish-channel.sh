#!/bin/bash
# Publish the WayangOS update channel tree to the website host.
#
# Serves, e.g.:
#   https://wayang.dalang.io/channel/stable/x86_64/manifest.json
#   https://wayang.dalang.io/channel/stable/x86_64/wayang-1.0.0-x86_64.wup
#
# Usage: ./scripts/publish-channel.sh [SOURCE_DIR]   (default: dist/channel)
# Env:
#   HOST            ssh target (default: root@10.0.0.251)
#   REMOTE_DIR      served directory (default: /root/wayang.dalang.io/public)
#   CHANNEL_SUBDIR  subdirectory under REMOTE_DIR (default: channel)
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${1:-$REPO_DIR/dist/channel}"
HOST="${HOST:-root@10.0.0.251}"
REMOTE_DIR="${REMOTE_DIR:-/root/wayang.dalang.io/public}"
DEST="$REMOTE_DIR/${CHANNEL_SUBDIR:-channel}"

[ -d "$SRC" ] || {
    echo "ERROR: $SRC not found — build a channel first (scripts/release-wayang.sh)" >&2
    exit 1
}

echo "=== publishing channel ==="
echo "  from: $SRC"
echo "  to:   $HOST:$DEST"

# shellcheck disable=SC2029 # the path is meant to expand locally
ssh "$HOST" "mkdir -p '$DEST'"

# No --delete: the channel is append/versioned; never wipe other channels here.
rsync -rlptz --chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r \
    "$SRC/" "$HOST:$DEST/"

echo "done."
echo "verify:"
echo "  curl -fsSL https://wayang.dalang.io/channel/stable/x86_64/manifest.json"
