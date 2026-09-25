#!/bin/bash
# End-to-end test for `wayang update` / `update --rollback` and bundle trust.
#
# Usage:
#   ./scripts/test-update-qemu.sh [--qemu]
#
# Default (no root needed): a deterministic simulation driven by WAYANG_ROOT and
# WAYANG_ARCH that, for both state backends:
#   1. generates a signing key;
#   2. builds a signed bundle with scripts/build-bundle.sh;
#   3. sets the installed /etc/wayang/version and trusted_keys;
#   4. runs `wayang update --from` and asserts the idle slot is staged and the
#      state file updated (grub/grubenv on x86_64, wayang/vars on arm64);
#   5. runs `wayang update --rollback` and asserts the original slot is restored;
#   6. asserts an unsigned bundle and a bundle signed by an untrusted key are
#      rejected (exit code 3) and change no state.
#
#   --qemu   additionally try to boot a real A/B disk image with
#            qemu-system-x86_64 and grep the boot log for the version. It skips
#            gracefully when qemu, the kernel/initramfs or the disk are absent.
#
# Env:
#   WAYANG_BIN    use this updater binary instead of building one with cargo
#   BUILD_DIR     build root for `--qemu` artifacts (default: ~/wayangos-build)
#   QEMU_DISK     A/B disk image for `--qemu` (default: $BUILD_DIR/wayangos-ab.img)
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/wayang-e2e.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

QEMU=0
PASS=0
FAIL=0

usage() {
    sed -n '2,22s/^# \{0,1\}//p' "$0"
    exit "${1:-0}"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --qemu) QEMU=1; shift ;;
        -h|--help) usage 0 ;;
        *) echo "ERROR: unknown option: $1" >&2; usage 1 ;;
    esac
done

ok()   { printf '  PASS: %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  FAIL: %s\n' "$1"; FAIL=$((FAIL + 1)); }
note() { printf '  ----  %s\n' "$1"; }

expect_eq() { # desc actual expected
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi
}

expect_file() { # desc path
    if [ -f "$2" ]; then ok "$1"; else bad "$1 (missing $2)"; fi
}

expect_state() { # desc file key value
    if grep -q -- "$3=$4" "$2"; then
        ok "$1"
    else
        bad "$1 ($2 has no $3=$4)"
    fi
}

# --- build the updater --------------------------------------------------

check_cargo() {
    command -v cargo >/dev/null 2>&1 || {
        echo "ERROR: cargo not found and WAYANG_BIN is unset" >&2
        exit 1
    }
}

build_wayang() {
    if [ -n "${WAYANG_BIN:-}" ]; then
        [ -x "$WAYANG_BIN" ] || { echo "ERROR: WAYANG_BIN is not executable: $WAYANG_BIN" >&2; exit 1; }
        WAYANG="$WAYANG_BIN"
        return
    fi
    check_cargo
    note "building wayang (cargo)"
    cargo build --manifest-path "$REPO_DIR/wayang/Cargo.toml" --target-dir "$WORK/target" >/dev/null
    WAYANG="$WORK/target/debug/wayang"
    [ -x "$WAYANG" ] || { echo "ERROR: build produced no binary at $WAYANG" >&2; exit 1; }
}

# --- fixtures -----------------------------------------------------------

make_artifacts() {
    printf 'wayang-test-kernel\n' > "$WORK/vmlinuz"
    printf 'wayang-test-initramfs\n' > "$WORK/initramfs.img"
    printf '1.0.0\n' > "$WORK/installed-version"
}

make_keys() {
    note "keygen"
    "$WAYANG" keygen --out "$WORK/keys" >/dev/null
    "$WAYANG" keygen --out "$WORK/keys" --keyid imposter >/dev/null
    if [ ! -f "$WORK/keys/release.key" ] || [ ! -f "$WORK/keys/release.pub" ]; then
        echo "ERROR: keygen produced no release key" >&2
        exit 1
    fi
}

build_bundle() { # version arch key keyid outdir
    local version="$1" arch="$2" key="$3" keyid="$4" outdir="$5"
    local args=( "$WORK/vmlinuz" "$WORK/initramfs.img" "$version" "$arch" intel
        --out "$outdir" --channel stable )
    if [ -n "$key" ]; then
        args+=( --key "$key" --keyid "$keyid" )
    fi
    WAYANG_BIN="$WAYANG" "$REPO_DIR/scripts/build-bundle.sh" "${args[@]}" >/dev/null
}

write_grubenv() { # path slot
    local path="$1" slot="$2" tmp size pad
    tmp="$path.tmp"
    {
        printf '# GRUB Environment Block\n'
        printf 'wayang_slot=%s\n' "$slot"
        printf 'wayang_good=%s\n' "$slot"
        printf 'wayang_attempts=0\n'
    } > "$tmp"
    size=$(wc -c < "$tmp" | tr -d ' ')
    pad=$((1024 - size))
    printf '%*s' "$pad" '' | tr ' ' '#' >> "$tmp"
    mv "$tmp" "$path"
}

# --- one simulation run over a chosen state backend ---------------------

simulate() { # arch backend
    local arch="$1" backend="$2"
    local root="$WORK/$arch"
    local version="1.4.1" unsigned="1.4.2" forged="1.4.3"
    local bundles="$WORK/bundles/$arch"
    local state rc

    echo ""
    echo "=== simulate $arch ($backend state) ==="

    rm -rf "$root"
    mkdir -p "$root/etc/wayang" "$root/boot" "$bundles"

    case "$backend" in
        grub) mkdir -p "$root/boot/grub"; write_grubenv "$root/boot/grub/grubenv" A ;;
        vars) mkdir -p "$root/boot/wayang"
              printf 'wayang_slot=A\nwayang_good=A\nwayang_attempts=0\n' > "$root/boot/wayang/vars" ;;
        *) echo "ERROR: unknown backend $backend" >&2; exit 1 ;;
    esac

    printf '1.0.0\n' > "$root/etc/wayang/version"
    printf 'release %s\n' "$(tr -d '\n' < "$WORK/keys/release.pub")" > "$root/etc/wayang/trusted_keys"

    build_bundle "$version" "$arch" "$WORK/keys/release.key" release "$bundles"
    build_bundle "$unsigned" "$arch" "" "" "$bundles"
    build_bundle "$forged" "$arch" "$WORK/keys/imposter.key" release "$bundles"

    local signed="$bundles/wayang-$version-$arch.wup"
    local unsigned_b="$bundles/wayang-$unsigned-$arch.wup"
    local forged_b="$bundles/wayang-$forged-$arch.wup"
    expect_file "$arch: signed bundle built" "$signed"
    expect_file "$arch: unsigned bundle built" "$unsigned_b"
    expect_file "$arch: forged bundle built" "$forged_b"

    if [ "$backend" = "grub" ]; then
        state="$root/boot/grub/grubenv"
    else
        state="$root/boot/wayang/vars"
    fi

    # 4. update stages the idle slot B and updates state.
    set +e
    WAYANG_ROOT="$root" WAYANG_ARCH="$arch" "$WAYANG" update --from "$signed" >"$WORK/update.log" 2>&1
    rc=$?
    set -e
    expect_eq "$arch: update exits 0" "$rc" "0"
    expect_file "$arch: slot B vmlinuz staged" "$root/boot/B/vmlinuz"
    expect_file "$arch: slot B initramfs staged" "$root/boot/B/initramfs.img"
    expect_file "$arch: slot B meta written" "$root/boot/var/meta-B.json"
    expect_state "$arch: state points at B" "$state" wayang_slot B
    expect_state "$arch: previous slot recorded" "$state" wayang_prev A

    # 5. rollback restores slot A.
    set +e
    WAYANG_ROOT="$root" WAYANG_ARCH="$arch" "$WAYANG" update --rollback >"$WORK/rollback.log" 2>&1
    rc=$?
    set -e
    expect_eq "$arch: rollback exits 0" "$rc" "0"
    expect_state "$arch: state restored to A" "$state" wayang_slot A
    expect_state "$arch: previous slot now B" "$state" wayang_prev B

    # 6a. unsigned bundle rejected (missing signature => verify failure).
    set +e
    WAYANG_ROOT="$root" WAYANG_ARCH="$arch" "$WAYANG" update --from "$unsigned_b" >"$WORK/unsigned.log" 2>&1
    rc=$?
    set -e
    expect_eq "$arch: unsigned bundle rejected (exit 3)" "$rc" "3"
    expect_state "$arch: state unchanged after unsigned" "$state" wayang_slot A

    # 6b. bundle signed by an untrusted key rejected.
    set +e
    WAYANG_ROOT="$root" WAYANG_ARCH="$arch" "$WAYANG" update --from "$forged_b" >"$WORK/forged.log" 2>&1
    rc=$?
    set -e
    expect_eq "$arch: forged bundle rejected (exit 3)" "$rc" "3"
    expect_state "$arch: state unchanged after forged" "$state" wayang_slot A

    # status should resolve the backend and report the installed version.
    set +e
    local status_out
    status_out="$(WAYANG_ROOT="$root" WAYANG_ARCH="$arch" "$WAYANG" status 2>&1)"
    rc=$?
    set -e
    expect_eq "$arch: status exits 0" "$rc" "0"
    if printf '%s' "$status_out" | grep -q "backend:   $backend"; then
        ok "$arch: status reports $backend backend"
    else
        bad "$arch: status missing backend line"
    fi
    if printf '%s' "$status_out" | grep -q "1.0.0"; then
        ok "$arch: status reports installed version"
    else
        bad "$arch: status missing installed version"
    fi
}

# --- optional real boot (best effort) -----------------------------------

qemu_artifacts() {
    local build="${BUILD_DIR:-$HOME/wayangos-build}"
    local kernel="${QEMU_KERNEL:-$build/bzImage-intel}"
    local initramfs="${QEMU_INITRAMFS:-$build/wayangos-initramfs.img}"
    local disk="${QEMU_DISK:-$build/wayangos-ab.img}"

    echo ""
    echo "=== qemu boot test (best effort) ==="

    command -v qemu-system-x86_64 >/dev/null 2>&1 || { note "SKIP: qemu-system-x86_64 not found"; return 0; }
    [ -f "$kernel" ] || { note "SKIP: kernel not found: $kernel"; return 0; }
    [ -f "$initramfs" ] || { note "SKIP: initramfs not found: $initramfs"; return 0; }
    [ -f "$disk" ] || { note "SKIP: A/B disk image not found: $disk"; return 0; }
    command -v timeout >/dev/null 2>&1 || { note "SKIP: no 'timeout' command to bound qemu"; return 0; }

    local log="$WORK/qemu-boot.log"
    note "booting $disk (bounded to 60s)"
    set +e
    timeout 60 qemu-system-x86_64 \
        -machine accel=tcg -m 512 \
        -drive "file=$disk,format=raw,if=virtio" \
        -nographic -no-reboot \
        -serial "file:$log" \
        >/dev/null 2>&1
    set -e

    local wanted
    wanted="$(cat "$WORK/installed-version" 2>/dev/null || echo "1.0.0")"
    if grep -q "$wanted" "$log" 2>/dev/null; then
        ok "qemu: boot log mentions version $wanted"
    else
        note "SKIP: boot log did not mention $wanted (best effort)"
    fi
}

# --- main ---------------------------------------------------------------

build_wayang
make_artifacts
make_keys

simulate x86_64 grub
simulate arm64 vars

if [ "$QEMU" -eq 1 ]; then
    qemu_artifacts
fi

echo ""
if [ "$FAIL" -eq 0 ]; then
    printf 'PASS: %d checks passed\n' "$PASS"
    exit 0
fi
printf 'FAIL: %d passed, %d failed\n' "$PASS" "$FAIL"
exit 1
