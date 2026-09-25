# Building WayangOS (details)

This file previously duplicated the main build guide with outdated instructions.
**The canonical, up-to-date build guide is [`../BUILDING.md`](../BUILDING.md).**

Use it for prerequisites, the full pipeline, and QEMU testing. The notes below
only cover per-component details.

## Components

| Component | Version | Source location | Provided by |
|-----------|---------|-----------------|-------------|
| Linux kernel | 7.2.7 | `$BUILD_DIR/linux-7.2.7` | `scripts/fetch-sources.sh` |
| Linux kernel (RT) | 6.19.3-rt1 | `$BUILD_DIR/linux-6.19.3-rt1` | `KERNEL_FLAVOR=rt scripts/fetch-sources.sh` |
| ARM64 kernel | 7.2.7 | `$BUILD_DIR/linux-7.2.7` | built with `ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu-` |
| BusyBox | 1.37.0 | `$BUILD_DIR/busybox-1.37.0/busybox` | `scripts/fetch-sources.sh` (prebuilt static) |
| Dropbear SSH | 2024.86 | `$BUILD_DIR/dropbear-2024.86` | `scripts/fetch-sources.sh` |
| SQLite | amalgamation | `$BUILD_DIR/sqlite3.c` / `.h` | `scripts/fetch-sources.sh` |
| POS application | in-repo | `wayangos-pos/fbpos-v3.c` | `scripts/build-pos.sh` |

`$BUILD_DIR` defaults to `~/wayangos-build` and can be overridden:

```bash
BUILD_DIR=/path/to/build ./scripts/build-pos-iso.sh defconfig-qemu
```

## Kernel

```bash
./scripts/build-kernel.sh <config-name> [output-name]
```

The script copies `configs/<config-name>` into the kernel tree (`$KDIR`), runs
`make olddefconfig`, builds the kernel, and writes the result to
`$BUILD_DIR/<output-name>`. It is ARCH-aware: set `ARCH` (`x86_64` or `arm64`)
and `CROSS_COMPILE` for non-native targets.

Maintained configs: `defconfig-qemu`, `defconfig-rt`, `defconfig-intel`,
`defconfig-amd`, `defconfig-nvidia`, `defconfig-arm64-rpi3`, and
`defconfig-arm64-orangepi-zero2w`. See
[`../configs/README.md`](../configs/README.md) for details.

## Rootfs

```bash
./scripts/build-rootfs.sh
```

Produces `$BUILD_DIR/wayangos-initramfs.img`: BusyBox (all applets) + Dropbear
SSH + static curl, with init scripts generated inline. See
[`../scripts/build-rootfs.sh`](../scripts/build-rootfs.sh).

## ISO

```bash
# Plain OS
./scripts/build-iso.sh <kernel> <initramfs> [output.iso]

# Full POS pipeline (kernel + rootfs + POS binary → ISO)
./scripts/build-pos-iso.sh defconfig-qemu wayangos-pos-qemu.iso
```

## Architecture

The kernel script is ARCH-aware and covers **x86_64** (`defconfig-qemu`,
`defconfig-rt`, `defconfig-intel`, `defconfig-amd`, `defconfig-nvidia`) and
**ARM64** (`defconfig-arm64-rpi3`, `defconfig-arm64-orangepi-zero2w`). RISC-V
remains a roadmap item — see [`ARCHITECTURE.md`](ARCHITECTURE.md).
