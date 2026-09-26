# Bisecting the router kernel options

1.0.13 enabled a large "router" kernel block; the test device locked up after
the shell prompt (keyboard + USB uplink both dead). 1.0.15 brought back only
`CONFIG_VLAN_8021Q`, `CONFIG_BRIDGE` and `CONFIG_BRIDGE_VLAN_FILTERING` and was
validated on the device, so the remaining options are the suspects. This is the
plan and tooling to re-enable them safely, one group at a time.

Harness: `scripts/bisect-router-opts.sh`. Background: `docs/INCIDENT-1.0.13.md`
(do not edit that file; it is the owner's incident log).

## Ground rules

- **The harness never reboots or otherwise touches the device.** It only builds
  things on the build box and prints the manual procedure. Rebooting into the
  idle slot is done by hand at the console, because a bad kernel kills SSH.
- **Builds go to a throwaway `/tmp` dir on `root@10.0.0.251`** (default
  `/tmp/wayangos-bisect`). The script refuses to use `/root/wayangos-build` as
  the work dir. Sources are only *symlinked* from `/root/wayangos-build`; the
  kernel tree is *copied* because `build-kernel.sh` builds in-tree, so the
  shared cache is never written to.
- Bundles are signed with `WAYANG_KEY` (the release key) so the device accepts
  them. Without it the script warns and produces an **unsigned** bundle the
  device will reject.
- Only one group per boot. The options create `bond0`, `dummy0`, `ifb0/1`,
  `gre0`, `gretap0`, `erspan0`, `tunl0` at boot; mixing groups makes a lockup
  unattributable.

## Groups

Groups that create **no netdev at boot** are tested first: if one of them locks
the device, the trigger is a driver/crypto path rather than a stray interface.

| # | group | options | boot netdev | why it matters |
|---|-------|---------|-------------|----------------|
| 1 | `veth-macvlan-tun` | `VETH`, `MACVLAN`, `TUN` | none | virtual links used by router tunnels/namespaces; created only on demand |
| 2 | `wireguard` | `WIREGUARD` + `CRYPTO_LIB_CHACHA{,_ARCH}`, `CRYPTO_LIB_POLY1305{,_ARCH}`, `CRYPTO_LIB_CURVE25519{,_ARCH}` | none | the x86 arch assembly crypto is a prime suspect for a hard lockup |
| 3 | `vrf-multipath` | `NET_VRF`, `IPV6_MULTIPLE_TABLES`, `IPV6_SUBTREES`, `IP_ROUTE_MULTIPATH`, `IP_MULTIPLE_TABLES` | none | policy routing / ECMP / VRF lookup; `IP_MULTIPLE_TABLES` and `IP_ROUTE_MULTIPATH` are already on in the base config |
| 4 | `ipsec` | `INET_ESP`, `XFRM_INTERFACE` | none | ESP / xfrm; interfaces are created explicitly |
| 5 | `dummy-bonding` | `DUMMY`, `BONDING` | `dummy0`, `bond0` | dummy + link aggregation devices |
| 6 | `gre-ipip` | `NET_IPIP`, `NET_IPGRE_DEMUX`, `NET_IPGRE`, `NET_UDP_TUNNEL` | `gre0`, `gretap0`, `erspan0`, `tunl0` | IP tunnels |
| 7 | `qos` | `IFB`, `NET_SCH_{HTB,FQ_CODEL,CAKE,INGRESS}`, `NET_CLS_{U32,FW}`, `NET_ACT_{POLICE,MIRRED}`, `NET_REDIRECT` | `ifb0`, `ifb1` | shaping/classification offloads |

The set is the 1.0.13 block **minus** `VLAN_8021Q`, `BRIDGE` and
`BRIDGE_VLAN_FILTERING`, which 1.0.15 already ships. `LLC`/`STP` are pulled in
automatically by `BRIDGE` and do not need their own entries.

## Using the harness

```sh
# list groups/options only, no build
scripts/bisect-router-opts.sh --list

# build one group (writes dist/router-bisect/bisect-<group>-<ver>-x86_64.wup)
WAYANG_KEY=/path/to/release.key scripts/bisect-router-opts.sh --group wireguard

# build every group, in the order above
WAYANG_KEY=/path/to/release.key scripts/bisect-router-opts.sh

# see what would happen without touching anything
scripts/bisect-router-opts.sh --dry-run --group wireguard
```

Useful flags: `--out DIR`, `--version VER` (default `1.0.99`),
`--kernel-version V` (default `7.2.7`), `--host`, `--work`, `--shared`, `--key`.

For each group the builder:

1. copies `configs/defconfig-intel` to a temporary fragment and appends the
   group's `CONFIG_*=y` lines (the `# WAYANG_BASE:` first line is preserved, so
   `build-kernel.sh` still resolves it as a one-level fragment);
2. runs `scripts/build-kernel.sh`, `scripts/build-rootfs.sh` and
   `scripts/build-bundle.sh` against a `/tmp` build dir;
3. copies the `.wup` back as `bisect-<group>-<ver>-x86_64.wup` and prints the
   device procedure below.

The device procedure is also printed by `--dry-run`, so it can be read without
building anything.

## Device procedure (per bundle)

1. Copy the bundle to the device. There is no `sftp-server`; large uploads can
   drop, so verify the hash:

   ```sh
   sha256sum bisect-<group>-1.0.99-x86_64.wup
   ssh root@163.128.55.3 'cat > /data/bisect.wup' < bisect-<group>-1.0.99-x86_64.wup
   ssh root@163.128.55.3 'sha256sum /data/bisect.wup'
   ```

2. Stage it into the idle slot (no reboot):

   ```sh
   ssh root@163.128.55.3 'wayang update --from /data/bisect.wup'
   ```

3. Reboot into the idle slot **by hand** — pick it in the GRUB menu (3 s
   timeout) or power-cycle. Do **not** run `ssh ... reboot`: if the new kernel
   locks up, that is the only remote path gone.

4. At the console, check the keyboard (Caps Lock LED, then Ctrl-Alt-Del). If
   the box is alive, verify over SSH and let it sit for a few minutes so a late
   lockup shows up:

   ```sh
   ssh root@163.128.55.3 'uname -a; cat /proc/net/dev; dmesg | tail -n 40'
   ```

   Good result: keyboard works, SSH answers, no stray `bond0/dummy0/ifb0/...`
   unless the group is expected to create one, and `active B`/`good B` in
   `wayang status` (after the `6db57a3` updater fix, present since 1.0.14).

5. **Recovery** if it locks up: power-cycle and pick slot A (the last good
   slot) in GRUB. If the menu does not appear, power-cycle and hold the slot-A
   entry. Then, on the device, confirm and re-mark the good slot:

   ```sh
   grep -o 'wayang.slot=[AB]' /proc/cmdline
   wayang update --rollback && wayang mark-ok
   wayang status
   ```

   The updater bug that could mark the wrong slot is fixed by `6db57a3` (1.0.14
   and later); if slot A is running an older release, run the two commands
   above after every manual recovery.

## Interpreting and folding back

Test groups in order until one locks the device, then split that group (halve
its options) and rebuild. Groups that pass can be folded into
`configs/defconfig-intel` immediately, but re-validate the accumulated set on
hardware before publishing:

1. Add the group's options to the router section of
   `configs/defconfig-intel`, with a comment naming the group and the date.
   Keep the file a single fragment (`# WAYANG_BASE: defconfig-qemu` stays the
   first line; do not point it at another fragment).
2. Rebuild the full image and re-test on the device (keyboard + SSH). Some
   symbols are `select`ed by others on x86 (`*_ARCH`, `CURVE25519`, `LLC`,
   `STP`) and can be left for `make olddefconfig` — check the resolved
   `.config` instead of assuming.
3. Publish only after the device boots the accumulated set. Tagging and
   publishing still need the owner's OK (AGENTS.md rule 3).

`WAYANG_KEY` is the release key; the matching public key is baked into the
rootfs (`wayang/trusted_keys`), so only a bundle signed with it is accepted.
