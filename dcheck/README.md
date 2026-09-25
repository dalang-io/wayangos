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

> Maintainers: start with [`HANDOVER.md`](HANDOVER.md) (status, release
> process, pitfalls, open work) and [`TODO.md`](TODO.md).

Landing page with screenshots and a technician guide:
<https://wayang.dalang.io/apps/dcheck.html>

Linux x86_64 / aarch64 (static binary, checksum-verified):

```bash
curl -fsSL https://wayang.dalang.io/dcheck/install.sh | sh
dcheck update            # later: upgrade in place (--check to only look)

# optional: also install smartmontools as a second SMART source
curl -fsSL https://wayang.dalang.io/dcheck/install.sh | sh -s -- --with-smartmontools
```

**Requirements:** none at runtime — the Linux binary is static (musl). The
installer uses `curl` or `wget`, `sha256sum`, `tar`, `awk` (all present on
stock Ubuntu/Debian/Fedora/RHEL). SMART needs root. Optional:
`smartmontools` (second source; SATA behind RAID/USB is read natively via
SAT), `dmidecode` (RAM modules; SMBIOS is also read from sysfs), `curl` for
`watch --webhook`.

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
dcheck board            # motherboard: maker, BIOS, PCIe/USB, sensors, BMC log (--json)
dcheck verify <dev>     # prove the real capacity (fake drives; writes test files)
dcheck recover <dev|path>  # deleted a file? chance, steps, disk map (read-only)
dcheck undelete <dev|image> [--to DIR]  # list / recover deleted files (NTFS, FAT32, exFAT; --carve)
dcheck update           # self-update from wayang.dalang.io (--check, --force)
dcheck snapshot DIR     # every TUI screen as SVG (--demo, --mask-serials, --host, --tools DEV)
dcheck --version
```

In the UI: `↑`/`↓` move, `Enter` open, `b`/`Esc` back, `1`/`2`/`3` jump to
storage/memory/processor, `PgUp`/`PgDn` and `Home`/`End` (`g`/`G`) scroll,
`r` rescan/refresh, `c` copy the log (OSC52), `?` command reference, `q` quit.
On a disk (storage list or its report): `u` opens RECOVERY (deleted-file
chance, steps and a coloured disk map; read-only) and `v` the CAPACITY TEST
(free-space test: plan and warnings first, `y` starts it, `Esc` stops it and
removes the test files; the destructive whole-disk test is CLI-only, and a
system disk only gets the quick size).
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
  `{ "temp_warn_c": 60, "watch_interval": 60, "theme": "light", "mouse": true, "plain": false, "transparent": false, "splash": true, "hdd_design_years": 5, "cpu_temp_warn_c": null, "cache_ttl_secs": 600 }`
  (`hdd_design_years` is the **assumed** HDD design life at 24/7 used for HDD
  life estimates — drives do not report one; default 5 years = 43,800 h;
  `temp_warn_c` is the **disk** temperature warning; CPUs are judged against
  each sensor's own high/critical limit (coretemp/k10temp), else 85°C, unless
  `cpu_temp_warn_c` is set)
- `~/.config/dcheck/tbw.json` (or `$DCHECK_TBW_JSON`):
  `{ "model substring": TBW_in_TB }`
- Env: `DCHECK_SYS_ROOT` (alternate fs root), `DCHECK_SMART_JSON` (parse a
  captured `smartctl -j` file), `DCHECK_SMART_ARGS` (force `-d ...`),
  `DCHECK_NATIVE` (force native SMART).

## Cache and dead drives

- SMART reads are cached so the UI and `dcheck storage` do not re-read slow
  disks (a SAS disk behind a PERC answers each SCSI command in 0.3–0.75 s):
  in-process, plus on disk for `cache_ttl_secs` (default 600) in
  `/var/cache/dcheck` (root) or `~/.cache/dcheck`, file mode 0600. Keyed by
  device + model + serial + size + WWID. `r` in the UI or `--fresh`
  re-reads; `DCHECK_NO_CACHE=1` disables it. `check`, `watch`, `prometheus`
  and `--json` always read the hardware (disks are read in parallel).
- A SATA drive that raises the link but never answers (the kernel logs
  "reset failed, giving up") has no `/dev/sdX`; dcheck reads the kernel log
  (`/dev/kmsg`, root) and lists the port, e.g. `ata1  REPLACE`, so
  `dcheck check` exits 3.

## Authenticity (fake / rebranded drives)

Each disk report has an **AUTHENTICITY** section that checks whether the
identity the firmware reports is consistent with the brand in the model name:

- **WWN / IEEE OUI**: SATA and SAS drives carry a World Wide Name holding
  the maker's IEEE OUI (Samsung `002538`, Seagate `000c50`, WD `0014ee`,
  Toshiba `000039`, …). dcheck embeds every OUI the IEEE registry lists for
  the drive makers it knows (`src/oui_table.rs`, regenerated with
  `scripts/gen-dcheck-oui.sh`). A brand with another maker's OUI → **LIKELY
  FAKE**; a brand with an all-zero or missing WWN → **SUSPICIOUS**.
- **NVMe controller**: makers that only ship their own controllers (Samsung
  `144d`, SK hynix `1c5c`) flag a foreign PCI vendor ID, e.g. a "Samsung
  980" on a Maxio controller.
- **Generic identity**: model strings like `SSD 1TB` → **UNBRANDED** (if the
  casing shows a brand, the drive is not what it claims); placeholder serials.

Sources: smartctl, native ATA IDENTIFY words 108–111 / SCSI VPD 0x83, sysfs
`wwid`, udev `/dev/disk/by-id/wwn-*` — works without root on most systems.
RAID volumes are skipped. A clean result means "consistent", not proven
genuine; capacity fraud needs the write-and-verify test below. JSON:
`authenticity` object in `storage --json`.

### `dcheck verify` — real capacity (fake flash)

A counterfeit drive reports more space than its flash holds; past the real
size, writes wrap onto earlier addresses or vanish. Only writing and reading
back finds it (like f3 / H2testw):

```
dcheck verify /dev/sdb              # free space of its mounted filesystem
dcheck verify /dev/sdb --size 8G    # quicker, proves only the first 8 GiB
dcheck verify /dev/sdb --destructive  # empty, unmounted drive: whole disk
```

- Every 4 KiB block carries its own address, so a bad block tells *why*:
  data of another address (wrap-around → "Real size: about …"), zeros,
  stale or corrupted. OS caches are dropped before reading.
- Writes are sequential; after each region earlier samples are re-read, so
  a wrapping fake fails as soon as the writes pass its real capacity.
- Default mode writes files to `.dcheck-verify-<pid>/` on the drive's
  mounted filesystem (free space minus 1%, min 256 MiB) and deletes them;
  existing files stay (back up anyway: on a fake, writes past its real
  capacity can hit them). Asks first; `--yes` for scripts. On a system
  disk (`/`, `/boot`, `/var`, `/home`, … on it) it refuses to fill the free
  space unless you pass `--size` or `--full`.
- `--destructive` overwrites the raw device. Refused when a partition is
  mounted, used as swap or held by LVM/RAID/dm. It lists what is on the
  drive (partitions, ext4/XFS/btrfs/NTFS/FAT/exFAT/LVM/LUKS/GPT/MBR…) and
  asks you to type the device name — or `ERASE <name>` when it holds data.
- Exit 0 pass, 3 bad data, 1 error/aborted. Linux only for now.

## Deleted a file? `dcheck recover`

Read-only. `dcheck recover /dev/sda` (a disk, partition or any path)
estimates whether deleted files can still come back and says what to do:

- **Chance** HIGH / MEDIUM / LOW / ALMOST NONE from the media (HDD keeps old
  contents until overwritten; an SSD with TRIM erases freed blocks), the
  `discard` mount option (erased within seconds), `fstrim.timer` (weekly),
  the filesystem (NTFS/FAT keep names; ext4/XFS lose them — carving),
  how full it is, and whether it is the running system disk.
- **Steps** with the real device filled in: check Trash / backups /
  snapshots, `systemctl stop fstrim.timer`, stop writing (umount, or boot a
  live USB for the system disk), image with `ddrescue` to another disk,
  then the right tool on the image (ntfsundelete, testdisk, ext4magic,
  xfs_undelete, btrfs restore, photorec).
- **Disk map** (root): samples the disk (512 cells × 4 blocks) and draws
  where it still holds data; per filesystem it compares that with the used
  space: e.g. an HDD at 63% used with data in 97% of samples → ~92% of its
  free space still holds old data; an SSD with `discard=async` → ~0%.

## Undelete: `dcheck undelete`

```
sudo dcheck undelete /dev/sdb1                    # list deleted files (read-only)
sudo dcheck undelete /dev/sdb1 --to /mnt/usb/rescue --match '*.xlsx'
sudo dcheck undelete disk.img --to /mnt/usb/rescue  # a ddrescue image (whole disk or partition)
sudo dcheck undelete /dev/sdb --carve --to /mnt/usb/rescue   # ext4 / XFS / btrfs
```

- **NTFS**: the deleted file's MFT record keeps name, folder, size and data
  runs (fragmented files too); small files are inside the record. Windows
  and ntfs-3g keep the name; Linux's ntfs3 driver drops it → recovered as
  `$NoName/record-N.<type>` (type from the contents).
- **FAT32**: long name, size and first cluster remain; data assumed
  contiguous. **exFAT**: name, size, first cluster and the contiguous flag.
- **Other filesystems**: `--carve` finds JPEG, PNG, PDF and ZIP / Office
  files in the raw data (names are lost).
- Each file is **INTACT** (its clusters are still free), **PARTLY REUSED** or
  **OVERWRITTEN** (from the FAT / exFAT bitmap / NTFS `$Bitmap`); overwritten
  files are skipped unless `--include-overwritten`.
- Safety: the source is only read; the destination must be on **another
  disk** (refused otherwise) and existing files are never overwritten.
  Whole-disk sources: MBR and GPT partitions are found.
- UI: RECOVERY screen → `d`: a **block map** of the volume (in use, free,
  deleted intact / reused, marked, selected), the file list (`space` mark,
  `a` all intact) and `w` to recover into a folder you type.
- Verified with images made on Linux (mkfs, write, delete, write again):
  every recovered file matched the original's SHA-256.

## Motherboard: `dcheck board`

Also in the UI as **04 MOTHERBOARD** (key `4`).

- **Identity**: system, board and chassis from DMI (serials as root);
  unfilled fields ("Default string") → noted as a generic / white-label board.
- **Firmware**: BIOS vendor, version, date and age, UEFI / legacy, Secure
  Boot, BMC firmware.
- **Sensors**: board hwmon chips (fans, voltages, temperatures with their
  alarms / limits) and, on servers, the **BMC over native IPMI**
  (`/dev/ipmi0`, root, no ipmitool): fans, temperatures, voltages, power
  draw, power supplies, redundancy, intrusion — with the BMC's own status.
- **BMC event log**: recent events (PSU AC lost, chassis opened, drive
  faults, memory / processor errors); warning/critical ones from the last
  30 days raise the verdict.
- **PCIe devices** (names from pci.ids when present): driver, link speed /
  width, AER error counters; **USB devices**.
- **Verdict** OK / MONITOR / CRITICAL: sensor out of range or critical,
  PSU without AC (MONITOR while another supply runs: redundancy lost),
  PCIe fatal / non-fatal errors, recent critical BMC events. Notes: old
  BIOS, devices without a driver, narrower PCIe links, Secure Boot off.
- macOS: model, chip, firmware version.

## Virtual machines

Inside a VM (KVM / QEMU, VMware, Hyper-V, Xen, VirtualBox; detected from DMI,
`/sys/hypervisor` and the CPU `hypervisor` flag) dcheck reports what a VM
can know:

- Virtual disks (virtio `vd*`, `xvd*`, "QEMU HARDDISK", "VMware Virtual
  disk", EBS …) show as type **VIRT** with verdict **VIRTUAL** (severity 0):
  the hypervisor exposes no SMART; the physical disks are the provider's.
  `dcheck check` therefore exits 0 in a healthy VM.
- RAM, CPU and board note that the physical hardware belongs to the host;
  the emulated firmware (e.g. SeaBIOS dated 2014) is not reported as old.
- `dcheck recover` on a virtual disk: a `discard` mount lowers the chance
  (the host may reclaim freed blocks) and the first step is a **snapshot in
  the provider's panel**, then the provider's rescue mode.

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
