# WayangOS networking — interface (frozen)

How the installer and the installed system choose an uplink and IP config.
Implemented by parallel work: **A** = rootfs/init (`scripts/build-rootfs.sh`),
**B** = installer UI (`installer/**`). Do not change without updating this file.

## Persisted config (on `/data`, survives updates)

```
/data/etc/network/primary     # interface name, e.g. eth0 / enp0s20u1
/data/etc/network/config      # key=value, shell-sourceable (see below)
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

Only the primary interface owns the default route and DNS.

## `wayang-net` CLI (in the rootfs)

```sh
wayang-net list                     # interfaces, link, driver, address, primary
wayang-net use <iface>              # primary = iface, DHCP (as today)
wayang-net set <iface> dhcp [ipv4|ipv6|both]
wayang-net set <iface> static --ipv4 A/P --ipv4-gw GW --ipv4-dns "D…" \
                               [--ipv6 A/P --ipv6-gw GW --ipv6-dns "D…"]
wayang-net auto                     # forget choice, auto-detect again
```
`set` writes `primary` + `config` and applies immediately (so `curl` works
before a reboot). `auto` removes both and restarts the network service.

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
  `shellcheck`. Document that static/IPv6 is not boot-tested here.
- B: `cargo test` (parse/clamp of the config values, target persistence path),
  `cargo run -- --screens` still works, `shellcheck` untouched scripts.

## Runtime TUI (`wayang`)

Two screens in the `wayang` HUD (reusing `wayang/src/hud.rs`), so a running box
can be reconfigured without editing files by hand:

- `wayang net` — list interfaces (name, link, driver, MAC, address, `*primary`),
  pick one, set **DHCP** or **Static** (IPv4/IPv6/Both + address/gateway/DNS),
  then **Apply** (writes `/data/etc/network/{primary,config}` and applies now).
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
  (`rtl8xxxu`/`rtw88`/`mt76x0u`/`ath9k_htc`/…), `FW_LOADER`;
- firmware blobs (e.g. `rtlwifi/`, `mt76/`, `ath9k_htc/`) in the rootfs;
- userspace: `wpa_supplicant` + `wpa_cli` (+ optional `iw`).

`configs/defconfig-wifi` enables the common USB WiFi drivers. The rootfs bundles
`wpa_supplicant`/`iw` **if present** under `$BUILD/wifi/` (fetched/built
separately) and starts `wpa_supplicant` on the primary wifi iface at boot.
