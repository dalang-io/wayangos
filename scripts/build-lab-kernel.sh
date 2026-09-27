#!/bin/bash
# Build the QEMU *lab* kernel: configs/defconfig-intel + the full 1.0.13 router
# option block (WireGuard, IPsec/xfrm, GRE/IPIP, VRF, multipath, IPv6 multiple
# tables, bonding, dummy, veth, macvlan, tun, HTB/fq_codel/CAKE/ingress/IFB,
# u32/fw, police/mirred) -> bzImage-lab.
#
#   *** QEMU-ONLY. NEVER SHIPPED, NEVER BOOTED ON THE TEST DEVICE. ***
#
# That block locked up the test device (docs/INCIDENT-1.0.13.md) and is being
# re-enabled one group at a time on hardware (docs/ROUTER-KERNEL-BISECT.md).
# The lab kernel exists so wayang-router can develop/test WireGuard, QoS and
# BGP against a kernel that has everything, in QEMU, meanwhile.
#
# There is deliberately no configs/defconfig-intel-lab: defconfig-intel is
# already a fragment and build-kernel.sh resolves only one level. Instead the
# option list is appended to a *temporary* copy of defconfig-intel (the list is
# taken from scripts/bisect-router-opts.sh --print-opts, so both stay in sync).
#
# Runs ON the build box (root@10.0.0.251). Builds in a throwaway /tmp dir with a
# *copy* of the kernel tree (build-kernel.sh builds in-tree); the shared source
# cache is only read.
#
# Usage: scripts/build-lab-kernel.sh [--out FILE]
# Env:   LAB_DIR         work dir            (default: /tmp/wayangos-lab-kernel)
#        SHARED          source cache, read-only (default: /root/wayangos-build)
#        KERNEL_VERSION  kernel source version   (default: 7.2.7)
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
LAB_DIR="${LAB_DIR:-/tmp/wayangos-lab-kernel}"
SHARED="${SHARED:-/root/wayangos-build}"
KERNEL_VERSION="${KERNEL_VERSION:-7.2.7}"
OUT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --out)
            [ $# -ge 2 ] || { echo "ERROR: --out needs a file" >&2; exit 1; }
            OUT="$2"; shift ;;
        -h|--help) sed -n '2,29s/^# \{0,1\}//p' "$0"; exit 0 ;;
        *) echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
    esac
    shift
done

case "$LAB_DIR" in
    /root/wayangos-build|/root/wayangos-build/*|"$SHARED"|"$SHARED"/*)
        echo "ERROR: refusing to build in the shared build dir ($LAB_DIR)" >&2
        exit 1 ;;
    /tmp/*) ;;
    *) echo "ERROR: LAB_DIR must be under /tmp (got $LAB_DIR)" >&2; exit 1 ;;
esac

SRC_KERNEL="$SHARED/linux-$KERNEL_VERSION"
KDIR="$LAB_DIR/linux-$KERNEL_VERSION"
[ -d "$KDIR" ] || [ -d "$SRC_KERNEL" ] || {
    echo "ERROR: kernel source $SRC_KERNEL missing (scripts/fetch-sources.sh on the builder)" >&2
    exit 1
}

mkdir -p "$LAB_DIR"
if [ ! -d "$KDIR" ]; then
    echo "  copying kernel tree (in-tree build; $SHARED stays untouched)..."
    cp -a "$SRC_KERNEL" "$KDIR"
fi

# Temporary one-level fragment (removed on exit): defconfig-intel + router block
FRAG=".lab-intel-$$"
trap 'rm -f "$REPO_DIR/configs/$FRAG"' EXIT
{
    cat "$REPO_DIR/configs/defconfig-intel"
    printf '\n# --- QEMU lab only: full 1.0.13 router block (never shipped) ---\n'
    "$REPO_DIR/scripts/bisect-router-opts.sh" --print-opts all
    # lab conveniences: QEMU's i6300esb watchdog (the selftest lab) and the
    # IPv6 ESP/xfrm user API the router IPsec work needs (already =y in base)
    echo "CONFIG_WATCHDOG_CORE=y"
    echo "CONFIG_I6300ESB_WDT=y"
    echo "CONFIG_XFRM_USER=y"
    echo "CONFIG_INET6_ESP=y"
} > "$REPO_DIR/configs/$FRAG"

ARCH=x86_64 BUILD_DIR="$LAB_DIR" KDIR="$KDIR" \
    "$REPO_DIR/scripts/build-kernel.sh" "$FRAG" bzImage-lab

# every requested option must survive olddefconfig (a silently dropped
# dependency would make the lab lie)
missing=0
while IFS= read -r opt; do
    case "$opt" in CONFIG_*=y) ;; *) continue ;; esac
    grep -qx "$opt" "$KDIR/.config" || { echo "  WARNING: $opt not in the resolved .config" >&2; missing=1; }
done < <("$REPO_DIR/scripts/bisect-router-opts.sh" --print-opts all)
[ "$missing" = 0 ] && echo "  all router options resolved =y"

cp "$KDIR/.config" "$LAB_DIR/config-lab"
if [ -n "$OUT" ]; then
    mkdir -p "$(dirname "$OUT")"
    cp "$LAB_DIR/bzImage-lab" "$OUT"
    echo "  -> $OUT"
fi
echo "  QEMU-ONLY lab kernel: $LAB_DIR/bzImage-lab (config: $LAB_DIR/config-lab)"
