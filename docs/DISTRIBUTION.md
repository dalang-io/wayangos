# Distributing WayangOS to clients

Status: **proposal / roadmap**, 2026-10-03. Anything marked `[x]` exists today; the
rest is the plan. Companion docs: [`MT7621A.md`](MT7621A.md) (the MIPS port this
document has to carry), [`CHANNEL.md`](CHANNEL.md) (the update channel as implemented),
[`UPDATE-DESIGN.md`](UPDATE-DESIGN.md) (bundle format and A/B state),
[`UPDATE.md`](UPDATE.md) (operator guide), [`BUILDING.md`](BUILDING.md) (how an image is
built), [`EDGEROUTER.md`](EDGEROUTER.md) + [`EDGEROUTER-RUNBOOK.md`](EDGEROUTER-RUNBOOK.md)
(the wayangi "Edge" unit).

Goal: a client can be handed a box (or ten, across sites), and from then on every
install, update, configuration push and revocation is a documented, repeatable
operation — with a rollback that works when the network to us is broken.

Status legend: `[ ]` open · `[~]` in progress · `[x]` done · `[?]` needs a decision.

---

## 1. What exists today (facts)

| Piece | State |
|---|---|
| Update client | `wayang update` / `upgrade` / `sign` / `verify`, A/B slots, GRUB `grubenv` fallback (`wayang_slot/good/attempts`, 3 failed boots → last good) — `docs/UPDATE-DESIGN.md:59-70,107-123` |
| Bundle | `wayang-<version>-<arch>.wup` (tar.gz, members at root) with `manifest.json` + a raw 64-byte ed25519 `manifest.json.sig` — `UPDATE-DESIGN.md:72-78` |
| Channel layout | `<base>/<channel>/<arch>/manifest.json` and `…/wayang-<version>-<arch>.wup`; `<base>` default `https://wayang.dalang.io/channel` (`wayang/src/fetch.rs` `DEFAULT_BASE`); `<channel>` = `stable` \| `edge` from `/etc/wayang/channel` or `--channel`; `<arch>` = `x86_64` \| `arm64` — `docs/CHANNEL.md` |
| Trust | `/etc/wayang/trusted_keys`, lines `<keyid> <64-hex-ed25519-pub>` — `UPDATE-DESIGN.md:11` |
| Exit codes | `0` ok · `1` error · `2` no update available · `3` verify/signature — `UPDATE-DESIGN.md:123` |
| Install | USB ISO + `wayang-installer` TUI: pick disk, add SSH keys, formats 512 MiB vfat ESP `WAYANGBOOT` + ext4 `WAYANGDATA` — `BUILDING.md:149-152`, `UPDATE-DESIGN.md:14-52` |
| **Arch shipped by the pipeline** | **x86_64 only** (`wayangos/scripts/ci-build.sh:28`, `wayangos/scripts/build-remote.sh:77,79`, meta `"arch":"x86_64"` at `wayangos/scripts/build-installer-iso.sh:169`); arm64 is wired but M7/ best-effort |
| Package manager | **none, deliberately**: everything is baked into the initramfs at build time or `scp`'d as a static binary to `/data/bin` (`README.md:44,130,162`) |
| Operators | three TUIs (`wayang`, `wayang-fw`, `wayang-router`) + opt-in HTTP APIs (`/etc/init.d/api` starts them only when `/data/etc/<tool>/api.args` exists) + the separate web console (`wayang-webui`, another repo, deploy it yourself) |

The consequence to keep in mind for everything below: **there is no "install a
package" step.** Any change reaches a client as (a) a new image, (b) a new signed
bundle, or (c) a static binary copied to `/data/bin`. The distribution plan is the
plan for choosing between those three, per change.

---

## 2. The fleet shape we are distributing into `[?]`

Decide (and write down) the shape before touching the pipeline, because it decides
which of §3–§6 are needed:

* **Unit**: one box per client, or a site with 2–10 (edge + core, or per-branch)?
  Multi-box sites change the update strategy (staggered rollout, not all at once).
* **Reachability**: does the box pull updates from our channel over the internet
  (today's model), or do we push? A box behind CGNAT/PPPoE with no static address can
  pull but not be pushed to — the pull model must stay the default.
* **Management path**: wayangi (the "Edge" unit, WireGuard tunnel to the hub,
  delegated prefix) or the client's own management (the web console pointed at the
  box's API, `[[device]]` + token)? Both exist; a client needs exactly one documented
  as *the* path.
* **Who owns the box**: we do (a managed service: we patch it) or the client does (we
  hand over an image and a runbook). This decides whether the update channel is
  yours-forever or a one-time delivery.

---

## 3. Per-arch delivery matrix (what a client receives)

| Arch | Client hardware | Install | Boot | Update | Console |
|---|---|---|---|---|---|
| `x86_64` `[x]` | mini-PC (the current Edge units) | USB ISO + installer TUI | GRUB/UEFI, A/B in ESP | `.wup` via channel (`wayang update`) | `wayang-webui` off-box or on-box |
| `arm64` `[~]` | RPi3 / Orange Pi Zero 2W (M7) | image written to SD/eMMC | vendor bootloader (best-effort) | `.wup`, boot partition slots | off-box |
| `mipsel` `[ ]` | MT7621A router (see [`MT7621A.md`](MT7621A.md)) | image written over U-Boot (TFTP/USB) or from an existing OpenWrt with `mtd write` | **U-Boot + NAND** — the GRUB A/B model does not apply | sysupgrade-style image write + U-Boot env flag for A/B | off-box (recommended) |

Two things this table makes obvious:

1. **The boot/update mechanism is per-arch, not universal.** The A/B *promise* (never
   brick a client's box; a failed boot returns to the last good one) must be honoured
   on every row — but it is implemented by GRUB on x86 and by U-Boot + NAND on MIPS.
   That work is [`MT7621A.md`](MT7621A.md) M2/M5, not something this doc can hand-wave.
2. **A client-facing "which image do I flash?" decision tree belongs in the runbook**,
   not in a maintainer's head.

---

## 4. Build and sign pipeline, per arch `[ ]`

What CI has to grow (each item is small, but all of them are required before a
non-x86 client exists):

* `[ ]` `ci-build.sh` / `build-remote.sh`: take `ARCH` (or `TARGET`) as a parameter,
  build the kernel + rootfs + image for it, and name artifacts
  `…-<arch>.<ext>` (the ISO naming at `wayangos/scripts/ci-build.sh:28` is hardcoded x86_64).
* `[ ]` Per-arch kernel fragment with the dataplane options that matter
  (`NF_TABLES`, `NF_FLOW_TABLE`, `NFT_FLOW_OFFLOAD`, `NF_CONNTRACK`, `WIREGUARD`,
  tc/qdisc set, watchdog, cpufreq). Today only `defconfig-intel` carries them; the
  arm64 fragments carry none.
* `[ ]` Per-arch static tool set (BusyBox, Dropbear, nft/iproute2, BIRD, wireguard-tools,
  radvd, dnsmasq, conntrackd) — the existing scripts already fail loudly if an asset
  is missing; the MIPS cross-toolchain is the new part.
* `[ ]` Per-arch `wayang-fw` / `wayang-router` artifacts, sha256-pinned, from their own
  release pipelines (`wayangos/scripts/build-wayang-fw.sh:19-23` is the pattern).
* `[ ]` A `.wup` per arch and a channel directory per arch
  (`<base>/<channel>/<arch>/…`), with the manifest carrying the arch so a device can
  refuse a wrong-arch bundle (`wayang verify` already exits `3` on a bad signature —
  make it exit non-zero on an arch mismatch too).
* `[ ]` **Key custody**: the ed25519 signing key must not live in a repo or a CI
  variable that anyone with build access can read. Decide: offline key + manual sign
  step, or CI with a restricted secret and an audit trail. Document rotation: a new
  `<keyid>` line in `/etc/wayang/trusted_keys` on the fleet *before* signing with it
  (an old box that has never seen the new key must still take the update that carries
  it — plan the two-step).
* `[ ]` Reproducibility note: pin kernel/tool versions and record them in the manifest
  (the repo already pins: kernel tarball, BusyBox 1.37.0, Dropbear 2024.86, nftables
  1.1.1, iproute2 7.2.0, BIRD 2.19.2, wireguard-tools 1.0.20260223, radvd 2.21,
  dnsmasq 2.93, conntrackd 1.4.9).

---

## 5. Provisioning a client (the bundle) `[ ]`

Today a box is configured by hand (`/data/etc/fw/config.toml`,
`/data/etc/router/config.toml`, `/data/etc/wayangi/token`, `/data/etc/<tool>/api.args`,
`/etc/wayang/channel`). For a fleet this becomes a *bundle* produced per client:

```
provision/
  site.toml            # client name, site id, channel, tags — the one file a human edits
  fw/config.toml       # wayang-fw candidate (never a committed ruleset: first boot applies + confirms)
  router/config.toml   # wayang-router config
  wayangi/token        # enrolment token (wayangi Edge units only)
  api/api.args         # which tool APIs listen, on what address, --ro or --rw
  ssh/authorized_keys  # the operator's keys (installer asks for these today, by hand)
  manifest.json        # bundle version + sha256 of each file
  manifest.json.sig    # ed25519, same key/trust story as the image
```

* `[ ]` Applying it: a one-shot at first boot (or `wayang provision --from …`) that
  verifies the signature, copies files into `/data/etc/*`, marks itself done in
  `/data/var/wayang/provision.done`, and never overwrites a config that a human has
  since edited (checksum + timestamp, same discipline the updater already uses).
* `[ ]` Idempotence: applying the bundle twice is a no-op; applying an older bundle is
  refused unless `--force`.
* `[ ]` What is *not* in the bundle: private keys for mTLS (per-box generation), the
  wayangi tunnel identity (per-box), and anything secret that would make one bundle
  leak the whole fleet.
* `[ ]` The runbook: one page per client ("plug in, hold reset, flash from TFTP at
  this address, then: it appears on the dashboard within N minutes"), with a
  pre-flight checklist and a rollback line.

---

## 6. Update, rollout and rollback for clients `[ ]`

* `[ ]` **Channel per client**: default `stable`; a pilot client may sit on `edge`.
  The file is `/etc/wayang/channel`; decide who may change it (client-side edit vs
  console push) and record it in the client's inventory row.
* `[ ]` **Staged rollout**: publish to `edge` → soak on one internal box (the test
  device, `docs/HARDWARE.md`) → promote to `stable`. This is a process, and the plan
  is to write it as a checklist with explicit soak time and rollback trigger
  (e.g. "3 boxes fail to report after 30 min → pull the manifest").
* `[ ]` **Rollback**: A/B by boot slot on x86 (exists); U-Boot/NAND equivalent on
  MT7621A (`MT7621A.md` M5). Acceptance for both: a deliberately corrupt image
  returns the box to the previous version with the client's config intact.
* `[ ]` **Visibility**: the web console already stores each box's `version` in its
  snapshot, so "which clients are behind" is one page away `[ ]`; make it an explicit
  acceptance item (a fleet version table + an alert when a client is N versions
  behind), because an update channel without drift visibility becomes archaeology.
* `[ ]` **Offline clients**: if a client's box cannot reach the channel, the runbook
  must cover `wayang update --from FILE.wup` (the bundle can be carried on a USB stick
  — the updater already accepts `--from FILE`).

---

## 7. Fleet operations `[ ]`

* `[ ]` **Inventory** (one row per deployed box): client, site, board, arch, serial,
  image version, channel, management path (wayangi or console), API scope (`--ro` /
  `--rw`), who has SSH, deployment date. Keep it in a file in this repo
  (`docs/FLEET.md`? `deploy/fleet.toml`?) so it is versioned with the images.
* `[ ]` **Remote management**: two supported paths, both already exist —
  wayangi (tunnel + dashboard) and the web console (`[[device]]` with url + token,
  optional mTLS). A client must have exactly one documented, and the other must be
  *off by default* (no listening API unless `api.args` says so — that is already the
  shipped default).
* `[ ]` **Revocation** (the boring, load-bearing part): rotate a box's API token
  (`wayang-fw api --gen-token … --append`, then remove the old line and commit);
  rotate a client's wayangi enrolment token; rotate the NOC wall's kiosk token
  (`[wall] kiosk_token` + restart — every screen loses access at once); rotate the
  image signing key (§4). Each of these needs one paragraph in the runbook and one
  line in the incident log when it is first exercised.
* `[ ]` **Support matrix** (publish it): for each arch/board — *supported* (we test
  every release), *community* (works, best-effort), *not supported*. The MT7621A row
  starts as "community" until `MT7621A.md` M4/M6 pass.
* `[ ]` **Remote recovery**: every client box needs a documented way back that does
  not need us on site — serial console pinout in the runbook, plus the NAND
  recovery procedure (`MT7621A.md` M2). A managed router that can only be fixed by
  driving to the site is not a product.

---

## 8. Acceptance criteria (per milestone)

1. `[ ]` **A fresh board becomes a client box** by following only the runbook: flash →
   provision bundle → appears in inventory → managed. No maintainer-only step.
2. `[ ]` **An update reaches a client** through the channel, is signature-verified,
   and a deliberately corrupt bundle is refused with exit `3` and no change.
3. `[ ]` **A bad update rolls back** by itself (3 failed boots) with the client's
   config intact — exercised on real hardware, both x86 and MT7621A.
4. `[ ]` **A revocation works**: an API token, a wayangi token and the wall's kiosk
   token each revoked and observed from the client side (403 / tunnel down / screen
   out), in one sitting.
5. `[ ]` **Drift is visible**: the console shows the fleet's image versions, and a
   client behind by N versions is visible without opening a terminal.
6. `[ ]` **Everything above is in docs**, with the commands, not just the intent:
   this file, the per-arch docs, and the one-page client runbook.

---

## 9. Open questions

1. **Key custody and rotation** (§4): offline signing vs CI secret; who can promote a
   channel (`edge` → `stable`).
2. **Commitment level**: is an update *ours to push* (managed service — the client's
   box may be patched without asking) or *the client's to pull* (delivery — the box is
   theirs to maintain)? This changes §6's rollout and §7's support matrix wording.
3. **Telemetry**: does a client box report anything to us beyond what wayangi already
   needs (health, version)? Opt-in, and where it is documented.
4. **Client reality in Indonesia**: PPPoE and CGNAT (no inbound; pull-only updates),
   DNS interception by upstream ISPs (the box's own dnsmasq matters), intermittent
   power (NAND wear, boot time), and one-bar 4G as WAN (the router's failover
   features get used for real).
5. **The "no package manager" consequence, stated for clients**: any fix that is not a
   full image is a static binary under `/data/bin` (the documented `scp` path,
   `README.md:130`) — decide whether client-visible changes always ship as an image
   (auditable, slower) or sometimes as a binary drop (faster, more states to reason
   about).
6. **Who owns the `.wup` for third-party boards**: if a client flashes our MT7621A
   image onto their own hardware, the support matrix and rollback promise must say
   what happens when the vendor's U-Boot differs (`MT7621A.md` risk 4).
