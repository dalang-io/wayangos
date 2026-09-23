# Architecture Guide

## Supported Platforms

### x86_64 (AMD64) — supported
- Intel/AMD 64-bit processors
- Tested on: QEMU/KVM, bare metal
- Primary development target
- Base, PREEMPT_RT, and GPU editions; GRUB ISOs are UEFI + BIOS hybrid

### ARM64 (AArch64) — supported
- ARMv8-A 64-bit processors
- Targets: Raspberry Pi 3, Orange Pi Zero 2W
- Configs: `defconfig-arm64-rpi3`, `defconfig-arm64-orangepi-zero2w`
- Cross-compile with `ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu-`

### RISC-V (rv64gc) — roadmap
- 64-bit RISC-V with G (IMAFDZicsr_Zifencei) and C extensions
- Targets: SiFive boards, StarFive VisionFive 2
- Requires cross-compile toolchain (`riscv64-linux-gnu-`)

> RISC-V is not yet wired into `scripts/`. Contributions welcome.

## Build Host Requirements

- Linux x86_64 (or WSL2 on Windows)
- GCC 12+ or Clang 16+
- GNU Make, flex, bison, bc
- libelf-dev, libssl-dev
- cpio, gzip, grub-pc-bin, grub-efi-amd64-bin, xorriso, mtools
- ~2GB disk space for the kernel build

## Boot Flow

```
Firmware (BIOS/UEFI)
  → GRUB (grub-mkrescue ISO)
    → Kernel (bzImage)
      → initramfs (cpio.gz)
        → /sbin/init (BusyBox init)
          → /etc/inittab
            → /etc/init.d/rcS  (mount fs, mdev, network, sshd)
              → /etc/init.d/pos-app  (framebuffer + POS binary, POS ISO only)
```

On ARM64 SBCs the vendor bootloader (the Raspberry Pi firmware, or U-Boot on
the Orange Pi Zero 2W) loads the kernel and device tree, after which the same
initramfs/`init` flow runs.

## Memory Layout (minimal profile)

| Component | Size |
|-----------|------|
| Kernel | ~14 MB (defconfig-qemu) |
| initramfs | ~2-3 MB |
| Runtime RAM | ~20 MB |
| **Recommended minimum** | **~128 MB** |
