# TODO — pending work (for the next agent)

Cross-repo backlog. Each repo also has its own roadmap: this file is the index
of what is *not* done, with pointers. Status: `[ ]` open, `[~]` in progress,
`[x]` done. Read `AGENTS.md` in each repo before starting. Current WayangOS
release: **1.0.30** (see `docs/HANDOVER.md`). **Product goal + roadmap:
[docs/GOAL.md](GOAL.md).** **M7 (router kernel block) is resolved and released**
— the execution plan is archived in
[docs/TODO-M7-UNBLOCK.md](TODO-M7-UNBLOCK.md).

**Golden rules learned 2026-09-29 (do not regress):** a *monitor* is read-only
(`wayang-router monitor`); a daemon that changes routes/networking (`wan-failover`)
must **never strand the box** — keep the last/only path when a check fails;
**nothing self-reboots on a fresh install** (the removed `wayang-selftest` looped);
and **never import a bundle/config without matching the box's real topology
first**. See `AGENTS.md` §Current state.

## Edge product (cross-repo — the full backlog is wayangi `docs/FUTURE-WORKS.md`)

The 2026-09-29 round (box **`163.128.55.4`**, site 5) surfaced these; they span
`wayang-router` (data plane), `wayangi` (generator/hub) and this repo (bake).

- [x] **wayang-router: program the `wan_group` table at bring-up** — **fixed in
      v0.3.3** (`d576fa8`): the group default is applied through **rtnetlink**
      (BusyBox `ip` cannot parse `nexthop` multipath), reinstalled whenever it is
      desired-but-absent, and `plan`/`status` flag the drift. Deployed on the box
      at `/data/bin/wayang-router`; re-verified reachability (Globalping, world +
      Indonesia). The IPv6 table `402` had the same bug and is fixed too.
- [x] **Pin `wayang-router` 0.3.3** in `scripts/build-wayang-router.sh` (mirror
      asset published; next release bakes it).
- [ ] **N-hub tunnels, arbitrary names** (raise quality of service: 3/4/6
      tunnels, retire `jkt`/`mlb`). Generator MVP in progress (config file +
      per-site `hubs_json`); provisioning automation + hub lifecycle after —
      wayangi `docs/EDGE-N-HUB.md`.
- [x] **Public SSH by default** — `fw.toml [management] allow_from` now
      `0.0.0.0/0` + `::/0` (pubkey-only); applied on the box 2026-09-29.
      Harden next (rate-limit, explicit no-password) — wayangi FUTURE-WORKS.
- [ ] **Clean up** the temporary hosted files used to get the box going:
      `https://wayang.dalang.io/tmp/dc5de5f99f23e677.tar.gz` (a bundle) and the
      `/tmp/*-router` / `/tmp/*-fw` binaries.
- [ ] **wayangi billing P0** — an early renewal paid inside the
      `renewalLeadDays = 3` window does not extend the period (the anchor is
      recomputed from `now`, not from `current_period_end`), so the customer is
      cut off on the anchor despite paying. Details + fix in wayangi
      `docs/FUTURE-WORKS.md`.
- [ ] **WAF / L7 (self-hosted, sovereign)** — static Go WAF (Coraza + OWASP
      CRS) exposed as `wayang-waf`, to replace foreign WAF products. Design +
      PoC plan: wayang-fw `docs/WAF.md`. **Platform note:** mode **A**
      (reverse-proxy) needs **no kernel change**; mode **B** (transparent
      inline) needs **TPROXY/NFQUEUE** added to `configs/defconfig-intel`
      (absent today) + a boot soak. Roadmap: wayang-fw `docs/ROADMAP.md` §v0.6.
- [ ] **TUI UX revamp (PRIORITISED)** — one navigation grammar across
      wayang-fw/wayang-router/dcheck (`↑↓` within, `←→` between tabs, `tab`,
      `/` jump, `?` help), inline keycap hints + visible tab rows, and
      **REVIEW-before-apply with no hidden side effects** (adding a static WAN
      must not silently promote it to primary/default). Plus wizards and a
      Winbox/FortiGate/Cloudflare concept map for migrants. Plan:
      [`docs/TUI-UX-REVAMP.md`](TUI-UX-REVAMP.md). Touches both product repos.
- [ ] **No-flash startup & cross-app handoff (all three HUDs)** — the first
      frame must be a splash drawn *before* sampling (never an empty alternate
      screen), the clear must be themed via OSC 11 (no white flash on a light
      terminal / SSH), and a transition frame must cover `wayang` ↔
      fw/router handoff. Shared `splash`/`transition` component in `wayang-tui`.
      Design: [`docs/TUI-UX-REVAMP.md`](TUI-UX-REVAMP.md) §5b.
- [ ] **Terminal robustness vs external output (all three HUDs)** — a service,
      kernel printk, or a spawned child writing to the tty garbles the HUD;
      fix: never leak child stdout/stderr (`Stdio::null()`), force a full
      repaint on startup/SIGWINCH/return/slow tick, and a `Ctrl-L` repaint —
      shared in `wayang-tui`.
      Design: [`docs/TUI-UX-REVAMP.md`](TUI-UX-REVAMP.md) §5c.
- [ ] **Layout consistency: one tab = one full-screen view (all three HUDs)** —
      remove the per-screen pane arrangements (side-by-side / stacked /
      full-screen) that make tab switching confusing; a tab owns the whole body,
      with a single identical DETAIL treatment where needed. Enterprise
      requirement: predictable, learn-once navigation.
      Design: [`docs/TUI-UX-REVAMP.md`](TUI-UX-REVAMP.md) §5d.
- [ ] **Product API (all three tools)** — `wayang`, `wayang-fw`,
      `wayang-router` each get an **`api` subcommand** (loopback + bearer token)
      exposing status/config/plan/commit (and per-tool reads: wan/routes/wg/bgp,
      policies/drops/logs). Lets wayangi/an orchestrator drive a box **without
      SSH**. Design + phases: [`docs/PRODUCT-API.md`](PRODUCT-API.md). Shared
      transport crate `wayang-api`; queued after the UI waves.
- [ ] **`wayang-tui` shared component library** (repo created) — extract
      theme/widgets/focus/overlays from fw/router/CLI so the three HUDs are
      consistent *by construction*; migrate all three to depend on it.

## wayangos (this repo)

- [x] **WayangOS as a wayangi EdgeRouter** — unit type in the dashboard
      (`internal/edgewos`), self-managed WireGuard (no agent), hub provisioned
      like RB; `wayang edgerouter apply/status/enroll/start/stop/restart`
      (the 09 EDGEROUTER console module was removed — it is CLI-only now);
      prod hub deployed with `WAYANGI_EDGE_WOS=1`;
      QEMU-proven against the production hubs. See
      [docs/EDGEROUTER.md](EDGEROUTER.md) + wayangi's `docs/edge-wayangos.md`,
      `docs/EDGE-PARITY.md`. **The kernel block it needs is now enabled (M7 resolved, below).**
- [x] **Router kernel block — ENABLED and RELEASED (M7 resolved 2026-09-29).**
      Every group passed the bisect individually; the accumulated set showed no
      failing subset (delta-debug: 3 boots + 30-min soak); a device boot-count
      soak ran **~30 consecutive clean block boots** with **one** unreproduced
      first-boot hang. The full 7-group block is enabled in
      `configs/defconfig-intel` (`106bb23`) and **shipped from 1.0.23**; the
      device cold-boot passed. The 1.0.20 freeze's `gw=''` stall class is fixed
      (`udhcpc -b`). **`wayang-selftest` (the kernel-baked `wayang.selftest=120` +
      `CONFIG_CMDLINE`) was REMOVED in 1.0.25** — it reboot-looped a fresh
      install. See [docs/GOAL.md](GOAL.md) (M7),
      [docs/ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md),
      [docs/INCIDENT-1.0.13.md](INCIDENT-1.0.13.md), [docs/HARDWARE.md](HARDWARE.md).
- [x] **Publish the update channel from CI.** Tagging builds the ISO + bundle
      and creates the GitHub release; the tag path of
      `.github/workflows/installer-iso.yml` now also publishes the channel in a
      dedicated best-effort `publish-channel` job (after the signed `.wup`, with
      `continue-on-error`, skipped with a `::notice::` until the deploy secrets
      exist). A `workflow_dispatch` input `publish_channel_version` (re)publishes
      an already-released version's channel without rebuilding, and
      `scripts/publish-channel.sh` gained `--dry-run` plus fully env-parameterised
      destination (`WAYANG_DEPLOY_HOST`, `WAYANG_DEPLOY_REMOTE_DIR`,
      `WAYANG_DEPLOY_CHANNEL_SUBDIR`; legacy `HOST`/`REMOTE_DIR`/`CHANNEL_SUBDIR`
      still accepted). Required repo secrets: `WAYANG_DEPLOY_HOST` (ssh target,
      e.g. the `root@10.0.0.251` default in `publish-channel.sh`) and
      `WAYANG_DEPLOY_KEY` (private key authorised on that host). Optional
      overrides: `WAYANG_DEPLOY_REMOTE_DIR`, `WAYANG_DEPLOY_CHANNEL_SUBDIR`.
      Layout, device consumption, manual publish and the one-time key setup are
      documented in [docs/CHANNEL.md](CHANNEL.md). **Done:** secrets provisioned
      (`WAYANG_DEPLOY_HOST` + `WAYANG_DEPLOY_KEY`) and a real tag publish verified
      end-to-end (a tag-build `wayangos-release`-artifact LCA bug was also fixed).
- [x] **Landing page: drop the removed POS build commands.**
      `landing-page/download.html` no longer references `scripts/build-pos.sh` /
      `build-pos-iso.sh` / `wayangos-pos` (deleted). POS is a separate project now.
- [x] **`wayang` console polish.** Determinate update progress (`Gauge` with
      `% · MB/s · elapsed`, streaming the download; indeterminate fallback) and a
      one-shot **boot-other-slot** action (`b` twice / `wayang update
      --boot-other`) are in; the SSH screen and `wayang-addkey` now share one
      implementation (`wayang addkey`).
- [x] **Secondary-NIC DHCP ergonomics.** `wayang-net dhcp <iface>` leases
      address-only; added a persisted "also lease these" list at
      `/data/etc/network/also` (`wayang-net also [<iface> on|off]`), leased
      address-only at boot on top of the one-shot secondary pass, plus a TUI
      toggle (`l`) in `wayang net`.
- [x] **Router userspace**: static `wg`, `tc`, `bird` bundled (optional,
      pinned + sha256), bird started by `/etc/init.d/router` only with
      `/data/etc/router/bird.conf`; QEMU-only lab kernel
      (`scripts/build-lab-kernel.sh`). `birdc` not built (no static readline on
      the builder) — build readline from source if an operator CLI is wanted.
- [x] Pin + verify sha256 in every tool-fetch script (`build-nft.sh`,
      `build-wg.sh`, `build-iproute2.sh`, `build-bird.sh`, and the
      `build-wayangi.sh` release fallback); `ALLOW_UNVERIFIED=1` is the
      explicit opt-out and a mismatch always aborts.
- [x] **`wayang.slot` on first install.** The installer copies `grub.cfg`
      verbatim from the ISO payload, and since the shared template (862c683,
      v1.0.9) that file carries `wayang.slot=A`/`B`; `mark-ok` refreshes it on
      already-installed boxes (`refresh_grub_cfg`). **Verified by a build**
      (2026-09-29): a freshly built installer ISO from current master has
      `/usr/share/wayang-install/grub.cfg` byte-identical to
      `wayang/grub-disk.cfg`, with `wayang.slot=A` at line 55. Added a
      build-time guard in `scripts/build-installer-iso.sh` that aborts the
      build if either entry is missing. Installing to a disk still needs a live
      box to see the on-ESP copy.

### New backlog (2026-09-29)

- [x] **1-NIC WAN-only Edge model** (wayangi `70b4b38`, 2026-09-29): new model
      `wos-x86-1` (WAN `eth0`, **no LAN**); the renderer skips LAN/DHCP-server/RA;
      the public `/32`+`/64` are pinned on the **primary tunnel** (the box owns
      them). The WAN health probe is no longer hard-coded `1.1.1.1` (gateway /
      `WAYANGI_EDGE_WOS_PROBE` / hub-endpoint fallback). *Manual device static
      connectivity still needed on the test box (SR9700 duplex - use eth0).*
- [x] **wayangi: the WAN health probe is no longer hard-coded** (`70b4b38`) —
      it uses the WAN gateway / `WAYANGI_EDGE_WOS_PROBE` / hub-endpoint fallback
      (wayang-router still requires a `check` on a gateway-less DHCP uplink). A
      false check on the sole uplink used to flap the box.
- [ ] **wayangi**: `edge_wos` site kind / DB `class`+`platform`; WOS-only
      deployment (`EnableEdgeWOS` still hard-requires `EnableEdgeRB`).
- [ ] **Verify IPv6 RA/SLAAC on a real device.** `radvd` is bundled and the
      delegated `/64` + `ra = true` rendering passed QEMU, but RA was only
      tool-checked on hardware (the kernel block has been in the image since
      1.0.23). Runnable procedure — `radvd -c -C /etc/radvd.conf`,
      `ip -6 addr show dev <lan>`, then `radvdump` + `ip -6 addr show scope
      global` on a LAN host — is in [docs/EDGEROUTER-RUNBOOK.md](EDGEROUTER-RUNBOOK.md#ipv6-ra--slaac-delegated-64)
      §3 and [docs/NETWORK.md](NETWORK.md) (radvd section). Needs a box with the
      WG kernel and a delegated `/64`.
- [ ] **Install media**: the 1.0.27 ISO is built + released but not copied to
      the Ventoy USB (it was unplugged).
- [x] **`wayang reset [--yes]`** (`dba2f53`, 1.0.26) — reset config to defaults.
- [x] **Remove `wayang-selftest`** + the kernel `CONFIG_CMDLINE` (`2c74e76`,
      1.0.25) — it reboot-looped a fresh install.
- [x] **All wired NICs run `udhcpc -b`** (`cf9ade9`, 1.0.27) — a cable plugged
      in later auto-DHCPs.
- [x] **`wayang edgerouter apply` installs the bundle's WG keys** (`8532055`).

## wayang-fw (dalang-io/wayang-fw)

See `docs/ROADMAP.md` (v0.2+). Remaining:

- [x] per-zone DHCP/DNS via **dnsmasq** (`7383298`): `[[dhcp]]`/`[[dns]]` per zone → one rendered dnsmasq config; engine start/SIGHUP/stop;
      `scripts/build-dnsmasq.sh` (static dnsmasq 2.93, pinned); `docs/DHCP-DNS.md`. *Not yet wired into the WayangOS image.*
- [ ] interface config from the HUD.
- [ ] Firewall enforcement is **QEMU-tested only** — run the lab
      `tests/lab/lab.py` and then validate on real hardware.
- [x] live conntrack / drop-log views → **DROPS** screen reads the `wfw:`
      counters + kernel drop log, grouped by source/service.
- [x] rule **schedules** (time windows) and **hairpin NAT** as config + render,
      with TUI fields.
- [x] **nft sets + FQDN objects** (named interval sets; the engine resolves
      FQDNs itself and updates a dynamic set atomically; FortiOS import).
- [x] per-family NAT incl. **NAT66** for a delegated IPv6 prefix; ip6
      objects/policies.

## wayang-router (dalang-io/wayang-router)

See `docs/ROADMAP.md`. v0.1 (MVP) is usable (ports, 802.1Q VLANs, bridges,
static v4/v6, DHCP client, forwarding, static routes):

- [x] **netlink backend** for addresses/links/VLAN/bridge (rtnetlink via raw
      `libc`, no new crate), falling back to `ip`/`vconfig`/`brctl`; `ip route`
      stays command-based for now.
- [x] **WireGuard** (config + `wg` bundled), **QoS** (CAKE/HTB + simple queues
      via `tc`), **BGP/OSPF** via BIRD (`bird` bundled), **VRF + weighted
      ECMP**, **multi-WAN failover + policy routing**, **delegated IPv6 prefix +
      SLAAC router advertisements** (radvd; warn-and-noop when absent).
      **All of this now ships**: the kernel block is enabled from 1.0.23.
- [x] **bridge port-level VLANs** (`d68e57d`, 2026-09-29): `[[interface.port]]` (pvid/tagged/untagged) under a `type="bridge"` interface → VLAN-aware bridging via rtnetlink, idempotent + rollback, caps-gated on `CONFIG_BRIDGE_VLAN_FILTERING`; real-kernel netns test.
- [ ] IPsec IKEv2; PPPoE; Wi-Fi AP.
- [ ] Edge 1-NIC (WAN-only) support — same open item as wayangi above.
- Lab: `/tmp/wayang-tools/{bzImage-lab,initramfs-lab.img}` on the builder
  (QEMU only; all router kernel options on).
- [ ] **RouterOS-parity gaps** (gap review 2026-09-29) — PPPoE, remote-access
      VPN, stateful HA (conntrack sync), DHCP relay, DNS server
      (static/conditional/DoT), multicast (IGMP proxy), L2 extras
      (STP/RSTP, IGMP snooping, port isolation, MAC-VLAN, loop protect),
      Netflow/sFlow + per-host accounting, scheduler + Netwatch,
      certificates + ACME, RADIUS. New phases **v0.8 (access & L2)** and
      **v0.9 (NOC & ops)** + a full gap matrix in wayang-router
      `docs/ROADMAP.md`.
- Released **v0.3.0** (Edge schema), **v0.3.1** (never strand the last uplink),
  **v0.3.2** (`wan-monitor` → `wan-failover`), **v0.3.3** (program the
  `wan_group` table at bring-up via rtnetlink).

## dcheck (dalang-io/dcheck)

- [x] Startup/scan optimizations for the console/HUD path: `smartctl` is only
      spawned when installed, the on-disk SMART cache is loaded once per
      process (not per device/rescan), and `check`/`watch`/`prometheus`/`--json`
      bypass it without rewriting it. Measured on the 60-disk fixture:
      `dcheck check` cache-file opens 180 → 0, ~0.4 s → ~0.02 s; snapshot
      opens 240 → 121. No idle-tick spawns.
- [x] Fake-drive / real-capacity / deleted-file recovery are **shipped, not
      preview** (implemented and tested on Linux): `dcheck verify` (write-and-
      read capacity test, free-space and `--destructive`), `dcheck recover`
      (read-only recovery chance, steps, disk map) and `dcheck undelete`
      (NTFS/FAT32/exFAT names + block map; `--carve` for other filesystems).
      Documented gaps, not preview labels: `undelete` now follows the NTFS
      `$ATTRIBUTE_LIST` and reads names from `$I30`, and `--carve --free`
      restricts carving to free clusters (NTFS/FAT32/exFAT bitmaps); still
      assumed contiguous for fragmented FAT32/exFAT files, and no name for
      ntfs3-deleted files without an index entry. `recover`/`verify` gained a
      macOS backend (diskutil/df; `--destructive` stays Linux-only); recover's
      disk-map sampling is still Linux-only.

## Done (recent)

- [x] **Edge bundle import**: `wayang edgerouter apply <dir|.tar.gz> [--force]`
      installs a wayangi WayangOS Edge bundle
      (`router.toml` + `fw.toml` → `/data/etc/{router,fw}/config.toml`, mode
      0644, refuse-overwrite/`.bak` on force, token enrolled from `token` or
      `install.sh`), printing the commit-confirm next steps and never applying
      automatically (docs/EDGEROUTER.md §C).
- [x] Router + firewall usable at the CLI: `/data/bin/{wayang-fw,wayang-router}`
      linked into `/usr/bin` at boot; VLAN/bridge MVP on; released in 1.0.15.
- [x] `wayang` console: 05 SSH (add/remove root keys), 06 DCHECK (opens the
      bundled app, green when installed), 07 FIREWALL, 08 ROUTER; UPDATES shows
      a progress bar.
- [x] Startup: `net::list` uses one `ip -o -4 addr` call (was one per iface).
- [x] Perf: wayang-fw samples `nft` at 2 s + reloads history on change only;
      wayang-router scans `/proc` once per poll (−27%); dcheck skips `smartctl`
      spawns when absent (0 vs 189 execve/scan).
- [x] WayangPOS removed from `wayang` (separate app / dedicated box).
- [x] `wayang`: determinate update progress + `b` boot-other-slot; SSH keys via
      `wayang addkey` (shared with the TUI).
- [x] wayang-fw: DROPS view; rule schedules; hairpin NAT.
- [x] wayang-router: rtnetlink backend (addr/link/VLAN/bridge).
- [x] dcheck: undelete ATTRIBUTE_LIST + `$I30` names + `--carve --free`; macOS
      recover/verify backends.
- [x] Router kernel bisect harness: `scripts/bisect-router-opts.sh` +
      `docs/ROUTER-KERNEL-BISECT.md` (hardware-gated).
- [x] Released **1.0.16** and **1.0.17** (tag + channel + device); see
      `docs/HANDOVER.md`.
- [x] **Persistent firewall/router monitoring.** `/etc/init.d/{fw,router}`
      start `wayang-fw` / `wayang-router monitor --daemon` when the binary and a
      confirmed config exist, appending JSON Lines to
      `/data/var/<app>/history.jsonl` (created 0755 at the `/data` mount in
      rcS, so history survives OS updates). Idempotent (pidfile under
      `/var/run`), backgrounded so it never blocks boot, output to
      `/var/log/wayang-{fw,router}-monitor.log`; `status` prints the collector
      line. **Retention: the newest 20 000 records per collector are kept
      (trimmed at boot).** See [docs/MONITORING.md](MONITORING.md).
- [x] Released **1.0.18 / 1.0.19 / 1.0.21 / 1.0.22** (tag + channel + device).
      **1.0.20 was never released** (its kernel froze). 1.0.22 = shipped safety
      net (watchdog + selftest cmdline) + DHCP resilience.
- [x] Released **1.0.23 … 1.0.27**: 1.0.23 = full router block; 1.0.25 = removed
      `wayang-selftest` (reboot-loop fix); 1.0.26 = `wayang reset`; 1.0.27 = all
      wired NICs hotplug-DHCP. wayang-router **v0.3.0/0.3.1/0.3.2** released. See
      `docs/HANDOVER.md`.
- [x] `radvd` bundled (`scripts/build-radvd.sh`, `/usr/sbin/radvd`) for IPv6
      RA/SLAAC.
- [x] Tech references written: wayang-router `docs/ROUTING-TECH.md`,
      `docs/DAEMONS-TECH.md`; wayang-fw `docs/NFT-REF.md`; wayangi
      `docs/EDGE-PARITY.md`.
- [x] WayangOS is a wayangi Edge unit type (self-managed WireGuard, no agent);
      prod hub deployed with `WAYANGI_EDGE_WOS=1`.
