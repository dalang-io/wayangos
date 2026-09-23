#!/bin/bash
# Build the dcheck static binary.
#
# Usage:
#   ./scripts/build-dcheck.sh            # default target (x86_64 linux musl)
#   ./scripts/build-dcheck.sh --native   # host target (quick local check)
#
# Env:
#   TARGET   rust target triple (default: x86_64-unknown-linux-musl)
#   OUT      output path (default: <repo>/dist/dcheck-<target>)
set -e

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DCHECK_DIR="$REPO_DIR/dcheck"

TARGET="${TARGET:-x86_64-unknown-linux-musl}"
if [ "${1:-}" = "--native" ]; then
    TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi

command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found in PATH" >&2; exit 1; }

echo "=== Building dcheck ($TARGET) ==="

# Ensure the rust std for the target is installed.
if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
    echo "ERROR: rust target '$TARGET' not installed. Run:" >&2
    echo "  rustup target add $TARGET" >&2
    exit 1
fi

# Cross-linking a musl/linux target from macOS needs a linker; use zig cc.
linker_var="CARGO_TARGET_$(printf '%s' "$TARGET" | tr 'a-z-' 'A-Z_')_LINKER"
if [ "$(uname -s)" = "Darwin" ] && [[ "$TARGET" == *linux* ]]; then
    if command -v zig >/dev/null 2>&1; then
        case "$TARGET" in
            x86_64-*)  zig_arch="x86_64" ;;
            aarch64-*) zig_arch="aarch64" ;;
            *)         zig_arch="" ;;
        esac
        if [ -n "$zig_arch" ]; then
            WRAP_DIR="$(mktemp -d)"
            WRAP="$WRAP_DIR/zigcc"
            # rustc >= 1.98 passes -Wl,--fix-cortex-a53-843419 for aarch64,
            # which zig's linker rejects; drop it (and nothing else).
            cat > "$WRAP" <<WRAPPER
#!/bin/sh
for a; do
    shift
    case "\$a" in
        -Wl,--fix-cortex-a53-843419) ;;
        *) set -- "\$@" "\$a" ;;
    esac
done
exec zig cc -target ${zig_arch}-linux-musl "\$@"
WRAPPER
            chmod +x "$WRAP"
            export "${linker_var}=${WRAP}"
            # zig provides the C runtime; disable rustc's bundled one to avoid
            # duplicate _start symbols.
            export RUSTFLAGS="${RUSTFLAGS:-} -C link-self-contained=no"
            echo "  Linker: zig cc ($zig_arch-linux-musl)"
        fi
    else
        echo "WARNING: cross-linking $TARGET on macOS needs zig or a musl toolchain" >&2
    fi
fi

cd "$DCHECK_DIR"
cargo build --release --target "$TARGET"

TARGET_DIR="${CARGO_TARGET_DIR:-$DCHECK_DIR/target}"
BIN="$TARGET_DIR/$TARGET/release/dcheck"
[ -f "$BIN" ] || { echo "ERROR: binary not found at $BIN" >&2; exit 1; }

OUT="${OUT:-$REPO_DIR/dist/dcheck-$TARGET}"
mkdir -p "$(dirname "$OUT")"
cp "$BIN" "$OUT"

echo ""
echo "=== dcheck built ==="
echo "  Output: $OUT"
echo "  Size:   $(du -h "$OUT" | cut -f1)"
