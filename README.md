# WayangOS

Ultra-minimal Linux distro for kiosks, POS terminals, and embedded/edge devices. No X11, no Wayland, no systemd — just a kernel, BusyBox, and direct framebuffer rendering. The base headless image is a ~20 MB ISO that boots in seconds and runs entirely from RAM.

Target market: Indonesian UMKM (warung, kafe, toko) and industrial kiosks. Fully offline — your data stays on the device.

## Editions

| Edition | Kernel | Arch | ISO size | Config |
|---------|--------|------|----------|--------|
| Headless | 7.2.7 | x86_64 | 20 MB | `defconfig-qemu` |
| Headless RT | 6.19.3-rt1 | x86_64 | 20 MB | `defconfig-rt` |
| GUI | 7.2.7 | x86_64 | 21 MB | `defconfig-qemu` |
| GUI RT | 6.19.3-rt1 | x86_64 | 21 MB | `defconfig-rt` |
| Intel GPU | 7.2.7 | x86_64 | 30 MB | `defconfig-intel` |
| AMD GPU | 7.2.7 | x86_64 | 35 MB | `defconfig-amd` |
| NVIDIA GPU | 7.2.7 | x86_64 | 31 MB | `defconfig-nvidia` |
| Raspberry Pi 3 | 7.2.7 | ARM64 | 18 MB | `defconfig-arm64-rpi3` |
| Orange Pi Zero 2W | 7.2.7 | ARM64 | 17 MB | `defconfig-arm64-orangepi-zero2w` |

RT editions use the PREEMPT_RT kernel tree (`6.19.3-rt1`); all other editions use
the base kernel (`7.2.7`).

## What's in this repo

| Path | Contents |
|------|----------|
| `configs/` | Kernel configs (base, RT, GPU, ARM64) — see `configs/README.md` |
| `scripts/` | Build pipeline: kernel → rootfs → ISO, plus POS builds |
| `scripts/deprecated/` | Historical build scripts, kept for reference only |
| `installer/` | `wayang-installer` — the USB installer's TUI (Rust, dcheck's HUD look): pick a disk, add SSH keys, install |
| `userspace/` | Reference init scripts (legacy — see `userspace/README.md`) |
| `docs/` | Architecture and per-component build notes |
| `landing-page/` | Static website (wayang.dalang.io) |

The rootfs ships exactly **3 static binaries**: BusyBox, `dropbearmulti`, and
curl. Everything else is deployed as a static binary via `scp` — there is no
package manager.

## Hardware Target

- **SBC:** Raspberry Pi 3 and Orange Pi Zero 2W (~$15)
- **Display:** 7" touchscreen LCD (1024×600)
- **Storage:** MicroSD card (any size)
- **Total cost:** Under $50 for a complete POS terminal

Also runs on any x86_64 machine via QEMU or bare metal.

**Minimum (x86_64, tested):** 72 MiB RAM, 1 CPU (any x86-64), no disk — it
runs from RAM. 128 MiB recommended. See [`docs/MINIMUM-SPEC.md`](docs/MINIMUM-SPEC.md).

## Architecture

- **Rendering:** Direct framebuffer writes to `/dev/fb0` (32-bit BGRA)
- **Input:** Linux evdev (`/dev/input/eventN`) for touch, mouse, keyboard
- **Database:** SQLite3 (compiled into the app)
- **Font:** Embedded bitmap font, no external dependencies
- **Boot:** Custom BusyBox initramfs → Dropbear SSH → app
- **Init:** BusyBox init, no systemd/openrc

## Networking

By default the uplink is chosen automatically: init brings up every wired NIC
and keeps the first that actually gets a DHCP lease (a dead onboard NIC or a
late USB-Ethernet adapter won't block it). Only the **primary** interface owns
the default route and DNS.

Pin an interface and its addressing — persisted on `/data`, applied
immediately, survives updates:

```sh
wayang-net list                          # interfaces, link, driver, address
wayang-net use enp0s20u1                 # primary = enp0s20u1, DHCPv4
wayang-net auto                          # forget the choice, auto-detect again

# DHCP (default IPv4), IPv6 SLAAC, or both:
wayang-net set enp0s20u1 dhcp ipv4
wayang-net set enp0s20u1 dhcp both       # IPv4 DHCP + IPv6 SLAAC

# Static addressing (address must include a /prefix):
wayang-net set enp0s20u1 static \
    --ipv4 192.168.1.50/24 --ipv4-gw 192.168.1.1 --ipv4-dns "1.1.1.1 8.8.8.8" \
    --ipv6 2001:db8::50/64 --ipv6-gw 2001:db8::1 --ipv6-dns "2001:4860:4860::8888"
```

Mode and family live in `/data/etc/network/config` (`MODE=dhcp|static`,
`FAMILY=ipv4|ipv6|both`) and the NIC in `/data/etc/network/primary`. DHCPv6 is
not implemented: a family including `ipv6` uses SLAAC (`accept_ra=2`).

### WiFi

USB WiFi needs three pieces: a kernel with the wireless stack, firmware blobs,
and `wpa_supplicant`/`iw`. The base image is unchanged — WiFi is opt-in.

- **Kernel:** build from `configs/defconfig-wifi` (a fragment on top of
  `defconfig-qemu`) — `./scripts/build-kernel.sh defconfig-wifi`.
- **Userspace + firmware:** drop static `wpa_supplicant`, `wpa_cli` and `iw`
  into `$BUILD/wifi/`, and firmware blobs (`rtlwifi/`, `mt76/`, `ath9k_htc/`,
  `brcm/`, …) into `$BUILD/wifi/firmware/`. `scripts/fetch-sources.sh` can
  fetch them best-effort: `WIFI_TOOLS_URL=<tar> FIRMWARE_URL=<tar>
  ./scripts/fetch-sources.sh`. `build-rootfs.sh` installs the tools to
  `/usr/sbin` and copies firmware to `/lib/firmware/` when present; it skips
  both silently if the directories are absent.

```sh
wayang wifi          # scan, pick an SSID, enter the passphrase, connect
```

`wayang wifi` writes `/data/etc/wpa_supplicant.conf` and sets the wifi interface
as primary. On boot (and on `wayang-net restart`) `/etc/init.d/network` starts
`wpa_supplicant -B -i <iface> -c /data/etc/wpa_supplicant.conf` before DHCP.
With no wireless NIC, driver, firmware, or `wpa_supplicant`, it prints a clear
message instead of failing.

## Quick Start

```bash
# 1. Download external sources (kernel, BusyBox, Dropbear, SQLite)
./scripts/fetch-sources.sh
#    For the RT tree instead: KERNEL_FLAVOR=rt ./scripts/fetch-sources.sh

# 2. Build a bootable POS ISO
./scripts/build-pos-iso.sh defconfig-qemu wayangos-pos-qemu.iso
```

Full instructions, prerequisites, the edition matrix, and QEMU testing are in
[`BUILDING.md`](BUILDING.md).

## Default Login

- **OS shell (serial/console):** root shell, no login prompt (development image)
- **SSH:** root, public key only — keys are added per box in the installer (GitHub/GitLab user, USB stick, or pasted), later with `wayang-addkey`, or baked in with `SSH_AUTHORIZED_KEYS=~/.ssh/id_ed25519.pub ./scripts/build-rootfs.sh`
- **POS app:** username `admin`, PIN `1234`

> **Security:** root SSH accepts public keys only; password logins are disabled. The local console still drops straight into a root shell. See the Security section in `BUILDING.md`.

## Security

The base rootfs is intended for development and controlled deployments. Before shipping to the field:

- Build with only the deployment's keys in `SSH_AUTHORIZED_KEYS`
- Restrict or disable Dropbear on port 22
- Change the default POS admin PIN

## Contributing

- Shell scripts are linted with `shellcheck` in CI (`.github/workflows/ci.yml`).
- Keep scripts POSIX `sh` where possible; they run inside a BusyBox initramfs.

## License

MIT — see [`LICENSE`](LICENSE).
