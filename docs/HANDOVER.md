# HANDOVER — WayangOS state (2026-09-28)

Read `AGENTS.md` first (rules, boot order, build). `docs/TODO.md` is the
pending-work index. **`docs/GOAL.md` is the product goal + roadmap** (what we are
building and the milestones). This file is *where things stand* across the repos.

## Releases / channel / device

| | |
|---|---|
| WayangOS release | **1.0.27** (tag `v1.0.27`; channel live `https://wayang.dalang.io/channel/stable/x86_64`, serving 1.0.27). 1.0.23+ carries the **full router block**; 1.0.25 removed `wayang-selftest`; 1.0.26 added `wayang reset`; 1.0.27 makes all wired NICs hotplug-DHCP |
| Test device `root@163.128.55.3` | fresh **1.0.27** (updated in place via `wayang update`), reachable over the USB LAN (`ssh root@192.168.2.2`) and the public uplink; uplink on the onboard `eth0` |
| Tags that exist | `v1.0.18`, `v1.0.19`, `v1.0.21`, `v1.0.22`, `v1.0.23`, `v1.0.24`, `v1.0.25`, `v1.0.26`, `v1.0.27` — **`v1.0.20` was NEVER released** (its kernel froze, see below) |

1.0.22 = 1.0.21 + the **shipped safety net** (watchdog + lockup/panic detectors +
`wayang.selftest=120 panic=10 …` baked into `configs/defconfig-intel`
`CONFIG_CMDLINE`) and the **DHCP-resilience fix** (primary NIC `udhcpc -b`, no
retry give-up; bounded wait-for-carrier). QEMU-proved (`test-selftest-qemu.sh`
nonet/hang/panic) and device-tested before tagging. The router kernel block is
now **enabled** (M7 resolved — see below). CI (see below) builds on the box itself.

1.0.21 ships the **safe kernel** (VLAN/bridge router MVP only) plus all the new
userspace: `radvd` (IPv6 RA), persistent monitoring daemons, the `wayang` console
(09 EDGEROUTER), `wayang edgerouter apply`, `dcheck`, `nft`/`wg`/`tc`/`bird`.

## ✅ Resolved: router-kernel block ENABLED (M7, 2026-09-29)

- 1.0.13 and 1.0.20 (both with the big "router" kernel block) **froze the
  device**. See `docs/INCIDENT-1.0.13.md`.
- 2026-09-28 delta-debug: the full accumulated block (all 7 groups + watchdog)
  **passed 3 boots + a 30-min soak** — no failing subset; the freeze's signature
  was a **network/USB-uplink stall** (`gw=''`, clean dmesg), not a CPU lockup.
- **M7 decision (2026-09-29): ENABLE the block.** A device boot-count soak of the
  block kernel ran **~30 consecutive clean boots** (connectivity verified at
  up=47 s) with **one** unreproduced first-boot hang (coinciding with the SR9700
  link flap at ~14 s); the fresh-staging first boot and every later boot were
  clean. Enabling is covered by two already-landed mitigations:
  - **DHCP fix** (`765ee81`): primary `udhcpc -b` (retries forever) after a
    bounded 10 s wait-for-carrier — removes the `gw=''` stall class.
  - **Shipped safety net** (`61c768f`): watchdogs (ITCO_WDT = the P320's PCH,
    I6300ESB for QEMU, softdog fallback) + lockup/hung-task detectors +
    `CONFIG_CMDLINE` (`panic=10 oops=panic … wayang.selftest=120`); a hang now
    self-recovers (watchdog reset / selftest fallback) instead of stranding a
    headless box. QEMU-proved (`test-selftest-qemu.sh` nonet/hang/panic).
- **State**: the full 7-group block is **enabled** in `configs/defconfig-intel`
  (`106bb23`). M8 (EdgeRouter E2E) is unblocked. **Remaining before a tag**: a
  real **cold-boot** (power-cycle) soak with the owner at the device —
  `docs/HW-SESSION-CHECKLIST.md`. Details: `docs/GOAL.md` (M7),
  `docs/TODO-M7-UNBLOCK.md`, `docs/ROUTER-KERNEL-BISECT.md`.

## WayangOS as a wayangi "Edge" unit type (dashboard)

End-to-end operations: [docs/EDGEROUTER-RUNBOOK.md](EDGEROUTER-RUNBOOK.md).

Goal: a WayangOS box becomes an Edge router on the wayangi dashboard — exactly
like the MikroTik `.rsc`, but the box **self-manages WireGuard from a generated
config** (no wayangi agent on the box).

- **wayangi (dashboard)** now has a **WayangOS unit type**: `internal/edgewos`
  renders `router.toml` + `fw.toml` + `install.sh` (writes `keys/<hub>.key` 0600).
  It reuses edgerb's facts/hub provisioning (keys + transit from the `edge_sites`
  row, both hubs provisioned exactly like RB). Models: `wos-x86-2/3/4/5`
  (4 NIC = 3 WAN + public LAN). Gated by `WAYANGI_EDGE_WOS=1`.
- **Deployed to prod**: the production hub (`ssh wayangi-hub`, `root@[2001:df6:d2c0::14]`)
  runs the new binary (contains `wos-x86-*`) and `/etc/wayangi/env` has
  `WAYANGI_EDGE_WOS=1` (+ the EdgeRB hubs/keys). The dropdown at
  `https://wayangi.dalang.io/admin/edge` shows the WayangOS models.
- **Sim site + bundle**: `~/dev/wayangi/scripts/dev-sim-wayangos.sh` creates
  site `sim-wayangos` (model `wos-x86-4`, public `…` , WG keys) and writes
  `~/dev/wayangi/dist/sim-wayangos.bundle.tar.gz` (gitignored).
- **On a box**: `wayang edgerouter apply sim-wayangos.bundle.tar.gz`
  (`wayang edgerouter status|enroll|clear|start|stop|restart` also exist).
- **Proven end-to-end in QEMU against the PRODUCTION hubs**: a pure-config
  WayangOS box connected to both hubs, served a public `/32` to a LAN host,
  inbound+outbound, failover JKT→MLB, hubs restored.
- **Caveat**: this needs the WireGuard/VRF kernel options, which are **gated by
  the open router-kernel question above** — so it does not run on shipped 1.0.22 yet.

## Persistent monitoring (forensics)

- `wayang-fw monitor --daemon` / `wayang-router monitor --daemon` append samples
  + events to `/data/var/<app>/history.jsonl` (rotation + retention); the HUDs
  replay it on open instead of resetting. `/etc/init.d/{fw,router}` start them at
  boot when a confirmed config exists. See `docs/MONITORING.md`.

## CI / release (self-hosted)

- Tagging `v*` (or `workflow_dispatch`) runs `.github/workflows/installer-iso.yml`
  **on the build box** via two self-hosted runners (`wosbuild-a`/`wosbuild-b`,
  user `wosrunner`, label `[self-hosted, wosbuild]`) — **~4m45s warm** (was
  ~30 min on GitHub-hosted). Jobs: `kernel` ∥ `tools` (10 builds concurrent),
  `tools → rootfs`, then `release` fans in kernel+rootfs+tools and builds the
  ISO ∥ signed `.wup` concurrently → GitHub release; `publish-channel` follows.
  `SKIP_KERNEL=1` keeps rootfs/release from re-downloading kernel source, and a
  kernel-tree existence guard avoids a 693 MB cache re-download when the
  persistent runner workspace already has the tree.
- CI stages wifi tools + firmware (from `/home/wosrunner/firmware-src`) and
  `dcheck`, so CI artifacts match manual builds. Cache keys are hash-of-inputs.
  The runner has no sudo (apt step is a no-op).
- Channel publish **runs in CI**: repo secrets `WAYANG_DEPLOY_HOST`
  (`root@163.128.54.5`) + `WAYANG_DEPLOY_KEY` (a dedicated ed25519 key,
  authorized in the box's `/root/.ssh/authorized_keys` — it is root-level, so
  harden with an `rrsync`/`restrict` command or move publish to a self-hosted
  runner). Republish an existing version without rebuilding: `workflow_dispatch`
  with `publish_channel_version=<ver>`. See `docs/CHANNEL.md`.

## Repos & tags

| Repo | HEAD / tag | Notes |
|---|---|---|
| `dalang-io/wayangos` (this) | `master` @ 1.0.27, tag `v1.0.27` | block enabled; `wayang reset`; selftest removed; hotplug DHCP; CI self-hosted (~5 m, channel-publish secrets set) |
| `dalang-io/wayang-fw` | `master` `a8518fc`, tag `v0.3.0` | DROPS, schedules, hairpin, FortiOS import, NAT66, VIP fix, `docs/NFT-REF.md`, monitor |
| `dalang-io/wayang-router` | `master` `84c796e`, tag `v0.3.2` | EdgeRouter role (public `/32` routed_prefixes+proxy_arp, delegated IPv6 + SLAAC/radvd, v6 policy routing, weighted-ECMP + failover, VRF, multi-WAN, monitor history); **never strand the last uplink** (v0.3.1); **`wan-monitor` → `wan-failover`** (v0.3.2); `engine` hardening (unparseable confirmed config = error); `docs/ROUTING-TECH.md` + `DAEMONS-TECH.md`, QEMU labs |
| `dalang-io/dcheck` | `master` `33dc1af` | health list + Prometheus + undelete/macOS |
| `dalang-io/wayangi` (dashboard) | `main` `b8b2227` | WayangOS unit type + self-managed WG + `docs/EDGE-PARITY.md`, deployed to prod |

## Tech references written this round (read these before editing configs)

- wayang-router `docs/ROUTING-TECH.md` (WireGuard/iproute2/VRF/ECMP/proxy_arp/IPv6)
- wayang-router `docs/DAEMONS-TECH.md` (BIRD/radvd/udhcpd)
- wayang-fw `docs/NFT-REF.md` (nftables)
- wayangi `docs/EDGE-PARITY.md` (RouterOS `.rsc` → WayangOS) + `docs/edge-wayangos.md`

## Not done / next steps

1. **M8 EdgeRouter on real hardware** — blocked by topology: the Edge model
   assumes a WAN **and** a public LAN, but the test box has essentially a single
   uplink NIC. Either use a box with ≥2 network-capable NICs, or add a
   **1-NIC WAN-only model** to wayangi (open design decision: where the public
   `/32` lives with no LAN) + renderer support (the current renderer requires a
   LAN port).
2. **wayangi**: stop hard-coding `1.1.1.1` as the WAN health probe — a false
   health-check failure on the sole uplink flapped the box (the 2026-09-29
   hardware incident); use the gateway or a configured target. Also: an
   `edge_wos` site kind / DB `class`+`platform`, and WOS-only deployment
   (`EnableEdgeWOS` still hard-requires `EnableEdgeRB`).
3. **Ventoy**: the **1.0.27 ISO is downloaded but not on the USB** (it was
   unplugged) — copy it when re-attached.
4. **wayang-fw**: per-zone DHCP/DNS (dnsmasq not bundled); interface config from
   the HUD; firewall enforcement is QEMU-tested only — validate on real hardware
   (`tests/lab/lab.py`).
5. **wayang-router**: IPsec IKEv2; PPPoE; bridge port-level VLANs; Wi-Fi AP.
6. `radvd` RA was tool-checked only — re-verify on a device with a WG kernel.
7. EdgeRouter own-traffic coverage / endpoint recursion / DHCP single-`/32` gaps
   (`docs/EDGE-PARITY.md`); phone-home "site online" heartbeat UI.
8. `wayang.slot` on first install — verify on a fresh install (older installers
   wrote grub.cfg without it).

## Gotchas

- Build on `root@10.0.0.251`; never write `/root/wayangos-build` for experiments.
- The device's USB `sr9700` NIC is **flaky** (10 Mbps, link flaps) — the uplink
  is now on the onboard **`eth0` (e1000e, 1G)**; don't use the SR9700 as a WAN.
  Large `.wup` transfers over SSH stall; `wayang.dalang.io` serves **no
  byte-range** (no resume). Prefer the device pulling via `wayang update`.
- **Never tag/publish without a device boot test** — 1.0.13 and 1.0.20 froze it.
- The `busybox ip` cannot do multipath/`xfrm`; the labs use a static iproute2 `ip`.
- POS is a separate project; not in `wayang`.
