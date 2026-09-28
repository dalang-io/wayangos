# TODO-M7-UNBLOCK — multi-agent execution plan

Goal: unblock **M7** (router kernel block on the released image) without
gambling. Written 2026-09-28 after the delta-debug found **no failing subset**
and the 1.0.20 freeze's signature turned out to be a **network stall**
(`gw=''`, clean dmesg), not a CPU lockup. Context:
[GOAL.md](GOAL.md) (M7), [ROUTER-KERNEL-INTERACTION.md](ROUTER-KERNEL-INTERACTION.md),
[INCIDENT-1.0.13.md](INCIDENT-1.0.13.md), [HARDWARE.md](HARDWARE.md).

## The new hypothesis every task builds on

> **Status (2026-09-28, end of day):** T1 landed this hypothesis's fix
> (`765ee81`): the primary boot path now runs `udhcpc -b` (retry forever) after
> a bounded 10 s wait-for-carrier (`scripts/build-rootfs.sh` `serve()` `:737`,
> `wait_carrier()` `:698`). The only remaining `udhcpc -n -q -t 5 -T 3` uses
> are the bounded auto-detect probe (`:727`) and the interactive `wayang-net
> dhcp`/`use` commands (`:1082`/`:1096`) — neither is on the boot path. T2
> shipped the safety net in the kernel (`61c768f`). The hypothesis itself is
> still **unconfirmed** — the boot-count runs + the hardware session decide
> (decision rule at the bottom).

**Primary-NIC DHCP gives up after ~15 s.** `scripts/build-rootfs.sh` runs
`udhcpc -n -q -t 5 -T 3` on the primary interface (`:712` and `:1041`) — exit on
no lease, no retry, no daemon. Secondary NICs use `-b` (retry forever, `:719`,
`:885`, `:899`). The device's public uplink is a **DHCP `/32` lease over a flaky
USB SR9700**; if the lease is slower than ~15 s at cold boot → `gw=''`, the box
is alive but unreachable — exactly the 1.0.20 failure signature. This failure
mode exists on **any kernel, including 1.0.21**. The kernel block may only have
shifted boot timing. Consequences:

1. The resilience fixes below must land **regardless of M7** — they are the
   product fix, not a debug tool.
2. The M7 decision must be made by **boot-count data** (failure rates), not by
   more combo bisects — there is no failing subset.

## Parallel-agent rules

- Straight commits to `master` (no PRs). **Never tag** — tag = release (AGENTS.md 3).
- File ownership per task (below) to avoid conflicts; both tasks touching
  `scripts/build-rootfs.sh` own **disjoint sections** — commit early, rebase often.
- `shellcheck scripts/*.sh` clean; heredoc'd device init scripts are POSIX `sh`.
- Build only on `root@10.0.0.251` via a `/tmp` BUILD_DIR (never `/root/wayangos-build`).
- **Do not touch the test device without the owner's OK** (T4/T3 execution).
- Docs are English; keep cross-links bidirectional.

## Workstreams

### T1 — Primary-NIC DHCP resilience  ·  agent-1  ·  starts immediately

**DONE — `765ee81` (2026-09-28).** Validation: QEMU boot with
`-device usb-net` + hostfwd leases via the `-b` path and SSH works;
shellcheck clean; [NETWORK.md](NETWORK.md) boot-behaviour section documents
the retry semantics. (A genuinely slow lease can only be proven on hardware —
that is T4's job, and QEMU user-net DHCP answers instantly.)

**Owns:** `scripts/build-rootfs.sh` **network/DHCP section only** (~lines
700–920 and the `/etc/init.d/network` heredoc ~1030–1050), `docs/NETWORK.md` (dhcp part).

1. Wait-for-carrier loop (bounded ~10 s) before DHCP on USB NICs (SR9700 link-up lags).
2. Primary interface: `udhcpc -b` (background retry) instead of `-n -q -t 5 -T 3`;
   boot must not block or fail on a missing lease — print a warning and continue.
3. Keep the boot-time secondary/`also` passes as-is (`-b` already).
4. Verify `rcS` → `wayang mark-ok` still happens, and that under
   `wayang.selftest` the selftest (not rcS) owns the mark (see T2 §3) — with
   `-b`, a slow lease now arrives *inside* the selftest window and the slot
   marks good; a persistent stall fails the selftest → GRUB falls back. That is
   the designed recovery.
5. shellcheck; QEMU smoke: boot with `-device usb-net` + hostfwd, confirm lease
   + SSH. (QEMU user-net DHCP answers instantly — slow-server behaviour is
   validated by T4 on hardware, note this in the doc.)

**DoD:** no `-n` on the primary path; shellcheck clean; QEMU boot leases + SSH;
`docs/NETWORK.md` documents the retry semantics.

### T2 — Safety net in the shipped kernel  ·  agent-2  ·  starts immediately

**DONE — `61c768f` (2026-09-28).** Validation: `test-selftest-qemu.sh`
scenarios nonet/hang/panic all PASS with the net coming from the shipped
`defconfig-intel` (no extra args); the rcS `mark-ok` deferral was verified
already-correct; grub template unchanged (the cmdline is kernel-baked via
`CONFIG_CMDLINE`). Router block stays off. Operational note documented in
[UPDATE.md](UPDATE.md) — with the net baked in, a permanently offline box
loops fallback/reboot instead of freezing.

**Owns:** `configs/defconfig-intel`, `wayang/grub-disk.cfg` (+ `wayang/src/staging.rs`
only if the template changes shape), `docs/UPDATE.md`. **Does not touch** the
DHCP section of `build-rootfs.sh` (T1's); may touch the rcS/selftest sections
(~330–400, ~1240+).

1. Add a watchdog driver to `defconfig-intel` (P320 Tiny = Intel PCH →
   `CONFIG_ITCO_WDT`; keep `CONFIG_I6300ESB_WDT` for the QEMU proof;
   `CONFIG_SOFTDOG` as universal fallback). Defconfig-intel currently has
   **no watchdog and no CMDLINE at all** — the bisect bundles carried the net,
   the shipped kernel never did.
2. Bake the safety cmdline into the shipped kernel via `CONFIG_CMDLINE`/
   `CONFIG_CMDLINE_BOOL` in `defconfig-intel` — copy the exact args from
   `scripts/bisect-router-opts.sh` (it already builds them): `wayang.selftest=120`,
   lockup/panic detectors, `panic=10`. Prefer kernel-baked over grub args so
   installer, both slots and updates all inherit it; keep `grub-disk.cfg`
   consistent.
3. Confirm the rcS gating: when `wayang.selftest=` is set, `wayang mark-ok` in
   rcS is deferred to the selftest (`build-rootfs.sh` ~391 comment) so GRUB's
   3-attempt budget + `wayang_good` fallback actually engage.
4. Prove it: run `scripts/test-selftest-qemu.sh` (scenarios `nonet hang panic`)
   on the build box **with the new defconfig-intel kernel and no extra args** —
   the net must come from the shipped config, not the test harness.
5. This changes the next tagged kernel (watchdog + cmdline only — **router
   block stays off**; do not conflate the two).

**DoD:** `test-selftest-qemu.sh` green for all 3 scenarios with default cmdline;
`configs/defconfig-intel` carries watchdog + safety cmdline; docs updated.

### T3 — Boot-count (soak) harness  ·  agent-3  ·  starts immediately

**DONE — `17ecaf0` (2026-09-28).** Validation: `scripts/boot-soak.sh` —
evidence-driven classification (`up+route` / `up+no-route` /
`watchdog-reset` / `unreachable>deadline`), 240 s liveness bound (from the
owner-measured 120–200 s healthy range) and a 480 s silence cap that bounds
only evidence-harvesting of self-recovered stalls, 34 assertions,
shellcheck clean, QEMU-probed. The two ready-to-fire runs (block vs safe
kernel, N≥12) are documented in
[ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md).

**Owns:** `scripts/bisect-router-opts.sh` (add a mode) or new
`scripts/boot-soak.sh`; `docs/ROUTER-KERNEL-BISECT.md` (new section).

1. `--boots N`: stage a bundle on the device and let it boot/recover
   autonomously — failures self-recover via the T2 safety net (selftest →
   `wayang update --fallback` → reboot), so **no human is needed** unless a
   stall survives a reboot (that itself is data).
2. Each boot outcome is classified from the device (`/data/selftest.log`,
   `/data/debug/*`, boot timestamp): `up+route`, `up+no-route`, `watchdog-reset`,
   `unreachable>deadline`. Local log + summary matrix per bundle tag.
3. Report tool: parse device logs → `bundle × boots × outcomes` + time-to-failure.
4. Smoke-test the harness logic in QEMU using `scripts/test-selftest-qemu.sh`
   scenarios (never run the device part without owner OK).
5. Prepare the two runs the decision needs: **block-kernel × N≥10** and
   **safe-kernel × N≥10** boots (the delta-debug's 3 boots cannot distinguish
   deterministic from ~25% stochastic failure).

**DoD:** harness documented + QEMU-smoked; the two device runs specified
(bundle tags, N, polling, evidence capture) and ready to fire on owner OK.

### T4 — Hardware session protocol (owner-assisted)  ·  agent-4  ·  doc now, run after T1+T2

**DONE — `4a00b2e` (2026-09-28).** Validation:
[HW-SESSION-CHECKLIST.md](HW-SESSION-CHECKLIST.md) is complete and
executable without agent context: Caps Lock first-response test (never
verified in either incident), evidence-capture commands, SR9700→e1000e swap
with the MAC-bound-`/32`-lease open question, cold-vs-warm boot-count
matrix, PASS criteria, and result-recording templates for
[HARDWARE.md](HARDWARE.md) / [INCIDENT-1.0.13.md](INCIDENT-1.0.13.md). The
session itself still needs the owner at the device.

**Owns:** new `docs/HW-SESSION-CHECKLIST.md`; appends results to
`HARDWARE.md` / `INCIDENT-1.0.13.md` later.

Write a checklist the owner can execute without agent context:

1. **Before power-cycling any "freeze":** Caps Lock test (LED toggles = kernel
   alive → network stall, block kernel is innocent; dead = real xHCI/CPU lockup
   → H1/H2 revive). **This claim was never verified in either incident** — it is
   the single most informative 10-second action.
2. **Evidence before cycle:** copy `/data/debug/*`, `/data/selftest.log`,
   `boots.log` off the box.
3. **SR9700 → e1000e uplink swap:** move the cable to the onboard NIC and boot
   the same kernels a few times. Open question to resolve in the doc: the
   public `/32` DHCP lease may be MAC-bound — check with the ISP/owner how the
   lease moves (MAC clone via `udhcpc -C`? config change in
   `/data/etc/network`?). Alternative control: a known-good USB NIC
   (AX88179/RTL8153). If stalls vanish off the SR9700 → dongle confirmed flaky;
   note the product MiniPC uses onboard NICs anyway.
4. **Run the T3 matrix** (block × N, safe × N) using the checklist above.
5. Cold vs warm boot distinction: record whether each boot was power-cycle or
   reboot — a stall that only appears cold needs power-cycle access.

**DoD:** checklist complete and executable; results slots prepared in
`HARDWARE.md`.

### T5 — Docs consolidation  ·  agent-5  ·  after T1–T4 land
**Owns:** `docs/TODO.md`, `docs/GOAL.md`, `docs/HANDOVER.md`,
`docs/INCIDENT-1.0.13.md`, `docs/ROUTER-KERNEL-INTERACTION.md`, this file.

**DONE — 2026-09-28 (this pass; uncommitted at write time).** M7 sections
rewritten from "decide (a)/(b)/(c)" to "run the plan, decide by boot-count"
([TODO.md](TODO.md), [GOAL.md](GOAL.md), [HANDOVER.md](HANDOVER.md));
T1–T4 marked done above with their hashes;
[HARDWARE.md](HARDWARE.md) linked from [TODO.md](TODO.md);
[ROUTER-KERNEL-INTERACTION.md](ROUTER-KERNEL-INTERACTION.md) hypotheses
downgraded to background research (status banner on the Bisect-result
section); [INCIDENT-1.0.13.md](INCIDENT-1.0.13.md) gained the
"the freeze was (probably) never a freeze" section; [HANDOVER.md](HANDOVER.md)
cross-repo table updated to `61c768f` (unpushed, nothing tagged). The final
M7 decision is **pending** — it gets recorded here once the boot-count runs
produce data.

1. Rewrite the M7 sections from "decide (a)/(b)/(c)" to "run the plan, decide by
   boot-count" with the udhcpc hypothesis + `build-rootfs.sh` line refs.
2. Keep `HARDWARE.md` linked from `TODO.md` (was missing; done in this pass).
3. Mark T1–T4 done as they land; record the final M7 decision + the data it was
   based on. Update `HANDOVER.md` cross-repo table.

**DoD:** no stale "interaction" claims left as fact; every decision links its data.

## Dependency graph

```
T1 (DHCP resilience) ──┐
T2 (shipped safety net) ─┼──► test release with block + net ──► M7 DECISION
T3 (soak harness) ───────┤        │
T4 (HW session doc) ─────┘        └─► owner runs T4/T3 on device ──► T5 (docs)
   T1,T2,T3,T4-doc: all startable in parallel now
   T4/T3 device execution: needs T1+T2 merged (meaningful + safe)
   T5: needs T1–T4 landed
```

## Decision rule (replaces "(a)/(b)/(c)")

After T1+T2 ship and the T3 matrix runs:

| Result | Meaning | Action |
|---|---|---|
| Block and safe kernels stall at ~equal rates | Environmental (USB/DHCP), not kernel | Enable the router block in `defconfig-intel`, ship with the safety net — **M7 unblocked** (still: T4 sr9700 swap for the lab) |
| Only the block kernel stalls | Real kernel interaction, but failure mode is a *network stall* — feed this into H1/H2 | Keep block off; resume `ROUTER-KERNEL-INTERACTION.md` plan with the new signature |
| Both clean for N≥10 | Earlier freezes were first-boot-after-flash events | Ship the block with the net + require the T4 cold-boot soak before each tag |

Either way T1+T2 go into the next release: they fix a real product risk
(a headless box stranded by a 15-second DHCP race) independent of the kernel
question.