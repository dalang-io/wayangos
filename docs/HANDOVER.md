# HANDOVER — WayangOS state (2026-09-28)

Read `AGENTS.md` first (rules, boot order, build). `docs/TODO.md` is the
pending-work index. **`docs/GOAL.md` is the product goal + roadmap** (what we are
building and the milestones). This file is *where things stand* across the repos.

## Releases / channel / device

| | |
|---|---|
| WayangOS release | **1.0.22** (tag `v1.0.22`; channel live `https://wayang.dalang.io/channel/stable/x86_64`, serving 1.0.22) |
| Test device `root@163.128.55.3` | **1.0.22** (slot B active/good after the pre-tag boot test + selftest PASS, route up; slot A holds 1.0.21) |
| Tags that exist | `v1.0.18`, `v1.0.19`, `v1.0.21`, `v1.0.22` — **`v1.0.20` was NEVER released** (its kernel froze, see below) |

1.0.22 = 1.0.21 + the **shipped safety net** (watchdog + lockup/panic detectors +
`wayang.selftest=120 panic=10 …` baked into `configs/defconfig-intel`
`CONFIG_CMDLINE`) and the **DHCP-resilience fix** (primary NIC `udhcpc -b`, no
retry give-up; bounded wait-for-carrier). QEMU-proved (`test-selftest-qemu.sh`
nonet/hang/panic) and device-tested before tagging. The router kernel block stays
**off** (VLAN/bridge MVP). CI (see below) now builds on the box itself.

1.0.21 ships the **safe kernel** (VLAN/bridge router MVP only) plus all the new
userspace: `radvd` (IPv6 RA), persistent monitoring daemons, the `wayang` console
(09 EDGEROUTER), `wayang edgerouter apply`, `dcheck`, `nft`/`wg`/`tc`/`bird`.

## ⚠️ Open: router-kernel block (goal-critical) + the freeze is NOT reproduced

- 1.0.13 and 1.0.20 (both with the big "router" kernel block) **froze the
  device**. See `docs/INCIDENT-1.0.13.md`.
- But a **delta-debug on 2026-09-28 did NOT reproduce it**: the full accumulated
  block (all 7 groups + watchdog) **passed 3 boots + a 30-min soak**; both halves
  pass too, so there is **no failing subset**. The freeze's `dmesg.boot` is clean
  and its selftest failed with **`gw=''` (no default route)`** → the signature is
  a **network/USB-uplink stall**, not a CPU lockup. The shipped 1.0.20 differed
  only by lacking the safety-net cmdline (no auto-recovery).
- Working conclusion: **intermittent/environmental** (cold-boot USB-NIC/DHCP
  stall), not a deterministic kernel-option interaction.
- **Leading hypothesis (unconfirmed — the boot-count runs + hw session settle
  it): the "freeze" was a DHCP stall.** The rootfs used to run a synchronous
  `udhcpc -n -q -t 5 -T 3` (~15 s give-up) on the primary interface; the public
  uplink is a DHCP `/32` lease over a flaky USB SR9700, so a slow cold-boot
  lease left the box alive but unreachable (`gw=''`). **Fixed on master**
  (`765ee81`, unreleased): primary DHCP is now `udhcpc -b` (retries forever)
  after a bounded 10 s wait-for-carrier, so a late lease still applies.
- **The safety net now ships in the kernel** (`61c768f`, unreleased):
  `configs/defconfig-intel` bakes the watchdogs (ITCO_WDT = the P320's PCH,
  I6300ESB for the QEMU proof, soft watchdog fallback), soft/hard-lockup +
  hung-task detectors and `CONFIG_CMDLINE` (`panic=10 oops=panic …
  nmi_watchdog=1 wayang.selftest=120`) into the kernel cmdline; grub template
  unchanged. `scripts/test-selftest-qemu.sh` scenarios nonet/hang/panic all
  PASS with the net coming from the shipped config. A stalled boot now
  self-recovers (selftest FAIL → fallback → reboot) instead of stranding a
  headless box.
- Current state: the block is **off** in `configs/defconfig-intel` (VLAN/bridge
  MVP). Advanced router features (WireGuard/VRF/ECMP/BGP) run **only in the QEMU
  lab kernel** until this is decided. The decision is no longer a path pick —
  it is data-driven: run the **boot-count matrix** (block-kernel vs safe-kernel,
  N≥12 boots each, `scripts/boot-soak.sh`, ready-to-fire commands in
  `docs/ROUTER-KERNEL-BISECT.md`) + the hardware session
  ([docs/HW-SESSION-CHECKLIST.md](HW-SESSION-CHECKLIST.md)), then apply the
  decision rule in `docs/TODO-M7-UNBLOCK.md`. Details:
  `docs/ROUTER-KERNEL-INTERACTION.md`, and the goal/roadmap `docs/GOAL.md` (M7/M8).

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
  user `wosrunner`, label `[self-hosted, wosbuild]`) — ~5 min warm. Three jobs:
  `kernel` ∥ `tools` (10 builds concurrently) → `installer` (fan-in: rootfs →
  ISO → signed `.wup` → release). `SKIP_KERNEL=1` keeps the installer job from
  re-downloading the kernel source it never compiles.
- CI stages wifi tools + firmware (`stage-firmware.sh` from
  `/home/wosrunner/firmware-src`) and `dcheck`, so CI artifacts match manual
  builds. Cache keys are hash-of-inputs. The runner has no sudo (apt step no-op).
- Channel publish is **best-effort in CI** and still needs repo secrets
  `WAYANG_DEPLOY_HOST` + `WAYANG_DEPLOY_KEY`; until then publish by hand
  (`rsync` the `channel/stable/x86_64/{manifest.json,wayang-*.wup}` into
  `/root/wayang.dalang.io/public/channel/stable/x86_64/`). See `docs/CHANNEL.md`.

## Repos & tags

| Repo | HEAD / tag | Notes |
|---|---|---|
| `dalang-io/wayangos` (this) | `master` @ 1.0.22, tag `v1.0.22` | released 1.0.22 (safety net + DHCP resilience); CI is self-hosted on the build box |
| `dalang-io/wayang-fw` | `master` `a8518fc`, tag `v0.3.0` | DROPS, schedules, hairpin, FortiOS import, NAT66, VIP fix, `docs/NFT-REF.md`, monitor |
| `dalang-io/wayang-router` | `master` `3027eed`, tag `v0.2.0` | self-managed WG, tunnel-as-uplink (`onlink`), weighted ECMP, VRF, BGP/OSPF, radvd, multi-WAN, `docs/ROUTING-TECH.md` + `DAEMONS-TECH.md`, QEMU labs |
| `dalang-io/dcheck` | `master` `33dc1af` | health list + Prometheus + undelete/macOS |
| `dalang-io/wayangi` (dashboard) | `main` `b8b2227` | WayangOS unit type + self-managed WG + `docs/EDGE-PARITY.md`, deployed to prod |

## Tech references written this round (read these before editing configs)

- wayang-router `docs/ROUTING-TECH.md` (WireGuard/iproute2/VRF/ECMP/proxy_arp/IPv6)
- wayang-router `docs/DAEMONS-TECH.md` (BIRD/radvd/udhcpd)
- wayang-fw `docs/NFT-REF.md` (nftables)
- wayangi `docs/EDGE-PARITY.md` (RouterOS `.rsc` → WayangOS) + `docs/edge-wayangos.md`

## Not done / next steps

1. **Decide the router-kernel block by boot counts** — there is no failing
   subset to find. Run the T3 matrix (block-kernel vs safe-kernel, N≥12 boots
   each, `scripts/boot-soak.sh`; commands in `docs/ROUTER-KERNEL-BISECT.md`;
   owner OK required) + the hardware session
   ([docs/HW-SESSION-CHECKLIST.md](HW-SESSION-CHECKLIST.md)), then apply the
   decision rule ([docs/TODO-M7-UNBLOCK.md](TODO-M7-UNBLOCK.md)). On a green
   light, re-enable the block in `configs/defconfig-intel`.
2. **Ship the landed-but-unreleased fixes in the next tag**: DHCP resilience
   (`765ee81`) + the shipped safety net (`61c768f`) — they fix a real product
   risk (a headless box stranded by a ~15 s DHCP race) independent of the M7
   decision ([docs/TODO-M7-UNBLOCK.md](TODO-M7-UNBLOCK.md)). Nothing after
   1.0.22 is tagged; never tag without a device boot test (gotcha below).
3. `radvd` is bundled but RA was only tool-checked, not seen on a device with a
   WG kernel — re-verify once WireGuard ships.
4. EdgeRouter: the box's own-traffic coverage / endpoint recursion / DHCP single
   `/32` gaps are documented in `docs/EDGE-PARITY.md`; phone-home heartbeat UI
   (site "online") still TODO.
5. CI channel publish secret provisioning (`docs/CHANNEL.md`).

## Gotchas

- Build on `root@10.0.0.251`; never write `/root/wayangos-build` for experiments.
- The device's USB `sr9700` uplink is **flaky**: large `.wup` transfers over SSH
  stall; `wayang.dalang.io` serves **no byte-range** (no resume). Prefer the
  device pulling via `wayang update` (single stream), or chunked SSH with retries.
- **Never tag/publish without a device boot test** — 1.0.13 and 1.0.20 froze it.
- The `busybox ip` cannot do multipath/`xfrm`; the labs use a static iproute2 `ip`.
- POS is a separate project; not in `wayang`.
