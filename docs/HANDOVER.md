# HANDOVER — WayangOS state (2026-09-28)

Read `AGENTS.md` first (rules, boot order, build). `docs/TODO.md` is the
pending-work index. **`docs/GOAL.md` is the product goal + roadmap** (what we are
building and the milestones). This file is *where things stand* across the repos.

## Releases / channel / device

| | |
|---|---|
| WayangOS release | **1.0.21** (tag `v1.0.21`; channel live `https://wayang.dalang.io/channel/stable/x86_64`) |
| Test device `root@163.128.55.3` | **1.0.21** (slot A, active/good), stable 6+ min, USB uplink works |
| Tags that exist | `v1.0.18`, `v1.0.19`, `v1.0.21` — **`v1.0.20` was NEVER released** (its kernel froze, see below) |

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
- Current state: the block is **off** in `configs/defconfig-intel` (VLAN/bridge
  MVP). Advanced router features (WireGuard/VRF/ECMP/BGP) run **only in the QEMU
  lab kernel** until this is decided. Details + the decision to make:
  `docs/ROUTER-KERNEL-INTERACTION.md`, `docs/ROUTER-KERNEL-BISECT.md`, and the
  goal/roadmap `docs/GOAL.md` (M7/M8).

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
  the interaction lockup above** — so it does not run on shipped 1.0.21 yet.

## Persistent monitoring (forensics)

- `wayang-fw monitor --daemon` / `wayang-router monitor --daemon` append samples
  + events to `/data/var/<app>/history.jsonl` (rotation + retention); the HUDs
  replay it on open instead of resetting. `/etc/init.d/{fw,router}` start them at
  boot when a confirmed config exists. See `docs/MONITORING.md`.

## Repos & tags

| Repo | HEAD / tag | Notes |
|---|---|---|
| `dalang-io/wayangos` (this) | `master` `5721ec7`, tag `v1.0.21` | kernel reverted, radvd, monitoring, edgerouter apply |
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

1. **Find the kernel interaction** (delta-debug on hardware; see the two docs).
   Then re-enable the minimal safe set in `configs/defconfig-intel`.
2. `radvd` is bundled but RA was only tool-checked, not seen on a device with a
   WG kernel — re-verify once WireGuard ships.
3. EdgeRouter: the box's own-traffic coverage / endpoint recursion / DHCP single
   `/32` gaps are documented in `docs/EDGE-PARITY.md`; phone-home heartbeat UI
   (site "online") still TODO.
4. CI channel publish secret provisioning (`docs/CHANNEL.md`).

## Gotchas

- Build on `root@10.0.0.251`; never write `/root/wayangos-build` for experiments.
- The device's USB `sr9700` uplink is **flaky**: large `.wup` transfers over SSH
  stall; `wayang.dalang.io` serves **no byte-range** (no resume). Prefer the
  device pulling via `wayang update` (single stream), or chunked SSH with retries.
- **Never tag/publish without a device boot test** — 1.0.13 and 1.0.20 froze it.
- The `busybox ip` cannot do multipath/`xfrm`; the labs use a static iproute2 `ip`.
- POS is a separate project; not in `wayang`.
