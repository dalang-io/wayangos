# Tested hardware

Real-device bring-up notes: what was found, which driver/firmware it needs, and
what to check when a new board shows up. Kernel config lives in
[`configs/`](../configs/README.md); firmware staging in
[`scripts/stage-firmware.sh`](../scripts/stage-firmware.sh); WiFi userspace in
[`scripts/build-wifi-tools.sh`](../scripts/build-wifi-tools.sh).

## Kaby Lake desktop (Intel 200-series PCH)

A ThinkStation-class mini desktop was used for the first real install. Detected
devices and status:

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
