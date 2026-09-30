# WayangOS docs — index

Where to read what. Start with `AGENTS.md` at the repo root (rules + boot order +
build), then these.

## Start here
* [GOAL.md](GOAL.md) — the product goal and roadmap.
* [ARCHITECTURE.md](ARCHITECTURE.md) — how the OS is put together.
* [BUILDING.md](BUILDING.md) — building the image on the build box.
* [HANDOVER.md](HANDOVER.md) — current state across the repos (releases, devices).
* [TODO.md](TODO.md) — the cross-repo pending-work index.

## Consoles / UX
* [HUD-USER-GUIDE.md](HUD-USER-GUIDE.md) — **operator guide** for the three
  consoles (`wayang`, `wayang-fw`, `wayang-router`): keys, tabs, REVIEW/commit,
  troubleshooting, and the Winbox/FortiGate/Cloudflare concept map.
* [TUI-UX-REVAMP.md](TUI-UX-REVAMP.md) — the design behind it (navigation
  grammar, visual spec, focus, no-flash startup, terminal robustness, one-tab=
  one-full-screen layout). Shared library: `dalang-io/wayang-tui`.
* [PRODUCT-API.md](PRODUCT-API.md) — the `api` subcommand design (drive a box
  by API instead of SSH).

## Edge / router / firewall product
* [EDGEROUTER.md](EDGEROUTER.md) — WayangOS as a wayangi “Edge” unit.
* [EDGEROUTER-RUNBOOK.md](EDGEROUTER-RUNBOOK.md) — field operations for an Edge box.
* [ROUTER-TODO.md](ROUTER-TODO.md) — router/firewall plan (points at
  `wayang-router`/`wayang-fw` roadmaps).
* [NETWORK.md](NETWORK.md) — networking model.
* Firewall WAF/IDS design: `dalang-io/wayang-fw` → `docs/WAF.md`, `docs/ROADMAP.md`.

## Router kernel
* [ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md) — the block that froze the
  device and how it was re-enabled.
* [ROUTER-KERNEL-INTERACTION.md](ROUTER-KERNEL-INTERACTION.md) — options that
  interact.
* [TODO-M7-UNBLOCK.md](TODO-M7-UNBLOCK.md) — the M7 execution plan (archived).
* [INCIDENT-1.0.13.md](INCIDENT-1.0.13.md) — what happened, for reference.

## Updates / channel
* [UPDATE.md](UPDATE.md), [UPDATE-DESIGN.md](UPDATE-DESIGN.md),
  [UPDATE-ARM.md](UPDATE-ARM.md), [UPDATE-TODO.md](UPDATE-TODO.md),
  [CHANNEL.md](CHANNEL.md) — signed A/B updates and the release channel.

## Ops / hardware
* [MONITORING.md](MONITORING.md), [HARDWARE.md](HARDWARE.md),
  [MINIMUM-SPEC.md](MINIMUM-SPEC.md), [HW-SESSION-CHECKLIST.md](HW-SESSION-CHECKLIST.md).

## Misc
* [APPS.md](APPS.md) — bundled apps; [MEMORY-TODO.md](MEMORY-TODO.md) — running
  notes; [HANDOVER.md](HANDOVER.md) — cross-repo state.
