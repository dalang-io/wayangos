# TODO — pending work (for the next agent)

Cross-repo backlog. Each repo also has its own roadmap: this file is the index
of what is *not* done, with pointers. Status: `[ ]` open, `[~]` in progress,
`[x]` done. Read `AGENTS.md` in each repo before starting.

## wayangos (this repo)

- [ ] **Re-enable the full router kernel block, bisected.** 1.0.13's block
      locked the test device (docs/INCIDENT-1.0.13.md). 1.0.15 restored only
      `VLAN_8021Q` + `BRIDGE` (+`BRIDGE_VLAN_FILTERING`). Re-add the rest **one
      group at a time**, booting the device after each, and confirm keyboard +
      SSH: `BONDING/DUMMY/VETH/MACVLAN/TUN`, `WIREGUARD` (+arch crypto),
      `NET_IPGRE*/NET_IPIP`, `NET_VRF` + `IP_ROUTE_MULTIPATH` +
      `IPV6_MULTIPLE_TABLES`, `INET_ESP`/`XFRM_INTERFACE`, `NET_SCH_*`/
      `NET_CLS_*`/`NET_ACT_*`/`IFB`.
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
- [ ] **`wayang` console polish.** Remaining ideas: a real (determinate)
      update progress bar once the updater reports byte counts; a SYSTEM-slot
      "boot other slot" action; consolidate the SSH screen with `wayang-addkey`.
- [x] **Secondary-NIC DHCP ergonomics.** `wayang-net dhcp <iface>` leases
      address-only; added a persisted "also lease these" list at
      `/data/etc/network/also` (`wayang-net also [<iface> on|off]`), leased
      address-only at boot on top of the one-shot secondary pass, plus a TUI
      toggle (`l`) in `wayang net`.
- [ ] **`wayang.slot` on first install.** Older installers wrote grub.cfg
      without `wayang.slot=`; `mark-ok` now refreshes it, verify on a fresh
      install.

## wayang-fw (dalang-io/wayang-fw)

See `docs/ROADMAP.md` (v0.2+). Not started:

- [ ] live conntrack / drop-log views (dashboard has charts + activity now);
- [ ] per-zone DHCP/DNS (dnsmasq);
- [ ] nft sets / FQDN objects, schedules (time-based rules);
- [ ] hairpin NAT; interface config from the HUD.
- [ ] Firewall enforcement is **QEMU-tested only** — run the lab
      `tests/lab/lab.py` and then validate on real hardware.

## wayang-router (dalang-io/wayang-router)

See `docs/ROADMAP.md`. v0.1 (MVP) is usable (ports, 802.1Q VLANs, bridges,
static v4/v6, DHCP client, forwarding, static routes):

- [ ] **netlink backend** to replace `vconfig`/`brctl` (fewer spawns);
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
      Documented gaps, not preview labels: verify/recover are Linux-only;
      undelete assumes FAT32/exFAT files are contiguous, does not follow NTFS
      `$ATTRIBUTE_LIST`, and recovers ntfs3-deleted files without a name.

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
