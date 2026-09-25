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
WAYANG_VERSION="${WAYANG_VERSION:-1.0.0}"
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
echo "WayangOS booting..."

mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null || true
mkdir -p /dev/pts /dev/shm /dev/input
mount -t devpts devpts /dev/pts
mount -t tmpfs tmpfs /dev/shm
mount -t tmpfs tmpfs /tmp
mount -t tmpfs tmpfs /var/run

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
        rm -rf /etc/dropbear && ln -s /data/etc/dropbear /etc/dropbear
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

echo "Starting network..."
/etc/init.d/network start

syslogd -O /var/log/messages -s 200 -b 2

echo "Starting SSH..."
/etc/init.d/sshd start
ntpd -p pool.ntp.org -S /bin/true &

echo ""
echo "  $(wayang-logo)WayangOS ready"
ip -4 addr show scope global 2>/dev/null | grep inet | awk '{print "  IP: " $2}'
echo ""

# The system booted: clear the GRUB attempt counter and mark this slot good
# (see docs/UPDATE-DESIGN.md). A no-op when the updater is not installed.
if [ -x /usr/bin/wayang ]; then
    wayang mark-ok >/dev/null 2>&1 || true
fi
INIT
chmod +x "$ROOTFS/etc/init.d/rcS"

# Shutdown script
cat > "$ROOTFS/etc/init.d/rcK" << 'SHUTDOWN'
#!/bin/sh
echo "WayangOS shutting down..."
killall dropbear 2>/dev/null
killall syslogd 2>/dev/null
killall ntpd 2>/dev/null
/etc/init.d/network stop
sync
umount -a -r 2>/dev/null
SHUTDOWN
chmod +x "$ROOTFS/etc/init.d/rcK"

# Network init script
cat > "$ROOTFS/etc/init.d/network" << 'NETWORK'
#!/bin/sh
# DHCP on every wired NIC, not just the first: a dead onboard NIC can still
# claim eth0, and USB adapters show up a few seconds after boot.

# physical, non-wireless interfaces (skips lo, bridges, VLANs, tunnels)
wired() {
    for d in /sys/class/net/*; do
        [ -e "$d/device" ] && [ ! -d "$d/wireless" ] && echo "${d##*/}"
    done
}

dhcp() {
    [ -e "/var/run/dhcp.$1" ] && return
    : > "/var/run/dhcp.$1"
    echo "  DHCP on $1..."
    ifconfig "$1" up
    # stays running to renew the lease; retries while there's no link
    udhcpc -f -S -i "$1" -s /etc/udhcpc.script -A 10 >/dev/null 2>&1 &
}

case "$1" in
    start)
        ifconfig lo 127.0.0.1 netmask 255.0.0.0 up
        for i in $(wired); do dhcp "$i"; done
        # pick up late (USB) adapters for the first 30 seconds
        (
            n=0
            while [ $n -lt 30 ]; do
                sleep 1; n=$((n + 1))
                for i in $(wired); do dhcp "$i"; done
            done
        ) &
        # give DHCP a moment so the boot banner can show the address
        n=0
        while [ $n -lt 10 ] && ! ip -4 addr show scope global | grep -q inet; do
            sleep 1; n=$((n + 1))
        done
        ;;
    stop)
        killall udhcpc 2>/dev/null
        for i in $(wired); do ifconfig "$i" down 2>/dev/null; done
        rm -f /var/run/dhcp.*
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
        ifconfig $interface $ip netmask $subnet up
        if [ -n "$router" ]; then
            route del default 2>/dev/null
            for gw in $router; do route add default gw $gw dev $interface; done
        fi
        : > /etc/resolv.conf
        for ns in $dns; do echo "nameserver $ns" >> /etc/resolv.conf; done
        echo "  $interface: $ip (gw: $router)"
        ;;
    deconfig) ifconfig $interface 0.0.0.0 ;;
esac
DHCP
chmod +x "$ROOTFS/etc/udhcpc.script"

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
