# Building WayangOS

Complete guide to building WayangOS from source. This is the canonical build
guide; `docs/BUILDING.md` only holds per-component details.

## Prerequisites

### Environment
- **Linux x86_64** (native) or **WSL2 Ubuntu** on Windows (tested with Ubuntu 22.04+)
- Or any Linux host with a cross toolchain: every x86_64 step honours
  `CROSS_COMPILE`, e.g. an arm64 Ubuntu VM on Apple Silicon with
  `gcc-x86-64-linux-gnu` and `CROSS_COMPILE=x86_64-linux-gnu-`
- Build dir: `~/wayangos-build/` (override with `BUILD_DIR`)
- Repo dir: wherever you cloned this repository

### Packages
```bash
sudo apt install build-essential gcc make flex bison bc libelf-dev libssl-dev \
    cpio gzip grub-pc-bin grub-efi-amd64-bin xorriso mtools \
    qemu-system-x86 wget git
```

### External sources
Kernel, BusyBox, Dropbear, and SQLite are **not** tracked in this repo (too
large). Download them into the build dir:

```bash
./scripts/fetch-sources.sh
# Real-time PREEMPT_RT kernel tree instead of the base kernel:
KERNEL_FLAVOR=rt ./scripts/fetch-sources.sh
```

| Component | Version | Path (under `$BUILD_DIR`) |
|-----------|---------|---------------------------|
| Linux kernel | 7.2.7 | `linux-7.2.7/` |
| Linux kernel (RT) | 6.19.3-rt1 | `linux-6.19.3-rt1/` |
| BusyBox | 1.37.0 | `busybox-1.37.0/busybox` (pre-built static) |
| Dropbear SSH | 2024.86 | `dropbear-2024.86/` |
| SQLite | amalgamation | `sqlite3.c`, `sqlite3.h` |

### POS application (external, private)
Wayang POS source lives in the private repo
[`dalang-io/wayang-pos`](https://github.com/dalang-io/wayang-pos) (direct
framebuffer, evdev input, SQLite backend). `scripts/build-pos.sh` fetches it at
a pinned ref (`POS_REF`, default `v3.2.3`); access needs git credentials for
that repo (`gh auth setup-git`, or `POS_TOKEN`), or a local checkout via
`POS_SRC_DIR`.

| Component | Path |
|-----------|------|
| POS source | `dalang-io/wayang-pos` → `$BUILD_DIR/wayang-pos/` |
| POS binary | `$BUILD_DIR/wayang-pos-static` |

---

## Quick Start

Build everything and get a bootable POS ISO:

```bash
./scripts/fetch-sources.sh
./scripts/build-pos-iso.sh defconfig-qemu wayangos-pos-qemu.iso
```

`build-pos-iso.sh` skips steps whose outputs already exist, so re-runs are cheap.

---

## Step-by-Step

### 1. Build Kernel

```bash
./scripts/build-kernel.sh defconfig-qemu bzImage-qemu
```

Usage: `./scripts/build-kernel.sh <config> [output]`. Environment variables:

| Variable | Default | Purpose |
|----------|---------|---------|
| `ARCH` | `x86_64` | Target architecture (`x86_64` or `arm64`) |
| `CROSS_COMPILE` | — | Toolchain prefix, e.g. `aarch64-linux-gnu-` |
| `BUILD_DIR` | `~/wayangos-build` | Kernel tree + output location |
| `KDIR` | `$BUILD_DIR/linux-7.2.7` | Kernel source tree (use the RT tree for RT configs) |
| `UTS_SYSNAME` | `WayangOS` | OS name reported by `uname` (the kernel's default is `Linux`) |

Available configs in `configs/`:

| Config | Target |
|--------|--------|
| `defconfig-qemu` | QEMU / x86_64 base (recommended starting point) |
| `defconfig-rt` | x86_64 PREEMPT_RT |
| `defconfig-intel` | x86_64 Intel i915 |
| `defconfig-amd` | x86_64 AMD amdgpu + radeon |
| `defconfig-nvidia` | x86_64 NVIDIA nouveau |
| `defconfig-arm64-rpi3` | ARM64 Raspberry Pi 3 |
| `defconfig-arm64-orangepi-zero2w` | ARM64 Orange Pi Zero 2W |

RT builds need the RT source tree first:

```bash
KERNEL_FLAVOR=rt ./scripts/fetch-sources.sh
KDIR=~/wayangos-build/linux-6.19.3-rt1 ./scripts/build-kernel.sh defconfig-rt bzImage-rt
```

ARM64 cross-compile:

```bash
sudo apt install gcc-aarch64-linux-gnu
ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- \
    ./scripts/build-kernel.sh defconfig-arm64-rpi3 Image-rpi3
```

See `configs/README.md` for GPU-specific config generation notes.

### 2. Build Rootfs

Creates a base initramfs with BusyBox + Dropbear SSH + curl:

```bash
SSH_AUTHORIZED_KEYS=~/.ssh/id_ed25519.pub ./scripts/build-rootfs.sh
# Output: ~/wayangos-build/wayangos-initramfs.img
```

Includes:
- BusyBox (static, all applets)
- Dropbear SSH server (auto-starts on port 22)
- curl with TLS
- Auto DHCP networking
- Root SSH login, public key only (`SSH_AUTHORIZED_KEYS` → `/root/.ssh/authorized_keys`)

### 3. Build POS Binary

Build the Wayang POS app (fetched from `dalang-io/wayang-pos`):

```bash
./scripts/build-pos.sh                           # pinned POS_REF
POS_SRC_DIR=~/wayang-pos ./scripts/build-pos.sh  # local checkout
# Output: $BUILD_DIR/wayang-pos-static
```

The POS app uses:
- Direct framebuffer rendering (`/dev/fb0`)
- Linux evdev for touch/keyboard/mouse input
- SQLite for transaction storage
- An embedded bitmap font, no external dependencies

### 4. Assemble ISO

**Plain OS (no POS):**
```bash
./scripts/build-iso.sh ~/wayangos-build/bzImage-qemu ~/wayangos-build/wayangos-initramfs.img
```

**POS ISO (includes POS app):**
```bash
./scripts/build-pos-iso.sh defconfig-qemu
```

### 5. USB Installer (bare metal)

Installs WayangOS onto a machine's internal disk — built and tested for a
Lenovo ThinkStation P320 Tiny (NVMe, USB-to-Ethernet adapter), and the same
image suits other Intel boxes such as the P330 Tiny.

```bash
./scripts/build-kernel.sh defconfig-intel bzImage-intel
./scripts/build-rootfs.sh          # no SSH_AUTHORIZED_KEYS: one ISO for everyone
./scripts/build-installer.sh       # the installer TUI (Rust; zig cc on macOS)
./scripts/build-installer-iso.sh
# Output: ~/wayangos-build/wayangos-installer.iso (hybrid, dd it to a USB stick)
```

`build-installer.sh` runs where Rust is (e.g. the Mac) and writes
`dist/wayang-installer-x86_64-unknown-linux-musl`; `build-installer-iso.sh`
picks it up from there (or from `INSTALLER_BIN`).

`defconfig-intel` adds what real hardware needs on top of the QEMU base: NVMe,
the UEFI framebuffer, USB-to-Ethernet adapters (Realtek r8152, ASIX, CDC) and
multi-port PCIe NICs (Intel igb/igc/ixgbe/i40e; onboard I219 `e1000e` and
Realtek `r8169` are already in the base), SAS HBAs and hardware RAID
(mpt3sas, megaraid_sas, smartpqi, hpsa, mptsas) and exFAT for key sticks.

Boot the stick and pick **Install WayangOS to disk**. The installer
(`installer/`, a ratatui TUI in dcheck's HUD style) walks through:

1. **Target** — every disk with its type (NVMe / SATA SSD / SATA HDD / SAS /
   RAID volume / USB / eMMC / virtual), model, size and what is on it; pick one
   with the arrow keys. Its own USB stick, read-only and too-small disks are
   shown but can't be picked.
2. **Access** — hostname, and root's SSH keys: fetched from
   `github.com/USER.keys` (or `gitlab:USER`), imported from `*.pub` /
   `authorized_keys` files on another USB stick, or pasted. Keys work on the
   live installer at once, so the rest can be done over SSH
   (`ssh root@<ip>`, then `wayang-installer`).
3. **Confirm** — summary and the new layout; type `YES`.
4. **Install** — progress per step, then remove the stick and reboot.

Each box gets its own keys and hostname, so one ISO serves every user. On the
installed system, `wayang-addkey github:USER` (or a `.pub` file, or a key
line) authorizes more keys and keeps them in `/data`.

The disk gets:

| Partition | Size | Contents |
|-----------|------|----------|
| 1 — EFI system, FAT32 `WAYANGBOOT` | 512 MiB | GRUB at `\EFI\BOOT\BOOTX64.EFI` (UEFI fallback path, no NVRAM entry needed), `boot/vmlinuz`, `boot/initramfs.img` |
| 2 — Linux, ext4 `WAYANGDATA` | rest | mounted at `/data`; keeps `etc/hostname`, `etc/ssh/authorized_keys` and the SSH host keys |

The installed system still runs from RAM; only `/data` survives a reboot. It
boots **UEFI only with Secure Boot disabled** (GRUB and the kernel are
unsigned). To update it, mount the ESP (`mount LABEL=WAYANGBOOT /mnt`) and
replace `boot/vmlinuz` / `boot/initramfs.img`.

Networking runs DHCP on every wired interface and keeps renewing, so a USB
adapter works even when a dead onboard NIC still shows up as `eth0`.

Test it on a Mac/PC with QEMU as an emulated P320 (UEFI, NVMe, USB stick, USB
NIC):

```bash
qemu-img create -f qcow2 nvme.qcow2 32G
cp /usr/share/OVMF/OVMF_VARS_4M.fd vars.fd     # edk2-i386-vars.fd on Homebrew
qemu-system-x86_64 -machine q35 -m 2G \
    -drive if=pflash,format=raw,readonly=on,file=/usr/share/OVMF/OVMF_CODE_4M.fd \
    -drive if=pflash,format=raw,file=vars.fd \
    -device qemu-xhci,id=xhci \
    -drive if=none,id=nv,file=nvme.qcow2 -device nvme,drive=nv,serial=NVME01 \
    -drive if=none,id=stick,format=raw,file=wayangos-installer.iso \
    -device usb-storage,bus=xhci.0,drive=stick,removable=on,bootindex=0 \
    -netdev user,id=n0,hostfwd=tcp::2222-:22 -device usb-net,bus=xhci.0,netdev=n0
# after installing, run again without the two stick lines
```

---

## Fast builds (remote builder)

Kernels can't be built on macOS; CI builds (~3 min cached) or a Linux box with
more cores is much faster. Build on a remote Linux host and pull the artifacts
back:

```bash
# default host root@10.0.0.251, warm build ~1 min (first run downloads sources)
WAYANG_KEY=~/wayangos-keys/release.key ./scripts/build-remote.sh
# -> dist/remote/wayangos-<ver>-linux-<kver>-installer-x86_64.iso (+ .wup, channel/)
```

It syncs the repo, installs build deps (apt + rustup), runs the whole pipeline
(`scripts/ci-build.sh`), and fetches the ISO/bundle back. Env: `HOST`,
`REMOTE_SRC`, `BUILD_DIR`, `WAYANG_VERSION`, `KERNEL_VERSION`, `KERNEL_CONFIG`,
`WAYANG_KEY`, `SKIP_DEPS=1`.

## QEMU Testing

### Basic boot test (serial console)
```bash
qemu-system-x86_64 \
    -kernel ~/wayangos-build/bzImage-qemu \
    -initrd ~/wayangos-build/wayangos-initramfs.img \
    -append "console=ttyS0" \
    -nographic -m 128M \
    -nic user
```

### GUI/POS test (graphical)
```bash
qemu-system-x86_64 \
    -cdrom ~/wayangos-build/wayangos-pos-qemu.iso \
    -m 256M -vga std -display gtk \
    -nic user,hostfwd=tcp::2222-:22
```

### SSH into QEMU instance
```bash
ssh -p 2222 root@localhost
```

### QEMU with monitor (for sendkey debugging)
```bash
qemu-system-x86_64 \
    -cdrom wayangos.iso \
    -m 256M -vga std -display gtk \
    -nic user,hostfwd=tcp::2222-:22 \
    -monitor unix:/tmp/qemu-mon,server,nowait

# Send keys via monitor
echo 'sendkey 1' | socat - UNIX-CONNECT:/tmp/qemu-mon
echo 'screendump /tmp/screen.ppm' | socat - UNIX-CONNECT:/tmp/qemu-mon
```

---

## Security

The base rootfs is a **development image**:

- Root SSH login is **public key only**: Dropbear is built without password authentication (`localoptions.h`) and root has no password (`*` in `etc/shadow`)
- Keys come from the installer (per box, stored in `/data`), `wayang-addkey`, or `SSH_AUTHORIZED_KEYS` baked in at build time
- The local console still drops straight into a root shell
- Dropbear listens on port 22 and generates host keys on first boot (`-R`)

Before deploying to the field:
- Build with only the deployment's keys in `SSH_AUTHORIZED_KEYS`
- Restrict or disable Dropbear
- Change the default POS admin PIN (`1234`)

---

## Known Issues

### Keyboard not working in QEMU
**Symptom:** Boot works, framebuffer shows UI, but keyboard input is dead.

**Cause:** Custom minimal kernel configs miss the i8042 PS/2 controller and AT keyboard drivers that QEMU's virtual keyboard requires.

**Fix:** Use `defconfig` as base (includes i8042/atkbd). The `defconfig-qemu` config has this working.

**Required kernel options:**
```
CONFIG_SERIO=y
CONFIG_SERIO_I8042=y
CONFIG_KEYBOARD_ATKBD=y
CONFIG_INPUT_EVDEV=y
```

### Large kernel size with AMD GPU
AMD GPU support (amdgpu + radeon) adds significant firmware handling code, so
`defconfig-amd` produces the largest kernel of the x86_64 GPU configs. Use
`defconfig-intel` or `defconfig-nvidia` where the hardware matches.

### UEFI boot
GRUB ISOs default to BIOS boot. For UEFI, ensure `grub-efi-amd64-bin` is installed and `grub-mkrescue` will automatically include EFI boot support.
The installer ISO needs both `grub-pc-bin` and `grub-efi-amd64-bin`; on a
non-x86 build host, extract the amd64 packages into `/` (`dpkg -x pkg.deb /`)
to get `/usr/lib/grub/i386-pc` and `/usr/lib/grub/x86_64-efi`.

---

## Directory Structure

```
wayangos/
├── configs/                        # Kernel configs (base/RT/GPU/ARM64)
│   ├── README.md
│   ├── defconfig-qemu
│   ├── defconfig-rt
│   ├── defconfig-intel
│   ├── defconfig-amd
│   ├── defconfig-nvidia
│   ├── defconfig-arm64-rpi3
│   └── defconfig-arm64-orangepi-zero2w
├── scripts/                        # Build scripts
│   ├── fetch-sources.sh            # Download kernel/BusyBox/Dropbear/SQLite
│   ├── build-kernel.sh             # Build kernel from config (ARCH-aware)
│   ├── build-rootfs.sh             # Build base rootfs
│   ├── build-pos.sh                # Fetch + build POS (dalang-io/wayang-pos)
│   ├── build-iso.sh                # Assemble ISO
│   ├── build-pos-iso.sh            # Full POS ISO pipeline
│   ├── build-installer.sh          # Build the wayang-installer binary
│   ├── build-installer-iso.sh      # USB installer ISO for bare metal
│   └── deprecated/                 # Historical scripts (reference only)
├── installer/                      # wayang-installer TUI (Rust), run by the installer ISO
├── userspace/                      # Reference init scripts (legacy)
├── docs/                           # Architecture notes
├── landing-page/                   # Website (wayang.dalang.io)
├── BUILDING.md                     # This file
└── README.md
```
