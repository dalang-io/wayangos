# HANDOVER — WayangOS state for the next agent

Read `AGENTS.md` first (rules, boot order, build). `docs/TODO.md` is the
pending-work index. This file is *where things stand* across the repos.

## Repos & current releases

| Repo | What | State |
|---|---|---|
| `dalang-io/wayangos` (this) | the OS: kernel configs, rootfs, ISO, updater | tag **v1.0.17** |
| `dalang-io/wayang-fw` | firewall (nftables) + HUD/CLI | master (v0.2.2 + DROPS/schedules/hairpin, untagged) |
| `dalang-io/wayang-router` | router (VLAN/bridge/static/DHCP) + HUD/CLI | master (v0.1.0 + netlink backend, untagged) |
| `dalang-io/dcheck` | storage-health app (bundled in the image) | master (`dcheck-v0.5.1` + undelete/macOS, untagged) |
| `dalang-io/wayang-pos` (private) | WayangPOS kiosk app, **separate project** | `v3.2.3` |

Local checkouts: `~/wayang-fw`, `~/wayang-router`, `~/dcheck`.

## Test device `root@163.128.55.3` (public IP — careful)

- WayangOS **1.0.17**, A/B: the newest slot is active/good.
- `/data/bin`: `wayang-fw` (0.2.2) + `wayang-router` (0.1.0), deployed, **nothing
  committed** for either. rcS links both into `/usr/bin` at boot → runnable from
  the CLI (`wayang-fw`, `wayang-router`).
- Network: uplink = USB `sr9700` (`eth1`), public `/32` (on-link route handled
  automatically); onboard `eth0` (e1000e) is DHCP-silent behind this router.
- SSH is flaky/slow; retry. **No `sftp-server`** → copy with
  `ssh host 'cat > FILE' < FILE`, and verify sha256 for big files.
- **Do not `ssh … reboot` a candidate kernel blindly** — if it locks there is no
  remote way back; use the GRUB menu at the console. See
  `docs/INCIDENT-1.0.13.md` and `docs/ROUTER-KERNEL-BISECT.md`.

## What is done (recent)

- WayangOS **1.0.17**: console UX/perf, SSH-key TUI, "also lease" secondary NICs,
  DCHECK module, POS removed, router VLAN/bridge MVP, CI channel-publish step
  (gated on secrets).
- `wayang` console modules: **01 SYSTEM · 02 UPDATES · 03 NETWORK · 04 WIFI ·
  05 SSH · 06 DCHECK · 07 FIREWALL · 08 ROUTER**. Determinate update progress;
  `b` = one-shot boot-other-slot.
- wayang-fw: **DROPS** screen, rule **schedules**, **hairpin NAT**.
- wayang-router: **rtnetlink** backend (addr/link/VLAN/bridge).
- dcheck: `undelete` follows `$ATTRIBUTE_LIST` + `$I30` names, `--carve --free`;
  macOS backends for recover/verify.
- Router kernel bisect harness ready (hardware-gated), see TODO.
- **Router userspace bundled** (master, unreleased): static `wg`
  (wireguard-tools 1.0.20260223, 1016K), `tc` (iproute2 7.2.0, 1.5M), `bird`
  (BIRD 2.19.2, 2.3M; no birdc — no static readline on the builder); initramfs
  +2.05 MiB gzip (21.9 → 24.1 MB). `/etc/init.d/router` starts bird only with
  `/data/etc/router/bird.conf`.
- **QEMU lab kernel** `scripts/build-lab-kernel.sh` (full 1.0.13 block, never
  shipped). On the builder: `/tmp/wayang-tools/{wg,tc,bird,bzImage-lab,initramfs-lab.img}`
  (initramfs has `/tmp/wayangos-tb/testkey.pub`).
- **Remote bisect safety net** (prepared, QEMU-proven, not used on the device):
  `bisect-router-opts.sh --group baseline --unattended 600`, `wayang-selftest`,
  `wayang update --fallback`; see `docs/ROUTER-KERNEL-BISECT.md` → "Remote
  bisect (unattended)" and `scripts/test-selftest-qemu.sh`.

## What is pending (see docs/TODO.md for detail)

- **Router kernel bisect on hardware** (the big one): re-enable the 1.0.13
  option groups one at a time, booting the device and checking keyboard + SSH.
  Groups + procedure in `docs/ROUTER-KERNEL-BISECT.md`. Start with the
  supervised `baseline` group (safety net only); once it proves the TCO
  watchdog/self-test on the device, the rest can run unattended.
- CI: provision `WAYANG_DEPLOY_HOST` + `WAYANG_DEPLOY_KEY` so a tag publishes the
  channel (currently manual `scripts/publish-channel.sh`).
- wayang-fw roadmap: per-zone DHCP/DNS (dnsmasq not bundled), nft sets/FQDN,
  interface config in the HUD; enforcement is QEMU-tested only.
- wayang-router roadmap: WireGuard, IPsec, VRF/policy/ECMP, QoS, BGP/OSPF —
  userspace (`wg`, `tc`, `bird`) is in the image; kernel-gated on the bisect
  (develop in QEMU with the lab kernel).
- dcheck: remaining undelete/recover gaps (listed in `~/dcheck/TODO.md`).

## Gotchas

- Build on `root@10.0.0.251`; **never write to `/root/wayangos-build`** for
  experiments — use a `/tmp` dir with symlinks to the sources and a copy of the
  kernel tree. `scripts/build-remote.sh` uses `/root/wayangos-build` for
  releases (fine).
- `build-kernel.sh` resolves **one** fragment level only (`defconfig-intel` is a
  fragment of `defconfig-qemu`).
- Kernel console font has no box-drawing/Javanese glyphs — the boot splash is
  plain ASCII on purpose (`wayang-splash`).
- Tagging `v*` publishes (CI builds the ISO + GitHub release); the update
  channel is published by hand. **Ask the owner before tagging.**
- POS is gone from `wayang`; don't reintroduce it (separate project).
- aarch64 musl rust target is not installed on the builder — the `wayang` ARM
  build is skipped (harmless).
