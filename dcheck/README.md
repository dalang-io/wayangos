# dcheck

Device health check — inspect storage (HDD/SSD/NVMe) identity, capacity, health
and estimated remaining life. Terminal UI, single static binary, no mandatory
external tools.

> Status: **M1–M6** — device enumeration; native SMART (ATA `HDIO_DRIVE_CMD`,
> NVMe ioctl, SCSI best-effort); native identity (ATA IDENTIFY, SCSI
> INQUIRY/VPD); link speed; terminal UI; `smartctl` enrichment; `--json`; and a
> FreeBSD backend (`sysctl` + smartctl).
> Full plan: [`../docs/DCHECK.md`](../docs/DCHECK.md).

## Usage

```
dcheck                  # terminal UI (menu -> storage -> report)
dcheck storage          # list attached storage devices (text)
dcheck storage <dev>    # report for one device (e.g. /dev/nvme0n1)
dcheck storage <dev> --bench           # read-only speed benchmark
dcheck storage <dev> --test short|long # start a SMART self-test
dcheck storage --json   # machine-readable device list
dcheck check            # one-shot health gate (exit code = worst verdict)
dcheck watch --interval 60 --webhook http://host/hook   # monitor + alert
dcheck prometheus       # Prometheus metrics
dcheck storage <dev> --json   # full JSON report for one device
dcheck tui              # force the terminal UI
dcheck demo             # run with built-in sample devices
dcheck ram | cpu        # coming soon
dcheck --version
```

In the UI: `↑`/`↓` move, `Enter` open, `b`/`Esc` back, `PgUp`/`PgDn` and
`Home`/`End` (`g`/`G`) scroll, `r` rescan, `?` help, `q` quit. The mouse wheel
scrolls. SMART reads run in the background with a spinner; the list shows
per-device health.
The palette adapts to a **light or dark terminal**. Force it with
`dcheck tui --light` / `--dark`, `DCHECK_THEME=light`, or `"theme": "light"` in
the config; otherwise it auto-detects via `COLORFGBG` (`NO_COLOR` disables color).

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
| Linux (any distro, static) | full: enumeration, native SMART (ATA/NVMe/SCSI), TUI, JSON |
| WayangOS (minimal rootfs) | full (native SMART — no smartctl needed) |
| macOS | physical disks via `diskutil` (model, size, SSD/HDD, SMART status); full attributes via smartmontools if installed |
| FreeBSD | enumeration + identity/health via smartctl |

## Configuration

- `~/.config/dcheck/config.json` (or `$DCHECK_CONFIG`):
  `{ "temp_warn_c": 60, "watch_interval": 60, "theme": "light" }`
- `~/.config/dcheck/tbw.json` (or `$DCHECK_TBW_JSON`):
  `{ "model substring": TBW_in_TB }`
- Env: `DCHECK_SYS_ROOT` (alternate fs root), `DCHECK_SMART_JSON` (parse a
  captured `smartctl -j` file), `DCHECK_SMART_ARGS` (force `-d ...`),
  `DCHECK_NATIVE` (force native SMART).

## Monitoring

- `dcheck check` → exit 0 ok / 1 unknown / 2 monitor / 3 backup-or-replace.
- `dcheck watch --interval 60 --webhook http://host/hook`
- `dcheck prometheus` for scrapers.

## Build & release

```bash
./scripts/build-dcheck.sh                 # one target (default linux/amd64 musl)
TARGET=aarch64-unknown-linux-musl ./scripts/build-dcheck.sh
./scripts/release-dcheck.sh               # matrix + tarballs + SHA256SUMS
GPG_KEY=0x... ./scripts/release-dcheck.sh # also GPG-sign SHA256SUMS
```

Man page: `dcheck/dcheck.1`.
