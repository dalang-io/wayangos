# HANDOVER — WayangOS state (2026-09-28)

Read `AGENTS.md` first (rules, boot order, build). `docs/TODO.md` is the
pending-work index. **`docs/GOAL.md` is the product goal + roadmap** (what we are
building and the milestones). This file is *where things stand* across the repos.

## Releases / channel / device

| | |
|---|---|
| WayangOS release | **1.0.30** (tag `v1.0.30`; channel live `https://wayang.dalang.io/channel/stable/x86_64`, serving 1.0.30). 1.0.23+ carries the **full router block**; 1.0.25 removed `wayang-selftest`; 1.0.26 added `wayang reset`; 1.0.27 makes all wired NICs hotplug-DHCP; **1.0.28–1.0.30 bake `wayang-router` + `wayang-fw` into `/usr/bin`, drop the 09 EDGEROUTER TUI module (8 modules), and open public SSH by default** |
| Test device `root@163.128.55.3` | **1.0.30** (updated in place via `wayang update`); uplink on onboard `eth0`; USB LAN `ssh root@192.168.2.2` (SR9700, duplex-flaky) |
| Prod Edge box `163.128.55.4` | site 5 `ThinkStation-P320-Tiny`, WayangOS **1.0.30**; managed via the hub tunnel (`ssh dell-jkt` → `ssh root@10.99.130.5` / `@163.128.55.4`); public ping + SSH now work. **`163.128.54.4` is a different MikroTik RouterOS — not ours.** |
| Tags that exist | `v1.0.18`, `v1.0.19`, `v1.0.21`, `v1.0.22`, `v1.0.23`, `v1.0.24`, `v1.0.25`, `v1.0.26`, `v1.0.27`, `v1.0.28`, `v1.0.29`, `v1.0.30` — **`v1.0.20` was NEVER released** (its kernel froze, see below) |

1.0.22 = 1.0.21 + the **shipped safety net** (watchdog + lockup/panic detectors +
`wayang.selftest=120 panic=10 …` baked into `configs/defconfig-intel`
`CONFIG_CMDLINE`) and the **DHCP-resilience fix** (primary NIC `udhcpc -b`, no
retry give-up; bounded wait-for-carrier). QEMU-proved (`test-selftest-qemu.sh`
nonet/hang/panic) and device-tested before tagging. The router kernel block is
now **enabled** (M7 resolved — see below). CI (see below) builds on the box itself.

1.0.21 ships the **safe kernel** (VLAN/bridge router MVP only) plus all the new
userspace: `radvd` (IPv6 RA), persistent monitoring daemons, the `wayang`
console, `wayang edgerouter apply`, `dcheck`, `nft`/`wg`/`tc`/`bird`.

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
| `dalang-io/wayangos` (this) | `master` @ 1.0.30, tag `v1.0.30` | block enabled; `wayang reset`; selftest removed; hotplug DHCP; **wayang-router/wayang-fw baked into the rootfs; EDGEROUTER TUI removed; CI self-hosted (~5 m)**; **tool pins now `wayang-router 0.6.0` / `wayang-fw 0.6.0`** |
| `dalang-io/wayang-fw` | `master` `4f5ccc0`, tag **`v0.6.0`** | DROPS, schedules, hairpin, FortiOS import, NAT66, VIP, monitor, per-zone DHCP/DNS; **HUD via the shared `wayang-tui`** (nav grammar, REVIEW, policy wizard, FortiOS import review, focused-pane highlight, **one tab = one full-screen view**, splash/`Ctrl-L` repaint, tty-safe children); **Rust 1.98.1 + edition 2024** |
| `dalang-io/wayang-router` | `master` `834d17e`, tag **`v0.6.0`** | EdgeRouter role (public `/32` routed_prefixes+proxy_arp, delegated IPv6 + SLAAC/radvd, v6 policy routing, weighted-ECMP + failover, VRF, multi-WAN, monitor); bridge port-level VLANs; **v0.3.3 `wan_group` table bring-up fix** (`d576fa8`); **HUD via `wayang-tui`** (nav grammar, REVIEW + no silent primary, WAN wizard, deep `/` jump, full-screen tabs, splash/repaint); **reading API `wayang-router api` (P0)**; **Rust 1.98.1 + edition 2024** |
| `dalang-io/dcheck` | `master` `12d4601`, tag **`dcheck-v0.5.1`** | health list + Prometheus + undelete/macOS; **Rust 1.98.1 + edition 2024** (`unsafe extern "C"`, `allow(collapsible_if)`) |
| `dalang-io/wayang-tui` (new) | `main` `a8c84b5`, tag **`v0.2.1`** | **shared ratatui component library** for the three HUDs: `theme` (palette/modes), `widgets` (`panel`/`panel_focused`/`header`/`footer`/`keycaps`/`badge`/…), `focus`, `overlay`, `term` (OSC-11, alt screen, repaint, tty-safe spawn), `splash`, `transition`, `layout`. `docs/ADD-A-SCREEN.md` |
| `dalang-io/wayangi` (dashboard) | `main` `7ee3170` | WayangOS unit type + self-managed WG + `docs/EDGE-PARITY.md`, deployed to prod; `wos-x86-1` 1-NIC WAN-only model; **public-SSH `allow_from` default**; **N-hub generator** (`docs/EDGE-N-HUB.md`), `docs/FUTURE-WORKS.md` |

## HUD & platform round (2026-09-30)

The consoles + toolchain round, newest first:

- **Shared component library `wayang-tui`** (new repo) — the three HUDs render
  through it, so consistency is *by construction* (not three copies).
- **UX revamp** ([docs/TUI-UX-REVAMP.md](TUI-UX-REVAMP.md)): one navigation
  grammar (`↑↓` within, `←→` between tabs, `all` keys shown); **REVIEW before
  apply** + commit-confirm; focused-pane highlight (colour-free `▸`); **one tab
  = one full-screen view**; policy wizard (fw) + WAN wizard (router); deep `/`
  jump; `?` help + migrant concept map; RECENT.
- **Terminal robustness**: first frame is a **full-screen splash** (framed,
  scaled logotype — no blank/white flash; OSC-11 themed background);
  **self-healing repaint** + `Ctrl-L`; children spawned with `Stdio::null`;
  handing the terminal to a sibling no longer **nests the alternate screen**
  (fixed the leftover blank lines); **raw mode is restored on exit** (fixed the
  “staircase” prompt).
- **Toolchain**: Rust **1.98.1 + edition 2024** across `wayang-tui`, the
  `wayang` CLI, `wayang-fw`, `wayang-router` and `dcheck` (`rust-toolchain.toml`
  + `rust-version`).
- **Released**: `wayang-router v0.6.0` + `wayang-fw v0.6.0` (tags + GitHub +
  mirror `https://wayang.dalang.io/edge/tools/`; OS pins bumped). The `wayang`
  CLI ships inside the OS image (no separate tag).
- **Box `163.128.55.4`** (site 5, `ThinkStation-P320-Tiny`): all four binaries
  under `/data/bin` (persistent) with `/usr/bin` symlinks — `wayang-router
  0.6.0`, `wayang-fw 0.6.0`, `wayang 0.1.0 (ux-p1)`, `dcheck 0.5.1`.
  **The box builds are newer than the `v0.6.0` tag** (wayang-tui `v0.2.1`
  full-screen splash + the CLI handoff/raw-mode fixes); a `v0.6.1` tag is the
  clean way to match, and the OS image still bakes `0.6.0`.
- **Docs**: [docs/README.md](README.md) (index),
  [HUD-USER-GUIDE.md](HUD-USER-GUIDE.md), [HUD-DEPLOY.md](HUD-DEPLOY.md),
  [PRODUCT-API.md](PRODUCT-API.md) + [PRODUCT-API-REFERENCE.md](PRODUCT-API-REFERENCE.md),
  [TUI-UX-REVAMP.md](TUI-UX-REVAMP.md); `wayang-tui/docs/ADD-A-SCREEN.md`.
- **wayangi**: N-hub generator (`7ee3170`), public-SSH `allow_from` default,
  hub DB at schema 55; deploy with `scripts/deploy.sh hub`.
- **Pending**: billing P0 (early renewal), WAF
  (`wayang-fw/docs/WAF.md`), API P1 (config/plan/commit), move raw-mode handling
  into `TermGuard`, fmt consistency (router/dcheck baselines), N-hub
  provisioning automation.

## Tech references written this round (read these before editing configs)

- wayang-router `docs/ROUTING-TECH.md` (WireGuard/iproute2/VRF/ECMP/proxy_arp/IPv6)
- wayang-router `docs/DAEMONS-TECH.md` (BIRD/radvd/udhcpd)
- wayang-fw `docs/NFT-REF.md` (nftables)
- wayangi `docs/EDGE-PARITY.md` (RouterOS `.rsc` → WayangOS) + `docs/edge-wayangos.md`

## Not done / next steps

1. ~~**M8 EdgeRouter on real hardware**~~ — **DONE**: wayangi gained the
   **1-NIC WAN-only model** (`wos-x86-1`; the box owns the public /32 on the
   tunnel, no LAN/DHCP/RA) and site 5 (`163.128.55.4`) runs it. Its public
   ping + SSH work once wayang-router programs the public group table (above).
2. **wayangi**: ~~stop hard-coding `1.1.1.1` as the WAN health probe~~ — **DONE**
   (gateway / configured target / hub endpoint). Still open: an `edge_wos` site
   kind / DB `class`+`platform`, and WOS-only deployment (`EnableEdgeWOS` still
   hard-requires `EnableEdgeRB`); **N-hub tunnels** (`docs/EDGE-N-HUB.md`).
3. **Ventoy**: the **1.0.30 ISO is downloaded but not on the USB** — copy it when
   re-attached.
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
