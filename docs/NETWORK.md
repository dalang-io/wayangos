# WayangOS networking — interface (frozen)

How the installer and the installed system choose an uplink and IP config.
Implemented by parallel work: **A** = rootfs/init (`scripts/build-rootfs.sh`),
**B** = installer UI (`installer/**`). Do not change without updating this file.

## Persisted config (on `/data`, survives updates)

```
/data/etc/network/primary     # interface name, e.g. eth0 / enp0s20u1
/data/etc/network/config      # key=value, shell-sourceable (see below)
/data/etc/network/also        # extra interfaces to lease address-only at boot
```

`config` is a POSIX `sh` snippet (no spaces around `=`, values may be quoted):

```sh
# /data/etc/network/config
MODE=dhcp | static
FAMILY=ipv4 | ipv6 | both          # static: which families to configure
IPV4_ADDRESS=192.168.1.50/24
IPV4_GATEWAY=192.168.1.1
IPV4_DNS="1.1.1.1 8.8.8.8"
IPV6_ADDRESS=2001:db8::50/64
IPV6_GATEWAY=2001:db8::1
IPV6_DNS="2001:4860:4860::8888"
```

Missing keys are ignored. Absent `primary`/`config` → auto mode (probe every
wired NIC, keep the first that gets DHCP; see the current init).

## Boot behavior (`/etc/init.d/network`)

1. Bring up all wired NICs.
2. If `/data/etc/network/primary` exists → that interface only.
   - `MODE=static` → set `IPV4_ADDRESS`/`IPV6_ADDRESS`, add default routes
     (`IPV4_GATEWAY`/`IPV6_GATEWAY`), write `/etc/resolv.conf` from the DNS
     lists. Log the applied addresses.
   - `MODE=dhcp` (or unset) → `udhcpc` (IPv4). For `FAMILY=ipv6|both`, also
     enable SLAAC on the interface (`accept_ra=2`) — DHCPv6 is **not**
     implemented (documented limitation).
3. Else auto-probe as today.
4. After the primary is up, lease every interface in `also` address-only
   (wired or wireless, one name per line, `#` comments and blank lines
   ignored, duplicates collapsed). This is in addition to the one-shot pass
   over the remaining wired NICs and exists so a specific secondary — e.g. a
   wireless adapter — is always leased at boot.

Only the primary interface owns the default route and DNS. `also` interfaces
get an address (and nothing else); DHCP route/DNS are still installed solely
when `udhcpc.script` sees the primary interface.

## `wayang-net` CLI (in the rootfs)

```sh
wayang-net list                     # interfaces, link, driver, address, primary
wayang-net use <iface>              # primary = iface, DHCP (as today)
wayang-net set <iface> dhcp [ipv4|ipv6|both]
wayang-net set <iface> static --ipv4 A/P --ipv4-gw GW --ipv4-dns "D…" \
                               [--ipv6 A/P --ipv6-gw GW --ipv6-dns "D…"]
wayang-net up <iface> | down <iface>  # bring a link up/down (no address change)
wayang-net dhcp <iface>             # lease a NIC without making it primary
wayang-net also                     # list interfaces and their "also" state
wayang-net also <iface> on|off      # always lease iface address-only at boot
wayang-net auto                     # forget choice, auto-detect again
```
`set` writes `primary` + `config` and applies immediately (so `curl` works
before a reboot). `auto` removes both and restarts the network service.
`up`/`down` toggle the link only. `dhcp` gets a lease on any interface while
leaving the default route/DNS on the current primary (`udhcpc.script` only
installs those on the primary), which is what you want when testing multi-NIC
boxes. `also` edits the persisted `/data/etc/network/also` list (it does not
lease now); those interfaces are leased address-only on every boot, so a
secondary is reachable after a reboot without a manual `dhcp`. The `wayang net`
HUD exposes the same actions on the selected row:
`u` = up, `d` = down, `h` = DHCP here, `l` = toggle "also lease at boot"
(the row shows `+` when set).

## Installer network screen (B)

New step between **Target** and **Access** (so keys can be fetched afterwards):

- Lists wired interfaces (name, link, driver, MAC).
- Mode: **DHCP** or **Static**.
- Family (static): **IPv4 / IPv6 / Both**.
- Static fields: address/prefix, gateway, DNS (IPv4 and/or IPv6).
- Applies the choice to the **live** installer environment immediately
  (reuse the same logic as `wayang-net set`) so key fetching works.
- Persists `primary` + `config` to the **target** `/data` (the ext4
  `WAYANGDATA` partition) as part of install, so the installed box uses it.
- Skippable (defaults to auto) — Enter on "Continue" with auto mode.

Input uses the existing `Modal::Input` pattern; add `InputKind` variants as
needed. Keep the installer's HUD style.

## Tests

- A: shell-only; verify `config` parsing with a temp tree (`DCHECK`-style) and
  `shellcheck`. Document that static/IPv6 is not boot-tested here. The `also`
  list is a plain newline file (`#` comments, deduped), exercised through the
  `wayang-net also` CLI.
- B: `cargo test` (parse/clamp of the config values, target persistence path,
  the `also` reader/writer: parse/dedupe/add/remove), `cargo run -- --screens`
  still works, `shellcheck` untouched scripts.

## Runtime TUI (`wayang`)

Two screens in the `wayang` HUD (reusing `wayang/src/hud.rs`), so a running box
can be reconfigured without editing files by hand:

- `wayang net` — list interfaces (name, link, driver, MAC, address, `*primary`,
  `+also`), pick one, set **DHCP** or **Static** (IPv4/IPv6/Both +
  address/gateway/DNS), then **Apply** (writes
  `/data/etc/network/{primary,config}` and applies now). `l` toggles the
  selected interface's boot lease (writes `/data/etc/network/also`).
- `wayang wifi` — list wireless interfaces; **Scan**; pick an SSID; enter the
  passphrase (and optional country code); **Connect**; persists to
  `/data/etc/wpa_supplicant.conf` and sets the wifi iface as primary.
  If `wpa_supplicant`/`iw` or a wireless interface is missing, show a clear
  message instead of failing.

Both reuse the apply/persist logic of `wayang-net`. Non-interactive hosts still
use the `wayang-net` CLI and `wayang net`/`wayang wifi` print help when stdout
is not a TTY.

### WiFi prerequisites (kernel + rootfs)

WiFi needs, per board:
- kernel drivers: `CFG80211`, `MAC80211`, `RFKILL`, a vendor driver
  (`iwlwifi`/`rtl8xxxu`/`rtw88`/`mt76x0u`/`ath9k_htc`/…), `FW_LOADER`;
- firmware blobs in the rootfs (`/lib/firmware`);
- userspace: `wpa_supplicant` + `wpa_cli` + `iw`.

The default kernel (`configs/defconfig-intel`) enables the common USB WiFi
drivers **and** Intel `iwlwifi`/`iwlmvm` + `btusb`. `configs/defconfig-wifi`
adds the other USB vendors on top of the QEMU base.

Firmware is staged from a `linux-firmware` tree by
[`scripts/stage-firmware.sh`](../scripts/stage-firmware.sh) into
`$BUILD/wifi/firmware/`, which `build-rootfs.sh` copies to `/lib/firmware`.
The kernel has no `CONFIG_FW_LOADER_COMPRESS`, so `.zst` sources are
decompressed while staging. The curated list currently covers Intel WiFi
(`iwlwifi-3160/3168/7265D/8265/9000/9260`), Intel Bluetooth (`ibt-12-16.*` and
the legacy `ibt-hw-37.8*.bseq`), `i915/kbl_dmc_ver1_04.bin` and `regulatory.db`.
Add entries when new hardware shows up — find the exact filename in
`dmesg | grep -i firmware` or with `wayang wifi detect` (see below).

Userspace tools are built statically by
[`scripts/build-wifi-tools.sh`](../scripts/build-wifi-tools.sh) (`iw` +
`wpa_supplicant` + `wpa_cli`, using static `libnl` and OpenSSL) into
`$BUILD/wifi/`; `build-rootfs.sh` installs them into `/usr/sbin`. Both scripts
are best-effort and called by `scripts/ci-build.sh` before the rootfs step.

`wayang wifi detect` lists WiFi hardware even when no driver is bound: it reads
`/sys/class/net` for bound interfaces and `/sys/bus/usb/devices` for unbound USB
adapters (USB vendor:product + a driver/firmware suggestion). Set `WAYANG_SYS` to
inspect an offline image. Devices already bound to a driver (e.g. a Bluetooth
`btusb` function, also USB class `0xe0`) are not reported as WiFi candidates.

At boot the network init starts `wpa_supplicant` on the primary wifi interface
when `/data/etc/wpa_supplicant.conf` exists.
