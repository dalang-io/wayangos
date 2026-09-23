# Building WayangOS

Complete guide to building WayangOS from source. This is the canonical build
guide; `docs/BUILDING.md` only holds per-component details.

## Prerequisites

### Environment
- **Linux x86_64** (native) or **WSL2 Ubuntu** on Windows (tested with Ubuntu 22.04+)
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
| Linux kernel | 6.19.7 | `linux-6.19.7/` |
| Linux kernel (RT) | 6.19.3-rt1 | `linux-6.19.3-rt1/` |
| BusyBox | 1.37.0 | `busybox-1.37.0/busybox` (pre-built static) |
| Dropbear SSH | 2024.86 | `dropbear-2024.86/` |
| SQLite | amalgamation | `sqlite3.c`, `sqlite3.h` |

### POS application (in this repo)
Wayang POS source is tracked here at `wayangos-pos/fbpos-v3.c` (direct
framebuffer, evdev input, SQLite backend).

| Component | Path |
|-----------|------|
| POS source | `wayangos-pos/fbpos-v3.c` |
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
| `KDIR` | `$BUILD_DIR/linux-6.19.7` | Kernel source tree (use the RT tree for RT configs) |

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
./scripts/build-rootfs.sh
# Output: ~/wayangos-build/wayangos-initramfs.img
```

Includes:
- BusyBox (static, all applets)
- Dropbear SSH server (auto-starts on port 22)
- curl with TLS
- Auto DHCP networking
- Root login (no password — add SSH key to `/root/.ssh/authorized_keys`)

### 3. Build POS Binary

Build the in-repo Wayang POS app (`wayangos-pos/fbpos-v3.c`):

```bash
./scripts/build-pos.sh
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

---

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

- Root SSH login has **no password** by default (`etc/shadow` root entry is empty)
- Dropbear listens on port 22 and generates host keys on first boot (`-R`)

Before deploying to the field:
- Set a root password (`passwd`) or install an SSH public key in `/root/.ssh/authorized_keys`
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
│   ├── build-pos.sh                # Build POS binary from wayangos-pos/
│   ├── build-iso.sh                # Assemble ISO
│   ├── build-pos-iso.sh            # Full POS ISO pipeline
│   └── deprecated/                 # Historical scripts (reference only)
├── wayangos-pos/                   # Wayang POS source (fbpos-v3.c)
├── userspace/                      # Reference init scripts (legacy)
├── docs/                           # Architecture notes
├── landing-page/                   # Website (wayang.dalang.io)
├── BUILDING.md                     # This file
└── README.md
```
