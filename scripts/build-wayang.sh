#!/bin/bash
# Build the wayang updater CLI for static musl targets.
#
# Usage: ./scripts/build-wayang.sh
# Env:
#   TARGETS  space-separated rust target triples
#            (default: x86_64-unknown-linux-musl aarch64-unknown-linux-musl)
#   TARGET   single target; overrides TARGETS
#   OUT      output path when building a single TARGET
#            (default: <repo>/dist/wayang-<target>)
#
# Cross-linking from macOS uses `zig cc` (as the installer and dcheck do).
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CRATE_DIR="$REPO_DIR/wayang"
BIN_NAME="wayang"

command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found in PATH" >&2; exit 1; }

if [ -n "${TARGET:-}" ]; then
    TARGETS="$TARGET"
else
    TARGETS="${TARGETS:-x86_64-unknown-linux-musl aarch64-unknown-linux-musl}"
fi

# shellcheck disable=SC2086
for TARGET in $TARGETS; do
    echo "=== Building $BIN_NAME ($TARGET) ==="

    if ! (cd "$CRATE_DIR" && rustup target list --installed 2>/dev/null) | grep -qx "$TARGET"; then
        echo "WARNING: rust target '$TARGET' not installed; skipping" >&2
        echo "  rustup target add $TARGET" >&2
        continue
    fi

    if [ "$(uname -s)" = "Darwin" ] && [[ "$TARGET" == *linux* ]]; then
        command -v zig >/dev/null 2>&1 || { echo "ERROR: cross-linking $TARGET on macOS needs zig" >&2; exit 1; }
        WRAP_DIR="$(mktemp -d)"
        trap 'rm -rf "$WRAP_DIR"' EXIT
        cat > "$WRAP_DIR/zigcc" << WRAPPER
#!/bin/sh
for a; do
    shift
    case "\$a" in
        -Wl,--fix-cortex-a53-843419) ;;
        *) set -- "\$@" "\$a" ;;
    esac
done
exec zig cc -target ${TARGET%%-*}-linux-musl "\$@"
WRAPPER
        chmod +x "$WRAP_DIR/zigcc"
        export "CARGO_TARGET_$(printf '%s' "$TARGET" | tr 'a-z-' 'A-Z_')_LINKER=$WRAP_DIR/zigcc"
        export RUSTFLAGS="${RUSTFLAGS:-} -C link-self-contained=no"
        echo "  Linker: zig cc (${TARGET%%-*}-linux-musl)"
    fi

    (cd "$CRATE_DIR" && cargo build --release --target "$TARGET")

    BIN="${CARGO_TARGET_DIR:-$CRATE_DIR/target}/$TARGET/release/$BIN_NAME"
    if [ -n "${OUT:-}" ] && [ "$TARGETS" = "$TARGET" ]; then
        DEST="$OUT"
    else
        DEST="$REPO_DIR/dist/$BIN_NAME-$TARGET"
    fi
    mkdir -p "$(dirname "$DEST")"
    cp "$BIN" "$DEST"

    echo "  Output: $DEST"
    echo "  Size:   $(du -h "$DEST" | cut -f1)"
    echo ""
done
