#!/bin/bash
# Deploy the landing page (https://wayang.dalang.io).
#
# Usage: ./scripts/deploy-site.sh
# Env:
#   HOST          ssh target (default: root@10.0.0.251)
#   REMOTE_DIR    served directory (default: /root/wayang.dalang.io/public)
#
# The server serves REMOTE_DIR with wayang.dalang.io.service (python http.server
# on 127.0.0.1) behind the Pingora proxy. The dcheck release channel under
# /dcheck/ is published from the dcheck repo (dalang-io/dcheck,
# scripts/publish-dcheck.sh) and is never touched here.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${HOST:-root@10.0.0.251}"
REMOTE_DIR="${REMOTE_DIR:-/root/wayang.dalang.io/public}"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo "=== staging landing page ==="
rsync -a \
    --exclude '*.mjs' --exclude '*.py' --exclude 'package*.json' \
    --exclude 'tailwind.config.js' --exclude 'node_modules' \
    "$REPO_DIR/landing-page/" "$STAGE/"

echo "=== uploading to $HOST:$REMOTE_DIR ==="
# shellcheck disable=SC2029 # the path is meant to expand locally
ssh "$HOST" "mkdir -p '$REMOTE_DIR'"
# --delete keeps the server in sync with the repo, except the dcheck channel.
rsync -rlptz --delete --chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r \
    --filter='P /dcheck/' \
    "$STAGE/" "$HOST:$REMOTE_DIR/"
echo "done."
