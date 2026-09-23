# dcheck

Device health check — inspect storage (HDD/SSD/NVMe) identity, capacity, health
and estimated remaining life. Terminal UI, single static binary, no mandatory
external tools.

> Status: **M1–M13** — storage (enumeration; native SMART ATA/NVMe/SCSI; ATA
> attributes+thresholds; self-test + read-only bench; link speed), RAM (usage,
> ECC, and SMBIOS modules via `dmidecode` → raw `/sys/firmware/dmi` → `lshw`:
> vendor/type DDR4/DDR5/speed/slots; DDR5 temperature),
> CPU (vendor/model/topology/clock/cache/temp/load), mounted-partition usage
> meters, terminal UI, `--json`,
> `prometheus`, monitoring/alerts, `smartctl` enrichment, TBW overrides, and a
> FreeBSD/macOS backend.
> Full plan: [`../docs/DCHECK.md`](../docs/DCHECK.md).

## Install

Landing page with screenshots and a technician guide:
<https://wayang.dalang.io/apps/dcheck.html>

Linux x86_64 / aarch64 (static binary, checksum-verified):

```bash
curl -fsSL https://wayang.dalang.io/dcheck/install.sh | sh
dcheck update            # later: upgrade in place (--check to only look)
```

`DCHECK_INSTALL_DIR` (default `/usr/local/bin`) and `DCHECK_VERSION` override
the defaults. Releases are published with `scripts/deploy-site.sh`.

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
dcheck ram | cpu        # memory / CPU report (or --json)
dcheck update           # self-update from wayang.dalang.io (--check, --force)
dcheck snapshot DIR     # every TUI screen as SVG (--demo, --mask-serials, --host)
dcheck --version
```

In the UI: `↑`/`↓` move, `Enter` open, `b`/`Esc` back, `1`/`2`/`3` jump to
storage/memory/processor, `PgUp`/`PgDn` and `Home`/`End` (`g`/`G`) scroll,
`r` rescan/refresh, `c` copy the log (OSC52), `?` command reference, `q` quit.
Text stays **selectable** by default; mouse-wheel scrolling is opt-in with
`dcheck tui --mouse` (it disables native selection).

The UI is a **sci-fi HUD**:

- a short boot splash (≤0.5 s, any key skips; `DCHECK_NO_SPLASH=1` or
  `"splash": false` disables it);
- a **command deck** menu with live summary cards for storage, memory and CPU
  (all read in the background while you navigate);
- a **storage array** table with coloured health, remaining-life gauges and
  temperatures, plus a detail strip for the selected device;
- a per-device **dashboard**: VITALS (verdict, life / temperature / endurance
  gauges, power-on time, life estimate, alerts) next to the scrollable
  TELEMETRY LOG (the full text report);
- memory (usage/swap gauges, DIMM slot map, ECC) and processor (load /
  temperature / clock gauges, thread grid) dashboards.

Colour: **neon truecolor** (cyan / magenta / amber on blue-black) when the
terminal sets `COLORTERM=truecolor`, otherwise the terminal's **ANSI palette**
(Linux console, older terminals). Force either with `DCHECK_COLOR=truecolor` or
`DCHECK_COLOR=ansi`. `--light` selects the light variant, `--transparent` (or
`"transparent": true` / `DCHECK_TRANSPARENT`) keeps your terminal background,
`NO_COLOR` disables colour, and `--plain` switches every glyph to ASCII for
fonts without box-drawing/geometric symbols. It only redraws on input or while
something is loading, so it stays cheap on low-spec devices.

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

Requires Rust 1.98.1 (pinned in `rust-toolchain.toml`).

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
  `{ "temp_warn_c": 60, "watch_interval": 60, "theme": "light", "mouse": true, "plain": false, "transparent": false, "splash": true, "hdd_design_years": 5 }`
  (`hdd_design_years` is the **assumed** HDD design life at 24/7 used for HDD
  life estimates — drives do not report one; default 5 years = 43,800 h)
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
