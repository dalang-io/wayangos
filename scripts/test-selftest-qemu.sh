#!/bin/bash
# QEMU proof of the remote-bisect safety net (docs/ROUTER-KERNEL-BISECT.md,
# "Remote bisect (unattended)"). Runs on the build box; touches no device.
#
# Builds a real UEFI A/B disk (OVMF + GRUB + wayang/grub-disk.cfg + grubenv +
# ext4 /data), boots slot A, stages the other slot with the real
# `wayang update --boot-other`, reboots into a *simulated bad* slot B and checks
# the VM comes back on A by itself:
#
#   nonet  B boots but its network never comes up -> wayang-selftest fails at
#          the deadline -> `wayang update --fallback` + `reboot -f` -> A.
#   hang   B freezes all userspace 15 s after boot (SIGSTOP to every process,
#          including the watchdog petter) -> the i6300esb hardware watchdog
#          resets the VM -> GRUB retries B until its 3-attempt budget is spent
#          -> A.
#   panic  B panics the kernel (sysrq c) -> panic=10 reboots -> GRUB budget -> A.
#
# Both slots get the same kernel (built with the bisect safety net and
# wayang.selftest in its built-in cmdline) and the same initramfs plus a small
# test hook: every boot is appended to /data/boots.log, and the failure mode
# in /data/selftest-sim is simulated only when running slot B.
#
# Usage: scripts/test-selftest-qemu.sh KERNEL INITRAMFS SSH_KEY [SCENARIO...]
#        (default scenarios: nonet hang)
# Env:   WORK (default /tmp/wayang-selftest-qemu), PORT (default 2388)
set -euo pipefail

KERNEL="${1:?usage: $0 KERNEL INITRAMFS SSH_KEY [SCENARIO...]}"
INITRAMFS="${2:?initramfs}"
KEY="${3:?ssh private key (its .pub must be in the initramfs)}"
shift 3
SCENARIOS=("$@")
[ ${#SCENARIOS[@]} -gt 0 ] || SCENARIOS=(nonet hang)
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORK="${WORK:-/tmp/wayang-selftest-qemu}"
PORT="${PORT:-2388}"
OVMF_CODE=/usr/share/OVMF/OVMF_CODE_4M.fd
OVMF_VARS=/usr/share/OVMF/OVMF_VARS_4M.fd

case "$WORK" in /tmp/*) ;; *) echo "ERROR: WORK must be under /tmp" >&2; exit 1 ;; esac
for t in qemu-system-x86_64 grub-mkstandalone mkfs.vfat mcopy mkfs.ext4 sgdisk cpio; do
    command -v "$t" >/dev/null || { echo "ERROR: $t missing" >&2; exit 1; }
done

rm -rf "$WORK"
mkdir -p "$WORK"
cd "$WORK"

SSH=(ssh -q -i "$KEY" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
     -o ConnectTimeout=3 -o BatchMode=yes -p "$PORT" root@127.0.0.1)
vm() { "${SSH[@]}" "$@"; }
log() { printf '[%s] %s\n' "$(date +%T)" "$*"; }

# --- test initramfs: the given one + boot log + slot-B failure simulation ---
mkdir root
(cd root && gzip -dc "$INITRAMFS" | cpio -id --quiet)
mv root/etc/init.d/network root/etc/init.d/network.real
cat > root/etc/init.d/network << 'EOF'
#!/bin/sh
# test hook: slot B with /data/selftest-sim = nonet never brings the network up
slot="$(grep -o 'wayang.slot=[AB]' /proc/cmdline)"
if [ "$slot" = wayang.slot=B ] && [ "$(cat /data/selftest-sim 2>/dev/null)" = nonet ]; then
    [ "$1" = start ] && echo "  [sim] network disabled on slot B"
    [ "$1" = wait ] && exit 1
    exit 0
fi
exec /etc/init.d/network.real "$@"
EOF
chmod +x root/etc/init.d/network
cat >> root/etc/init.d/rcS << 'EOF'

# --- test hook (scripts/test-selftest-qemu.sh) ---
echo "$(date '+%F %T') boot $(grep -o 'wayang.slot=[AB]' /proc/cmdline) wdt=$([ -c /dev/watchdog ] && echo yes || echo no)" >> /data/boots.log
if grep -q 'wayang.slot=B' /proc/cmdline; then
    case "$(cat /data/selftest-sim 2>/dev/null)" in
        hang)  sync; ( sleep 15; echo "[sim] freezing userspace" > /dev/console; sync; kill -STOP -1 ) & ;;
        panic) sync; ( sleep 15; sync; echo c > /proc/sysrq-trigger ) & ;;
    esac
fi
EOF
(cd root && find . | cpio -o -H newc -R 0:0 2>/dev/null | gzip -1 > ../initramfs-test.img)

# --- UEFI A/B disk: p1 ESP (WAYANGBOOT), p2 ext4 (WAYANGDATA) ---
cat > embedded.cfg << 'EOF'
insmod part_gpt
insmod fat
insmod loadenv
insmod test
search --no-floppy --label --set=root WAYANGBOOT
configfile ($root)/boot/grub/grub.cfg
EOF
grub-mkstandalone -O x86_64-efi -o BOOTX64.EFI --locales="" --fonts="" --themes="" \
    "boot/grub/grub.cfg=embedded.cfg"
{
    printf '# GRUB Environment Block\nwayang_slot=A\nwayang_good=A\nwayang_attempts=0\n'
} > grubenv
pad=$((1024 - $(wc -c < grubenv)))
printf '%*s' "$pad" '' | tr ' ' '#' >> grubenv

rm -f esp.img
mkfs.vfat -C -n WAYANGBOOT esp.img $((200 * 1024)) >/dev/null
export MTOOLS_SKIP_CHECK=1
mmd -i esp.img ::/EFI ::/EFI/BOOT ::/boot ::/boot/grub ::/boot/A ::/boot/B
mcopy -i esp.img BOOTX64.EFI ::/EFI/BOOT/BOOTX64.EFI
mcopy -i esp.img "$REPO_DIR/wayang/grub-disk.cfg" ::/boot/grub/grub.cfg
mcopy -i esp.img grubenv ::/boot/grub/grubenv
for s in A B; do
    mcopy -i esp.img "$KERNEL" ::/boot/$s/vmlinuz
    mcopy -i esp.img initramfs-test.img ::/boot/$s/initramfs.img
done
mkdir -p datafs/etc/ssh
cp "$KEY.pub" datafs/etc/ssh/authorized_keys
truncate -s 64M data.img
mkfs.ext4 -q -F -L WAYANGDATA -d datafs data.img

truncate -s 300M disk.img
sgdisk -n 1:2048:+200M -t 1:ef00 -n 2:0:+64M -t 2:8300 disk.img >/dev/null
p1="$(sgdisk -i 1 disk.img | awk '/First sector/ {print $3}')"
p2="$(sgdisk -i 2 disk.img | awk '/First sector/ {print $3}')"
dd if=esp.img of=disk.img bs=512 seek="$p1" conv=notrunc status=none
dd if=data.img of=disk.img bs=512 seek="$p2" conv=notrunc status=none
cp "$OVMF_VARS" vars.fd

esp_env() { # print the grubenv from the (stopped or running) disk
    dd if=disk.img of=esp-now.img bs=512 skip="$p1" count=$((200 * 2048)) status=none
    mtype -i esp-now.img ::/boot/grub/grubenv | tr '#' '\n' | grep '^wayang_' | tr '\n' ' '
    echo
}

log "booting VM (UEFI, i6300esb watchdog, action reset)"
qemu-system-x86_64 -enable-kvm -m 1024 -smp 2 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file=vars.fd \
    -drive file=disk.img,format=raw,if=none,id=d0 -device nvme,serial=st,drive=d0 \
    -device i6300esb -action watchdog=reset \
    -netdev user,id=n,hostfwd=tcp:127.0.0.1:"$PORT"-:22 -device virtio-net-pci,netdev=n \
    -display none -serial file:serial.log -monitor unix:mon.sock,server,nowait \
    -daemonize -pidfile qemu.pid
trap 'kill "$(cat "$WORK/qemu.pid" 2>/dev/null)" 2>/dev/null || true' EXIT

wait_ssh() { # SECONDS
    local i=0
    while [ $i -lt "$1" ]; do vm true 2>/dev/null && return 0; sleep 3; i=$((i + 3)); done
    return 1
}
wait_slot_pass() { # SLOT SECONDS: SSH up on SLOT and its self-test PASSed this boot
    local end=$(( $(date +%s) + $2 ))
    while [ "$(date +%s)" -lt "$end" ]; do
        if out="$(vm "grep -o 'wayang.slot=[AB]' /proc/cmdline; tail -n 1 /data/selftest.log" 2>/dev/null)"; then
            case "$out" in
                "wayang.slot=$1"*"slot=$1 PASS"*) return 0 ;;
            esac
        fi
        sleep 5
    done
    return 1
}

fail=0
wait_ssh 120 || { log "FAIL: VM never came up"; exit 1; }
wait_slot_pass A 180 || { log "FAIL: slot A self-test did not pass"; exit 1; }
# shellcheck disable=SC2016  # expands in the VM
log "slot A up, self-test PASS; grubenv: $(vm 'mkdir -p /mnt/e; mount -t vfat $(blkid | grep WAYANGBOOT | cut -d: -f1) /mnt/e 2>/dev/null; tr "#" "\n" < /mnt/e/boot/grub/grubenv | grep ^wayang_ | tr "\n" " "; umount /mnt/e')"

for sc in "${SCENARIOS[@]}"; do
    log "=== scenario $sc: boot-other into slot B (simulated $sc) ==="
    vm "echo $sc > /data/selftest-sim; sync; wayang update --boot-other" || true
    t0=$(date +%s)
    vm 'reboot' || true
    sleep 20
    budget=600
    if wait_slot_pass A "$budget"; then
        log "PASS: back on slot A after $(( $(date +%s) - t0 ))s"
    else
        log "FAIL: not back on a passing slot A within ${budget}s"
        fail=1
        break
    fi
    vm 'echo "--- boots.log"; tail -n 6 /data/boots.log; echo "--- selftest.log"; tail -n 8 /data/selftest.log; echo "--- status"; wayang status' | sed 's/^/    /'
    vm 'rm -f /data/selftest-sim; sync'
done

vm 'sync; poweroff -f' >/dev/null 2>&1 || true
sleep 3
kill "$(cat qemu.pid 2>/dev/null)" 2>/dev/null || true
log "final grubenv on disk: $(esp_env)"
if [ "$fail" = 0 ]; then log "ALL SCENARIOS PASSED"; else log "SOME SCENARIOS FAILED"; fi
exit "$fail"
