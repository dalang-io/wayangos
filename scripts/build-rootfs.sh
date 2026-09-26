#!/bin/bash
# Build WayangOS base rootfs (BusyBox + Dropbear SSH + curl)
# Output: initramfs image at $BUILD/wayangos-initramfs.img
# Based on v0.5.0 build-rootfs-v2.sh
set -e
# rootfs files are packed as-is: no group/world-writable dirs from the builder's umask
umask 022

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
ROOTFS="$BUILD/rootfs"
INITRAMFS="$BUILD/wayangos-initramfs.img"
BUSYBOX="$BUILD/busybox-1.37.0/busybox"
BUSYBOX_DIR="$BUILD/busybox-1.37.0"
DROPBEAR_DIR="$BUILD/dropbear-2024.86"
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# Public key(s) allowed to log in as root over SSH (root is pubkey-only)
SSH_AUTHORIZED_KEYS="${SSH_AUTHORIZED_KEYS:-}"

# Update metadata baked into the rootfs.
#   wayang/version      what the installed system reports (see `wayang status`)
#   wayang/channel      stable | edge
#   wayang/trusted_keys release signing keys, if any
WAYANG_VERSION="${WAYANG_VERSION:-1.0.12}"
WAYANG_CHANNEL="${WAYANG_CHANNEL:-stable}"
WAYANG_TRUSTED_KEYS="${WAYANG_TRUSTED_KEYS:-}"

ARCH="${ARCH:-x86_64}"
case "$ARCH" in
    x86_64) CROSS_COMPILE="${CROSS_COMPILE:-}" ;;
    arm64)  CROSS_COMPILE="${CROSS_COMPILE:-aarch64-linux-gnu-}" ;;
    *)
        echo "ERROR: unsupported ARCH=$ARCH (allowed: x86_64, arm64)" >&2
        exit 1
        ;;
esac

echo "=== Building WayangOS Rootfs ($ARCH) ==="

# Validate dependencies
[ -f "$BUSYBOX" ] || { echo "ERROR: Missing $BUSYBOX — run scripts/fetch-sources.sh first"; exit 1; }

# Clean rootfs
rm -rf "$ROOTFS"
mkdir -p "$ROOTFS"/{bin,sbin,usr/bin,usr/sbin,etc,proc,sys,dev,tmp,var/log,var/run,root/.ssh,mnt,data}

# ============================================
# 1. BusyBox
# ============================================
echo "[1/4] Installing BusyBox..."
if [ "$ARCH" = "arm64" ]; then
    echo "  Cross-building BusyBox for arm64..."
    (
        cd "$BUSYBOX_DIR"
        make ARCH=arm64 CROSS_COMPILE="$CROSS_COMPILE" distclean >/dev/null 2>&1 || true
        make ARCH=arm64 CROSS_COMPILE="$CROSS_COMPILE" defconfig >/dev/null
        sed -i 's/# CONFIG_STATIC is not set/CONFIG_STATIC=y/' .config
        sed -i 's/^CONFIG_TC=y/# CONFIG_TC is not set/' .config
        make ARCH=arm64 CROSS_COMPILE="$CROSS_COMPILE" -j"$(nproc)" LDFLAGS=-static >/dev/null
    )
fi
cp "$BUSYBOX" "$ROOTFS/bin/busybox"
chmod 755 "$ROOTFS/bin/busybox"

# Install applets from the list the BusyBox build writes: a cross-built
# binary can't run here to report them with --list
[ -f "$BUSYBOX_DIR/busybox.links" ] || { echo "ERROR: Missing $BUSYBOX_DIR/busybox.links — rebuild BusyBox" >&2; exit 1; }
while read -r link; do
    [ -e "$ROOTFS$link" ] || ln -s /bin/busybox "$ROOTFS$link"
done < "$BUSYBOX_DIR/busybox.links"

# The kernel runs /init from the initramfs
cat > "$ROOTFS/init" << 'EOF'
#!/bin/sh
exec /sbin/init
EOF
chmod 755 "$ROOTFS/init"

# ============================================
# 2. Dropbear SSH
# ============================================
echo "[2/4] Installing Dropbear SSH..."
# Root is pubkey-only: build without password auth at all (which also drops
# the need for crypt(), often missing from cross toolchains)
cat > "$DROPBEAR_DIR/localoptions.h" << 'EOF'
#define DROPBEAR_SVR_PASSWORD_AUTH 0
EOF
if [ "$ARCH" = "arm64" ]; then
    echo "  Cross-building Dropbear for arm64..."
    cd "$DROPBEAR_DIR"
    if [ -f Makefile ]; then
        make distclean >/dev/null 2>&1 || true
    fi
    CC="${CROSS_COMPILE}gcc" ./configure --host=aarch64-linux-gnu \
        --enable-static --disable-zlib --disable-pam --disable-harden \
        --disable-lastlog --disable-utmp --disable-utmpx --disable-wtmp --disable-wtmpx \
        LDFLAGS="-static" CFLAGS="-Os -s" 2>&1 | tail -3
    make PROGRAMS="dropbear dropbearkey dbclient scp" MULTI=1 STATIC=1 -j"$(nproc)" 2>&1 | tail -5
    cd "$BUILD"
elif [ ! -f "$DROPBEAR_DIR/dropbearmulti" ]; then
    echo "  Building Dropbear from source..."
    cd "$DROPBEAR_DIR"
    [ -f Makefile ] || CC="${CROSS_COMPILE}gcc" ./configure ${CROSS_COMPILE:+--host="${CROSS_COMPILE%-}"} --enable-static --disable-zlib --disable-pam --disable-harden \
        --disable-lastlog --disable-utmp --disable-utmpx --disable-wtmp --disable-wtmpx \
        LDFLAGS="-static" CFLAGS="-Os -s" 2>&1 | tail -3
    make PROGRAMS="dropbear dropbearkey dbclient scp" MULTI=1 STATIC=1 -j"$(nproc)" 2>&1 | tail -5
    cd "$BUILD"
fi

cp "$DROPBEAR_DIR/dropbearmulti" "$ROOTFS/usr/bin/dropbearmulti"
chmod 755 "$ROOTFS/usr/bin/dropbearmulti"
ln -sf /usr/bin/dropbearmulti "$ROOTFS/usr/sbin/dropbear"
ln -sf /usr/bin/dropbearmulti "$ROOTFS/usr/bin/dropbearkey"
ln -sf /usr/bin/dropbearmulti "$ROOTFS/usr/bin/dbclient"
ln -sf /usr/bin/dropbearmulti "$ROOTFS/usr/bin/scp"
ln -sf /usr/bin/dbclient "$ROOTFS/usr/bin/ssh"
echo "  Dropbear: $(du -h "$ROOTFS/usr/bin/dropbearmulti" | cut -f1)"

# ============================================
# 3. Static curl
# ============================================
echo "[3/4] Installing static curl..."
if [ "$ARCH" != "x86_64" ]; then
    echo "  WARNING: static curl is x86_64-only; skipping for $ARCH" >&2
elif [ -f "$ROOTFS/usr/bin/curl" ]; then
    echo "  curl already present"
else
    CURL_URL="https://github.com/moparisthebest/static-curl/releases/latest/download/curl-amd64"
    wget -q "$CURL_URL" -O "$ROOTFS/usr/bin/curl"
    chmod 755 "$ROOTFS/usr/bin/curl"
fi
if [ -f "$ROOTFS/usr/bin/curl" ]; then
    echo "  curl: $(du -h "$ROOTFS/usr/bin/curl" | cut -f1)"
fi
# CA certificates for HTTPS (the static curl has none built in); used by the
# installer and wayang-addkey to fetch keys from GitHub/GitLab
[ -f "$BUILD/cacert.pem" ] || wget -q https://curl.se/ca/cacert.pem -O "$BUILD/cacert.pem"
mkdir -p "$ROOTFS/etc/ssl/certs"
cp "$BUILD/cacert.pem" "$ROOTFS/etc/ssl/certs/ca-certificates.crt"

# ============================================
# 3b. WiFi tools & firmware (optional)
# ============================================
# wpa_supplicant/wpa_cli/iw are static binaries built elsewhere (they need
# libnl/openssl and there is no package manager). Drop them at the top level of
# $BUILD/wifi/ (scripts/fetch-sources.sh WIFI_TOOLS_URL does this) and they are
# installed here. Firmware blobs go under $BUILD/wifi/firmware/.
echo "[3b/4] Installing WiFi tools (optional)..."
WIFI_DIR="$BUILD/wifi"
mkdir -p "$ROOTFS/usr/sbin" "$ROOTFS/etc/wpa_supplicant"
wifi_tools=""
for tool in wpa_supplicant wpa_cli iw; do
    if [ -f "$WIFI_DIR/$tool" ]; then
        install -m 755 "$WIFI_DIR/$tool" "$ROOTFS/usr/sbin/$tool"
        wifi_tools="$wifi_tools $tool"
    fi
done
if [ -n "$wifi_tools" ]; then
    echo "  installed:$wifi_tools (in /usr/sbin)"
else
    echo "  no static wpa_supplicant/wpa_cli/iw in $WIFI_DIR — WiFi tools skipped"
    echo "  (provide them via WIFI_TOOLS_URL; see scripts/fetch-sources.sh)"
fi
if [ -d "$WIFI_DIR/firmware" ]; then
    mkdir -p "$ROOTFS/lib/firmware"
    cp -a "$WIFI_DIR/firmware/." "$ROOTFS/lib/firmware/"
    echo "  firmware: $(du -sh "$ROOTFS/lib/firmware" | cut -f1) in /lib/firmware"
else
    echo "  no $WIFI_DIR/firmware — no WiFi firmware bundled"
fi

# ============================================
# 4. Init scripts & config
# ============================================
echo "[4/4] Writing init scripts..."

# /etc/passwd & shadow
cat > "$ROOTFS/etc/passwd" << 'EOF'
root:x:0:0:root:/root:/bin/sh
nobody:x:65534:65534:nobody:/:/bin/false
EOF

cat > "$ROOTFS/etc/shadow" << 'EOF'
root:*:0:0:99999:7:::
nobody:!:0:0:99999:7:::
EOF
chmod 640 "$ROOTFS/etc/shadow"

# Root SSH is pubkey-only: install authorized_keys at build time
chmod 700 "$ROOTFS/root" "$ROOTFS/root/.ssh"
if [ -n "$SSH_AUTHORIZED_KEYS" ]; then
    [ -f "$SSH_AUTHORIZED_KEYS" ] || { echo "ERROR: SSH_AUTHORIZED_KEYS=$SSH_AUTHORIZED_KEYS not found" >&2; exit 1; }
    cp "$SSH_AUTHORIZED_KEYS" "$ROOTFS/root/.ssh/authorized_keys"
    chmod 600 "$ROOTFS/root/.ssh/authorized_keys"
    echo "  authorized_keys: $(grep -c . "$ROOTFS/root/.ssh/authorized_keys") key(s)"
else
    echo "  No SSH key baked in: add keys in the installer or with wayang-addkey"
fi

cat > "$ROOTFS/etc/group" << 'EOF'
root:x:0:
nobody:x:65534:
EOF

echo "wayangos" > "$ROOTFS/etc/hostname"

cat > "$ROOTFS/etc/hosts" << 'EOF'
127.0.0.1   localhost wayangos
::1         localhost
EOF

# /etc/profile
cat > "$ROOTFS/etc/profile" << 'PROFILE'
export PATH="/usr/sbin:/usr/bin:/sbin:/bin"
export HOME="/root"
export TERM="linux"
export PS1='\[\e[1;33m\]\h\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]# '
alias ll='ls -la'
alias ..='cd ..'
echo ""
echo "  $(wayang-logo)WayangOS"
echo "  The Shadow that Powers the Machine"
echo ""
PROFILE

# /etc/inittab — cttyhack gives the console shell its real tty (e.g. tty1) as
# controlling terminal; on bare /dev/console, Ctrl+C and job control don't work
cat > "$ROOTFS/etc/inittab" << 'EOF'
::sysinit:/etc/init.d/rcS
::respawn:/bin/cttyhack /bin/sh -l
tty2::askfirst:-/bin/sh
tty3::askfirst:-/bin/sh
::ctrlaltdel:/sbin/reboot
::shutdown:/etc/init.d/rcK
EOF

mkdir -p "$ROOTFS/etc/init.d" "$ROOTFS/etc/dropbear"

# Master init script
cat > "$ROOTFS/etc/init.d/rcS" << 'INIT'
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
mkdir -p /dev/pts /dev/shm /dev/input
mount -t devpts devpts /dev/pts
mount -t tmpfs tmpfs /dev/shm
mount -t tmpfs tmpfs /tmp
mount -t tmpfs tmpfs /var/run
[ -x /usr/bin/wayang-splash ] && wayang-splash

# Populate /dev
mdev -s 2>/dev/null || true

hostname -F /etc/hostname
dmesg -n 1

# Persistent data partition: installed systems boot with
# wayang.data=LABEL=WAYANGDATA (see wayang-install)
DATA=""
for arg in $(cat /proc/cmdline); do
    case "$arg" in wayang.data=*) DATA="${arg#wayang.data=}" ;; esac
done
if [ -n "$DATA" ]; then
    # NVMe/SATA probing can finish after init starts
    n=0
    until dev="$(findfs "$DATA" 2>/dev/null)" || [ $n -ge 10 ]; do sleep 1; n=$((n + 1)); done
    if [ -n "$dev" ] && mount -t ext4 -o noatime "$dev" /data; then
        echo "  /data on $dev"
        # keep SSH host keys across reboots
        mkdir -p /data/etc/dropbear
        # persisted network uplink choice (wayang-net use <iface>)
        mkdir -p /data/etc/network
        # persisted wpa_supplicant config dir (wayang wifi writes
        # /data/etc/wpa_supplicant.conf; the dir is symlinked for other tools)
        mkdir -p /data/etc/wpa_supplicant
        rm -rf /etc/dropbear && ln -s /data/etc/dropbear /etc/dropbear
        rm -rf /etc/wpa_supplicant && ln -s /data/etc/wpa_supplicant /etc/wpa_supplicant
        # this box's name and root's SSH keys (set by the installer / wayang-addkey)
        [ -s /data/etc/hostname ] && hostname -F /data/etc/hostname
        if [ -s /data/etc/ssh/authorized_keys ]; then
            cat /data/etc/ssh/authorized_keys >> /root/.ssh/authorized_keys
            chmod 600 /root/.ssh/authorized_keys
        fi
    else
        echo "  WARNING: $DATA not found — /data is in RAM and lost on reboot"
    fi
fi

# Firewall before the network: interfaces get addresses only after the last
# confirmed wayang-fw ruleset is loaded (no-op without wayang-fw/config).
/etc/init.d/fw start

echo "Starting network..."
/etc/init.d/network start

syslogd -O /var/log/messages -s 200 -b 2

echo "Starting SSH..."
/etc/init.d/sshd start
ntpd -p pool.ntp.org -S /bin/true &

echo ""
echo "  $(wayang-logo)WayangOS ready"
echo "  Kernel: $(uname -r)"
# Network runs in the background: announce the address when it comes up.
( /etc/init.d/network wait 20 >/dev/null 2>&1; \
  ip -4 addr show scope global 2>/dev/null | grep inet | awk '{print "  IP: " $2}' ) &
echo ""

# The system booted: clear the GRUB attempt counter and mark this slot good
# (see docs/UPDATE-DESIGN.md). A no-op when the updater is not installed.
if [ -x /usr/bin/wayang ]; then
    wayang mark-ok >/dev/null 2>&1 || true
fi

# Point-of-sale kiosk: starts only when a POS binary is installed and
# autostart is on (see `wayang pos`, /etc/init.d/pos).
/etc/init.d/pos start
INIT
chmod +x "$ROOTFS/etc/init.d/rcS"

# Shutdown script
cat > "$ROOTFS/etc/init.d/rcK" << 'SHUTDOWN'
#!/bin/sh
echo "WayangOS shutting down..."
/etc/init.d/pos stop >/dev/null 2>&1
killall dropbear 2>/dev/null
killall syslogd 2>/dev/null
killall ntpd 2>/dev/null
/etc/init.d/network stop
sync
umount -a -r 2>/dev/null
SHUTDOWN
chmod +x "$ROOTFS/etc/init.d/rcK"

# Firewall boot loader (wayang-fw + nft; both optional)
cat > "$ROOTFS/etc/init.d/fw" << 'FW'
#!/bin/sh
# /etc/init.d/fw — loads the last *confirmed* wayang-fw ruleset at boot, before
# the network starts. An unconfirmed commit (pending) is discarded, so a
# reboot always lands on a config that was known to work.
#
# wayang-fw is looked up in /data/bin (deployed, survives OS updates) then
# /usr/bin; config lives in /data/etc/fw (see `wayang-fw --help`).

find_fw() {
    for b in /data/bin/wayang-fw /usr/bin/wayang-fw; do
        [ -x "$b" ] && { echo "$b"; return 0; }
    done
    return 1
}

case "$1" in
    start)
        # a deployed firewall in /data/bin (persistent) is linked into /usr/bin so
        # `wayang-fw` is reachable in PATH after every boot
        if [ -x /data/bin/wayang-fw ]; then
            ln -sf /data/bin/wayang-fw /usr/bin/wayang-fw 2>/dev/null || true
        fi
        fw="$(find_fw)" || exit 0
        [ -f /data/etc/fw/config.toml ] || exit 0
        echo "Loading firewall..."
        if ! "$fw" boot 2>&1 | sed 's/^/  /'; then
            logger -t wayang-fw "boot: could not load the firewall"
        fi
        ;;
    stop)
        # keep filtering until power-off
        ;;
    status)
        fw="$(find_fw)" || { echo "wayang-fw not installed"; exit 1; }
        "$fw" status
        ;;
    *) echo "usage: $0 {start|stop|status}" >&2; exit 1 ;;
esac
FW
chmod +x "$ROOTFS/etc/init.d/fw"

# Point-of-sale kiosk service (supervisor + autostart setting)
cat > "$ROOTFS/etc/init.d/pos" << 'POS'
#!/bin/sh
# /etc/init.d/pos — WayangPOS kiosk service. Usually driven via `wayang pos`.
#
#   start     boot entry: start the POS if one is installed and autostart is on
#   stop      stop it; the console goes back to the terminal
#   restart   stop, then start regardless of autostart
#   status    binary, settings, state
#   enable    autostart at boot (and start now)
#   disable   no autostart (and stop now)
#   log       last lines of the POS log
#
# A supervisor restarts the POS when it crashes. When the POS exits on
# purpose (admin: Settings -> F10 "Exit to terminal") it stays stopped until
# `wayang pos start` or the next boot.
#
# The binary is /data/bin/wayang-pos (deployed, survives OS updates) or else
# /usr/bin/wayang-pos (baked into a POS image).
#
# /data/etc/pos.conf:
#   AUTOSTART=1   start at boot when a POS binary is installed (0 = never)
#   ALLOW_EXIT=1  admins may exit to the terminal from Settings (0 = never)

CONF=/data/etc/pos.conf
PIDFILE=/var/run/wayang-pos.supervisor.pid
STOPFLAG=/var/run/wayang-pos.stop
if grep -q ' /data ' /proc/mounts 2>/dev/null; then LOGDIR=/data/log; else LOGDIR=/var/log; fi
LOG="$LOGDIR/wayang-pos.log"

AUTOSTART=1
ALLOW_EXIT=1
# shellcheck disable=SC1090
[ -f "$CONF" ] && . "$CONF"

find_bin() {
    for b in /data/bin/wayang-pos /usr/bin/wayang-pos; do
        [ -x "$b" ] && { echo "$b"; return 0; }
    done
    return 1
}

set_conf() {
    mkdir -p "${CONF%/*}"
    { grep -v "^$1=" "$CONF" 2>/dev/null; echo "$1=$2"; } > "$CONF.tmp" && mv "$CONF.tmp" "$CONF"
}

supervisor_pid() {
    [ -f "$PIDFILE" ] || return 1
    pid="$(cat "$PIDFILE")"
    kill -0 "$pid" 2>/dev/null && { echo "$pid"; return 0; }
    rm -f "$PIDFILE"
    return 1
}

rotate_log() {
    mkdir -p "$LOGDIR"
    if [ -f "$LOG" ] && [ "$(wc -c < "$LOG")" -gt 524288 ]; then
        mv -f "$LOG" "$LOG.1"
    fi
}

# Runs in the background; restarts the POS until it exits cleanly or is stopped.
supervise() {
    bin="$1"
    fails=0
    window="$(date +%s)"
    while [ ! -f "$STOPFLAG" ]; do
        rotate_log
        echo "=== $(date '+%Y-%m-%d %H:%M:%S') starting $bin" >> "$LOG"
        WAYANG_POS_ALLOW_EXIT="$ALLOW_EXIT" "$bin" >> "$LOG" 2>&1 < /dev/null
        rc=$?
        [ -f "$STOPFLAG" ] && break
        if [ "$rc" -eq 0 ]; then
            echo "=== exited to the terminal" >> "$LOG"
            logger -t wayang-pos "exited to the terminal (wayang pos start to resume)"
            printf '\n  WayangPOS stopped. To return to the POS, type:  wayang pos start\n\n' > /dev/console 2>/dev/null
            break
        fi
        now="$(date +%s)"
        if [ $((now - window)) -gt 60 ]; then fails=0; window="$now"; fi
        fails=$((fails + 1))
        echo "=== exited with status $rc, restarting" >> "$LOG"
        logger -t wayang-pos "exited with status $rc, restarting"
        # back off when it keeps failing (e.g. no framebuffer)
        if [ "$fails" -ge 5 ]; then sleep 30; else sleep 2; fi
    done
    rm -f "$PIDFILE"
}

do_start() {
    if supervisor_pid >/dev/null; then echo "POS already running"; return 0; fi
    if pidof wayang-pos >/dev/null; then
        echo "POS already running (not supervised; use: wayang pos restart)"
        return 0
    fi
    bin="$(find_bin)" || { echo "no POS installed (/data/bin/wayang-pos or /usr/bin/wayang-pos)"; return 1; }
    rm -f "$STOPFLAG"
    setsid "$0" supervise "$bin" < /dev/null > /dev/null 2>&1 &
    echo $! > "$PIDFILE"
    echo "POS started ($bin)"
}

do_stop() {
    touch "$STOPFLAG"
    pid="$(supervisor_pid)" && kill "$pid" 2>/dev/null
    rm -f "$PIDFILE"
    # SIGTERM: the POS restores the text console before exiting
    killall wayang-pos 2>/dev/null || { rm -f "$STOPFLAG"; return 0; }
    n=0
    while pidof wayang-pos >/dev/null && [ $n -lt 3 ]; do sleep 1; n=$((n + 1)); done
    pidof wayang-pos >/dev/null && killall -9 wayang-pos 2>/dev/null
    rm -f "$STOPFLAG"
    echo "POS stopped"
}

case "$1" in
    start)
        # at boot only when installed and enabled; quiet otherwise
        find_bin >/dev/null || exit 0
        if [ "$AUTOSTART" != 1 ]; then echo "  POS autostart is off (wayang pos enable)"; exit 0; fi
        do_start
        ;;
    stop) do_stop ;;
    restart) do_stop >/dev/null; do_start ;;
    enable) set_conf AUTOSTART 1 && echo "POS autostart: on"; do_start ;;
    disable) set_conf AUTOSTART 0 && echo "POS autostart: off"; do_stop ;;
    supervise) supervise "$2" ;;
    log) tail -n 40 "$LOG" 2>/dev/null || echo "no log yet ($LOG)" ;;
    status|"")
        bin="$(find_bin)" || bin="not installed"
        echo "binary:     $bin"
        echo "autostart:  $([ "$AUTOSTART" = 1 ] && echo on || echo off)   ($CONF)"
        echo "exit:       $([ "$ALLOW_EXIT" = 1 ] && echo "allowed (admin: Settings -> F10)" || echo disabled)"
        if pid="$(supervisor_pid)"; then
            echo "state:      running (supervisor $pid, pos $(pidof wayang-pos))"
        elif pidof wayang-pos >/dev/null; then
            echo "state:      running, not supervised (pid $(pidof wayang-pos))"
        else
            echo "state:      stopped"
        fi
        echo "log:        $LOG"
        ;;
    *) echo "usage: $0 {start|stop|restart|status|enable|disable|log}" >&2; exit 1 ;;
esac
POS
chmod +x "$ROOTFS/etc/init.d/pos"

# Network init script
cat > "$ROOTFS/etc/init.d/network" << 'NETWORK'
#!/bin/sh
# Pick a wired uplink and make it the *primary* route.
#
# With no persisted choice (/data/etc/network/primary absent) it probes every
# wired NIC and keeps the first that actually gets a DHCP lease — so a dead
# onboard NIC or a late USB-Ethernet adapter can't stop networking.
#
# With a persisted choice (set with `wayang-net set`/`use`) only that interface
# is used, and /data/etc/network/config selects DHCP vs static and IPv4 vs IPv6.
# Only the primary interface owns the default route + DNS.

PRIMARY_FILE=/data/etc/network/primary
CONFIG_FILE=/data/etc/network/config
PRIMARY_RUN=/var/run/wayang-primary
PIDFILE=/var/run/network.pid

# physical, non-wireless interfaces (skips lo, bridges, VLANs, tunnels)
wired() {
    for d in /sys/class/net/*; do
        [ -e "$d/device" ] && [ ! -d "$d/wireless" ] && echo "${d##*/}"
    done
}

carrier() { [ "$(cat "/sys/class/net/$1/carrier" 2>/dev/null)" = "1" ]; }

# If $1 is a wireless interface with a saved wpa_supplicant config, associate
# first: DHCP cannot run until the link is up. `wayang wifi` writes
# /data/etc/wpa_supplicant.conf and makes the iface primary.
wifi_associate() {
    i="$1"
    [ -d "/sys/class/net/$i/wireless" ] || return 0
    command -v wpa_supplicant >/dev/null 2>&1 || return 0
    [ -s /data/etc/wpa_supplicant.conf ] || return 0
    ifconfig "$i" up 2>/dev/null || true
    killall wpa_supplicant 2>/dev/null
    if wpa_supplicant -B -i "$i" -c /data/etc/wpa_supplicant.conf >/dev/null 2>&1; then
        echo "  wpa_supplicant on $i"
    else
        echo "  WARNING: wpa_supplicant failed on $i" >&2
    fi
}

# synchronous: 0 if a lease was obtained
probe() {
    udhcpc -n -q -t 5 -T 3 -i "$1" -s /etc/udhcpc.script >/dev/null 2>&1
}

# persistent: daemonizes after the lease, then renews
serve() {
    echo "  DHCP on $1 (primary)"
    echo "$1" > "$PRIMARY_RUN"
    udhcpc -b -i "$1" -s /etc/udhcpc.script >/dev/null 2>&1
}

adopt() {
    mkdir -p /data/etc/network 2>/dev/null
    echo "$1" > "$PRIMARY_FILE" 2>/dev/null
    serve "$1"
}

# --- persisted config helpers ---------------------------------------------

# FAMILY gates which address families we touch (default: ipv4)
want_v4() { case "${FAMILY:-ipv4}" in ipv6) return 1 ;; *) return 0 ;; esac; }
want_v6() { case "${FAMILY:-ipv4}" in ipv6|both) return 0 ;; *) return 1 ;; esac; }

have_ip() { command -v ip >/dev/null 2>&1; }

# IPv6 SLAAC: DHCPv6 is NOT implemented, so we rely on router advertisements.
# accept_ra=2 accepts RAs even when forwarding is enabled.
enable_slaac() {
    ra="/proc/sys/net/ipv6/conf/$1/accept_ra"
    if [ -w "$ra" ]; then
        echo 2 > "$ra" 2>/dev/null || true
        ifconfig "$1" up 2>/dev/null || true
        echo "  IPv6 SLAAC on $1 (accept_ra=2)"
    else
        echo "  WARNING: IPv6 unavailable on $1 (no $ra)" >&2
    fi
}

write_resolv() {
    : > /etc/resolv.conf
    if want_v4; then
        for ns in $IPV4_DNS; do echo "nameserver $ns" >> /etc/resolv.conf; done
    fi
    if want_v6; then
        for ns in $IPV6_DNS; do echo "nameserver $ns" >> /etc/resolv.conf; done
    fi
}

apply_static() {
    i="$1"
    echo "$i" > "$PRIMARY_RUN"
    if want_v4; then
        if [ -n "$IPV4_ADDRESS" ]; then
            if have_ip; then
                ip addr add "$IPV4_ADDRESS" dev "$i" 2>/dev/null || true
            else
                ifconfig "$i" "$IPV4_ADDRESS" up 2>/dev/null || true
            fi
            ifconfig "$i" up 2>/dev/null || true
        fi
        if [ -n "$IPV4_GATEWAY" ]; then
            route del default 2>/dev/null
            if have_ip; then
                # A /32 lease leaves no connected route, so the gateway is
                # unreachable until we add an on-link host route to it.
                ip route add default via "$IPV4_GATEWAY" dev "$i" 2>/dev/null || {
                    ip route add "$IPV4_GATEWAY" dev "$i" 2>/dev/null
                    ip route add default via "$IPV4_GATEWAY" dev "$i" 2>/dev/null || true
                }
            else
                # BusyBox `route`: the interface is positional, no `dev` keyword
                route add default gw "$IPV4_GATEWAY" "$i" 2>/dev/null || true
            fi
        fi
    fi
    if want_v6; then
        if [ -n "$IPV6_ADDRESS" ]; then
            if have_ip; then
                ip -6 addr add "$IPV6_ADDRESS" dev "$i" 2>/dev/null || true
            else
                echo "  WARNING: no 'ip' — cannot set IPv6 on $i" >&2
            fi
        fi
        if [ -n "$IPV6_GATEWAY" ] && have_ip; then
            ip -6 route del default 2>/dev/null
            ip -6 route add default via "$IPV6_GATEWAY" dev "$i" 2>/dev/null || true
        fi
    fi
    write_resolv
    echo "  $i: static ${IPV4_ADDRESS:-}${IPV6_ADDRESS:+ $IPV6_ADDRESS}"
}

apply_dhcp() {
    i="$1"
    echo "$i" > "$PRIMARY_RUN"
    if want_v4; then
        probe "$i" || true
        serve "$i"
    fi
    if want_v6; then enable_slaac "$i"; fi
}

# load config (if any) and apply it to the chosen primary
apply_primary() {
    i="$1"
    # wireless: associate before DHCP (no-op for wired interfaces)
    wifi_associate "$i"
    MODE=""; FAMILY=""
    IPV4_ADDRESS=""; IPV4_GATEWAY=""; IPV4_DNS=""
    IPV6_ADDRESS=""; IPV6_GATEWAY=""; IPV6_DNS=""
    # shellcheck source=/dev/null
    [ -r "$CONFIG_FILE" ] && . "$CONFIG_FILE"
    case "$MODE" in
        static) apply_static "$i" ;;
        *)      apply_dhcp "$i" ;;
    esac
}

# no persisted choice: probe every wired NIC, first DHCP lease wins
auto_detect() {
    # prefer interfaces that report a link
    cand=""
    for i in $(wired); do carrier "$i" && cand="$cand $i"; done
    [ -z "$cand" ] && cand="$(wired)"
    for i in $cand; do
        if probe "$i"; then adopt "$i"; break; fi
    done

    # keep looking for late (USB) adapters for a while if nothing worked
    if [ ! -e "$PRIMARY_RUN" ]; then
        (
            n=0
            while [ $n -lt 60 ]; do
                sleep 2; n=$((n + 2))
                for i in $(wired); do
                    carrier "$i" || continue
                    [ -e "/var/run/probe.$i" ] && continue
                    : > "/var/run/probe.$i"
                    if probe "$i"; then adopt "$i"; exit 0; fi
                done
            done
        ) &
    fi
}

case "$1" in
    start)
        ifconfig lo 127.0.0.1 netmask 255.0.0.0 up
        for i in $(wired); do ifconfig "$i" up 2>/dev/null; done
        # Bring-up is instant, but probing/DHCP can take seconds. Run it in the
        # background so boot (and SSH) never waits on the network. `wait` blocks
        # only for callers that actually need an address.
        (
            sleep 1
            [ -r "$PRIMARY_FILE" ] && PRIMARY="$(tr -d '[:space:]' < "$PRIMARY_FILE")"
            if [ -n "$PRIMARY" ] && [ -d "/sys/class/net/$PRIMARY" ]; then
                apply_primary "$PRIMARY"
            else
                auto_detect
            fi
            # Lease the remaining wired NICs too, address-only: udhcpc.script
            # gives the default route + DNS to the primary interface alone, so a
            # second NIC plugged into a router still gets an IP for management.
            prim="$(tr -d '[:space:]' < "$PRIMARY_RUN" 2>/dev/null)"
            for i in $(wired); do
                [ "$i" = "$prim" ] && continue
                carrier "$i" || continue
                echo "  DHCP (secondary) on $i"
                udhcpc -b -i "$i" -s /etc/udhcpc.script >/dev/null 2>&1
            done
        ) >/var/log/network.log 2>&1 &
        echo $! > "$PIDFILE"
        ;;
    wait)
        # block until an address is up (up to ${2:-20}s); 0 when ready
        n=0; lim="${2:-20}"
        while [ "$n" -lt "$lim" ] && [ ! -e "$PRIMARY_RUN" ]; do sleep 1; n=$((n + 1)); done
        [ -e "$PRIMARY_RUN" ]
        ;;
    stop)
        killall udhcpc 2>/dev/null
        killall wpa_supplicant 2>/dev/null
        [ -r "$PIDFILE" ] && kill "$(cat "$PIDFILE")" 2>/dev/null
        for i in $(wired); do ifconfig "$i" down 2>/dev/null; done
        rm -f "$PRIMARY_RUN" "$PIDFILE" /var/run/probe.*
        ;;
    restart) $0 stop; sleep 1; $0 start ;;
esac
NETWORK
chmod +x "$ROOTFS/etc/init.d/network"

# DHCP client script
cat > "$ROOTFS/etc/udhcpc.script" << 'DHCP'
#!/bin/sh
case "$1" in
    bound|renew)
        if [ -n "$subnet" ]; then
            ifconfig "$interface" "$ip" netmask "$subnet" up
        else
            ifconfig "$interface" "$ip" up
        fi
        # Only the primary interface owns the default route and DNS, so several
        # NICs can't fight over them.
        prim="$(tr -d '[:space:]' < /var/run/wayang-primary 2>/dev/null)"
        if [ -z "$prim" ]; then prim="$interface"; echo "$interface" > /var/run/wayang-primary; fi
        if [ "$interface" = "$prim" ]; then
            if [ -n "$router" ]; then
                route del default 2>/dev/null
                for gw in $router; do
                    # A /32 lease (common in datacenters) has no connected route,
                    # so add an on-link host route to the gateway before the
                    # default route, or the kernel rejects it as unreachable.
                    if command -v ip >/dev/null 2>&1; then
                        ip route add default via "$gw" dev "$interface" 2>/dev/null && continue
                        ip route add "$gw" dev "$interface" 2>/dev/null
                        ip route add default via "$gw" dev "$interface" 2>/dev/null && continue
                    fi
                    # BusyBox `route` takes the interface positionally (no `dev`)
                    route add default gw "$gw" "$interface" 2>/dev/null || true
                done
            fi
            : > /etc/resolv.conf
            for ns in $dns; do echo "nameserver $ns" >> /etc/resolv.conf; done
        fi
        echo "  $interface: $ip (gw: ${router:-none})"
        ;;
    deconfig) ifconfig "$interface" 0.0.0.0 ;;
esac
DHCP
chmod +x "$ROOTFS/etc/udhcpc.script"

# wayang-net — inspect and choose the primary uplink
cat > "$ROOTFS/usr/bin/wayang-net" << 'WAYANGNET'
#!/bin/sh
# wayang-net — choose which wired NIC provides internet and how it is addressed.
#   wayang-net list
#   wayang-net use <iface>                     primary = iface, DHCPv4 (as before)
#   wayang-net set <iface> dhcp [ipv4|ipv6|both]
#   wayang-net set <iface> static --ipv4 A/P --ipv4-gw GW --ipv4-dns "D…" \
#                                  [--ipv6 A/P --ipv6-gw GW --ipv6-dns "D…"]
#   wayang-net auto                            forget the choice, auto-detect
PATH=/usr/sbin:/usr/bin:/sbin:/bin
PRIMARY_FILE=/data/etc/network/primary
CONFIG_FILE=/data/etc/network/config
PRIMARY_RUN=/var/run/wayang-primary

wired() { for d in /sys/class/net/*; do [ -e "$d/device" ] && [ ! -d "$d/wireless" ] && echo "${d##*/}"; done; }
link_of() { [ "$(cat "/sys/class/net/$1/carrier" 2>/dev/null)" = 1 ] && echo link || echo no-link; }
drv_of() { basename "$(readlink -f "/sys/class/net/$1/device/driver" 2>/dev/null)" 2>/dev/null; }
ip_of() { ifconfig "$1" 2>/dev/null | awk '/inet /{print $2; exit}'; }

usage() {
    cat >&2 <<'EOF'
usage: wayang-net list
       wayang-net use <iface>
       wayang-net set <iface> dhcp [ipv4|ipv6|both]
       wayang-net set <iface> static --ipv4 A/P --ipv4-gw GW --ipv4-dns "D…"
                                      [--ipv6 A/P --ipv6-gw GW --ipv6-dns "D…"]
       wayang-net up <iface> | down <iface>
       wayang-net dhcp <iface>
       wayang-net auto
EOF
}

# $1 = address, $2 = max prefix length; require a numeric /prefix
valid_addr() {
    a="$1"; p="${a##*/}"
    case "$a" in */*) ;; *) return 1 ;; esac
    case "$p" in ''|*[!0-9]*) return 1 ;; esac
    [ "$p" -ge 0 ] && [ "$p" -le "$2" ]
}

write_choice() {
    mkdir -p /data/etc/network 2>/dev/null || true
    echo "$1" > "$PRIMARY_FILE" 2>/dev/null || true
    printf '%s\n' "$2" > "$CONFIG_FILE" 2>/dev/null || true
}

case "$1" in
    list|status|"")
        prim="$(tr -d '[:space:]' < "$PRIMARY_FILE" 2>/dev/null)"
        printf '%-12s %-8s %-12s %-16s %s\n' IFACE LINK DRIVER IPV4 NOTE
        for i in $(wired); do
            note=""
            [ "$i" = "$prim" ] && note="<- primary"
            printf '%-12s %-8s %-12s %-16s %s\n' "$i" "$(link_of "$i")" "$(drv_of "$i")" "$(ip_of "$i")" "$note"
        done
        echo "default route: $(ip route show default 2>/dev/null | head -1)"
        ;;
    up|down)
        i="$2"
        [ -n "$i" ] && [ -d "/sys/class/net/$i" ] || { echo "usage: wayang-net $1 <iface>" >&2; exit 1; }
        ifconfig "$i" "$1" || { echo "cannot set $i $1" >&2; exit 1; }
        echo "$i: link $1"
        ;;
    dhcp)
        i="$2"
        [ -n "$i" ] && [ -d "/sys/class/net/$i" ] || { echo "usage: wayang-net dhcp <iface>" >&2; exit 1; }
        # Pin the current primary so this lease can't take over the route/DNS
        # (udhcpc.script only installs those on the primary interface).
        prim="$(tr -d '[:space:]' < "$PRIMARY_FILE" 2>/dev/null)"
        if [ ! -s "$PRIMARY_RUN" ] && [ -n "$prim" ]; then echo "$prim" > "$PRIMARY_RUN"; fi
        ifconfig "$i" up 2>/dev/null
        udhcpc -n -q -t 5 -T 3 -i "$i" -s /etc/udhcpc.script || { echo "no DHCP lease on $i" >&2; exit 1; }
        udhcpc -b -i "$i" -s /etc/udhcpc.script >/dev/null 2>&1
        echo "$i: DHCP lease $(ip_of "$i") (primary unchanged)"
        ;;
    use)
        i="$2"
        [ -n "$i" ] && [ -d "/sys/class/net/$i" ] || { echo "usage: wayang-net use <iface>" >&2; exit 1; }
        killall udhcpc 2>/dev/null
        route del default 2>/dev/null
        mkdir -p /data/etc/network 2>/dev/null
        echo "$i" > "$PRIMARY_FILE" 2>/dev/null
        rm -f "$CONFIG_FILE"
        echo "$i" > "$PRIMARY_RUN"
        ifconfig "$i" up
        udhcpc -n -q -t 5 -T 3 -i "$i" -s /etc/udhcpc.script || { echo "no lease on $i" >&2; exit 1; }
        udhcpc -b -i "$i" -s /etc/udhcpc.script >/dev/null 2>&1
        echo "primary -> $i ($(ip_of "$i"))"
        ;;
    set)
        i="$2"; mode="$3"
        [ -n "$i" ] && [ -d "/sys/class/net/$i" ] || { usage; exit 1; }
        case "$mode" in
            dhcp)
                fam="${4:-ipv4}"
                case "$fam" in
                    ipv4|ipv6|both) ;;
                    *) echo "wayang-net: bad family '$fam' (want ipv4|ipv6|both)" >&2; exit 1 ;;
                esac
                write_choice "$i" "MODE=dhcp
FAMILY=$fam"
                ;;
            static)
                shift 3
                IPV4_ADDRESS=""; IPV4_GATEWAY=""; IPV4_DNS=""
                IPV6_ADDRESS=""; IPV6_GATEWAY=""; IPV6_DNS=""
                while [ $# -gt 0 ]; do
                    case "$1" in
                        --ipv4)     [ -n "$2" ] || { echo "wayang-net: $1 needs a value" >&2; exit 1; }; IPV4_ADDRESS="$2"; shift 2 ;;
                        --ipv4-gw)  [ -n "$2" ] || { echo "wayang-net: $1 needs a value" >&2; exit 1; }; IPV4_GATEWAY="$2"; shift 2 ;;
                        --ipv4-dns) [ -n "$2" ] || { echo "wayang-net: $1 needs a value" >&2; exit 1; }; IPV4_DNS="$2"; shift 2 ;;
                        --ipv6)     [ -n "$2" ] || { echo "wayang-net: $1 needs a value" >&2; exit 1; }; IPV6_ADDRESS="$2"; shift 2 ;;
                        --ipv6-gw)  [ -n "$2" ] || { echo "wayang-net: $1 needs a value" >&2; exit 1; }; IPV6_GATEWAY="$2"; shift 2 ;;
                        --ipv6-dns) [ -n "$2" ] || { echo "wayang-net: $1 needs a value" >&2; exit 1; }; IPV6_DNS="$2"; shift 2 ;;
                        *) echo "wayang-net: unknown option '$1'" >&2; exit 1 ;;
                    esac
                done
                [ -n "$IPV4_ADDRESS$IPV6_ADDRESS" ] || { echo "wayang-net: static needs --ipv4 and/or --ipv6 address" >&2; exit 1; }
                [ -z "$IPV4_ADDRESS" ] || valid_addr "$IPV4_ADDRESS" 32 || { echo "wayang-net: bad IPv4 address '$IPV4_ADDRESS' (need A/P, e.g. 192.168.1.50/24)" >&2; exit 1; }
                [ -z "$IPV6_ADDRESS" ] || valid_addr "$IPV6_ADDRESS" 128 || { echo "wayang-net: bad IPv6 address '$IPV6_ADDRESS' (need A/P, e.g. 2001:db8::50/64)" >&2; exit 1; }
                if [ -n "$IPV4_ADDRESS" ] && [ -n "$IPV6_ADDRESS" ]; then FAMILY=both
                elif [ -n "$IPV6_ADDRESS" ]; then FAMILY=ipv6
                else FAMILY=ipv4; fi
                cfg="MODE=static
FAMILY=$FAMILY"
                [ -n "$IPV4_ADDRESS" ] && cfg="$cfg
IPV4_ADDRESS=$IPV4_ADDRESS"
                [ -n "$IPV4_GATEWAY" ] && cfg="$cfg
IPV4_GATEWAY=$IPV4_GATEWAY"
                [ -n "$IPV4_DNS" ] && cfg="$cfg
IPV4_DNS=\"$IPV4_DNS\""
                [ -n "$IPV6_ADDRESS" ] && cfg="$cfg
IPV6_ADDRESS=$IPV6_ADDRESS"
                [ -n "$IPV6_GATEWAY" ] && cfg="$cfg
IPV6_GATEWAY=$IPV6_GATEWAY"
                [ -n "$IPV6_DNS" ] && cfg="$cfg
IPV6_DNS=\"$IPV6_DNS\""
                write_choice "$i" "$cfg"
                ;;
            *)
                usage; exit 1
                ;;
        esac
        # apply now so curl works without a reboot
        /etc/init.d/network restart
        echo "primary -> $i ($(ip_of "$i"))"
        ;;
    auto)
        killall udhcpc 2>/dev/null
        rm -f "$PRIMARY_FILE" "$CONFIG_FILE" "$PRIMARY_RUN" /var/run/probe.*
        /etc/init.d/network restart
        ;;
    *)
        usage
        exit 1
        ;;
esac
WAYANGNET
chmod +x "$ROOTFS/usr/bin/wayang-net"

# SSH init script
cat > "$ROOTFS/etc/init.d/sshd" << 'SSHD'
#!/bin/sh
case "$1" in
    start)
        if [ ! -f /etc/dropbear/dropbear_ed25519_host_key ]; then
            echo "  Generating SSH host keys..."
            dropbearkey -t ed25519 -f /etc/dropbear/dropbear_ed25519_host_key 2>/dev/null
        fi
        # built without password auth (localoptions.h): root is pubkey-only
        # logs to syslog (/var/log/messages), keeping the console clean
        if dropbear -R -p 22; then
            echo "  SSH listening on port 22"
        else
            echo "  ERROR: dropbear failed to start"
        fi
        ;;
    stop) killall dropbear 2>/dev/null ;;
    restart) $0 stop; sleep 1; $0 start ;;
esac
SSHD
chmod +x "$ROOTFS/etc/init.d/sshd"

# Prints "ꦮꦪꦁ  " for the terminal on stdin, or nothing on a local VT: the
# kernel console has no Javanese glyphs (and drops the cecak as zero-width).
# Readlink, not tty(1): the console shell's stdin predates the devtmpfs mount,
# so ttyname() can't find it.
cat > "$ROOTFS/usr/bin/wayang-logo" << 'LOGO'
#!/bin/sh
t="$(readlink /proc/$$/fd/0)"
# the last entry of console/active is the device behind /dev/console
[ "$t" = /dev/console ] && t="/dev/$(awk '{print $NF}' /sys/class/tty/console/active 2>/dev/null)"
case "$t" in
    /dev/tty[0-9]*) ;;
    *) printf 'ꦮꦪꦁ  ' ;;
esac
LOGO
chmod +x "$ROOTFS/usr/bin/wayang-logo"

# Boot splash: homepage wordmark + tagline + version/kernel/slot. Plain ASCII,
# because the kernel console font has no Javanese/box-drawing glyphs.
cat > "$ROOTFS/usr/bin/wayang-splash" << 'SPLASH'
#!/bin/sh
ver="$(cat /etc/wayang/version 2>/dev/null)"; [ -n "$ver" ] || ver="?"
kver="$(uname -r 2>/dev/null)"; [ -n "$kver" ] || kver="?"
slot="-"
for w in $(cat /proc/cmdline 2>/dev/null); do
    case "$w" in wayang.slot=*) slot="${w#wayang.slot=}" ;; esac
done
# Only draw on a real console, not when rcS output is piped/redirected.
t="$(readlink /proc/$$/fd/1 2>/dev/null)"
case "$t" in
    /dev/tty[0-9]*|/dev/console) ;;
    *) exit 0 ;;
esac
c="\033[1;36m"; m="\033[1;35m"; g="\033[1;32m"; y="\033[1;33m"; r="\033[0m"
printf '\033[2J\033[H'
printf "${c}+-----------------------------------------------------+${r}\n"
printf "  ${m}W   A   Y   A   N   G   O   S${r}\n"
printf "  The Shadow that Powers the Machine\n"
printf "  ${g}version %s${r}   linux ${g}%s${r}   slot ${y}%s${r}\n" "$ver" "$kver" "$slot"
printf "${c}+-----------------------------------------------------+${r}\n\n"
SPLASH
chmod +x "$ROOTFS/usr/bin/wayang-splash"

# Authorize SSH keys for root, now and (with /data) across reboots
cat > "$ROOTFS/usr/bin/wayang-addkey" << 'ADDKEY'
#!/bin/sh
# wayang-addkey — let an SSH public key log in as root.
#   wayang-addkey github:USER | gitlab:USER    keys published on GitHub/GitLab
#   wayang-addkey FILE                          a .pub or authorized_keys file
#   wayang-addkey 'ssh-ed25519 AAAA... you@laptop'
case "$1" in
    "" | -h | --help) sed -n '3,5s/^# *//p' "$0"; exit 1 ;;
    github:* | gitlab:*)
        url="https://${1%%:*}.com/${1#*:}.keys"
        keys="$(curl -fsSL --max-time 20 --cacert /etc/ssl/certs/ca-certificates.crt "$url")" ||
            { echo "cannot fetch $url" >&2; exit 1; } ;;
    *) if [ -f "$1" ]; then keys="$(cat "$1")"; else keys="$*"; fi ;;
esac
keys="$(printf '%s\n' "$keys" | grep -E '^(ssh-(ed25519|rsa|dss)|ecdsa-sha2-nistp(256|384|521)|sk-(ssh-ed25519|ecdsa-sha2-nistp256)@openssh\.com) [A-Za-z0-9+/]+=*( |$)')"
[ -n "$keys" ] || { echo "no SSH public key found" >&2; exit 1; }

files=/root/.ssh/authorized_keys
if mountpoint -q /data; then
    mkdir -p /data/etc/ssh && chmod 700 /data/etc/ssh
    files="$files /data/etc/ssh/authorized_keys"
fi
new=0
for k in $(printf '%s\n' "$keys" | tr ' ' '\001'); do
    k="$(printf '%s' "$k" | tr '\001' ' ')"
    added=
    for f in $files; do
        touch "$f" && chmod 600 "$f"
        grep -qxF "$k" "$f" || { echo "$k" >> "$f"; added=1; }
    done
    [ -n "$added" ] && new=$((new + 1))
done
if [ "$new" = 0 ]; then
    echo "already authorized"
elif mountpoint -q /data; then
    echo "authorized $new new key(s), saved in /data"
else
    echo "authorized $new new key(s) until reboot (no /data)"
fi
ADDKEY
chmod +x "$ROOTFS/usr/bin/wayang-addkey"

# ============================================
# Update metadata (/etc/wayang) and the `wayang` CLI
# ============================================
mkdir -p "$ROOTFS/etc/wayang"
printf '%s\n' "$WAYANG_VERSION" > "$ROOTFS/etc/wayang/version"
printf '%s\n' "$WAYANG_CHANNEL" > "$ROOTFS/etc/wayang/channel"
if [ -n "${KERNEL_VERSION:-}" ]; then
    printf '%s\n' "$KERNEL_VERSION" > "$ROOTFS/etc/wayang/kernel"
fi
if [ -n "$WAYANG_TRUSTED_KEYS" ]; then
    [ -f "$WAYANG_TRUSTED_KEYS" ] || { echo "ERROR: WAYANG_TRUSTED_KEYS=$WAYANG_TRUSTED_KEYS not found" >&2; exit 1; }
    cp "$WAYANG_TRUSTED_KEYS" "$ROOTFS/etc/wayang/trusted_keys"
else
    cat > "$ROOTFS/etc/wayang/trusted_keys" << 'KEYS'
# WayangOS release signing keys: <keyid> <64-hex-ed25519-public-key>
# (one per line). Empty until the project publishes release keys.
KEYS
fi
chmod 644 "$ROOTFS/etc/wayang/version" "$ROOTFS/etc/wayang/channel" "$ROOTFS/etc/wayang/trusted_keys"
echo "  /etc/wayang: version=$WAYANG_VERSION channel=$WAYANG_CHANNEL"

# `wayang` updater CLI — built by another job (wayang/); optional here.
case "$ARCH" in
    x86_64) WAYANG_TRIPLE="x86_64-unknown-linux-musl" ;;
    arm64)  WAYANG_TRIPLE="aarch64-unknown-linux-musl" ;;
esac
WAYANG_BIN=""
for cand in "$REPO_DIR/dist/wayang-$WAYANG_TRIPLE" "$REPO_DIR/dist/wayang-$ARCH"; do
    if [ -f "$cand" ]; then WAYANG_BIN="$cand"; break; fi
done
if [ -z "$WAYANG_BIN" ]; then
    for cand in "$REPO_DIR"/dist/wayang-*; do
        case "${cand##*/}" in wayang-installer-*) continue ;; esac
        if [ -f "$cand" ]; then WAYANG_BIN="$cand"; break; fi
    done
fi
if [ -n "$WAYANG_BIN" ]; then
    install -m 755 "$WAYANG_BIN" "$ROOTFS/usr/bin/wayang"
    echo "  wayang: $(du -h "$ROOTFS/usr/bin/wayang" | cut -f1)"
else
    echo "  wayang CLI not in dist/ (optional; install it with an update)"
fi

# dcheck — default storage-health app (static musl binary; fetched by
# scripts/fetch-dcheck.sh). Optional: skipped when not staged.
if [ -f "$BUILD/dcheck/dcheck" ]; then
    install -m 755 "$BUILD/dcheck/dcheck" "$ROOTFS/usr/bin/dcheck"
    echo "  dcheck: $(du -h "$ROOTFS/usr/bin/dcheck" | cut -f1)"
else
    echo "  dcheck not staged (run scripts/fetch-dcheck.sh to bundle it)"
fi

# nft — nftables CLI for wayang-fw (static; scripts/build-nft.sh). Optional.
if [ -f "$BUILD/nft/nft" ]; then
    install -m 755 "$BUILD/nft/nft" "$ROOTFS/usr/sbin/nft"
    echo "  nft: $(du -h "$ROOTFS/usr/sbin/nft" | cut -f1)"
else
    echo "  nft not staged (run scripts/build-nft.sh to bundle it)"
fi

# DNS fallback
cat > "$ROOTFS/etc/resolv.conf" << 'EOF'
nameserver 1.1.1.1
nameserver 8.8.8.8
EOF

# ============================================
# Build initramfs
# ============================================
echo ""
echo "=== Building initramfs ==="
cd "$ROOTFS"
# -R 0:0: everything owned by root, or dropbear rejects /root and authorized_keys
find . | cpio -o -H newc -R 0:0 2>/dev/null | gzip -9 > "$INITRAMFS"

echo ""
echo "=== Rootfs Complete ==="
echo "  BusyBox: $(du -h bin/busybox | cut -f1)"
echo "  Dropbear: $(du -h usr/bin/dropbearmulti | cut -f1)"
if [ -f usr/bin/curl ]; then
    echo "  curl: $(du -h usr/bin/curl | cut -f1)"
else
    echo "  curl: skipped ($ARCH)"
fi
echo "  Initramfs: $(du -h "$INITRAMFS" | cut -f1)"
