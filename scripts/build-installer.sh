#!/bin/bash
# Build the wayang-installer static binary (the installer ISO's TUI).
#
# Usage: ./scripts/build-installer.sh
# Env:
#   TARGET   rust target triple (default: x86_64-unknown-linux-musl)
#   OUT      output path (default: <repo>/dist/wayang-installer-<target>)
#
# Cross-linking from macOS uses `zig cc` (as dcheck does, dalang-io/dcheck).
set -e

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CRATE_DIR="$REPO_DIR/installer"
TARGET="${TARGET:-x86_64-unknown-linux-musl}"

command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found in PATH" >&2; exit 1; }

echo "=== Building wayang-installer ($TARGET) ==="

if ! (cd "$CRATE_DIR" && rustup target list --installed 2>/dev/null) | grep -qx "$TARGET"; then
    echo "ERROR: rust target '$TARGET' not installed. Run:" >&2
    echo "  rustup target add $TARGET" >&2
    exit 1
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

cd "$CRATE_DIR"
cargo build --release --target "$TARGET"

BIN="${CARGO_TARGET_DIR:-$CRATE_DIR/target}/$TARGET/release/wayang-installer"
OUT="${OUT:-$REPO_DIR/dist/wayang-installer-$TARGET}"
mkdir -p "$(dirname "$OUT")"
cp "$BIN" "$OUT"

echo ""
echo "=== wayang-installer built ==="
echo "  Output: $OUT"
echo "  Size:   $(du -h "$OUT" | cut -f1)"
