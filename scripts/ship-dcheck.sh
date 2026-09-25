#!/usr/bin/env bash
# Ship a dcheck release end to end, quietly: every long step writes to a log
# file and prints one line (so running it costs little time — or tokens).
#
#   scripts/ship-dcheck.sh 0.5.2 -m "fix(dcheck): … (0.5.2)"
#   scripts/ship-dcheck.sh 0.5.2 --dry-run          # checks only
#   scripts/ship-dcheck.sh 0.5.2 -m "…" --test-host idch --test-host biznetgio
#
# Steps:
#   1. preflight   branch master, version newer than the published LATEST
#   2. bump        dcheck/Cargo.toml (+ Cargo.lock) and the landing page
#   3. checks      cargo test, clippy (host + x86_64 Linux), e2e fixtures
#   4. scan        the diff for credentials / public IPs (public repo!)
#   5. commit      dcheck/ landing-page/ docs/ scripts/ with -m MESSAGE
#   6. deploy      scripts/deploy-site.sh (4 targets, upload, LATEST, site)
#   7. verify      LATEST + landing page online, `dcheck update` from the
#                  previous version locally and on each --test-host
#   8. tag + push  dcheck-vX.Y.Z, master and the tag
#
# Options:
#   -m, --message MSG   commit message (required unless --dry-run)
#   --test-host HOST    also verify `dcheck update` on HOST over ssh (repeatable)
#   --dry-run           preflight, checks and scan of the current tree; nothing
#                       is bumped, committed or published
#   --skip-checks       skip step 3 (only when the same tree was just tested)
#   --allow-ip          do not stop on public IPv4 addresses in the diff
#
# Env: HOST / REMOTE_DIR are passed to deploy-site.sh. SHIP_LOG sets the log
# file (default: a temp file, printed at the end).
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SITE="https://wayang.dalang.io"
VERSION=""
MESSAGE=""
DRY=0
SKIP_CHECKS=0
ALLOW_IP=0
TEST_HOSTS=()

usage() { sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

while [ $# -gt 0 ]; do
    case "$1" in
        -m|--message) MESSAGE="${2:-}"; shift ;;
        --test-host) TEST_HOSTS+=("${2:-}"); shift ;;
        --dry-run) DRY=1 ;;
        --skip-checks) SKIP_CHECKS=1 ;;
        --allow-ip) ALLOW_IP=1 ;;
        -h|--help) usage 0 ;;
        -*) echo "unknown option: $1" >&2; usage 2 ;;
        *) VERSION="$1" ;;
    esac
    shift
done
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "usage: $0 X.Y.Z -m MESSAGE [--dry-run]" >&2; exit 2; }
[ "$DRY" = 1 ] || [ -n "$MESSAGE" ] || { echo "error: -m MESSAGE is required (or --dry-run)" >&2; exit 2; }

LOG="${SHIP_LOG:-$(mktemp -t ship-dcheck.XXXXXX)}"
: > "$LOG"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${TMPDIR:-/tmp}/dcheck-ship-target}"
START=$(date +%s)

# step NAME CMD… — run quietly, one status line, stop with the log tail on failure.
step() {
    local name="$1"; shift
    local t0=$(date +%s)
    printf '  %-10s ' "$name"
    echo "===== $name: $*" >> "$LOG"
    if "$@" >> "$LOG" 2>&1; then
        echo "ok ($(( $(date +%s) - t0 ))s)"
    else
        echo "FAILED — this step's output (full log: $LOG):"
        # Only the failing step, last 20 lines.
        awk -v m="===== $name:" 'index($0, m) == 1 { buf = "" } { buf = buf $0 "\n" } END { printf "%s", buf }' "$LOG" \
            | tail -20 | sed 's/^/      /'
        exit 1
    fi
}

newer() { # $1 > $2 (semver)
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ]
}

cd "$ROOT"
CURRENT=$(sed -n 's/^version = "\(.*\)"/\1/p' dcheck/Cargo.toml | head -1)
PUBLISHED=$(curl -fsS --max-time 10 "$SITE/dcheck/LATEST" 2>/dev/null || echo "0.0.0")
echo "dcheck ship: $CURRENT → $VERSION (published: $PUBLISHED)$([ "$DRY" = 1 ] && echo ', dry run')"

preflight() {
    [ "$(git rev-parse --abbrev-ref HEAD)" = master ] || { echo "not on master"; return 1; }
    newer "$VERSION" "$PUBLISHED" || { echo "$VERSION is not newer than the published $PUBLISHED"; return 1; }
    for t in cargo git curl zig; do command -v "$t" >/dev/null || { echo "missing: $t"; return 1; }; done
    ! git rev-parse -q --verify "refs/tags/dcheck-v$VERSION" >/dev/null || { echo "tag dcheck-v$VERSION exists"; return 1; }
}
step preflight preflight

bump() {
    if [ "$CURRENT" != "$VERSION" ]; then
        sed -i.bak "s/^version = \"$CURRENT\"/version = \"$VERSION\"/" dcheck/Cargo.toml && rm -f dcheck/Cargo.toml.bak
        local re="${CURRENT//./\\.}"
        sed -i.bak "s/$re/$VERSION/g" landing-page/apps/dcheck.html && rm -f landing-page/apps/dcheck.html.bak
    fi
    (cd dcheck && cargo metadata --format-version 1 >/dev/null) # refreshes Cargo.lock
    grep -q "version = \"$VERSION\"" dcheck/Cargo.toml
}
[ "$DRY" = 1 ] || step bump bump

if [ "$SKIP_CHECKS" = 0 ]; then
    step test bash -c 'cd dcheck && cargo test --quiet'
    step clippy bash -c 'cd dcheck && cargo clippy --all-targets -- -D warnings'
    step clippy-lx bash -c 'cd dcheck && cargo clippy --target x86_64-unknown-linux-musl --all-targets -- -D warnings'
    step e2e ./scripts/test-dcheck.sh
fi

scan() {
    # Added lines only, in the parts of the repo a release touches.
    local diff new
    # (This script is skipped: it contains the patterns it looks for.)
    local paths=(dcheck landing-page docs scripts ':!scripts/ship-dcheck.sh')
    diff=$(git diff HEAD -- "${paths[@]}" | grep '^+' | grep -v '^+++')
    # New files are not in `git diff`: scan them whole.
    new=$(git ls-files --others --exclude-standard -- "${paths[@]}")
    [ -z "$new" ] || diff="$diff
$(echo "$new" | while read -r f; do [ -f "$f" ] && sed 's/^/+/' "$f"; done)"
    local bad
    bad=$(echo "$diff" | grep -inE 'password *[:=]|passwd|sshpass|BEGIN [A-Z ]*PRIVATE KEY|api[_-]?key *[:=]|secret *[:=]' || true)
    if [ -n "$bad" ]; then echo "possible credentials:"; echo "$bad"; return 1; fi
    if [ "$ALLOW_IP" = 0 ]; then
        # Public IPv4 (private ranges, localhost and version-like dotted
        # numbers in code are ignored).
        local ips
        ips=$(echo "$diff" | grep -oE '\b([0-9]{1,3}\.){3}[0-9]{1,3}\b' \
            | grep -vE '^(10\.|127\.|0\.|192\.168\.|172\.(1[6-9]|2[0-9]|3[01])\.|255\.)' | sort -u || true)
        if [ -n "$ips" ]; then echo "public IPs in the diff (use --allow-ip if intended):"; echo "$ips"; return 1; fi
    fi
}
step scan scan

if [ "$DRY" = 1 ]; then
    echo "dry run done in $(( $(date +%s) - START ))s — log: $LOG"
    exit 0
fi

step commit bash -c 'git add -A dcheck landing-page docs scripts && { git diff --cached --quiet || git commit -q -F -; }' <<< "$MESSAGE"
step deploy ./scripts/deploy-site.sh

verify() {
    local ok=0
    for _ in $(seq 1 12); do
        [ "$(curl -fsS --max-time 10 "$SITE/dcheck/LATEST?$RANDOM")" = "$VERSION" ] && { ok=1; break; }
        sleep 5
    done
    [ "$ok" = 1 ] || { echo "LATEST is not $VERSION"; return 1; }
    curl -fsS "$SITE/apps/dcheck.html?$RANDOM" | grep -q "v$VERSION" || { echo "landing page does not show v$VERSION"; return 1; }
    # Self-update from the previous release (temp dir; nothing installed).
    local t; t=$(mktemp -d)
    curl -fsSL "$SITE/dcheck/install.sh" | DCHECK_VERSION="$PUBLISHED" DCHECK_INSTALL_DIR="$t" sh
    "$t/dcheck" update
    "$t/dcheck" --version | grep -q "$VERSION" || { echo "local update did not reach $VERSION"; rm -rf "$t"; return 1; }
    rm -rf "$t"
}
step verify verify

for h in "${TEST_HOSTS[@]}"; do
    step "on $h" ssh -o BatchMode=yes -o ConnectTimeout=15 "$h" \
        "t=\$(mktemp -d); curl -fsSL $SITE/dcheck/install.sh | DCHECK_VERSION=$PUBLISHED DCHECK_INSTALL_DIR=\$t sh >/dev/null 2>&1; \$t/dcheck update; v=\$(\$t/dcheck --version); \$t/dcheck check; rm -rf \$t; echo \"\$v\" | grep -q $VERSION"
done

step tag git tag -a "dcheck-v$VERSION" -m "dcheck $VERSION"
step push bash -c "git push -q origin master && git push -q origin dcheck-v$VERSION"

echo "shipped dcheck $VERSION in $(( $(date +%s) - START ))s — log: $LOG"
