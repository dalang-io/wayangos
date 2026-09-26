#!/bin/bash
# Bisect the 1.0.13 "router" kernel options one group at a time.
#
# The 1.0.13 router block (docs/INCIDENT-1.0.13.md) locked up the test device.
# 1.0.15 re-enabled only VLAN_8021Q + BRIDGE + BRIDGE_VLAN_FILTERING; the rest
# is still off. This harness builds ONE group at a time on the build box into a
# throwaway /tmp dir, signs a .wup (WAYANG_KEY) and prints the exact device
# procedure. It NEVER reboots or otherwise touches the test device itself.
#
# Usage:
#   scripts/bisect-router-opts.sh --list
#   scripts/bisect-router-opts.sh [--group NAME] [options]
#
# Build options:
#   --group NAME       build only this group (default: all groups, in order)
#   --out DIR          local dir for the copied .wup (default: dist/router-bisect)
#   --version VER      bundle version (default: 1.0.99)
#   --kernel-version V kernel source version on the builder (default: 7.2.7)
#   --host HOST        ssh builder (default: root@10.0.0.251)
#   --work DIR         throwaway dir on the builder (default: /tmp/wayangos-bisect)
#   --shared DIR       existing source cache on the builder
#                      (default: /root/wayangos-build; never written to)
#   --key FILE         release signing key (default: $WAYANG_KEY, else unsigned)
#   --dry-run          print what would run; touch nothing
#   -h, --help         this help
#
# Env: WAYANG_KEY (release key path), HOST, REMOTE_WORK, SHARED, OUT_DIR
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
HOST="${HOST:-root@10.0.0.251}"
REMOTE_WORK="${REMOTE_WORK:-/tmp/wayangos-bisect}"
SHARED="${SHARED:-/root/wayangos-build}"
OUT_DIR="${OUT_DIR:-$REPO_DIR/dist/router-bisect}"
VERSION="${BISECT_VERSION:-1.0.99}"
KERNEL_VERSION="${KERNEL_VERSION:-7.2.7}"
WAYANG_KEY="${WAYANG_KEY:-}"
GROUP=""
DRY_RUN=0
LIST=0

# One group at a time. Safe first: the groups that do NOT create a netdev at
# boot can't be confused with a boot-time netdev problem (bond0/dummy0/
# ifb0-1/gre0/gretap0/erspan0/tunl0 come from the later groups).
BISECT_GROUPS=(
    veth-macvlan-tun
    wireguard
    vrf-multipath
    ipsec
    dummy-bonding
    gre-ipip
    qos
)

usage() {
    sed -n '2,30s/^# \{0,1\}//p' "$0"
    exit "${1:-0}"
}

group_note() {
    case "$1" in
        veth-macvlan-tun) echo "no boot netdev (veth/macvlan/tun need explicit creation)" ;;
        wireguard)        echo "no boot netdev; WIREGUARD arch crypto is a prime suspect" ;;
        vrf-multipath)    echo "no boot netdev (VRF devices are created explicitly)" ;;
        ipsec)            echo "no boot netdev (xfrm only; no XFRM interface at boot)" ;;
        dummy-bonding)    echo "creates dummy0 + bond0 at boot" ;;
        gre-ipip)         echo "creates gre0/gretap0/erspan0/tunl0 at boot" ;;
        qos)              echo "creates ifb0/ifb1 at boot" ;;
        *)                echo "" ;;
    esac
}

group_opts() {
    case "$1" in
        veth-macvlan-tun)
            echo "CONFIG_VETH=y"
            echo "CONFIG_MACVLAN=y"
            echo "CONFIG_TUN=y"
            ;;
        wireguard)
            echo "CONFIG_WIREGUARD=y"
            echo "CONFIG_CRYPTO_LIB_CHACHA=y"
            echo "CONFIG_CRYPTO_LIB_CHACHA_ARCH=y"
            echo "CONFIG_CRYPTO_LIB_POLY1305=y"
            echo "CONFIG_CRYPTO_LIB_POLY1305_ARCH=y"
            echo "CONFIG_CRYPTO_LIB_CURVE25519=y"
            echo "CONFIG_CRYPTO_LIB_CURVE25519_ARCH=y"
            ;;
        vrf-multipath)
            echo "CONFIG_NET_VRF=y"
            echo "CONFIG_IPV6_MULTIPLE_TABLES=y"
            echo "CONFIG_IPV6_SUBTREES=y"
            echo "CONFIG_IP_ROUTE_MULTIPATH=y"
            echo "CONFIG_IP_MULTIPLE_TABLES=y"
            ;;
        ipsec)
            echo "CONFIG_INET_ESP=y"
            echo "CONFIG_XFRM_INTERFACE=y"
            ;;
        dummy-bonding)
            echo "CONFIG_DUMMY=y"
            echo "CONFIG_BONDING=y"
            ;;
        gre-ipip)
            echo "CONFIG_NET_IPIP=y"
            echo "CONFIG_NET_IPGRE_DEMUX=y"
            echo "CONFIG_NET_IPGRE=y"
            echo "CONFIG_NET_UDP_TUNNEL=y"
            ;;
        qos)
            echo "CONFIG_IFB=y"
            echo "CONFIG_NET_SCH_HTB=y"
            echo "CONFIG_NET_SCH_FQ_CODEL=y"
            echo "CONFIG_NET_SCH_CAKE=y"
            echo "CONFIG_NET_SCH_INGRESS=y"
            echo "CONFIG_NET_CLS_U32=y"
            echo "CONFIG_NET_CLS_FW=y"
            echo "CONFIG_NET_ACT_POLICE=y"
            echo "CONFIG_NET_ACT_MIRRED=y"
            echo "CONFIG_NET_REDIRECT=y"
            ;;
        *)
            return 1
            ;;
    esac
}

is_group() {
    local g
    for g in "${BISECT_GROUPS[@]}"; do
        [ "$g" = "$1" ] && return 0
    done
    return 1
}

list_groups() {
    local i g opt
    echo "Router kernel bisect groups (1.0.13 block minus 1.0.15's VLAN/BRIDGE)"
    echo "====================================================================="
    echo "Run ONE group at a time. The ones that create no netdev at boot go first."
    echo
    i=0
    for g in "${BISECT_GROUPS[@]}"; do
        i=$((i + 1))
        printf '[%d] %-18s %s\n' "$i" "$g" "($(group_note "$g"))"
        while IFS= read -r opt; do
            printf '      %s\n' "$opt"
        done < <(group_opts "$g")
        echo
    done
    echo "Build:  scripts/bisect-router-opts.sh [--group NAME] [--out DIR]"
    echo "Device procedure and fold-back: docs/ROUTER-KERNEL-BISECT.md"
}

write_config() {
    local group="$1" dest="$2"
    cp "$REPO_DIR/configs/defconfig-intel" "$dest"
    {
        printf '\n# --- router bisect group: %s ---\n' "$group"
        group_opts "$group"
    } >> "$dest"
}

device_procedure() {
    local wup="$1" group="$2"
    cat <<EOF

Device test procedure for group $group
--------------------------------------
Bundle: $wup

 1. Copy it (no sftp-server on the device; verify the hash):
      sha256sum '$wup'
      ssh root@163.128.55.3 'cat > /data/bisect.wup' < '$wup'
      ssh root@163.128.55.3 'sha256sum /data/bisect.wup'

 2. Stage it into the idle slot (this does NOT reboot):
      ssh root@163.128.55.3 'wayang update --from /data/bisect.wup'

 3. Reboot into the idle slot BY HAND (pick it in GRUB / power-cycle).
    Do NOT run 'ssh ... reboot': if the new kernel locks up, that loses the
    only remote path back.

 4. At the console: does the keyboard work? Then, over SSH:
      ssh root@163.128.55.3 'uname -a; cat /proc/net/dev; dmesg | tail -n 40'
    Wait a few minutes for a late lockup before declaring the group good.

 5. Recovery if it locks up: power-cycle, pick slot A (the last good slot) in
    GRUB. If GRUB does not show, power-cycle and hold the slot-A entry. Then on
    the device confirm and re-mark the good slot:
      grep -o 'wayang.slot=[AB]' /proc/cmdline
      wayang update --rollback && wayang mark-ok
EOF
}

# ---------------------------------------------------------------- remote side
write_remote_script() {
    local dest="$1"
    cat > "$dest" <<'REMOTE'
#!/bin/bash
# Throwaway builder-side driver for scripts/bisect-router-opts.sh. Runs with
# BISECT_* in the environment; never writes to $SHARED (sources are symlinked,
# the kernel tree is copied).
set -euo pipefail

GROUP="${BISECT_GROUP:?missing BISECT_GROUP}"
REPO="${BISECT_REPO:?missing BISECT_REPO}"
TMPCONFIG="${BISECT_TMPCONFIG:?missing BISECT_TMPCONFIG}"
VERSION="${BISECT_VERSION:?missing BISECT_VERSION}"
KEY="${BISECT_KEY:-}"
WORK="${BISECT_WORK:?missing BISECT_WORK}"
SHARED="${BISECT_SHARED:?missing BISECT_SHARED}"
KV="${BISECT_KERNEL_VERSION:?missing BISECT_KERNEL_VERSION}"

BUILD="$WORK/build-$GROUP"
OUT="$WORK/out"
KDIR="$BUILD/linux-$KV"
SRC_KERNEL="$SHARED/linux-$KV"

echo "=== bisect build: group=$GROUP kernel=$KV on $(hostname) ==="
[ -d "$SHARED" ] || { echo "ERROR: shared source cache $SHARED missing" >&2; exit 1; }
[ -d "$SRC_KERNEL" ] || { echo "ERROR: kernel source $SRC_KERNEL missing (fetch-sources.sh on the builder)" >&2; exit 1; }
[ -f "$REPO/configs/$TMPCONFIG" ] || { echo "ERROR: staged config $REPO/configs/$TMPCONFIG missing" >&2; exit 1; }

rm -rf "$BUILD"
mkdir -p "$BUILD" "$OUT"

link_src() {
    local src="$SHARED/$1" dst="$BUILD/$1" f
    if [ ! -e "$src" ]; then
        echo "  note: no $src (that optional tool is skipped)"
        return 0
    fi
    mkdir -p "$dst"
    for f in "$src"/* "$src"/.[!.]*; do
        [ -e "$f" ] || continue
        ln -sfn "$f" "$dst/$(basename "$f")"
    done
}

# Symlink the staged sources (read-only use). The kernel tree is copied below
# because build-kernel.sh builds in-tree.
link_src busybox-1.37.0
link_src dropbear-2024.86
link_src wifi
link_src nft
link_src dcheck
# build-rootfs.sh writes its own localoptions.h -> drop the shared symlink so it
# lands in $BUILD, not /root/wayangos-build.
rm -f "$BUILD/dropbear-2024.86/localoptions.h"
[ -f "$SHARED/cacert.pem" ] && ln -sfn "$SHARED/cacert.pem" "$BUILD/cacert.pem"

echo "  copying kernel tree (in-tree build, leaving $SHARED untouched)..."
cp -a "$SRC_KERNEL" "$KDIR"

cd "$REPO"

echo "--- kernel ($TMPCONFIG) ---"
ARCH=x86_64 BUILD_DIR="$BUILD" KDIR="$KDIR" \
    ./scripts/build-kernel.sh "$TMPCONFIG" "bzImage-bisect-$GROUP"

echo "--- rootfs ---"
BUILD_DIR="$BUILD" WAYANG_VERSION="$VERSION" ./scripts/build-rootfs.sh

echo "--- bundle ---"
KEYARG=()
if [ -n "$KEY" ] && [ -f "$KEY" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
    if ! ls "$REPO"/dist/wayang-x86_64-unknown-linux-musl >/dev/null 2>&1; then
        echo "  building wayang CLI for signing..."
        TARGET=x86_64-unknown-linux-musl ./scripts/build-wayang.sh
    fi
    KEYARG=(--key "$KEY" --keyid release)
else
    echo "WARNING: no signing key (set WAYANG_KEY) -> UNSIGNED bundle;" >&2
    echo "         the device updater will reject it unless it trusts the key." >&2
fi

./scripts/build-bundle.sh "$BUILD/bzImage-bisect-$GROUP" "$BUILD/wayangos-initramfs.img" \
    "$VERSION" x86_64 "bisect-$GROUP" \
    --out "$OUT" --channel stable --notes "router kernel bisect group: $GROUP" \
    "${KEYARG[@]}"

mv "$OUT/wayang-$VERSION-x86_64.wup" "$OUT/bisect-$GROUP-$VERSION-x86_64.wup"
echo "BISECT_WUP=$OUT/bisect-$GROUP-$VERSION-x86_64.wup"
REMOTE
}

rsh() {
    if [ "$DRY_RUN" = 1 ]; then
        printf 'DRY-RUN: ssh %s %s\n' "$HOST" "$*"
        return 0
    fi
    # shellcheck disable=SC2029  # caller controls the remote command string
    ssh -o BatchMode=yes "$HOST" "$@"
}

build_group() {
    local group="$1"
    local remote_repo="$REMOTE_WORK/repo"
    local remote_out="$REMOTE_WORK/out"
    local remote_script="$REMOTE_WORK/bisect-remote.sh"
    local remote_key="$REMOTE_WORK/release.key"
    local local_cfg="$TMP/config-$group"
    local local_wup="$OUT_DIR/bisect-$group-$VERSION-x86_64.wup"
    local remote_wup="$remote_out/bisect-$group-$VERSION-x86_64.wup"

    echo ""
    echo "=== group: $group ==="
    echo "  $(group_note "$group")"
    while IFS= read -r opt; do echo "    $opt"; done < <(group_opts "$group")

    write_config "$group" "$local_cfg"

    if [ "$DRY_RUN" = 1 ]; then
        echo "  DRY-RUN: rsync repo -> $HOST:$remote_repo"
        echo "  DRY-RUN: copy $local_cfg -> $HOST:$remote_repo/configs/.bisect-$group"
        echo "  DRY-RUN: copy remote driver -> $HOST:$remote_script"
        echo "  DRY-RUN: ssh $HOST BISECT_GROUP=$group ... bash $remote_script"
        echo "  DRY-RUN: copy $HOST:$remote_wup -> $local_wup"
        device_procedure "$local_wup" "$group"
        return 0
    fi

    # The config fragment name must be valid on the builder's configs/ dir.
    scp -q "$local_cfg" "$HOST:$remote_repo/configs/.bisect-$group"

    local cmd
    cmd="BISECT_GROUP='$group' BISECT_REPO='$remote_repo' BISECT_TMPCONFIG='.bisect-$group' \
BISECT_VERSION='$VERSION' BISECT_KEY='$remote_key' BISECT_WORK='$REMOTE_WORK' \
BISECT_SHARED='$SHARED' BISECT_KERNEL_VERSION='$KERNEL_VERSION' \
bash '$remote_script'"

    rsh "$cmd"

    mkdir -p "$OUT_DIR"
    scp -q "$HOST:$remote_wup" "$local_wup"

    echo ""
    echo "Built: $local_wup"
    echo "  sha256: $(sha256_of "$local_wup")"
    device_procedure "$local_wup" "$group"
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# ---------------------------------------------------------------------- parse
while [ $# -gt 0 ]; do
    case "$1" in
        --list) LIST=1 ;;
        --group)
            [ $# -ge 2 ] || { echo "ERROR: --group needs a name" >&2; exit 1; }
            GROUP="$2"; shift ;;
        --out)
            [ $# -ge 2 ] || { echo "ERROR: --out needs a directory" >&2; exit 1; }
            OUT_DIR="$2"; shift ;;
        --version)
            [ $# -ge 2 ] || { echo "ERROR: --version needs a value" >&2; exit 1; }
            VERSION="$2"; shift ;;
        --kernel-version)
            [ $# -ge 2 ] || { echo "ERROR: --kernel-version needs a value" >&2; exit 1; }
            KERNEL_VERSION="$2"; shift ;;
        --host)
            [ $# -ge 2 ] || { echo "ERROR: --host needs a host" >&2; exit 1; }
            HOST="$2"; shift ;;
        --work)
            [ $# -ge 2 ] || { echo "ERROR: --work needs a directory" >&2; exit 1; }
            REMOTE_WORK="$2"; shift ;;
        --shared)
            [ $# -ge 2 ] || { echo "ERROR: --shared needs a directory" >&2; exit 1; }
            SHARED="$2"; shift ;;
        --key)
            [ $# -ge 2 ] || { echo "ERROR: --key needs a file" >&2; exit 1; }
            WAYANG_KEY="$2"; shift ;;
        --dry-run) DRY_RUN=1 ;;
        -h|--help) usage 0 ;;
        --*) echo "ERROR: unknown option: $1" >&2; usage 1 ;;
        *) echo "ERROR: unexpected argument: $1" >&2; usage 1 ;;
    esac
    shift
done

if [ "$LIST" = 1 ]; then
    list_groups
    exit 0
fi

# A /tmp work dir only; refuse anything that could be the real build tree.
case "$REMOTE_WORK" in
    /root/wayangos-build|"$SHARED")
        echo "ERROR: refusing to use the shared build dir as the work dir" >&2
        exit 1 ;;
    /tmp/*) ;;
    *) echo "WARNING: work dir $REMOTE_WORK is not under /tmp; that is only safe" >&2
       echo "         if it is truly throwaway and never $SHARED." >&2 ;;
esac

if [ -n "$GROUP" ]; then
    is_group "$GROUP" || { echo "ERROR: unknown group '$GROUP' (try --list)" >&2; exit 1; }
    selected=("$GROUP")
else
    selected=("${BISECT_GROUPS[@]}")
fi

if [ -z "$WAYANG_KEY" ]; then
    echo "WARNING: WAYANG_KEY is unset -> bundles will be UNSIGNED and the" >&2
    echo "         device will reject them unless a matching key is trusted." >&2
fi
if [ -n "$WAYANG_KEY" ] && [ ! -f "$WAYANG_KEY" ]; then
    echo "ERROR: WAYANG_KEY not found: $WAYANG_KEY" >&2
    exit 1
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/wayang-bisect.XXXXXX")"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

remote_repo="$REMOTE_WORK/repo"
write_remote_script "$TMP/bisect-remote.sh"

if [ "$DRY_RUN" != 1 ]; then
    echo "=== syncing repo -> $HOST:$remote_repo ==="
    ssh -o BatchMode=yes "$HOST" "mkdir -p '$remote_repo' '$REMOTE_WORK/out'"
    rsync -az --delete \
        --exclude '.git' --exclude 'dist' --exclude 'target' --exclude 'node_modules' \
        --exclude 'landing-page' \
        "$REPO_DIR/" "$HOST:$remote_repo/"
    scp -q "$TMP/bisect-remote.sh" "$HOST:$REMOTE_WORK/bisect-remote.sh"
    if [ -n "$WAYANG_KEY" ]; then
        scp -q "$WAYANG_KEY" "$HOST:$REMOTE_WORK/release.key"
    fi
fi

for g in "${selected[@]}"; do
    build_group "$g"
done

echo ""
echo "=== done. One group at a time: boot each on the device and check keyboard + SSH. ==="
