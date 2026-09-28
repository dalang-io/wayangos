# Tested hardware

Real-device bring-up notes: what was found, which driver/firmware it needs, and
what to check when a new board shows up. Kernel config lives in
[`configs/`](../configs/README.md); firmware staging in
[`scripts/stage-firmware.sh`](../scripts/stage-firmware.sh); WiFi userspace in
[`scripts/build-wifi-tools.sh`](../scripts/build-wifi-tools.sh).

## Kaby Lake desktop (Intel 200-series PCH)

The main **test device `root@163.128.55.3`** is a **Lenovo ThinkCentre M710q Tiny**
(family 6 model 158 = Kaby Lake). Full spec, for build/ISA and driver reference:

| | |
|---|---|
| CPU | Intel **Core i3-7100** @ 3.90 GHz (2c/4t, Kaby Lake, family 6 model 158) |
| ISA flags | `sse4_2 avx avx2 fma aes pclmulqdq bmi2 movbe` — **no `avx512f`**, no `sha_ni` |
| RAM | **8 GB** (`MemTotal ≈ 8020880 kB`) |
| Board / product | Lenovo `board_vendor 310C`, `product_name 30C1S0QS00` (`sys_vendor LENOVO`) |
| BIOS | `M1AKT36A` |
| Storage | NVMe (installer p1 ESP `WAYANGBOOT` + p2 ext4 `WAYANGDATA`) |
| Network | onboard I219-LM `e1000e` (`eth0`), USB SR9700 `sr9700` (`eth1`, the uplink) |
| WiFi | Intel 3168 (`iwlwifi`) + Intel Bluetooth (`btusb`) |

Detected devices and status:

| PCI/USB id | Device | Driver | Firmware | Status |
|---|---|---|---|---|
| `8086:5912` | HD Graphics 630 | `i915` | `i915/kbl_dmc_ver1_04.bin` | OK (DMC firmware optional) |
| `8086:15b7` | Ethernet I219-LM | `e1000e` | — | OK |
| `8086:a2c6` | 200-series PCH SATA | `ahci` | — | OK |
| `8086:a2b1` | PCH PCIe root port | `pcieport` | — | OK |
| `8086:a2a1` | PCH thermal subsystem | `intel_pch_thermal` | — | not enabled (harmless) |
| `8086:24fb` | **Wireless-AC 3168** (`24fb/2110`) | `iwlwifi`/`iwlmvm` | `iwlwifi-3168-29.ucode` | OK |
| `8087:0aa7` | Intel Bluetooth (combo) | `btusb`/`btintel` | `intel/ibt-hw-37.8.10-fw-22.50.19.14.f.bseq` | OK |
| `0fe6:9702` | USB 2.0 10/100 Ethernet (SR9700) | `sr9700` | — | OK |

### Build target / CPU ISA (avoid a wrong-arch compile)

- **Builds are generic x86-64**, not tuned to the builder: no `-march=native`,
  `GOAMD64` unset (Go baseline), `RUSTFLAGS` only adds link flags. So a binary
  built on the **builder** (Xeon **E5-2673 v4**, Broadwell, family 6 model 79)
  still runs on the i3-7100 (Kaby Lake) — both share `sse4_2/avx/avx2/fma/aes`,
  and **neither has AVX-512** (so no `avx512` codegen risk either way).
- Kernel: `CONFIG_GENERIC_CPU*`, and in the shipped kernel
  `CONFIG_CRYPTO_AES_NI_INTEL is not set` (no AES-NI SIMD path). Keep an eye on
  any `CRYPTO_LIB_*_ARCH` / `*_X86_64` options added for WireGuard/IPsec — the
  router-kernel interaction investigation names x86 SIMD crypto as a suspect
  ([docs/ROUTER-KERNEL-INTERACTION.md](ROUTER-KERNEL-INTERACTION.md)).
- If a future build ever sets `-march=`/`target-cpu` or `GOAMD64=v3`, it must be
  validated against this CPU's flags above. Baseline `v1`/`x86-64` is required.

### Gotchas

- **`8086:24fb` is a family id.** The actual chip (here *3168*, not 8265) comes
  from the subsystem id and shows up in `dmesg` as
  `Detected Intel(R) Dual Band Wireless-AC 3168`. Bundle the matching
  `iwlwifi-<chip>-*.ucode`, not the one implied by the top-level PCI id.
- **Intel Bluetooth firmware naming changed over time.** Older parts
  (3168/8260/7265-era) load `intel/ibt-hw-37.8*.bseq`; newer ones use
  `intel/ibt-<hw>-<rev>.sfi` + `.ddc`. Bundle both if unsure — the driver logs
  the exact name it tried (`dmesg | grep -i bluetooth`).
- **The kernel has no `CONFIG_FW_LOADER_COMPRESS`**, so firmware must be
  decompressed; `stage-firmware.sh` does this from Ubuntu's `.zst` blobs.
- **`/32` DHCP leases** (common in datacenters) leave no connected route, so the
  gateway is unreachable until an on-link host route is added. The network init
  script handles this automatically (`ip route add <gw> dev <iface>` before the
  default route).
- **Root SSH is pubkey-only.** Add keys on the console with
  `wayang-addkey github:USER` (or paste a key); they persist in
  `/data/etc/ssh/authorized_keys`.

## Owner's MiniPC (EdgeRouter target) — FILL IN

The owner's own WayangOS box (intended Edge router) is **not yet documented**.
Capture it with:

```sh
grep -m1 'model name' /proc/cpuinfo
grep -m1 flags /proc/cpuinfo | tr ' ' '\n' | grep -E 'sse4_2|avx|avx2|avx512f|aes|sha_ni'
grep MemTotal /proc/meminfo
cat /sys/class/dmi/id/{sys_vendor,product_name,board_name,bios_version}
lspci -nn | grep -iE 'ethernet|network|wireless'
lsusb
```

Record the CPU family/ISA here so any `-march`/`GOAMD64` change is validated
against it (see "Build target / CPU ISA" above).

## Identifying new hardware

```sh
lspci -nn | grep -iE 'ethernet|network|wireless'   # PCI NICs / WiFi
lsusb                                              # USB adapters
wayang wifi detect                                 # WiFi chips + firmware hints
dmesg | grep -iE 'firmware|iwlwifi|bluetooth'      # what the driver asked for
```

Add the resulting firmware filename to `scripts/stage-firmware.sh` and any new
kernel option to the relevant `configs/defconfig-*` fragment, then rebuild.

## Update / A-B state layout

On x86 the ESP nests everything under `boot/`:
`<ESP>/boot/grub/grubenv` (A/B state), `<ESP>/boot/{A,B}/{vmlinuz,initramfs.img}`
and `<ESP>/boot/var/meta-{A,B}.json`. See
[`docs/UPDATE-DESIGN.md`](UPDATE-DESIGN.md#on-disk-layout).
