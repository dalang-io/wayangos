# TODO — pending work (for the next agent)

Cross-repo backlog. Each repo also has its own roadmap: this file is the index
of what is *not* done, with pointers. Status: `[ ]` open, `[~]` in progress,
`[x]` done. Read `AGENTS.md` in each repo before starting.

## wayangos (this repo)

- [~] **Re-enable the full router kernel block, bisected.** 1.0.13's block
      locked the test device (docs/INCIDENT-1.0.13.md). 1.0.15 restored only
      `VLAN_8021Q` + `BRIDGE` (+`BRIDGE_VLAN_FILTERING`). A safe harness is now
      in place: `scripts/bisect-router-opts.sh --list` (build one group at a
      time on the builder, never `/root/wayangos-build`) and
      `docs/ROUTER-KERNEL-BISECT.md` (device procedure + recovery). **Needs a
      supervised hardware session** — booting each group on the device and
      confirming keyboard + SSH, recovering via GRUB slot A / power-cycle.
      Groups, safest first: veth-macvlan-tun, wireguard, vrf-multipath, ipsec,
      dummy-bonding, gre-ipip, qos.
- [~] **Publish the update channel from CI.** Tagging builds the ISO + bundle
      and creates the GitHub release, but `wayang.dalang.io/channel` is
      published by hand (`scripts/publish-channel.sh`). Wired into the tag path
      of `.github/workflows/installer-iso.yml` (best-effort, tag-only); it stays
      dormant until the deploy secrets exist. Required repo secrets:
      `WAYANG_DEPLOY_HOST` (ssh target, e.g. the `root@10.0.0.251` default in
      `publish-channel.sh`) and `WAYANG_DEPLOY_KEY` (private key authorised on
      that host). Optional overrides: `WAYANG_DEPLOY_REMOTE_DIR`,
      `WAYANG_DEPLOY_CHANNEL_SUBDIR`. Still to do: provision the secrets and
      verify a real tag publishes.
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
- [ ] **`wayang.slot` on first install.** Older installers wrote grub.cfg
      without `wayang.slot=`; `mark-ok` now refreshes it, verify on a fresh
      install.

## wayang-fw (dalang-io/wayang-fw)

See `docs/ROADMAP.md` (v0.2+). Remaining:

- [ ] per-zone DHCP/DNS (dnsmasq — not bundled yet);
- [ ] nft sets / FQDN objects;
- [ ] interface config from the HUD.
- [ ] Firewall enforcement is **QEMU-tested only** — run the lab
      `tests/lab/lab.py` and then validate on real hardware.
- [x] live conntrack / drop-log views → **DROPS** screen (module 08) reads the
      `wfw:` counters + kernel drop log, grouped by source/service.
- [x] rule **schedules** (time windows, `meta day`/`meta hour`) and **hairpin
      NAT** (port-forward reflection + masquerade) as config + render, with TUI
      fields.

## wayang-router (dalang-io/wayang-router)

See `docs/ROADMAP.md`. v0.1 (MVP) is usable (ports, 802.1Q VLANs, bridges,
static v4/v6, DHCP client, forwarding, static routes):

- [x] **netlink backend** for addresses/links/VLAN/bridge (rtnetlink via raw
      `libc`, no new crate), falling back to `ip`/`vconfig`/`brctl`; `ip route`
      stays command-based for now.
- [ ] WireGuard (generic-netlink, no `wg` binary), then IPsec IKEv2;
- [ ] VRF, policy routing, ECMP, multi-WAN failover;
- [ ] QoS (HTB + fq_codel) per subnet/host/VLAN;
- [ ] BGP/OSPF/BFD; PPPoE; Wi-Fi AP.
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
