# TODO — pending work (for the next agent)

Cross-repo backlog. Each repo also has its own roadmap: this file is the index
of what is *not* done, with pointers. Status: `[ ]` open, `[~]` in progress,
`[x]` done. Read `AGENTS.md` in each repo before starting. Current WayangOS
release: **1.0.23** (see `docs/HANDOVER.md`). **Product goal + roadmap:
[docs/GOAL.md](GOAL.md).** **Execution plan to unblock M7 (multi-agent, do this before touching the
kernel block): [docs/TODO-M7-UNBLOCK.md](TODO-M7-UNBLOCK.md)** — T1–T4 landed
2026-09-28 (see that file for hashes); the device runs need owner OK.

## wayangos (this repo)

- [x] **WayangOS as a wayangi EdgeRouter** — unit type in the dashboard
      (`internal/edgewos`), self-managed WireGuard (no agent), hub provisioned
      like RB; `wayang edgerouter apply/status/enroll/start/stop/restart` + the
      09 EDGEROUTER console module; prod hub deployed with `WAYANGI_EDGE_WOS=1`;
      QEMU-proven against the production hubs. See
      [docs/EDGEROUTER.md](EDGEROUTER.md) + wayangi's `docs/edge-wayangos.md`,
      `docs/EDGE-PARITY.md`. **The kernel block it needs is now enabled (M7 resolved, below).**
- [x] **Router kernel block — ENABLED (M7 resolved 2026-09-29).** Every group
      passed the bisect individually; the accumulated set showed no failing
      subset (delta-debug: 3 boots + 30-min soak); a device boot-count soak of
      the block kernel ran **~30 consecutive clean boots** (connectivity at
      up=47 s) with **one** unreproduced first-boot hang (coinciding with the
      SR9700 link flap at ~14 s). The 1.0.20 freeze's signature was a
      network/uplink stall (`gw=''`), cause fixed (`udhcpc -b`, `765ee81`), and
      the shipped safety net (watchdogs + `wayang.selftest=120`, `61c768f`)
      auto-recovers a hang. The full 7-group block is enabled in
      `configs/defconfig-intel` (`106bb23`). Remaining before a tag: a real
      **cold-boot (power-cycle) soak** — [docs/HW-SESSION-CHECKLIST.md](HW-SESSION-CHECKLIST.md).
      See [docs/GOAL.md](GOAL.md) (M7),
      [docs/ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md),
      [docs/INCIDENT-1.0.13.md](INCIDENT-1.0.13.md),
      [docs/HARDWARE.md](HARDWARE.md).
- [~] **Publish the update channel from CI.** Tagging builds the ISO + bundle
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
      documented in [docs/CHANNEL.md](CHANNEL.md). Still to do: provision the
      secrets and verify a real tag publishes.
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
- [ ] **`wayang.slot` on first install.** Older installers wrote grub.cfg
      without `wayang.slot=`; `mark-ok` now refreshes it, verify on a fresh
      install.

## wayang-fw (dalang-io/wayang-fw)

See `docs/ROADMAP.md` (v0.2+). Remaining:

- [ ] per-zone DHCP/DNS (dnsmasq — not bundled yet);
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
      *All gated on the kernel bisect above — the options are off in shipped
      images, present in the QEMU lab kernel.*
- [ ] IPsec IKEv2; PPPoE; bridge port-level VLANs; Wi-Fi AP.
- Lab: `/tmp/wayang-tools/{bzImage-lab,initramfs-lab.img}` on the builder
  (QEMU only; all router kernel options on).
- Kernel side for most of the above is gated on the bisect item above.

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
      + the 09 EDGEROUTER `a`/`A` keys install a wayangi WayangOS Edge bundle
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
- [x] `radvd` bundled (`scripts/build-radvd.sh`, `/usr/sbin/radvd`) for IPv6
      RA/SLAAC.
- [x] Tech references written: wayang-router `docs/ROUTING-TECH.md`,
      `docs/DAEMONS-TECH.md`; wayang-fw `docs/NFT-REF.md`; wayangi
      `docs/EDGE-PARITY.md`.
- [x] WayangOS is a wayangi Edge unit type (self-managed WireGuard, no agent);
      prod hub deployed with `WAYANGI_EDGE_WOS=1`.
