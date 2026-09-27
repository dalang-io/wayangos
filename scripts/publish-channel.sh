#!/bin/bash
# Publish the WayangOS update channel tree to the website host.
#
# Serves, e.g.:
#   https://wayang.dalang.io/channel/stable/x86_64/manifest.json
#   https://wayang.dalang.io/channel/stable/x86_64/wayang-1.0.0-x86_64.wup
#
# Usage: ./scripts/publish-channel.sh [--dry-run] [SOURCE_DIR]
#   SOURCE_DIR defaults to <repo>/dist/channel.
#   --dry-run prints exactly what rsync would transfer and makes NO change on
#   the remote (it still contacts the host read-only to compare); it never runs
#   the remote `mkdir`.
#
# Env (all optional; defaults keep the historical behaviour):
#   WAYANG_DEPLOY_HOST           ssh target      (default: root@10.0.0.251)
#   WAYANG_DEPLOY_REMOTE_DIR     served dir      (default: /root/wayang.dalang.io/public)
#   WAYANG_DEPLOY_CHANNEL_SUBDIR subdir under it (default: channel)
#   WAYANG_DEPLOY_SSH_KEY        path to the private key for the deploy host.
#                                When set, ssh/rsync use it with IdentitiesOnly
#                                and StrictHostKeyChecking=accept-new.
#   WAYANG_DEPLOY_SSH_PORT       ssh port (only used when set)
# Legacy aliases still accepted: HOST, REMOTE_DIR, CHANNEL_SUBDIR.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

usage() {
    sed -n '2,29s/^# \{0,1\}//p' "$0"
    exit "${1:-0}"
}

DRY_RUN=0
SRC=""
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)   usage 0 ;;
        --dry-run)   DRY_RUN=1; shift ;;
        --)          shift; break ;;
        -*)          echo "ERROR: unknown option: $1" >&2; usage 1 ;;
        *)           SRC="$1"; shift ;;
    esac
done

SRC="${SRC:-$REPO_DIR/dist/channel}"
HOST="${WAYANG_DEPLOY_HOST:-${HOST:-root@10.0.0.251}}"
REMOTE_DIR="${WAYANG_DEPLOY_REMOTE_DIR:-${REMOTE_DIR:-/root/wayang.dalang.io/public}}"
CHANNEL_SUBDIR="${WAYANG_DEPLOY_CHANNEL_SUBDIR:-${CHANNEL_SUBDIR:-channel}}"
DEST="$REMOTE_DIR/$CHANNEL_SUBDIR"

[ -d "$SRC" ] || {
    echo "ERROR: $SRC not found — build a channel first (scripts/release-wayang.sh)" >&2
    exit 1
}

# Build the per-invocation ssh options once, and a matching rsync `-e` string.
SSH_OPTS=()
if [ -n "${WAYANG_DEPLOY_SSH_KEY:-}" ]; then
    [ -f "$WAYANG_DEPLOY_SSH_KEY" ] || {
        echo "ERROR: WAYANG_DEPLOY_SSH_KEY not found: $WAYANG_DEPLOY_SSH_KEY" >&2
        exit 1
    }
    SSH_OPTS+=(-i "$WAYANG_DEPLOY_SSH_KEY"
               -o IdentitiesOnly=yes
               -o StrictHostKeyChecking=accept-new)
fi
if [ -n "${WAYANG_DEPLOY_SSH_PORT:-}" ]; then
    SSH_OPTS+=(-p "$WAYANG_DEPLOY_SSH_PORT")
fi

RSYNC_E=()
if [ "${#SSH_OPTS[@]}" -gt 0 ]; then
    printf -v ssh_opts_str ' %q' "${SSH_OPTS[@]}"
    RSYNC_E=(-e "ssh${ssh_opts_str}")
fi

# shellcheck disable=SC2054 # the commas are part of the --chmod value
RSYNC_OPTS=(-rlptz --chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r)
if [ "$DRY_RUN" -eq 1 ]; then
    RSYNC_OPTS+=(--dry-run --itemize-changes --stats)
fi

echo "=== publishing channel ==="
echo "  from: $SRC"
echo "  to:   $HOST:$DEST"
if [ "$DRY_RUN" -eq 1 ]; then
    echo "  mode: dry-run (no changes on the remote)"
fi

# shellcheck disable=SC2029 # the path is meant to expand locally
if [ "$DRY_RUN" -eq 0 ]; then
    ssh "${SSH_OPTS[@]+"${SSH_OPTS[@]}"}" "$HOST" "mkdir -p '$DEST'"
fi

# No --delete: the channel is append/versioned; never wipe other channels here.
if [ "$DRY_RUN" -eq 1 ]; then
    echo "  (dry-run) would run: ssh ${SSH_OPTS[*]:-} $HOST \"mkdir -p '$DEST'\""
fi
# shellcheck disable=SC2029 # the path is meant to expand locally
rsync "${RSYNC_E[@]+"${RSYNC_E[@]}"}" "${RSYNC_OPTS[@]}" \
    "$SRC/" "$HOST:$DEST/"

echo "done."
echo "verify:"
echo "  curl -fsSL https://wayang.dalang.io/channel/stable/x86_64/manifest.json"
