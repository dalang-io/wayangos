#!/bin/bash
# Deploy the landing page (https://wayang.dalang.io) plus the dcheck release
# channel (/dcheck/install.sh, /dcheck/LATEST, /dcheck/v<ver>/...).
#
# Usage: ./scripts/deploy-site.sh
# Env:
#   HOST          ssh target (default: root@10.0.0.251)
#   REMOTE_DIR    served directory (default: /root/wayang.dalang.io/public)
#   RELEASE_DIR   prebuilt dcheck artifacts (default: build with release-dcheck.sh)
#   SKIP_DCHECK   1 to deploy only the landing page
#
# The server serves REMOTE_DIR with wayang.dalang.io.service (python http.server
# on 127.0.0.1) behind the Pingora proxy. Old dcheck versions are kept.
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

if [ -z "${SKIP_DCHECK:-}" ]; then
    VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$REPO_DIR/dcheck/Cargo.toml" | head -1)"
    RELEASE_DIR="${RELEASE_DIR:-}"
    if [ -z "$RELEASE_DIR" ]; then
        RELEASE_DIR="$STAGE.release"
        OUT_DIR="$RELEASE_DIR" "$REPO_DIR/scripts/release-dcheck.sh"
    fi
    echo "=== staging dcheck $VERSION ==="
    mkdir -p "$STAGE/dcheck/v$VERSION"
    cp "$RELEASE_DIR"/dcheck-"$VERSION"-*.tar.gz "$RELEASE_DIR/SHA256SUMS" "$STAGE/dcheck/v$VERSION/"
    cp "$REPO_DIR/dcheck/install.sh" "$STAGE/dcheck/install.sh"
    printf '%s\n' "$VERSION" > "$STAGE/dcheck/LATEST"
fi

echo "=== uploading to $HOST:$REMOTE_DIR ==="
ssh "$HOST" "mkdir -p '$REMOTE_DIR'"
# --delete keeps the server in sync with the repo, but never prunes older
# dcheck releases (clients may pin DCHECK_VERSION), nor the whole channel when
# it was not staged.
PROTECT=(--filter='P /dcheck/v*/')
[ -n "${SKIP_DCHECK:-}" ] && PROTECT=(--filter='P /dcheck/')
rsync -rlptz --delete --chmod=Du=rwx,Dgo=rx,Fu=rw,Fgo=r \
    "${PROTECT[@]}" \
    "$STAGE/" "$HOST:$REMOTE_DIR/"
echo "done."
