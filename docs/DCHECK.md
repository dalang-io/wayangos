# dcheck — Device Health Check (Plan)

Status: **M1–M11 done** (Linux + FreeBSD + macOS; native + smartctl; ATA attributes+thresholds; self-test + bench; link speed; TUI; `--json`; monitoring/alerts; TBW overrides; passthrough probing; NVMe extras; config; Prometheus; man page) · Owner: TBD

`dcheck` is a single, self-contained command-line tool to check the health of a
machine's hardware. MVP focuses on **storage** (HDD/SSD/NVMe), with RAM and CPU
as follow-ups.

## 1. Goal

- One tool, one UX, runs on:
  - **WayangOS** (minimal initramfs rootfs, running as root)
  - Any Linux: Debian, Ubuntu, Fedora, Arch, Kali, Alpine (static binary, no libc deps)
  - **FreeBSD** (best-effort; see platform layer)
- Terminal UI (TUI), plus scriptable output.
- Never writes to disks, never triggers destructive operations by default.

### Non-goals (MVP)

- RAM diagnosis and CPU benchmarks (menu entries exist, marked "coming soon").
- Filesystem repair, partitioning, RAID reconstruction.
- Windows/macOS support.

## 2. One codebase, per-platform binaries

"One binary" = **one source tree** that cross-compiles to static binaries. A
single artifact cannot serve Linux *and* FreeBSD kernels, so we ship a matrix:

| GOOS    | GOARCH | Notes |
|---------|--------|-------|
| linux   | amd64  | Debian/Ubuntu/Fedora/Arch/Kali/WayangOS |
| linux   | arm64  | RPi/Orange Pi WayangOS editions |
| freebsd | amd64  | via `smartctl`/`camcontrol` |
| macos   | arm64/amd64 | via `diskutil` (identity + SMART status); smartctl for full attributes |

A static Linux binary built with `CGO_ENABLED=0` runs across glibc/musl distros
and inside the WayangOS initramfs (no shared libraries).

## 3. Language & libraries

**Rust**, native-first (no mandatory external tools).

Rationale: no runtime/GC and instant startup (ideal for a minimal WayangOS
rootfs), static binaries via the `musl` targets, ergonomic native `ioctl`
through `nix`/`libc`, a strong TUI stack, and a supported FreeBSD target.

- Allocator/std: `std` only for M1 (no crates); grow later.
- Syscalls/ioctl: `libc` (M3) or `nix`.
- TUI: `ratatui` + `crossterm` (vendored → static).
- Structured output: `serde_json` (M6).
- Native SMART: raw ioctl (NVMe admin; ATA pass-through via `SG_IO`).
- Enrichment: optionally shell out to `smartctl -j` when available.

> Alternative considered: Go. Rejected in favor of Rust for smaller/leaner
> static binaries and first-class raw-device ioctl; Go's only real edge is
> trivial cross-compilation. Tradeoff: Rust needs a musl/`zig cc` linker for
> cross builds.

## 4. CLI / UX flow

```
dcheck                      # TUI: main menu
dcheck storage              # TUI: storage device list
dcheck storage <dev>        # direct report for one device (e.g. /dev/nvme0n1)
dcheck storage --json       # machine-readable report for all devices
dcheck ram | dcheck cpu     # "coming soon" placeholders (MVP)
dcheck --version
```

### TUI screens

```
Screen 1 — Main menu
  ┌ dcheck ──────────────────────┐
  │  ▸ Storage                   │
  │    RAM          (coming soon)│
  │    CPU          (coming soon)│
  │    Quit                      │
  └──────────────────────────────┘

Screen 2 — Storage list (includes unmounted, as long as attached)
  DEV          TYPE   MODEL                 CAP    BUS    HEALTH  MOUNT
  /dev/nvme0n1 NVMe   Samsung SSD 980 500GB 465 GB PCIe   PASS    / 
  /dev/sda     SSD    Kingston A400 240G    223 GB SATA   PASS    -
  /dev/sdb     HDD    WDC WD10SPZX-00Z     931 GB SATA   WARN    -
  /dev/sdc     USB    Generic Flash Disk     14 GB USB    ?       -

Screen 3 — Report (scroll: j/k, b=back, r=refresh, q=quit)
  [ Identity ] [ Capacity ] [ Interface ] [ Health ] [ Life estimate ]
```

Keys: `↑/↓` move, `Enter` select, `b/Esc` back, `r` refresh, `q` quit.

## 5. Architecture

```
dcheck/
├── Cargo.toml
├── src/
│   ├── main.rs                 # arg parsing → routes to TUI or report
│   ├── model.rs                # Device, SmartData, Report structs
│   ├── platform/
│   │   ├── mod.rs
│   │   ├── linux.rs            # /sys, /proc, ioctl, smartctl
│   │   └── freebsd.rs          # camcontrol, smartctl, sysctl
│   ├── enumerate.rs            # discover block devices (attached ≠ mounted)
│   ├── smart/
│   │   ├── mod.rs
│   │   ├── smartctl.rs         # parse `smartctl -j` JSON (enrichment/fallback)
│   │   ├── nvme_linux.rs       # NVMe SMART/Health log via ioctl (log 0x02)
│   │   └── ata_linux.rs        # ATA SMART via SG_IO pass-through
│   ├── health.rs               # TBW, wear, bad sectors, verdicts
│   ├── life.rs                 # endurance → remaining-life estimates
│   ├── speed.rs                # interface link speed (+ optional read bench)
│   ├── tui.rs                  # ratatui screens
│   └── report.rs               # text table + JSON emitters
└── README.md
```

## 6. Storage discovery (Linux)

Everything under `/sys` so it works without `lsblk`/`udev` on a minimal rootfs:

| Field | Source |
|-------|--------|
| Devices | `/sys/block/*` (skip `loop*`, `ram*`, `zram*`) |
| Type SSD/HDD | `/sys/block/<d>/queue/rotational` (0=SSD, 1=HDD); `nvme*` → NVMe |
| Capacity | `/sys/block/<d>/size` × logical block size |
| Vendor/Model | `/sys/block/<d>/device/{vendor,model}` (SATA); `/sys/class/nvme/nvme0/{model,serial,firmware_rev}` |
| Firmware | `device/rev` or nvme `firmware_rev` |
| Serial | `device/serial` or nvme `serial` |
| Bus/Transport | resolve `/sys/block/<d>/device` symlink → `ata*/` (SATA), `nvme`, USB |
| Partitions | `/sys/block/<d>/<d>N`, `/proc/partitions` |
| Mountpoint/FS | `/proc/self/mountinfo` (join by major:minor) |
| **Unmounted devices** | always listed (they still appear in `/sys/block`) |

**Testability:** the filesystem root is overridable via `DCHECK_SYS_ROOT`, so
enumeration/reporting can be tested against a synthetic `/sys` + `/proc` fixture
(`scripts/test-dcheck.sh`) on any host, and against a chroot/offline image.

## 7. SMART / health data

Priority order:

1. **Native ioctl** (implemented, M3; no external deps):
   - ATA: `HDIO_DRIVE_CMD` SMART READ DATA (0xD0) + SMART RETURN STATUS (0xDA)
     → attributes, power-on hours/cycles, temperature, TBW, pass/fail.
     (`SG_IO` ATA PASS-THROUGH was prototyped but rejected by libata on test
     hardware; `HDIO_DRIVE_CMD` is simpler and reliable.)
   - NVMe: admin Get Log Page, `LOG_ID 0x02` (SMART/Health) → health %,
     temperature, data units written/read, power-on hours/cycles, critical
     warning. Parsed from the 512-byte log.
2. **`smartctl -a -j`** (implemented, M2) — used automatically when present and
   the native path yields nothing. Adds SATA link speed, form factor, thresholds.
   Parsed with a built-in, std-only JSON reader. `DCHECK_SMART_JSON=<file>`
   parses a captured file; `DCHECK_NATIVE=1` forces the native path.
3. If neither works: show identity/capacity, mark health unavailable.

### Field mapping (examples)

| Metric | NVMe | ATA/SATA |
|--------|------|----------|
| Overall health | `critical_warning` | SMART RETURN STATUS + attributes |
| Percentage used | `percentage_used` (0–255%) | attr 231/233 (vendor) |
| Power-on hours | `power_on_hours` | attr 9 |
| Temp | `temperature` | attr 194 |
| Total bytes written | `data_units_written` × 512 000 | attr 241 (Total LBAs Written) × LBA |
| Media errors | `media_errors` | attr 197/198 (pending/uncorrectable) |
| Reallocated sectors | n/a | attr 5 |
| Available spare | `available_spare` | n/a |

### Permissions

SMART requires root (`CAP_SYS_ADMIN` / raw device access). WayangOS runs as
root; other distros: warn clearly if not root.

### Kernel requirements

The WayangOS kernel configs must expose the relevant block drivers:

- SATA: `CONFIG_ATA`, `CONFIG_ATA_SFF`, `CONFIG_SATA_AHCI`
- NVMe: `CONFIG_BLK_DEV_NVME`
- USB storage: `CONFIG_USB_STORAGE`
- SCSI (for `SG_IO`/SMART on SATA): `CONFIG_BLK_DEV_SD`, `CONFIG_SCSI`
- MMC (ARM SBCs): `CONFIG_MMC*`

> TODO: audit `configs/defconfig-*` and add any missing options (esp. NVMe/SG on
> WayangOS minimal builds).

## 8. Report contents (per selected device)

1. **Identity** — brand/vendor, model, serial, firmware revision, form factor,
   bus/transport (SATA/NVMe/USB/MMC).
2. **Capacity** — raw size, logical/physical sector size, partitions, filesystem,
   mountpoint, used/free (if mounted).
3. **Interface speed** — negotiated link:
   - SATA: `/sys/class/ata_link/link*/sata_spd` (e.g. `6.0 Gbps`)
   - NVMe: `/sys/class/nvme/nvme0/device/current_link_speed` + `current_link_width`
     (e.g. `16.0 GT/s x4`)
   - USB: `/sys/block/<d>/device/speed`
   - Optional read-only benchmark behind `--bench` (see §9).
4. **Health** — overall verdict, power-on hours, power cycles, temperature, SMART
   attributes summary, SSD wear / HDD bad sectors.
5. **Life estimate** — TBW written vs rated, remaining endurance, and projected
   remaining life under **24/7** and **8/7 (daily office)** usage (see §10).
6. **Recommendation** — e.g. `OK`, `Monitor`, `Back up now`, `Replace soon`.

## 9. Speed

- Default: report **negotiated interface speed** (non-invasive, instant).
- `--bench` (opt-in): read-only sequential (e.g. 1 GB) and 4K QD1 read using
  `O_DIRECT`; clearly warn and require root. Never write.

## 10. Health & remaining-life model

### SSD (NVMe + ATA)

```
written_TB      = data_units_written * 512000 / 1e12        # NVMe
                = total_lbas_written * lba_size / 1e12      # ATA
write_rate      = written_TB / power_on_hours               # TB/hour
rated_TBW       = vendor_db(model) or fallback(capacity)
remaining_TBW   = max(rated_TBW - written_TB, 0)
remaining_POH   = remaining_TBW / write_rate                # device-on hours left

life_days_247   = remaining_POH / 24          # powered 24h/day
life_days_87    = remaining_POH / 8           # office: 8h/day, 7 days/week

wear_pct        = percentage_used (NVMe) or vendor attrs (231/233)
# cross-check: min(life from TBW, life from wear_pct)
```

**Rated TBW lookup** (embedded, best-effort table by model/serial pattern):
e.g. Samsung 870 EVO 500GB → 300 TBW, Kingston A400 240G → 80 TBW.
Fallback when unknown, conservative consumer assumption:

```
rated_TBW ≈ capacity_TB * 0.3 * 365 * 5     # ≈0.3 DWPD for 5 years
```

### HDD

No TBW. Use SMART health + ageing heuristic:

```
design_life ≈ 5 years (43800 h)     # typical consumer/enterprise range
remaining   ≈ max(design_life - power_on_hours, 0)
```

Flag on: reallocated sectors > 0, pending sectors > 0, offline uncorrectable > 0,
spin retry > 0, SMART FAILED, temperature beyond range.

> Estimates are **projections, not guarantees**. The doc will state assumptions
> and confidence (High when `percentage_used`/TBW present, Low otherwise).

## 11. Output modes

- TUI (default, interactive).
- `--json` for automation/scripts (stable schema, compact, keys sorted):
  - `dcheck storage --json` → array of devices (identity + capacity + partitions)
  - `dcheck storage <dev> --json` → full object incl. interface and health
- Future: `--prometheus`, `--quiet` (exit code = worst health).

## 12. Safety principles

- Read-only by default; no writes to block devices.
- No destructive SMART self-tests by default (`--test short` may come later).
- Root access used only for SMART/ioctl reads.
- Works on unmounted devices without mounting them.

## 13. Build & distribution

```
dcheck/
  Cargo.toml
  scripts/build-dcheck.sh
```

- Static release build:
  `cargo build --release --target x86_64-unknown-linux-musl`
  (profile: `opt-level="z"`, `lto`, `panic="abort"`, `strip`).
- Cross-linking on a non-Linux host uses `zig cc` as the linker (or a musl
  toolchain). `scripts/build-dcheck.sh` wires this up.
- `scripts/release-dcheck.sh` builds the matrix and tars each binary + README.
- Matrix: linux/amd64, linux/arm64 (`aarch64-unknown-linux-musl`), freebsd/amd64.
  (FreeBSD binaries are produced on FreeBSD or with a FreeBSD cross-linker.)
- FreeBSD is implemented via `sysctl kern.disks` for enumeration and `smartctl`
  for identity/health (the Linux ioctl paths are `cfg`-gated out).
- Ship alongside WayangOS ISOs; also installable standalone.
- CI: matrix build + tests + JSON smoke (`.github/workflows/ci.yml`).

## 14. Milestones

| # | Deliverable | Notes |
|---|-------------|-------|
| M0 | This plan | done |
| M1 | CLI skeleton + enumerate devices + identity/capacity | **done** (no SMART yet) |
| M2 | Health via `smartctl -j` (if present) | **done** (ATA + NVMe parsing) |
| M3 | Native NVMe ioctl + ATA `SG_IO` fallback | **done** (ATA via `HDIO_DRIVE_CMD`; NVMe ioctl, not live-tested) |
| M4 | TBW / wear / life-estimate + vendor DB + link speed + SCSI identity | **done** (SCSI health best-effort; some HBAs/PERC block LOG SENSE) |
| M5 | TUI polish (menu, list, report) + `storage` shorthand | **done** (ratatui; ↑/↓, Enter, b/Esc, PgUp/PgDn, q) |
| M6 | FreeBSD backend, `--json`, packaging, CI | **done** (FreeBSD via `sysctl`+smartctl; compact JSON; release script; CI matrix) |

MVP = M1–M5 (Linux). RAM/CPU are post-MVP.

## 15. Open questions

1. ~~**Language**~~ — **decided: Rust, native-first** (no mandatory `smartctl`).
2. **smartctl**: keep as optional enrichment only (native ioctl is the baseline)?
3. **Vendor TBW DB**: maintain embedded table, or allow user overrides
   (`~/.config/dcheck/tbw.json`)?
4. **Benchmark**: include opt-in read benchmark in MVP, or defer?
5. **Where it lives**: `dcheck/` in this repo (current) vs a separate repo?
   Landing/`apps.html` could promote it later.
6. **FreeBSD depth**: smartctl/camcontrol best-effort is enough for MVP?

## 16. Roadmap to parity with CrystalDiskInfo / Hard Disk Sentinel (M7+)

Goal: close the feature gap with mature tools. Ordered by value and by what we
can verify on real hardware (Fedora SATA + Dell R630 SAS).

### M7 — Full ATA SMART attributes + thresholds (accuracy) — DONE
- Read `SMART READ THRESHOLDS` (0xD1) alongside `SMART READ DATA` (0xD0). ✅
- Parse all 30 attributes: id, name, flags (pre-failure/advisory), value, worst,
  threshold, raw. ✅
- Verdict uses real pre-failure failures (`value <= threshold`, threshold > 0),
  not just issue counts → closer to CDI "Caution/Bad". ✅
- Show the attribute table in report, TUI and JSON. ✅
- **AC:** verified on real SATA (attribute table + thresholds shown; verdict uses
  failing attributes).

### M8 — Self-test + read-only benchmark — DONE
- `dcheck storage <dev> --test short|long` (native ATA; smartctl fallback). ✅
- `dcheck storage <dev> --bench` read-only (sequential + 4K QD1, `O_DIRECT`). ✅
- **AC:** verified on real SATA (self-test completed; 128 MB/s seq, ~3.3k IOPS 4K).

### M9 — Monitoring & alerts — DONE
- `dcheck check [--json]` one-shot gate; exit code 0/1/2/3 by worst verdict. ✅
- `dcheck watch [--interval S] [--json] [--quiet] [--webhook URL]` loop. ✅
- Alerts on verdict worsening or new issues; webhook POST via `curl`. ✅
- **AC:** verified on real SATA (check exit 0; watch alerts once on device appear).

### M10 — Hardware coverage + endurance overrides — DONE
- Vendor TBW overrides from `~/.config/dcheck/tbw.json` (or `$DCHECK_TBW_JSON`). ✅
- Passthrough probing: bus-aware `-d` candidates (USB `sat`/`usbjmicron`/…,
  SCSI `megaraid,N`/`3ware`/`areca`/`cciss`); `DCHECK_SMART_ARGS="-d X"` forces
  one. Auto-tried when the default fails. ✅ (unit-tested; USB/RAID hardware not
  available here)
- NVMe extras: media errors, available spare (+threshold), warning/critical
  temperature time, error-log entries — shown in report/JSON and flagged when
  spare < threshold or media errors > 0. ✅ (unit-tested)

### M11 — Polish — DONE
- Config file `~/.config/dcheck/config.json` (`temp_warn_c`, `watch_interval`,
  `theme`). ✅
- TUI palette adapts to light/dark terminals (`--light`/`--dark`,
  `DCHECK_THEME`, auto via `COLORFGBG`; `NO_COLOR` disables color). ✅
- TUI UX: background SMART reads with spinner, per-device health in the list,
  contextual report title with position indicator, `?` help overlay, mouse
  scrolling, `r` rescan, `Home`/`End`/`g`/`G`. ✅
- `dcheck prometheus` metrics. ✅
- Man page `dcheck/dcheck.1`. ✅
- Release artifacts: tarballs + `SHA256SUMS`, optional GPG signing via `GPG_KEY`. ✅

## 17. Risks

- ATA SMART via `SG_IO` is fiddly across controllers/USB bridges (USB-SATA often
  needs `-d sat`); native path may fail on some USB enclosures.
- Vendor TBW values are not standardized; the DB is approximate.
- Life estimates depend on load assumptions and can mislead if presented
  without confidence/assumptions (mitigate via explicit assumptions + confidence).
- Minimal WayangOS kernels may lack NVMe/SG options; must audit/update configs.
