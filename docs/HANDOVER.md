# HANDOVER — WayangOS state (2026-09-28)

Read `AGENTS.md` first (rules, boot order, build). `docs/TODO.md` is the
pending-work index. **`docs/GOAL.md` is the product goal + roadmap** (what we are
building and the milestones). This file is *where things stand* across the repos.

## Releases / channel / device

| | |
|---|---|
| WayangOS release | **1.0.32** (tag `v1.0.32`, 2026-10-03; channel live `https://wayang.dalang.io/channel/stable/x86_64`, serving 1.0.32; **bakes `wayang-router 0.7.0` and `wayang-fw 0.6.4`**, nothing else changed — see the 1.0.32 entry below). Before it: **1.0.31** (tag `v1.0.31`). 1.0.23+ carries the **full router block**; 1.0.25 removed `wayang-selftest`; 1.0.26 added `wayang reset`; 1.0.27 makes all wired NICs hotplug-DHCP; **1.0.28–1.0.30 bake `wayang-router` + `wayang-fw` into `/usr/bin`, drop the 09 EDGEROUTER TUI module (8 modules), and open public SSH by default**; **1.0.31 bundles `conntrackd`** (conntrack-tools 1.4.9, static — HA connection-state sync stops being a no-op), pins **`wayang-fw 0.6.3`** (the HA round: the VRRPv2 daemon, the virtual-MAC VIP, `sync_iface`, and the ICMP protocol-number fix that made an `icmp`/ping policy impossible to commit on WayangOS) and **`wayang-router 0.6.1`** — with one honest caveat: the *released* 1.0.31 images were built while the router pin was still `0.6.0` (the 0.6.1 asset reached the mirror minutes after the channel published), so the **next** build takes the RFC-3768 VRRP fix — and carries `wayang-waf 0.1.1` (Go + Coraza + OWASP CRS) and `dnsmasq 2.93` |
| Test device `root@163.128.55.4` (was `.3`; moved) | **1.0.30** + manual `/data/bin` overrides (checked 2026-10-03: up 22 h, SSH works, `edge-thinkstation-p320-tiny`, kernel 7.2.7). `/data/bin/wayang-fw` is **0.6.4** (deployed by hand 2026-10-03, previous kept as `wayang-fw.0.6.0.bak`) and `/data/bin/wayang-router` is an old build; **both win over the `/usr/bin` copies baked into an OS release, so remove them before testing an upgrade**. Its `wayang-fw` has no drift baseline until the next commit/boot. Older notes: uplink on onboard `eth0`; USB LAN `ssh root@192.168.2.2` (SR9700, duplex-flaky); it was powered off on purpose on 2026-09-30, no longer true |
| Prod Edge box `163.128.55.4` | site 5 `ThinkStation-P320-Tiny`, WayangOS **1.0.30**; managed via the hub tunnel (`ssh dell-jkt` → `ssh root@10.99.130.5` / `@163.128.55.4`); public ping + SSH now work. **`163.128.54.4` is a different MikroTik RouterOS — not ours.** |
| Tags that exist | `v1.0.18`, `v1.0.19`, `v1.0.21`, `v1.0.22`, `v1.0.23`, `v1.0.24`, `v1.0.25`, `v1.0.26`, `v1.0.27`, `v1.0.28`, `v1.0.29`, `v1.0.30`, `v1.0.31`, `v1.0.32` — **`v1.0.20` was NEVER released** (its kernel froze, see below) |

## 2026-10-03 — 1.0.32 (tool pins only)

- **What changed vs 1.0.31:** only the tool pins — `wayang-router 0.6.0 → 0.7.0`
  (BGP per-neighbour policy and named route maps, `bgp table`, `backup`/`restore`,
  OSPFv3 + `ospf`, TCP health probes for multi-WAN, OSPF adjacency info) and
  `wayang-fw 0.6.3 → 0.6.4` (drift detection, HUD clarity round). Kernel,
  rootfs scripts, `wayang` CLI and the other tools are as in 1.0.31. (1.0.31
  shipped router `0.6.0`, not the `0.6.1` its pin said — the asset reached the
  mirror after the channel published; 1.0.32 does not have that problem, both
  assets were on the mirror and sha256-verified before tagging.)
- **Release procedure used** (`docs/HUD-DEPLOY.md` §4): build musl on the builder
  from the exact tagged commit → tag + push → copy the asset to
  `/root/wayang.dalang.io/public/edge/tools/` on `10.0.0.251` and check the sha256
  over HTTPS → `gh release create` → bump the pin in
  `scripts/build-wayang-{router,fw}.sh` and verify it with
  `BUILD_DIR=/tmp/x bash scripts/<script>` → tag the OS. CI (kernel ∥ tools →
  rootfs → release → publish channel) took ~7 min, all green; the channel
  manifest says `"version": "1.0.32"`.
- **Pins:** router `0.7.0` sha256 `b2249181…f933`, fw `0.6.4` sha256
  `d1b57a4d…19d8e`. Assets on the mirror: `wayang-router-v0.7.0-…`,
  `wayang-fw-v0.6.4-…`.
- **Not tested here:** the upgrade itself (manual `wayang update` and the
  automatic channel path) on a device — the owner is testing it. Test box caveat:
  `/data/bin/wayang-{fw,router}` override `/usr/bin`, see the device row above.

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
- **The pid tracking in those two init scripts was wrong until `e5746e1`**
  (`$!` recorded the exiting wrapper, not the detached collector), so on any
  image ≤ 1.0.30 `status` reports "monitor: stopped" and `stop_monitor` kills
  nothing. The collectors themselves ran fine the whole time — the daemonise
  behaviour is the same trap as the bug itself, so re-check the pid when touching
  these scripts.

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

## 2026-09-30 — firewall correctness + dnsmasq round

A maintenance round rather than a feature round. Four things landed, none of
them tagged — see **Carry-over** at the end for what still has to ship.

### wayang-fw `a516443` — three bugs on the commit-confirm path

All three sat on the mechanism the product's safety story rests on (AGENTS.md
invariants 1 and 4). Each fix has a regression test, and each test was
confirmed to **fail against the pre-fix code** before the fix was kept.

1. **Orphan revision.** `commit()` wrote `history/NNNNNN.toml` *before* the real
   `nft -f`, so a failed apply returned "nothing changed" while leaving a
   revision on disk that was never loaded — and burning a number in the
   sequence. The write now happens after the transaction succeeds.
2. **Wedged rollback.** `watchdog` did `let _ = rollback_pending(…); return
   true` — the error was swallowed and success was reported for a rollback that
   never happened. `pending.toml` survived, so every later commit was refused at
   the pending check and nothing was left to retry. It now logs the failure and
   **retries until the undo lands**; the refusal names `wayang-fw confirm` /
   `wayang-fw rollback`.
3. **Leaked sysctls on first-commit rollback.** `apply_sysctls` writes four
   keys but the no-previous-revision branch restored only `net.ipv4.ip_forward`,
   leaving `nf_conntrack_tcp_loose=0` (strict TCP tracking with no synproxy
   policy needing it) and `net.ipv6.conf.all.forwarding=1` — the latter keeps
   **SLAAC off on the LAN**. `Pending.previous_forwarding: bool` became
   `previous_sysctls: SysctlSnapshot {ipv4_forwarding, ipv6_forwarding,
   conntrack_tcp_loose}`. `pending.toml` is transient, so no migration.

Verification: 195 tests (was 191), fmt + clippy clean, `nft -c` (1.0.9) and
`dnsmasq --test` accept the rendered demo, QEMU lab **22/22** including the
anti-lockout and watchdog self-rollback checks that drive this path end to end.

### wayangos `e5746e1` — dnsmasq bundled, monitor pid tracking fixed

- **dnsmasq 2.93 now ships in the image** (optional, like radvd). The build
  script moved from `wayang-fw/scripts/` and is wired into *every* path that
  stages tools: `install_tool` in `build-rootfs.sh`, `ci-build.sh`, **and** the
  `installer-iso.yml` tools job with its artifact + cache lists — the two
  build pipelines in this repo are independent, so all of them needed it.
  Pinned version + sha256, static, no DNSSEC/Lua, best-effort. `wayang-fw`
  keeps its copy as the reference.
- **`d259447` recorded the wrong pid for both collectors.** It started them as
  `"$bin" monitor --daemon &` and stored `$!` — the *wrapper's* pid, which
  exits at once, because `--daemon` re-execs the collector detached (`setsid`).
  So `$!` was always dead: `status` always reported "monitor: stopped" while
  the collector was running, and `stop_monitor` killed nothing. The collector
  itself was never wrong (and being detached, it did not die with the init
  script either) — only the reporting and the stop path. Fixed for **both**
  `init.d/fw` and `init.d/router`, which had the same bug twice:
  - `fw` now takes the pid from the file **wayang-fw writes for itself**
    (`/data/etc/fw/monitor.pid`) — the same file the tool checks to refuse a
    second collector — and validates it against `/proc/$pid/cmdline`, so a
    recycled pid cannot pass.
  - `router` has **no pidfile of its own**, so it matches `/proc/*/cmdline` for
    `wayang-router` + `monitor`. That is the one reason the two differ.
  - `start_monitor` no longer backgrounds the wrapper and waits boundedly
    (20 × 50 ms) for the collector to appear, so `status` is correct right
    after boot; both log a warning if `--daemon` returns with no live collector.

  Verified: shellcheck clean on `scripts/*.sh` *and* on both init scripts
  extracted and checked as POSIX sh; the new logic exercised on Linux against
  live / dead / recycled / corrupt / empty / missing pid files, and the `/proc`
  scan for the router.

### Box `163.128.55.4` — dnsmasq installed out of band

The image change is **not built yet**, so the feature was installed directly to
close the gap:

| | |
|---|---|
| Path | `/data/bin/dnsmasq` (persistent — `/usr/sbin` is wiped every boot) |
| Version | 2.93, static-pie x86_64, 1 476 152 B, DHCP + DHCPv6, no DNSSEC/Lua |
| sha256 | `be50e7055dc6e278382732537ec1438d8e03fbf59dfe57ec8ec46c46add10379` (identical on build box, in transit, on the box) |
| Picked up by | `wayang-fw` lookup order ends at `/data/bin/dnsmasq` (`src/dhcp.rs`) |

It is **inert until a config with `[[dhcp]]`/`[[dns]]` is committed**:
`apply_dhcp_with` calls `dhcp::render()` first, and on `None` it stops dnsmasq
and removes the conf **without ever touching the binary**. This box's config has
no such blocks (only `[[zone]]`, `[[address]]`, `[[service]]`, `[[policy]]`),
so nothing started; verified after install — no process, no `dnsmasq.conf`, no
new `fw.log` line, still `active: rev 1 / loaded: yes`.

Tested on the box: `dnsmasq --test` **accepts the config `wayang-fw dnsmasq`
renders** (with a `[[dhcp]]` + reservation + `[[dns]]` + host record for
`eth0`), and the binary starts, binds, accepts `SIGHUP` (the reload path a
commit uses) and creates its leasefile at the real path. **Not** tested: a real
DHCP client actually taking a lease — that would squat IPs on the production
LAN, so it needs the owner's say-so.

### Docs corrected

`wayang-fw`: new `TODO.md`; `docs/ROADMAP.md` reconciled (its `v0.x` headings
are milestone names, **not** the crate version — `Cargo.toml` is `0.6.0` while
§v0.6 is entirely unticked); §v0.4/§v0.5 repointed at `wayang-router` since
WireGuard / BGP / SD-WAN / shaping are already built there; `AGENTS.md`
invariant 8 + module map still described `src/tui/theme.rs` and `widgets.rs` as
copies of dcheck's — they were replaced by the `wayang-tui` crate and no longer
exist, and the map was missing four live files. Test counts were wrong in three
places (176/130/~130 → **195**); historical entries inside HANDOVER were left
as written, since those counts were true when recorded.

## FortiOS parity — work plan

The backlog lives in `wayang-fw/docs/ROADMAP.md` §v0.3 and `wayang-fw/TODO.md`
§B; this is the **order of work**, with the files each step touches. Every step
must end with the AGENTS.md gate: `fmt --check`, `clippy -D warnings`, `test`,
`nft -c` on `examples/office.toml`, and the QEMU lab 22/22
(`LAB=… python3 tests/lab/lab.py`) for anything touching `engine.rs` or
`render.rs`.

**Step 1 — address/service negation + exclude-member.** Highest value per line:
`import-fortios` currently *disables* any policy that needs these, so a config
can import "successfully" while silently dropping security rules — the worst
possible outcome. Touches `config.rs` (field + `deny_unknown_fields`), `render.rs`
(`!` in set members / `!=` in matches), `validate.rs` (conflict with exclude),
`import.rs` (stop disabling), `tui/form.rs` (a toggle), `docs/CONFIG.md`, a
`tests/fixtures/` case. Decide explicitly what `exclude` does on a nested group.
*Verify:* an importing fixture that previously dropped a policy now keeps it,
and `nft -c` passes.

**Step 2 — read-only API.** A working template already exists at
`wayang-router/src/api.rs` (HTTP/1.1 JSON, bearer token in a 0600 file,
constant-time compare, loopback by default, refuses non-loopback binds without a
token). Adapt rather than start fresh. Keep it read-only; the commit-confirm
story is not something to expose over HTTP.
*Verify:* 12-ish handler tests ported from the router; a non-loopback bind
without a token must fail.

**Step 3 — output-chain policies (`from = "self"`).** Closes the ingress-only
gap. Touches `render.rs` (output chain + `ct state` ordering), `validate.rs`
(what a self-source policy may touch), `config.rs`, TUI, `NFT-REF.md`.
*Think about:* the interaction with the per-policy NAT ct marks and with the
management SSH rule — that rule currently only guards `input`.

**Step 4 — observability before the big features.** Prometheus exporter (the
counters already exist as `wfw:` nft comments and are parsed by `metrics.rs`),
then an audit log. You cannot tune a WAF or chase a DoS policy blind.
*Note:* CI regenerates `docs/screenshots/*.svg` but never **diffs** them
(`wayang-fw/TODO.md` §C5), so a UI regression cannot fail CI.

**Step 5 — the remaining v0.3 gaps, cheapest first.** GeoIP (country sets from
a local DB into nft interval sets) → session-end logging (bytes/duration per
session via conntrack events, completes FortiView "sessions"/"bandwidth") →
vip6 / VIP groups / VIP LB → NAT46/NAT64 → one-time schedules + schedule groups
→ ICMP type/code and source ports in services → wildcard FQDN (needs DNS
snooping).

**Step 6 — WAF P0** (see `wayang-fw/docs/WAF.md`). PoC only, mode A reverse
proxy, **no kernel change**: static Go + Coraza + OWASP CRS behind wayang-fw,
same one-config → `check` / `commit --confirm` / `rollback` model. P2 (transparent
inline) needs a new `defconfig-*` fragment for TPROXY/NFQUEUE plus a boot soak —
do not start there.

**Deliberately not on this list:** anything in ROADMAP §v0.4/§v0.5 (WireGuard,
BGP/OSPF, SD-WAN, shaping) — built in `wayang-router`. The only leftover is HA
(VRRP + config sync + **conntrack sync**), which is a router P0.

## Repos & tags

| Repo | HEAD / tag | Notes |
|---|---|---|
| `dalang-io/wayangos` (this) | `master` `47368f0`, tag **`v1.0.31`** | block enabled; `wayang reset`; selftest removed; hotplug DHCP; **wayang-router/wayang-fw baked into the rootfs; EDGEROUTER TUI removed; CI self-hosted**; tool pins **`wayang-fw 0.6.3`**, **`wayang-waf 0.1.1`** (Go + Coraza + OWASP CRS, into `/usr/bin`) and `wayang-router 0.6.1` (**published 2026-10-01**, sha256 pinned in `build-wayang-router.sh`; the 1.0.31 images themselves were built with `0.6.0`); **`dnsmasq 2.93` and `conntrackd 1.4.9` (static, `/usr/sbin/conntrackd`) bundled**; **monitor pid tracking fixed** in `init.d/{fw,router}` |
| `dalang-io/wayang-fw` | `master` `374b9bb`, tag **`v0.6.3`** | DROPS, schedules, hairpin, FortiOS import, NAT66, VIP, monitor, per-zone DHCP/DNS; **HUD via the shared `wayang-tui`**; Rust 1.98.1 + edition 2024; **read-only HTTP JSON API** (`wayang-fw api`, hardened request limits); **output-chain policies** (`from = "self"`) + address-group `exclude_member`; CI pins the toolchain, runs **`nft -c` over the rendered rulesets**, and has a second job for the Go module; **WAF P0** (`[waf]` config + validation, commit/boot/rollback lifecycle, and the real **`wayang-waf`** Go + Coraza + OWASP CRS, static); **v0.6.3 = the HA round** — `[ha]`, a first-party **VRRPv2** daemon holding the VIP on a **macvlan** child whose MAC *is* the RFC 3768 virtual MAC, `conntrackd` FTFW **state sync** (`sync_iface`), the HA channel rendered before every policy, `ha status` with the parity hash, and the fixes a **two-box QEMU lab (79 checks)** forced — plus **ICMP rendered as protocol numbers** (`l4proto 1`/`58`; a named `ipv6-icmp` needs `/etc/protocols`, which the image does not ship, and took the whole ruleset with it) |
| `dalang-io/wayang-router` | `master` `ec013cf`, tag **`v0.6.1`** | EdgeRouter role (public `/32` routed_prefixes+proxy_arp, delegated IPv6 + SLAAC/radvd, v6 policy routing, weighted-ECMP + failover, VRF, multi-WAN, monitor); bridge port-level VLANs; **v0.3.3 `wan_group` table bring-up fix** (`d576fa8`); **HUD via `wayang-tui`** (nav grammar, REVIEW + no silent primary, WAN wizard, deep `/` jump, full-screen tabs, splash/repaint); **reading API `wayang-router api` (P0)**; **Rust 1.98.1 + edition 2024**; **v0.6.1**: **VRRPv2 adverts are RFC 3768 to the byte** (byte 1 the VRID, byte 3 the address count, interval in seconds, message-only checksum — the previous layout was self-consistent but spoke to nothing else, not even `wayang-fw`'s daemon) + an optional `vrid` derived exactly as `wayang-fw` does, and two CI repairs (a Format step red since `6776e58`, and an `ioctl` request type that made the crate **fail to compile for glibc**) |
| `dalang-io/dcheck` | `master` `f4e3e3e`, tag **`dcheck-v0.5.1`** | health list + Prometheus + undelete/macOS; **Rust 1.98.1 + edition 2024** (`unsafe extern "C"`, `allow(collapsible_if)`) |
| `dalang-io/wayang-tui` (new) | `main` `a8c84b5`, tag **`v0.2.1`** | **shared ratatui component library** for the three HUDs: `theme` (palette/modes), `widgets` (`panel`/`panel_focused`/`header`/`footer`/`keycaps`/`badge`/…), `focus`, `overlay`, `term` (OSC-11, alt screen, repaint, tty-safe spawn), `splash`, `transition`, `layout`. `docs/ADD-A-SCREEN.md` |
| `dalang-io/wayangi` (dashboard) | `main` `0e477e7` | WayangOS unit type + self-managed WG + `docs/EDGE-PARITY.md`, deployed to prod; `wos-x86-1` 1-NIC WAN-only model; **public-SSH `allow_from` default**; **N-hub generator** (`docs/EDGE-N-HUB.md`), `docs/FUTURE-WORKS.md` |

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
- **Released**: `wayang-router v0.6.0` + **`wayang-fw v0.6.1`** (tags + mirror
  `https://wayang.dalang.io/edge/tools/`; OS pins bumped — wayang-fw's to `0.6.1`
  with sha256 `42110e67…`, which `scripts/build-wayang-fw.sh` fetches and
  verifies). The `wayang` CLI ships inside the OS image (no separate tag).
- **Box `163.128.55.4`** (site 5, `ThinkStation-P320-Tiny`): all four binaries
  under `/data/bin` (persistent) with `/usr/bin` symlinks — `wayang-router
  0.6.0`, `wayang-fw 0.6.0`, `wayang 0.1.0 (ux-p1)`, `dcheck 0.5.1`.
  **The box builds are newer than the `v0.6.0` tag** (wayang-tui `v0.2.1`
  full-screen splash + the CLI handoff/raw-mode fixes); `v0.6.1` is now the tag
  that matches, the **OS image bakes `wayang-fw 0.6.1`** (pin bumped, binary
  published and verified), and the box's `/data/bin/wayang-fw` copy is still the
  older build until it is deployed there by hand.
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
4. **wayang-fw**: interface config from the HUD; firewall enforcement is
   QEMU-tested only — validate on real hardware (`tests/lab/lab.py`). Per-zone
   DHCP/DNS is **no longer blocked**: dnsmasq is bundled in the image
   (`e5746e1`) and installed on `163.128.55.4` by hand in the meantime, so the
   only thing missing is an image build. Then see the **FortiOS parity — work
   plan** above.
5. **wayang-router**: IPsec IKEv2; PPPoE; bridge port-level VLANs; Wi-Fi AP.
6. `radvd` RA was tool-checked only — re-verify on a device with a WG kernel.
7. EdgeRouter own-traffic coverage / endpoint recursion / DHCP single-`/32` gaps
   (`docs/EDGE-PARITY.md`); phone-home "site online" heartbeat UI.
8. `wayang.slot` on first install — verify on a fresh install (older installers
   wrote grub.cfg without it).

## Carry-over from the 2026-09-30 round (nothing is tagged yet)

Everything below is **on `master` but unreleased**. `HEAD` is past the last tag
in both repos, so no device has any of it.

| | Where it is | Ships when |
|---|---|---|
| wayang-fw: 3 commit-confirm fixes | `8885c58` (`a516443` is the fix) | next `wayang-fw` tag — **and see the box note below** |
| wayang-fw: address negation (FortiOS `set invert`) + a literal-mix bug it did not catch | `704f89f`, `3e99645` | with the next tag |
| wayang-fw: read-only HTTP API (`wayang-fw api`, 7 routes) | `9ab4563` | with the next tag |
| wayang-fw: docs — WAF.md kernel facts corrected | `d2388b3` | with the next tag |
| wayangos: dnsmasq + monitor pid fix | `e5746e1` | next `v1.0.31` **or** a `workflow_dispatch` build |
| wayangos: WAF mode-B kernel fragment, **in no pipeline** | `b5345c1` | deliberately not yet — needs bisect + boot soaks |
| wayang-router: commit path crash-safe (record + watchdog armed before the first step) | `6776e58` | next `wayang-router` tag |
| wayang-router: VRRP (RFC 5798 subset) + macOS portability + the `snapshot --live` panic | `39fbe8c`, `7eb0b96` | with the next tag |
| dcheck: renders through `wayang-tui` (−528 lines) | `f4e3e3e` | next `dcheck` tag |
| wayangi: first CI (4 jobs, 293 tests now run automatically) + billing P0 | `0e477e7`, `43b4045` | already live on `main` |
| `dnsmasq` on `163.128.55.4` | `/data/bin/dnsmasq`, installed by hand | already there; **delete it once an image with dnsmasq is on the box** |

Lab status at the time of writing (all on the build box): `wayang-fw` 25/25,
`wayang-router` `lab_vrrp` 37/37 and `lab_vpn_bgp` 28/28, with
`lab.py` 19/22 and `edge_multiwans` 28/29 — those two shortfalls are
**pre-existing at `v0.6.0`** and are a reboot-path problem in
`/etc/init.d/network` handing the interfaces to `/etc/init.d/router`, not
anything in the commits above. Details and the next step are in
`wayang-router/HANDOVER.md`.

Before tagging: **boot-test.** 1.0.13 and 1.0.20 both froze the device, and the
`install_tool`/init-script changes in `e5746e1` touch the boot path
(`rcS` → `init.d/fw`). AGENTS.md rule 3: ask the owner before tagging, because
`v*` publishes the update channel and **every device sees it**.

Two things to watch on the first real boot with `e5746e1`:
- `/etc/init.d/{fw,router}` now **wait up to 1 s** for the collector to appear.
  That is inside the boot path; if it ever shows up as a boot-time stall, drop
  the wait rather than the fix.
- The firewall still starts before the network (AGENTS.md rule 2) — unchanged,
  but re-confirm on the cold-boot soak.

Also still open, from earlier rounds: **`v0.6.1` for wayang-fw** (the test box
runs a post-`v0.6.0` build), the **wayangi billing P0** (an early renewal does
not extend the period — systematic for payments on the 29th–31st), and
`wayang-fw` having **no drift detection** while `wayang-router` does.

## Gotchas

- Build on `root@10.0.0.251`; never write `/root/wayangos-build` for experiments.
- The device's USB `sr9700` NIC is **flaky** (10 Mbps, link flaps) — the uplink
  is now on the onboard **`eth0` (e1000e, 1G)**; don't use the SR9700 as a WAN.
  Large `.wup` transfers over SSH stall; `wayang.dalang.io` serves **no
  byte-range** (no resume). Prefer the device pulling via `wayang update`.
- **Never tag/publish without a device boot test** — 1.0.13 and 1.0.20 froze it.
- The `busybox ip` cannot do multipath/`xfrm`; the labs use a static iproute2 `ip`.
- POS is a separate project; not in `wayang`.
