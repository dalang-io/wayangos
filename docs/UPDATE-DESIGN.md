# WayangOS update — frozen interfaces (v0)

Single source of truth for the parallel implementation of
[`UPDATE-TODO.md`](UPDATE-TODO.md). Do not change these without updating this
file.

## Versions & runtime files (rootfs)

- `/etc/wayang/version` — plain text semver, e.g. `1.4.1` (one line).
- `/etc/wayang/channel` — `stable` (default) or `edge` (one line, optional).
- `/etc/wayang/trusted_keys` — lines `<keyid> <64-hex-ed25519-pub>`; `#` comments.
- `wayang` binary at `/usr/bin/wayang` (static), built from the `wayang/` crate.

## ESP layout (installed system)

The installer creates one **512 MiB vfat ESP labelled `WAYANGBOOT`** and one
ext4 `WAYANGDATA` (`/data`). On the ESP:

```
/boot/A/vmlinuz
/boot/A/initramfs.img
/boot/B/vmlinuz
/boot/B/initramfs.img
/boot/grub/grub.cfg
/boot/grub/grubenv            # GRUB environment block (fallback state)
/boot/var/meta-A.json
/boot/var/meta-B.json
```

Runtime find of the ESP: `WAYANG_ESP` env (partition device) → else
`/dev/disk/by-label/WAYANGBOOT` → else parse `blkid` for `LABEL="WAYANGBOOT"`.
`wayang` mounts it read-write at a temp dir when needed.

Environment overrides (for testing / cross-staging): `WAYANG_ROOT` (fake `/etc`
+ `/boot` tree), `WAYANG_ESP`, `WAYANG_ARCH` (override the host arch token),
`WAYANG_REPO_URL`.

## Fallback state (GRUB env)

Stored in `/boot/grub/grubenv` using GRUB's 1024-byte format
(`# GRUB Environment Block\n` + `key=value\n` lines, `#`-padded):

- `wayang_slot` — slot to boot next (`A`|`B`).
- `wayang_good` — last slot that booted successfully (`A`|`B`).
- `wayang_attempts` — integer.

Rules:
- `wayang` stages an update into the idle slot, then sets
  `wayang_slot=<target>`, `wayang_attempts=0`.
- GRUB (`/boot/grub/grub.cfg`) on every boot: if `wayang_attempts >= 3` use
  `wayang_good`, else `wayang_slot`; then bump `wayang_attempts` and
  `save_env`.
- init (`rcS`) runs `wayang mark-ok` once the system is ready; that sets
  `wayang_good=<active>`, `wayang_attempts=0`.

## Bundle `*.wup` (tar.gz, members at archive root)

```
manifest.json
vmlinuz
initramfs.img
manifest.json.sig     # raw 64-byte ed25519 signature over manifest.json bytes
```

`manifest.json`:
```json
{
  "product": "wayangos",
  "channel": "stable",
  "version": "1.4.1",
  "major": 1,
  "arch": "x86_64",
  "edition": "intel",
  "kernel_sha256": "<hex>",
  "initramfs_sha256": "<hex>",
  "min_from": "1.0.0",
  "notes": "text",
  "time": 1760000000,
  "keyid": "release"
}
```
Verification: SHA-256 of `vmlinuz`/`initramfs.img` must match; `manifest.json.sig`
must verify against the pubkey for `keyid` in `trusted_keys`.

## `wayang` CLI

```
wayang version
wayang status [--json]
wayang update  [--check] [--from FILE.wup] [--channel C] [--esp DEV] [--reboot]
wayang upgrade [--check] [--from FILE.wup] [--esp DEV] [--reboot]
wayang update --rollback [--esp DEV] [--reboot]
wayang keygen --out DIR [--keyid NAME]
wayang sign   --key FILE [--keyid NAME] MANIFEST.json
wayang verify FILE.wup [--esp DEV]
wayang mark-ok [--esp DEV]
```

Semantics:
- `update`: minor/patch within the installed **major**.
- `upgrade`: cross-major (explicit).
- `--check`: report only, change nothing.
- `--from`: offline bundle; otherwise fetch the channel manifest over HTTPS.
- `--reboot`: reboot after a successful stage (default: print and exit).

Exit codes: `0` ok · `1` error · `2` no update available · `3` verify/signature
failure · `4` incompatible (arch/min_from).

## HTTP endpoints (channel)

```
GET <base>/<channel>/<arch>/manifest.json      # same schema as bundle manifest
GET <base>/<channel>/<arch>/wayang-<version>-<arch>.wup
```
`base` default: `https://github.com/dalang-io/wayangos/releases/latest/download`
(overridable with `WAYANG_REPO_URL`). `arch` ∈ `x86_64`, `arm64`.

## File ownership (parallel work)

- **A — updater crate**: `wayang/**`, `scripts/build-wayang.sh`, CI job.
- **B — boot/installer/rootfs**: `installer/**`, `scripts/build-iso.sh`,
  `scripts/build-installer-iso.sh`, `scripts/build-rootfs.sh`.
- **C — bundles/release/docs**: `scripts/build-bundle.sh`,
  `scripts/release-wayang.sh`, `docs/UPDATE-TODO.md`, `landing-page/**`.
- **Orchestrator**: this file.

## Channel publishing layout (for online `wayang update`)

Releases publish this tree (e.g. as GitHub release assets or a CDN/Pages dir):

```
<base>/<channel>/<arch>/manifest.json
<base>/<channel>/<arch>/wayang-<version>-<arch>.wup
```

`<base>` default `https://github.com/dalang-io/wayangos/releases/latest/download`
(override `WAYANG_REPO_URL`). `manifest.json` is the same schema as the bundle's
manifest. `scripts/release-wayang.sh` must emit this tree under `dist/channel/…`
plus `SHA256SUMS`, and print (not run) the publish command.

## ARM64 slots (M7)

No GRUB on ARM. The FAT **boot partition** is the ESP-equivalent; slot files live
under a per-slot prefix:

```
Raspberry Pi (config.txt):   /A/vmlinuz /A/initramfs.img /B/...  + /config.txt (os_prefix)
Orange Pi (U-Boot/extlinux): /A/... /B/...  + /extlinux/extlinux.cfg (two labels)
```

Fallback state uses the **same keys** as GRUB env (`wayang_slot`, `wayang_good`,
`wayang_attempts`) but stored as a plain `key=value` file at
`<boot>/wayang/vars` (since GRUB env only exists on x86).

`wayang` state backend resolution:
- if `<boot>/grub/grubenv` exists → use it (x86);
- else if `<boot>/wayang/vars` exists → use it (ARM);
- else error with a clear message.

RPi fallback: rely on the firmware `tryboot`/`autoboot.txt` mechanism or a small
bootloader script; Orange Pi: U-Boot picks the label from the state file (best
effort). Keep `wayang update --rollback` working on both (swap `wayang_slot`).