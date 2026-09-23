#!/bin/bash
# Build WayangOS base rootfs (BusyBox + Dropbear SSH + curl)
# Output: initramfs image at $BUILD/wayangos-initramfs.img
# Based on v0.5.0 build-rootfs-v2.sh
set -e

BUILD="${BUILD_DIR:-$HOME/wayangos-build}"
ROOTFS="$BUILD/rootfs"
INITRAMFS="$BUILD/wayangos-initramfs.img"
BUSYBOX="$BUILD/busybox-1.37.0/busybox"
BUSYBOX_DIR="$BUILD/busybox-1.37.0"
DROPBEAR_DIR="$BUILD/dropbear-2024.86"

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

# Install applets
cd "$ROOTFS"
for applet in $("$ROOTFS/bin/busybox" --list 2>/dev/null); do
    for dir in bin sbin usr/bin usr/sbin; do
        [ -e "$dir/$applet" ] && continue 2
    done
    ln -sf /bin/busybox "bin/$applet" 2>/dev/null || true
done
cd "$BUILD"

# ============================================
# 2. Dropbear SSH
# ============================================
echo "[2/4] Installing Dropbear SSH..."
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
    [ -f Makefile ] || ./configure --enable-static --disable-zlib --disable-pam --disable-harden \
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
root::0:0:99999:7:::
nobody:!:0:0:99999:7:::
EOF
chmod 640 "$ROOTFS/etc/shadow"

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
export PS1='\[\e[1;33m\]wayangos\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]# '
alias ll='ls -la'
alias ..='cd ..'
echo ""
echo "  ꦮꦪꦁ  WayangOS"
echo "  The Shadow that Powers the Machine"
echo ""
PROFILE

# /etc/inittab
cat > "$ROOTFS/etc/inittab" << 'EOF'
::sysinit:/etc/init.d/rcS
::respawn:-/bin/sh
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

echo "Starting network..."
/etc/init.d/network start

echo "Starting SSH..."
/etc/init.d/sshd start

syslogd -O /var/log/messages -s 200 -b 2
ntpd -p pool.ntp.org -S /bin/true &

echo ""
echo "  ꦮꦪꦁ  WayangOS ready"
ip -4 addr show scope global 2>/dev/null | grep inet | awk '{print "  IP: " $2}'
echo ""
INIT
chmod +x "$ROOTFS/etc/init.d/rcS"

# Shutdown script
cat > "$ROOTFS/etc/init.d/rcK" << 'SHUTDOWN'
#!/bin/sh
echo "WayangOS shutting down..."
killall dropbear 2>/dev/null
killall syslogd 2>/dev/null
killall ntpd 2>/dev/null
ifconfig eth0 down 2>/dev/null
umount -a -r 2>/dev/null
SHUTDOWN
chmod +x "$ROOTFS/etc/init.d/rcK"

# Network init script
cat > "$ROOTFS/etc/init.d/network" << 'NETWORK'
#!/bin/sh
case "$1" in
    start)
        ifconfig lo 127.0.0.1 netmask 255.0.0.0 up
        for iface in eth0 enp0s3 ens3 ens33; do
            if [ -d "/sys/class/net/$iface" ]; then
                echo "  DHCP on $iface..."
                ifconfig $iface up
                udhcpc -i $iface -q -s /etc/udhcpc.script -t 5 -T 3 2>/dev/null &
                break
            fi
        done
        ;;
    stop)
        killall udhcpc 2>/dev/null
        for iface in eth0 enp0s3 ens3 ens33; do
            ifconfig $iface down 2>/dev/null
        done
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
        dropbear -R -E -p 22
        echo "  SSH listening on port 22"
        ;;
    stop) killall dropbear 2>/dev/null ;;
    restart) $0 stop; sleep 1; $0 start ;;
esac
SSHD
chmod +x "$ROOTFS/etc/init.d/sshd"

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
find . | cpio -o -H newc 2>/dev/null | gzip -9 > "$INITRAMFS"

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
