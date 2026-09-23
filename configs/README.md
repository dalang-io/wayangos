# WayangOS Kernel Configs

Configs for the WayangOS Linux kernel. The base kernel version is **6.19.7**
(x86_64); the real-time flavor uses the **6.19.3-rt1** PREEMPT_RT tree.

## Config format

A config file is either a **full `.config`** or a small **fragment**:

- If the first line is `# WAYANG_BASE: <name>`, the build script loads
  `configs/<name>` as the base, appends the rest of this file, then runs
  `make olddefconfig` to resolve dependencies.
- `# WAYANG_BASE: defconfig` is special: it runs `make ARCH=$ARCH defconfig`
  (used by the ARM64 fragments).
- No marker → the whole file is used verbatim (this is how `defconfig-qemu`
  works).

Fragments are intentional: they stay readable and are resolved by
`olddefconfig`, so we don't duplicate a 5000-line config for every GPU.

## Available configs

| Config | Arch | Base | Purpose |
|--------|------|------|---------|
| `defconfig-qemu` | x86_64 | full config | QEMU/VM testing (recommended starting point) |
| `defconfig-intel` | x86_64 | `defconfig-qemu` | Intel i915 integrated GPU |
| `defconfig-amd` | x86_64 | `defconfig-qemu` | AMD GPU (amdgpu + radeon) |
| `defconfig-nvidia` | x86_64 | `defconfig-qemu` | NVIDIA open driver (nouveau) |
| `defconfig-rt` | x86_64 | `defconfig-qemu` | PREEMPT_RT real-time (needs RT-patched tree) |
| `defconfig-arm64-rpi3` | arm64 | `defconfig` | Raspberry Pi 3 (BCM2837, VC4/V3D) |
| `defconfig-arm64-orangepi-zero2w` | arm64 | `defconfig` | Orange Pi Zero 2W (Allwinner H618, Panfrost) |

### Real-time (`defconfig-rt`)

`CONFIG_PREEMPT_RT` only exists in an RT-patched kernel tree. Fetch it first:

```bash
KERNEL_FLAVOR=rt ./scripts/fetch-sources.sh   # Linux 6.19.3-rt1 → $BUILD/linux-6.19.3-rt1
./scripts/build-kernel.sh defconfig-rt
```

`build-kernel.sh` automatically uses `$BUILD/linux-6.19.3-rt1` for any config
whose name contains `rt`.

## Building

```bash
# x86_64 (default)
./scripts/build-kernel.sh defconfig-qemu
./scripts/build-kernel.sh defconfig-intel
./scripts/build-kernel.sh defconfig-rt

# ARM64 (cross-compile)
ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- ./scripts/build-kernel.sh defconfig-arm64-rpi3
ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- ./scripts/build-kernel.sh defconfig-arm64-orangepi-zero2w
```

Output: `$BUILD_DIR/bzImage-<config>` (x86_64) or `$BUILD_DIR/Image-<config>`
(arm64). See [`../BUILDING.md`](../BUILDING.md) for prerequisites and the full
pipeline.

## Creating a new config

Start from an existing one and add only what changes:

```bash
cp configs/defconfig-intel configs/defconfig-mybox
# edit the options you need, then:
./scripts/build-kernel.sh defconfig-mybox
```

## Status / caveats

- `defconfig-qemu` is the only config confirmed booting in QEMU with working
  PS/2 keyboard input.
- The GPU and ARM64 fragments enable the right drivers, but they have **not
  been re-verified on real hardware** from this repo yet. Treat them as the
  reproducible starting point for the released editions.
- NVIDIA uses the open `nouveau` driver, not the proprietary blob.
- RISC-V is roadmap only (no config yet).

## Common pitfalls

1. **Custom minimal configs break keyboard input in QEMU.** The i8042 PS/2
   controller and AT keyboard driver are required. `defconfig-qemu` includes
   them; hand-tuned minimal configs often don't. Required options:
   ```
   CONFIG_SERIO=y
   CONFIG_SERIO_I8042=y
   CONFIG_KEYBOARD_ATKBD=y
   CONFIG_INPUT_EVDEV=y
   ```
2. **`olddefconfig` can drop options whose dependencies are unmet.** If a GPU
   option disappears after building, enable its parent (`CONFIG_DRM`,
   `CONFIG_FB`, platform `CONFIG_ARCH_*`, etc.) in the fragment.
