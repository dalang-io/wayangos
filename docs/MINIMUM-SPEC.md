# WayangOS — minimum spec

Measured 2026-09-25 on WayangOS core (kernel 7.2.7, rootfs from
`scripts/build-rootfs.sh`: BusyBox + Dropbear + curl + CA bundle), in QEMU
(TCG on Apple Silicon), direct kernel boot (`-kernel` / `-initrd`).

## Result

| | Minimum |
|---|---|
| **RAM** | **72 MiB** (`defconfig-intel`), **70 MiB** (`defconfig-qemu`) — 1 vCPU |
| RAM with 2 vCPU | 80 MiB |
| **CPU** | any x86-64 (`-cpu qemu64`: SSE2 only); `core2duo`, `Nehalem` also fine |
| **Disk** | none — the whole system runs from RAM |
| Recommended | 128 MiB (headroom for apps deployed via `scp`) |

"Works" means all of: boot reaches `WayangOS ready`, DHCP gets an address,
SSH login succeeds, ping to the gateway works. At 72 MiB an HTTPS `curl` to
github.com also succeeded with no OOM (~6 MB free afterwards).

## Per RAM size (1 vCPU, `-cpu qemu64`)

| RAM | `defconfig-qemu` | `defconfig-intel` |
|-----|------------------|-------------------|
| 256M | works (14 s) | — |
| 128M | works (14 s) | works (16 s) |
| 96M | works (14 s) | works (12 s) |
| 88M, 80M | works | works |
| 72M | works (12 s) | **works (13 s) — minimum** |
| 70M | **works — minimum** | initramfs only partly unpacked: no `rcS`, no SSH |
| 68M | initramfs unpacking failed | same |
| 66M | initramfs unpacking failed | panic: Unable to mount root fs |
| 64M | OOM during boot | panic: Unable to mount root fs |
| 48M, 40M, 32M | dies before the console comes up | same |

Boot times are under emulation (TCG); real x86 hardware is much faster.

## Why ~72 MiB

The limit is the boot, not the running system:

- **Kernel** reserves ~41 MB that never becomes usable (code 21 MB, rodata
  7 MB, data/init/bss ~7 MB, plus page tables); at 72M `MemTotal` is 31 MB.
- **Initramfs** must fit twice while booting: 4.7 MB compressed + 10.3 MB
  unpacked (curl 6.2 MB, BusyBox 2.4 MB, Dropbear 1.5 MB, CA bundle 0.2 MB).
  Below the limit the kernel reports `Initramfs unpacking failed: write
  error` — `rootfstype=ramfs` does not help.
- **Running**, the system uses ~12 MB, plus ~10 MB of rootfs files in RAM.
- Each extra vCPU costs the kernel a little memory (per-CPU areas), hence
  80 MiB with 2 vCPU.

## Going lower

Plan and checklist: [`MEMORY-TODO.md`](MEMORY-TODO.md).

- Leaner kernel config: `defconfig-intel` carries i915, NVMe, SAS/RAID and
  NIC drivers a small VM or embedded box does not need.
- Drop `curl` from the rootfs (6.2 MB unpacked) — BusyBox `wget` covers plain
  downloads, but not the HTTPS key fetch in `wayang-addkey` / the installer.
- Keep the router edition's extra tools in its own overlay
  (`docs/ROUTER-TODO.md`) so the core does not grow.

## Stale claims to update

- The landing page hero says **"64MB minimum RAM"**; current builds need
  72 MiB (70 MiB with `defconfig-qemu`).
- `docs/ARCHITECTURE.md` memory table (updated with these numbers).

## Reproduce

```sh
qemu-system-x86_64 -m 72M -smp 1 -cpu qemu64 -vga std \
  -kernel bzImage-intel -initrd wayangos-initramfs.img -append "console=tty0" \
  -nic user,model=e1000,hostfwd=tcp::2230-:22
```

For SSH, the initramfs needs a key (`SSH_AUTHORIZED_KEYS` at build time).
Bisect RAM by 8 MB steps, then 2 MB, and check the four "works" conditions
above for each size.
