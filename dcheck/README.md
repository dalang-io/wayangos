# dcheck

Device health check — inspect storage (HDD/SSD/NVMe) identity, capacity, health
and estimated remaining life. Terminal UI, single static binary, no mandatory
external tools.

> Status: **M1–M5** — device enumeration; native SMART (ATA `HDIO_DRIVE_CMD`,
> NVMe ioctl, SCSI best-effort); native identity (ATA IDENTIFY, SCSI
> INQUIRY/VPD); link speed; terminal UI; `smartctl` enrichment when available.
> FreeBSD/JSON/packaging (M6) pending.
> Full plan: [`../docs/DCHECK.md`](../docs/DCHECK.md).

## Usage

```
dcheck                  # terminal UI (menu -> storage -> report)
dcheck storage          # list attached storage devices (text)
dcheck storage <dev>    # report for one device (e.g. /dev/nvme0n1)
dcheck tui              # force the terminal UI
dcheck demo             # run with built-in sample devices
dcheck ram | cpu        # coming soon
dcheck --version
```

In the UI: `↑`/`↓` move, `Enter` select, `b`/`Esc` back, `PgUp`/`PgDn` scroll,
`q` quit.

Devices are read from `/sys/block`, so they are listed as soon as they are
**attached**, mounted or not. Health is read **natively** (ATA `HDIO_DRIVE_CMD`,
NVMe admin ioctl) and enriched by `smartctl -a -j` when installed. **SMART reads
require root** (`sudo dcheck storage`). Set `DCHECK_NATIVE=1` to force the
native path.

**On macOS / any host without `/sys`**, dcheck automatically falls back to
built-in demo data, so you can try it with just:

```bash
cd dcheck
cargo run                 # menu
cargo run -- storage      # device list
cargo run -- storage /dev/nvme0n1
```

## Build

Requires Rust (1.74+).

```bash
# Native debug build for local testing
cargo build
cargo test

# Static Linux release binary
./scripts/build-dcheck.sh
# -> dist/dcheck-x86_64-unknown-linux-musl
```

`scripts/build-dcheck.sh` cross-links with `zig cc` on non-Linux hosts; on Linux
it uses the musl target directly. Targets: `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl`, `x86_64-unknown-freebsd`.

## Testing

```bash
# Unit tests
cargo test

# End-to-end against a synthetic /sys + /proc fixture (works on any host)
./scripts/test-dcheck.sh
```

The fixture test points the tool at a fake filesystem root via the
`DCHECK_SYS_ROOT` environment variable, covering NVMe/SATA/USB/MMC devices,
mounted and unmounted partitions, and ignored virtual devices.

## Design notes

- Data is sourced from `/sys` and `/proc` (no `lsblk`/`udev` dependency), so it
  runs inside a minimal WayangOS initramfs.
- Read-only: never writes to block devices.
- Native SMART via raw ioctl is the baseline; `smartctl -j` is optional
  enrichment (M2).

## Platform support

| OS | Status |
|----|--------|
| Linux (any distro, static) | M1 enumeration; SMART in M3 |
| WayangOS (minimal rootfs) | M1 enumeration |
| FreeBSD | planned (M6) |
