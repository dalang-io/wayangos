# TODO — WayangOS as a wayangi EdgeRouter

Goal: turn a WayangOS box (wayang-router + wayang-fw) into an **EdgeRouter**
registered on the **wayangi** dashboard (`~/dev/wayangi`, `wayangi.dalang.io`):
it enrols with a token, brings up a WireGuard tunnel to the hub, receives a
**delegated IPv6 prefix** (`/64` sub-leasing tier) and routes + firewalls it to
a LAN.

wayangi today: a per-device Go agent (`cmd/wayangi`) that calls
`POST /api/v1/bootstrap` (Bearer token + `{public_key}`), gets a **signed**
response (addresses, allowed_ips, routes, hub pubkey/endpoint, dns, mtu,
keepalive, `full_duplex`), brings up `wayangi0`, and installs the delegated
prefix with an AnyIP `ip route add local <prefix> dev wayangi0`
(`internal/tunnel/platform_linux.go`). The hub (`cmd/wayangi-hub`) manages the
WG peers and the dashboard. Prefix delegation + the `/64` tier are documented in
`docs/prefix-delegation.md`; the enterprise routing grant is
`routing_mode=full-duplex` (`class=enterprise`).

Status: `[ ]` open, `[~]` in progress, `[x]` done.

## A. Kernel (blockers, one bisect)

- [ ] **WireGuard** (`CONFIG_WIREGUARD` + arch crypto) — bisect group
      `wireguard`; required for the tunnel. Ship only after device validation
      (docs/ROUTER-KERNEL-BISECT.md).
- [ ] **IPv6 forwarding + policy routing / VRF / multiple tables** — bisect
      group `vrf-multipath`; needed for the reply-path v6 policy routing the
      agent installs and for handing the prefix to a LAN.
- [ ] (optional) `NET_SCH_*`/`tc` if the EdgeRouter does QoS — bisect `qos`.
- [ ] Confirm firewall (`nftables`, already on) covers IPv6 for the delegated
      prefix.

## B. Agent / tunnel client

- [x] Decide: **run the wayangi-go agent** (bundle `wayangi`) vs implement the
      bootstrap+tunnel in Rust in `wayang`. Bundling is faster; a native
      implementation avoids a second updater. (Decision: bundle the agent first,
      revisit later.)
- [x] Bundle `wayangi` (static) like `nft`/`wg`/`tc`/`bird` —
      `scripts/build-wayangi.sh`, installed to `/usr/sbin/wayangi`. Builds a
      local `WAYANGI_SRC` checkout (default `~/dev/wayangi`) on the Linux
      builder, or fetches a pinned binary from `WAYANGI_BASE_URL` when set;
      best-effort (skips the image without it). Wired into `ci-build.sh` and
      `installer-iso.yml`.
- [x] Persistent identity in `/data/etc/wayangi/`: token, device id, WG private
      key, last bootstrap; survives OS updates (never in the image).
      `build-rootfs.sh` creates the dir (mode 700) at first boot like
      `/data/etc/network`; the token itself is written at enrolment, not built.

## C. Enrollment + dashboard flow

- [ ] **Enrol**: how a box becomes a device on the dashboard — dashboard
      creates a device + token (see `internal/web/install.go` `--token=`), the
      operator pastes the token on the box.
- [ ] **TUI/CLI** on WayangOS to paste the token and run bootstrap (like the
      installer SSH-key step / `wayang addkey`): a new `wayang edgerouter`
      command + a console module showing tunnel state, handed prefix, handshake,
      account/plan (from the informational bootstrap fields).
- [ ] Boot order: `/data` → firewall → **WireGuard tunnel up (bootstrap)** →
      router applies addresses/routes/prefix → network; the hub peer comes up
      as soon as the tunnel does.
- [ ] Token lifecycle: renewal/revocation (`internal/web/renewal.go` drops the
      peer on expiry) — surface "token expired / device revoked" clearly.

## D. Prefix delegation → LAN (EdgeRouter role)

- [ ] Receive the delegated prefix from bootstrap; install it (AnyIP or routed)
      on the WG/TUN device.
- [ ] **Advertise to the LAN**: router advertisements (SLAAC) + RDNSS, or
      DHCPv6-PD downstream. Needs an RA daemon (radvd or implement in
      `wayang-router`) — none bundled yet.
- [ ] IPv6 forwarding + a default route toward the hub (inbound-only by
      default; `full_duplex` only when the hub grants enterprise).
- [ ] Firewall zones/policies for the delegated prefix (wayang-fw IPv6).
- [ ] (optional) NAT66 if the LAN is meant to be hidden; reverse DNS is
      out of scope (wayangi support ticket).

## E. Dashboard / telemetry

- [ ] Determine what the dashboard needs beyond the WG handshake (peer up =
      online via the hub's reconciler). If it needs agent-pushed metrics
      (throughput, sessions, drops, prefix usage), define + implement an
      authenticated report endpoint on the hub and a `wayang`/`wayangi` push.
- [ ] Show the box on the dashboard with the right **class/role** (edge router,
      `/64` tier) — check how `class`/`routing_mode` are set at device create.

## F. Ops & edge cases

- [ ] Two updaters: `wayang update` (OS, A/B) vs `wayangi update` (agent
      self-update). Reconcile — the agent should update with the OS bundle, or
      at least not fight it.
- [ ] Reconnect/backoff if the hub is unreachable; keep the LAN routing stable.
- [ ] Anti-lockout: the management SSH rule must survive IPv6 prefix changes and
      the WG interface appearing/disappearing (wayang-fw invariant).
- [ ] Document the whole flow in `docs/NETWORK.md` + a wayangi-side note.

## First concrete steps

1. Run the **WireGuard** (+ `vrf-multipath`) bisect groups on the device
   (`scripts/bisect-router-opts.sh --unattended …`, docs/ROUTER-KERNEL-BISECT.md)
   and land them in `configs/defconfig-intel`.
2. Bundle `wayangi` and add `/data/etc/wayangi/` (**done**: `build-wayangi.sh` +
   `/usr/sbin/wayangi` install + persisted dir, no token baked); a
   `wayang edgerouter` enrol command remains, then prove bootstrap + tunnel +
   prefix install on the device.
3. Add an RA daemon and IPv6 forwarding so a LAN client gets a global address
   from the delegated prefix; firewall it with wayang-fw.
