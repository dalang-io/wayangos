# TODO — pending work (for the next agent)

Cross-repo backlog. Each repo also has its own roadmap: this file is the index
of what is *not* done, with pointers. Status: `[ ]` open, `[~]` in progress,
`[x]` done. Read `AGENTS.md` in each repo before starting. Current WayangOS
release: **1.0.17** (see `docs/HANDOVER.md`).

## wayangos (this repo)

- [ ] **WayangOS as a wayangi EdgeRouter** — enrol on the wayangi dashboard,
      tunnel via WireGuard, receive a delegated IPv6 prefix and route/firewall
      it to a LAN. Full design + required system/flow: [docs/EDGEROUTER.md](EDGEROUTER.md).
      Blocked by the WireGuard + vrf-multipath kernel groups (below).
- [~] **Re-enable the full router kernel block, bisected.** 1.0.13's block
      locked the test device (docs/INCIDENT-1.0.13.md). 1.0.15 restored only
      `VLAN_8021Q` + `BRIDGE` (+`BRIDGE_VLAN_FILTERING`). A safe harness is now
      in place: `scripts/bisect-router-opts.sh --list` (build one group at a
      time on the builder, never `/root/wayangos-build`) and
      `docs/ROUTER-KERNEL-BISECT.md` (device procedure + recovery). **Needs a
      supervised hardware session** — booting each group on the device and
      confirming keyboard + SSH, recovering via GRUB slot A / power-cycle.
      Groups, safest first: baseline (safety net only), veth-macvlan-tun,
      wireguard, vrf-multipath, ipsec, dummy-bonding, gre-ipip, qos.
      **Remote option now prepared**: every bisect kernel carries a safety net
      (TCO watchdog, lockup panics, `panic=10`) and `--unattended SECS` adds
      `wayang.selftest` (self-test → mark-ok, or `wayang update --fallback` +
      reboot). QEMU-proven (nonet/hang/panic all return to A); validate the
      `baseline` group supervised on the device first
      (docs/ROUTER-KERNEL-BISECT.md → "Remote bisect (unattended)").
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
