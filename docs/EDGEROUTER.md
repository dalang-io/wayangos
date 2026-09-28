# TODO — WayangOS as a wayangi EdgeRouter

Practical end-to-end operations: [docs/EDGEROUTER-RUNBOOK.md](EDGEROUTER-RUNBOOK.md).

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

- [x] **Enrol**: how a box becomes a device on the dashboard — dashboard
      creates a device + token (see `internal/web/install.go` `--token=`), the
      operator pastes the token on the box (`wayang edgerouter enroll <token>`)
      or installs the whole Edge bundle (`wayang edgerouter apply`, below).
- [x] **Import/apply a dashboard bundle**: `wayang edgerouter apply <dir|.tar.gz>`
      (+ the **09 EDGEROUTER** `a`/`A` keys) unpacks a wayangi **WayangOS Edge
      bundle** to a temp dir, verifies it carries non-empty `router.toml` +
      `fw.toml` that look like the right kind (best-effort `[[…]]` markers),
      writes them to `/data/etc/router/config.toml` and `/data/etc/fw/config.toml`
      (mode 0644; refuses to clobber without `--force`, keeping a `.bak` when
      forced), and enrols the bundle's `token` file — or the token parsed out of
      `install.sh` — through the existing `enroll()` path (never printed). It
      deliberately does **not** apply/commit: the CLI prints the exact next steps
      (`wayang-router check` → commit, `wayang-fw check` → commit; commit-confirm
      rolls back if unconfirmed) and the boot order. The HUD runs it as a
      background job (`edge_job`), so it never blocks.
- [x] **TUI/CLI** on WayangOS to paste the token: `wayang edgerouter
      status|start|stop|restart|enroll <token>|clear` + the **09 EDGEROUTER**
      console module (token written to `/data/etc/wayangi/token`, mode 0600;
      shows agent/token/tunnel, the delegated prefix, handshake, account/plan
      and last bootstrap). `start|stop|restart` drive the agent in the
      background (bounded, output captured; never argv so the token can't leak
      via `ps`) and then print the state.
- [x] Boot order: `/data` → firewall → **WireGuard tunnel up (bootstrap)** →
      router applies addresses/routes/prefix → network; the hub peer comes up
      as soon as the tunnel does. `/etc/init.d/edgerouter start` runs after `fw`
      and before `network`; it is a no-op unless the agent binary and the token
      at `/data/etc/wayangi/token` both exist, and it backgrounds the agent
      (log: `/var/log/wayangi.log`) so boot never waits on the hub.
- [~] Token lifecycle: renewal/revocation (`internal/web/renewal.go` drops the
      peer on expiry). `wayang edgerouter status` and the console now classify
      "token missing", "hub rejected the token (revoked/expired/unauthorized)",
      "subscription not active" and "tunnel down" with a clear next step, read
      from the agent's JSON state and the tail of its log (no secrets shown).
      Dashboard-driven rotation still means re-running `enroll`.

## D. Prefix delegation → LAN (EdgeRouter role)

- [ ] Receive the delegated prefix from bootstrap; install it (AnyIP or routed)
      on the WG/TUN device. (The agent does this; `wayang` can carry it via
      `wayang-router` `delegated_prefix`/`prefix_from`.)
- [x] **Advertise to the LAN**: `wayang-router` renders SLAAC/RDNSS/DNSSL via
      `/etc/radvd.conf` and drives `radvd`. The image bundles a static `radvd`
      at `/usr/sbin/radvd` (`scripts/build-radvd.sh`, optional/best-effort);
      without it the prefix and addresses are still installed and
      `wayang-router` warns instead (warn-and-no-op).
- [ ] IPv6 forwarding + a default route toward the hub (inbound-only by
      default; `full_duplex` only when the hub grants enterprise). (Router sets
      `ipv6_forwarding` when RA/delegation is on; the tunnel default route is
      the agent's.)
- [x] Firewall zones/policies for the delegated prefix: `wayang-fw` IPv6
      objects/policies + per-family NAT (NAT66) are in.
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
   `/usr/sbin/wayangi` install + persisted dir, no token baked); `wayang
   edgerouter enroll` + `apply` (**done**) consume the dashboard's token/bundle;
   prove bootstrap + tunnel + prefix install on the device.
3. Add an RA daemon and IPv6 forwarding so a LAN client gets a global address
   from the delegated prefix; firewall it with wayang-fw.
