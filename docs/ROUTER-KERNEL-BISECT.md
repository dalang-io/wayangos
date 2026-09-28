# Bisecting the router kernel options

1.0.13 enabled a large "router" kernel block; the test device locked up after
the shell prompt (keyboard + USB uplink both dead). 1.0.15 brought back only
`CONFIG_VLAN_8021Q`, `CONFIG_BRIDGE` and `CONFIG_BRIDGE_VLAN_FILTERING` and was
validated on the device, so the remaining options are the suspects. This is the
plan and tooling to re-enable them safely, one group at a time.

Harness: `scripts/bisect-router-opts.sh`. Background: `docs/INCIDENT-1.0.13.md`
(do not edit that file; it is the owner's incident log).

## Ground rules

- **The harness never reboots or otherwise touches the device.** It only builds
  things on the build box and prints the manual procedure. Rebooting into the
  idle slot is done by hand at the console, because a bad kernel kills SSH.
- **Builds go to a throwaway `/tmp` dir on `root@10.0.0.251`** (default
  `/tmp/wayangos-bisect`). The script refuses to use `/root/wayangos-build` as
  the work dir. Sources are only *symlinked* from `/root/wayangos-build`; the
  kernel tree is *copied* because `build-kernel.sh` builds in-tree, so the
  shared cache is never written to.
- Bundles are signed with `WAYANG_KEY` (the release key) so the device accepts
  them. Without it the script warns and produces an **unsigned** bundle the
  device will reject.
- Only one group per boot. The options create `bond0`, `dummy0`, `ifb0/1`,
  `gre0`, `gretap0`, `erspan0`, `tunl0` at boot; mixing groups makes a lockup
  unattributable.

## Groups

Groups that create **no netdev at boot** are tested first: if one of them locks
the device, the trigger is a driver/crypto path rather than a stray interface.

| # | group | options | boot netdev | why it matters |
|---|-------|---------|-------------|----------------|
| 0 | `baseline` | none (safety net only) | none | proves the watchdog / panic / self-test path on this hardware before any router option is tried |
| 1 | `veth-macvlan-tun` | `VETH`, `MACVLAN`, `TUN` | none | virtual links used by router tunnels/namespaces; created only on demand |
| 2 | `wireguard` | `WIREGUARD` + `CRYPTO_LIB_CHACHA{,_ARCH}`, `CRYPTO_LIB_POLY1305{,_ARCH}`, `CRYPTO_LIB_CURVE25519{,_ARCH}` | none | the x86 arch assembly crypto is a prime suspect for a hard lockup |
| 3 | `vrf-multipath` | `NET_L3_MASTER_DEV`, `NET_VRF`, `IPV6_MULTIPLE_TABLES`, `IPV6_SUBTREES`, `IP_ROUTE_MULTIPATH`, `IP_MULTIPLE_TABLES` | none | policy routing / ECMP / VRF lookup; `IP_MULTIPLE_TABLES` and `IP_ROUTE_MULTIPATH` are already on in the base config; `NET_VRF` needs `NET_L3_MASTER_DEV` (without it `olddefconfig` drops VRF silently — 1.0.13 never had VRF) |
| 4 | `ipsec` | `INET_ESP`, `XFRM_INTERFACE` | none | ESP / xfrm; interfaces are created explicitly |
| 5 | `dummy-bonding` | `DUMMY`, `BONDING` | `dummy0`, `bond0` | dummy + link aggregation devices |
| 6 | `gre-ipip` | `NET_IPIP`, `NET_IPGRE_DEMUX`, `NET_IPGRE`, `NET_UDP_TUNNEL` | `gre0`, `gretap0`, `erspan0`, `tunl0` | IP tunnels |
| 7 | `qos` | `IFB`, `NET_SCH_{HTB,FQ_CODEL,CAKE,INGRESS}`, `NET_CLS_{U32,FW}`, `NET_ACT_{POLICE,MIRRED}`, `NET_REDIRECT` | `ifb0`, `ifb1` | shaping/classification offloads |

The set is the 1.0.13 block **minus** `VLAN_8021Q`, `BRIDGE` and
`BRIDGE_VLAN_FILTERING`, which 1.0.15 already ships. `LLC`/`STP` are pulled in
automatically by `BRIDGE` and do not need their own entries.

## Using the harness

```sh
# list groups/options only, no build
scripts/bisect-router-opts.sh --list

# build one group (writes dist/router-bisect/bisect-<group>-<ver>-x86_64.wup)
WAYANG_KEY=/path/to/release.key scripts/bisect-router-opts.sh --group wireguard

# build every group, in the order above
WAYANG_KEY=/path/to/release.key scripts/bisect-router-opts.sh

# see what would happen without touching anything
scripts/bisect-router-opts.sh --dry-run --group wireguard
```

Useful flags: `--unattended SECS` (see "Remote bisect" below), `--no-safety`,
`--print-opts [GROUP|all|safety]`, `--out DIR`, `--version VER` (default `1.0.99`),
`--kernel-version V` (default `7.2.7`), `--host`, `--work`, `--shared`, `--key`.

For each group the builder:

1. copies `configs/defconfig-intel` to a temporary fragment and appends the
   group's `CONFIG_*=y` lines (the `# WAYANG_BASE:` first line is preserved, so
   `build-kernel.sh` still resolves it as a one-level fragment);
2. runs `scripts/build-kernel.sh`, `scripts/build-rootfs.sh` and
   `scripts/build-bundle.sh` against a `/tmp` build dir;
3. copies the `.wup` back as `bisect-<group>-<ver>-x86_64.wup` and prints the
   device procedure below.

The device procedure is also printed by `--dry-run`, so it can be read without
building anything.

## Device procedure (per bundle)

1. Copy the bundle to the device. There is no `sftp-server`; large uploads can
   drop, so verify the hash:

   ```sh
   sha256sum bisect-<group>-1.0.99-x86_64.wup
   ssh root@163.128.55.3 'cat > /data/bisect.wup' < bisect-<group>-1.0.99-x86_64.wup
   ssh root@163.128.55.3 'sha256sum /data/bisect.wup'
   ```

2. Stage it into the idle slot (no reboot):

   ```sh
   ssh root@163.128.55.3 'wayang update --from /data/bisect.wup'
   ```

3. Reboot into the idle slot **by hand** — pick it in the GRUB menu (3 s
   timeout) or power-cycle. Do **not** run `ssh ... reboot`: if the new kernel
   locks up, that is the only remote path gone.

4. At the console, check the keyboard (Caps Lock LED, then Ctrl-Alt-Del). If
   the box is alive, verify over SSH and let it sit for a few minutes so a late
   lockup shows up:

   ```sh
   ssh root@163.128.55.3 'uname -a; cat /proc/net/dev; dmesg | tail -n 40'
   ```

   Good result: keyboard works, SSH answers, no stray `bond0/dummy0/ifb0/...`
   unless the group is expected to create one, and `active B`/`good B` in
   `wayang status` (after the `6db57a3` updater fix, present since 1.0.14).

5. **Recovery** if it locks up: power-cycle and pick slot A (the last good
   slot) in GRUB. If the menu does not appear, power-cycle and hold the slot-A
   entry. Then, on the device, confirm and re-mark the good slot:

   ```sh
   grep -o 'wayang.slot=[AB]' /proc/cmdline
   wayang update --rollback && wayang mark-ok
   wayang status
   ```

   The updater bug that could mark the wrong slot is fixed by `6db57a3` (1.0.14
   and later); if slot A is running an older release, run the two commands
   above after every manual recovery.

## Interpreting and folding back

Test groups in order until one locks the device, then split that group (halve
its options) and rebuild. Groups that pass can be folded into
`configs/defconfig-intel` immediately, but re-validate the accumulated set on
hardware before publishing:

1. Add the group's options to the router section of
   `configs/defconfig-intel`, with a comment naming the group and the date.
   Keep the file a single fragment (`# WAYANG_BASE: defconfig-qemu` stays the
   first line; do not point it at another fragment).
2. Rebuild the full image and re-test on the device (keyboard + SSH). Some
   symbols are `select`ed by others on x86 (`*_ARCH`, `CURVE25519`, `LLC`,
   `STP`) and can be left for `make olddefconfig` — check the resolved
   `.config` instead of assuming.
3. Publish only after the device boots the accumulated set. Tagging and
   publishing still need the owner's OK (AGENTS.md rule 3).

`WAYANG_KEY` is the release key; the matching public key is baked into the
rootfs (`wayang/trusted_keys`), so only a bundle signed with it is accepted.

## QEMU lab kernel (never shipped)

`scripts/build-lab-kernel.sh` builds, **on the build box**, `defconfig-intel`
plus every group above (`scripts/bisect-router-opts.sh --print-opts all`) into
`bzImage-lab`, in a `/tmp` dir with a copy of the kernel tree. It is for QEMU
only — wayang-router develops WireGuard / QoS / VRF / tunnels against it while
the hardware bisect is pending. **Never stage it on the device or ship it.**

There is no `configs/defconfig-intel-lab` on purpose: `defconfig-intel` is
already a fragment and `build-kernel.sh` resolves one level only. The script
writes a temporary fragment, builds, and checks that every requested option
survived `olddefconfig`.

```sh
# on root@10.0.0.251, from a /tmp checkout of this repo
scripts/build-lab-kernel.sh --out /tmp/wayang-tools/bzImage-lab
BUILD_DIR=/tmp/wayang-lab SSH_AUTHORIZED_KEYS=/tmp/wayangos-tb/testkey.pub \
    scripts/build-rootfs.sh   # with wg/, iproute2/, bird/ staged in /tmp/wayang-lab
qemu-system-x86_64 -enable-kvm -m 1024 -kernel /tmp/wayang-tools/bzImage-lab \
  -initrd /tmp/wayang-tools/initramfs-lab.img -append "console=ttyS0" -display none \
  -netdev user,id=n,hostfwd=tcp:127.0.0.1:2377-:22 -device virtio-net-pci,netdev=n ...
```

Checked in QEMU: `ip link add wg0 type wireguard` + `wg set/show`; `tc` htb +
fq_codel + u32/fw filters, cake, ingress + `police` + `mirred` to `ifb0`;
`bird` starts with a BGP config and opens `/var/run/bird.ctl`.

## Remote bisect (unattended)

The procedure above needs someone at the console, because a bad kernel can
kill the keyboard *and* the USB uplink (1.0.13). The safety net below lets a
bisect bundle be booted remotely and still come back to the good slot on its
own. **Prepared and QEMU-proven only; it has not been used on the device yet.**
Start with the `baseline` group, supervised, before trusting it unattended.

### Pieces

1. **Kernel (bisect builds only, never shipped)** — every group built by
   `scripts/bisect-router-opts.sh` gets, unless `--no-safety`:
   `WATCHDOG_CORE`, `ITCO_WDT` (+`ITCO_VENDOR_SUPPORT`, `LPC_ICH`; the
   i3-7100's Sunrise Point PCH exposes TCO through `i2c-i801`, already `=y`),
   `I6300ESB_WDT` (QEMU), `SOFTLOCKUP_DETECTOR`, `HARDLOCKUP_DETECTOR`,
   `DETECT_HUNG_TASK` (via `DEBUG_KERNEL`), and a built-in command line
   (`CMDLINE_BOOL`, prepended to GRUB's):
   `panic=10 oops=panic softlockup_panic=1 hardlockup_panic=1 hung_task_panic=1 nmi_watchdog=1`.
   `--unattended SECS` also adds `wayang.selftest=SECS` to it. The builder
   warns when any requested option does not survive `olddefconfig`.
2. **Userspace** — `wayang.selftest=SECS` on the cmdline (and only then) makes
   rcS run `/usr/sbin/wayang-selftest start SECS` right after `/data` is mounted:
   - it pets `/dev/watchdog` (BusyBox `watchdog -T 60 -t 10`, else a shell
     loop), so a hard lockup or frozen userspace ends in a hardware reset;
   - it pings a reachability target every 5 s, and at the deadline (min. 30 s)
     requires it to *still* answer. Target: `wayang.selftest_host=IP`, else the
     first line of `/data/etc/selftest.host`, else **the default gateway *and*
     `1.1.1.1`** (many gateways — including the test device's `/32` uplink —
     do not answer ICMP echo, so gateway-only probing would report a healthy
     box as failed);
   - pass → `wayang mark-ok` (rcS skips its own unconditional mark-ok under
     the flag); fail → `wayang update --fallback` (new: points the next boot
     at `wayang_good`; refuses on the good slot itself or without
     `wayang.slot=`) and `reboot -f`;
   - everything is logged to `/data/selftest.log` (with slot and uptime) and
     the console.
   Without the flag nothing changes (no watchdog opened, mark-ok as before).
3. **Boot loader (existing)** — staging (`wayang update --from` or
   `--boot-other`) sets `wayang_slot=<new>`, `wayang_attempts=0` and leaves
   `wayang_good` alone; GRUB counts every boot and falls back to `wayang_good`
   after 3 attempts without a `mark-ok`. So a slot that resets (watchdog,
   panic) instead of reaching the self-test verdict still returns to the good
   slot after three tries.

### Verified in QEMU

`scripts/test-selftest-qemu.sh KERNEL INITRAMFS SSH_KEY [nonet|hang|panic ...]`
(on the build box) builds a real UEFI disk (OVMF, GRUB with
`wayang/grub-disk.cfg`, grubenv, ext4 `/data`), boots slot A, runs the real
`wayang update --boot-other` + `reboot`, and simulates a bad slot B:

| scenario | what B does | path back | result |
|---|---|---|---|
| `nonet` | network never comes up | self-test FAIL at 60 s → `--fallback` → `reboot -f` → A | back on A (PASS, marked good) after 146 s |
| `hang` | SIGSTOP to every process 15 s after boot (incl. the watchdog petter) | i6300esb reset ×3 → GRUB budget → A | B booted 3×, then A (PASS) after 300 s |
| `panic` | `sysrq c` 15 s after boot | `panic=10` reboot ×3 → GRUB budget → A | B booted 3×, then A (PASS) after 169 s |

Kernel: `defconfig-intel` + safety net + `wayang.selftest=60`; final grubenv
`wayang_slot=A wayang_good=A wayang_attempts=0`. Logs of the runs:
`/tmp/wayang-tools/selftest-qemu-evidence.log` on the build box.

### Device procedure (when the owner OKs it)

1. Build `baseline` **supervised** first:
   `WAYANG_KEY=… scripts/bisect-router-opts.sh --group baseline --unattended 600`.
   Boot it with someone at the console and check `dmesg | grep -i -e tco -e wdt`
   (a "failed to reset NO_REBOOT flag, reboot disabled by hardware" line means
   the TCO watchdog cannot reset this board — then only the panic/self-test
   paths are available), `ls /dev/watchdog`, `/data/selftest.log` → PASS.
   Optionally prove the reset once: `kill -STOP $(pidof watchdog)` and wait
   ~60 s for the box to reset and come back.
2. Set a probe target the uplink can always reach, if the gateway may drop
   ICMP: `echo 1.1.1.1 > /data/etc/selftest.host`.
3. Then, per group: `--group NAME --unattended 600`, copy + verify, then
   `ssh root@163.128.55.3 'wayang update --from /data/bisect.wup && reboot'`.
   Wait ~15 min before declaring anything; then read `/data/selftest.log`,
   `/data/debug/` (if the bundle's cmdline had `wayang.debug`) and
   `wayang status` on whatever slot answers.

### Limits

- **Only the self-test window is covered.** A failure that starts *after* the
  PASS (e.g. a USB stall 20 min in) leaves a slot already marked good; the box
  stays unreachable and will keep booting it. Use a long window (600 s or
  more) and let each group soak inside it. The watchdog keeps being petted
  after PASS, so a later *hard* lockup still resets — into the same (now
  "good") slot.
- **Hangs before rcS opens `/dev/watchdog`** (early boot, driver probe) are
  caught only by the lockup/hung-task detectors (`panic=10`); a hang with
  interrupts off on every CPU that the NMI watchdog cannot see stays stuck
  until a power-cycle. The TCO timer is not started by firmware on this box.
- The TCO watchdog may be disabled by the BIOS (NO_REBOOT locked) — check in
  the supervised baseline boot.
- The fallback runs on the *bisect* slot (built from master, which has
  `wayang update --fallback`); the good slot only needs a working `mark-ok`.
  The GRUB budget path needs nothing from userspace at all.
- The probe proves L3 reachability of one host, not SSH; a firewall/sshd
  problem with a live uplink passes. The keyboard cannot be checked remotely.
- Bundles must be signed with the release key (`WAYANG_KEY`), as before.

## Result (2026-09-28)

All **8 groups PASS individually** on the real device (no lockup, watchdog never
fired, SSH alive throughout): baseline, veth-macvlan-tun, wireguard,
vrf-multipath, ipsec, dummy-bonding, gre-ipip, qos. No single group reproduced
the 1.0.13 lockup, so the trigger is an **interaction** (or not only these
options). The full set (plus the Intel TCO / i6300esb watchdog) is now enabled in
`configs/defconfig-intel` and **must be soak-tested as one bundle on the device**
before publishing.

## Interaction check (2026-09-28)

The full accumulated set (all 7 groups + watchdog) was booted with the safety net
and **passed** (3 boots + 30-min soak); the halves pass too — see
[ROUTER-KERNEL-INTERACTION.md](ROUTER-KERNEL-INTERACTION.md#bisect-result--the-interaction-did-not-reproduce-2026-09-28).
No failing subset was found, so the router block stays off pending a reproducer;
the 1.0.20 freeze looks like an intermittent USB-uplink/DHCP stall. The combo
harness (`--groups`, `--opts-file`, `--combo-name`) is in `bisect-router-opts.sh`.
