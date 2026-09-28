# Hardware session checklist — diagnosing the "freezes" at the device

A self-contained protocol for a supervised session on the test device
`root@163.128.55.3` (ThinkStation P320 Tiny, WayangOS ≥ 1.0.21 in slot A).
No agent context needed. Background: [INCIDENT-1.0.13.md](INCIDENT-1.0.13.md),
[HARDWARE.md](HARDWARE.md), [ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md),
plan: [TODO-M7-UNBLOCK.md](TODO-M7-UNBLOCK.md) (tasks T1–T4).

**Owner-measured boot baseline on this device (2026-09-28):** the OS is up a
few seconds after power-on, and **SSH becomes active after ~120–200 s**.
Use this to judge "slow" vs "broken": a boot that has **no SSH after ~240 s**
(a little over 200 s headroom) is **broken — do not keep waiting**, go to §1/§2.
The T3 soak harness uses the same ~240 s per-boot bound
([ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md)).

What we are testing: the two 1.0.13 / 1.0.20 "freezes" look, on re-inspection,
like **network stalls** (alive box, `gw=''`), not CPU lockups — most likely the
primary-NIC DHCP race (`udhcpc -n -q -t 5 -T 3` gives up after ~15 s; the SR9700
USB NIC link-up is slow). The kernel-block question is decided by **boot counts**
(e.g. 0/10 vs 4/10 failures), not by "it froze once".

Device facts you will need:

| | |
|---|---|
| SSH | `ssh root@163.128.55.3` (pubkey only) |
| Uplink | `eth1` = CoreChip SR9700 **USB** Ethernet (usb 1-5), DHCP public `163.128.55.3/32`, gw `163.128.55.254` |
| Onboard | `eth0` = e1000e I219-LM (cable currently present, unused) |
| Keyboard | USB, **usb 1-8** — same xHCI controller as the SR9700 |
| Config | `/data/etc/network/primary` = `eth1`, `/data/etc/network/config` = `MODE=dhcp` |
| Copying files | **no sftp** — use `ssh host 'cat > FILE' < FILE` and `ssh host 'cat FILE'` |

---

## 1. First response to any "freeze" (10 seconds)

**Do the Caps Lock test before anything else.** Press Caps Lock on the USB
keyboard and watch the LED:

| Caps Lock LED | Meaning | What it tells us |
|---|---|---|
| **Toggles** | Kernel is alive and handling USB input interrupts | **Not a lockup.** It is a network stall (DHCP/USB-uplink). The router kernel block is innocent. |
| **No reaction** | Real xHCI or CPU lockup | Revives hypotheses H1/H2 (xHCI/SIMD-crypto). Note the exact kernel/slot and time. |

> This test was **never performed** in either incident (1.0.13, 1.0.20). It is
> the single most informative 10-second action in this whole plan.

Then try Ctrl-Alt-Del (inittab → reboot) as a second, independent liveness probe
— but **only after the LED observation is recorded**, and only after the
evidence capture below if SSH still works.

## 2. Capture evidence BEFORE power-cycling

Do this while the box is in the failed state — an SSH connection may still be
possible even when the uplink "looks" down, and anything you skip here is lost
if you power-cycle into the other slot (which changes `/proc/*`).

**Give-up rule:** retry SSH until **~240 s after the boot started** (healthy
boots answer within ~120–200 s here). Past that, the boot is broken — stop
retrying, capture/record what you have, and power-cycle.

```sh
HOST=root@163.128.55.3

# Kernel log, IRQs, load (written every second only when wayang.debug was on
# the cmdline — check first):
ssh $HOST 'ls -la /data/debug/ 2>/dev/null'
ssh $HOST 'cat /data/debug/dmesg.boot'        > dmesg.$(date +%s).txt
ssh $HOST 'cat /data/debug/interrupts.boot'   > irq.$(date +%s).txt
ssh $HOST 'cat /data/debug/loadavg.boot'      > loadavg.$(date +%s).txt

# Self-test log (slot, uptime, gw verdict per boot):
ssh $HOST 'cat /data/selftest.log'            > selftest.$(date +%s).log

# Boot log (harness; may not exist on a stock image — harmless if empty):
ssh $HOST 'cat /data/boots.log'               > boots.$(date +%s).log

# Live state snapshot (if the shell answers):
ssh $HOST 'ip addr; ip route; dmesg | tail -n 60; wayang status; cat /proc/cmdline' \
    > state.$(date +%s).txt
```

**If SSH is dead:** record that fact (it is data), note the wall-clock time, and
power-cycle. `/data/debug/*` and `/data/selftest.log` **persist on the ext4
`/data` disk** — after the box comes back on the good slot, pull them with the
same `ssh $HOST 'cat /data/...'` commands above. That is exactly why
`wayang.debug` writes to `/data` every second.

Only then power-cycle (or Ctrl-Alt-Del if the LED reacted to Caps Lock).
After recovery on the good slot, confirm and re-mark it:

```sh
ssh $HOST 'grep -o "wayang.slot=[AB]" /proc/cmdline; wayang status'
ssh $HOST 'wayang update --rollback && wayang mark-ok'   # only if you booted the other slot by hand
```

## 3. SR9700 → e1000e uplink swap test

Goal: determine whether the stalls are caused by the **SR9700 USB dongle**
(hardware under test) or by the **kernels**. The product MiniPCs use the
onboard NIC, so "stalls vanish off the SR9700" ⇒ dongle flaky, case closed.

### ✅ RESOLVED (owner, 2026-09-28): the /32 comes from the local router, lease TTL 1 min

The public `163.128.55.3/32` is **not MAC-bound**: it is leased by the local
router's DHCP with a **1-minute lease**. Swap the cable to `eth0`, wait ~1
minute for the lease to expire, and the same IP is offered to the new MAC —
no MAC cloning, no provider call. With T1's `udhcpc -b` (retries forever) the
box simply keeps asking until the lease frees up. ISPs differ (some hand out
one IP, some many) — **do not try to accommodate ISP behaviour**; the only
requirement is that the box gets an IP.

If `eth0` shows no lease right after the swap, that is expected for up to a
minute — re-check, don't debug. Only if it stays empty for several minutes,
look at cabling/port.

Alternative control (no lease question at all): swap in a **known-good USB NIC
(AX88179 or RTL8153)** in place of the SR9700, keeping the same cabling. If
stalls vanish there too, the SR9700 is the culprit either way.

### Steps

```sh
# 1. Record the current MACs and config
ssh $HOST 'cat /sys/class/net/eth1/address /sys/class/net/eth0/address; cat /data/etc/network/primary /data/etc/network/config'

# 2. Physically move the public uplink cable from the SR9700 dongle to the
#    onboard Ethernet jack. (At the console, or: unplug the dongle entirely.)

# 3. Point the primary at eth0
ssh $HOST 'echo eth0 > /data/etc/network/primary'
ssh $HOST 'printf "MODE=dhcp\n" > /data/etc/network/config'

# 4. Boot the same kernels several times (see §4), alternating cold (power
#    cycle) and warm (reboot) boots. After each boot check:
ssh $HOST 'ip -4 addr show eth0; ip route; wayang status'
```

To restore afterwards: move the cable back, then
`ssh $HOST 'echo eth1 > /data/etc/network/primary'`.

**Interpretation:**

| Outcome | Conclusion |
|---|---|
| Stalls reproduce on `eth0` (e1000e) too | Not the dongle — DHCP server/timing or kernel after all; the boot-count matrix (§4) decides |
| Stalls only happen on the SR9700 | Dongle (or USB path) flaky — product MiniPCs use onboard NICs; the router block can ship with the safety net |
| No lease at all on `eth0` | Expected for up to ~1 min (lease TTL 1 min — owner-confirmed); `udhcpc -b` keeps retrying. Still empty after several minutes → cabling/port |

## 4. Running the boot-count matrix

The soak harness (`scripts/boot-soak.sh`, standalone on purpose — the bisect
harness never touches the device; see
[ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md)) stages a bundle and boots
autonomously; with the safety net each failure self-recovers (self-test fail →
`wayang update --fallback` → reboot; hard hangs → watchdog reset ×3 → GRUB falls
back to the good slot), so no one needs to babysit every boot.

Two runs, each **N ≥ 10 boots** (3 boots cannot distinguish deterministic from
~25 % stochastic failure):

| Run | Kernel | Boots |
|---|---|---|
| A | router-block kernel (bundle from `scripts/bisect-router-opts.sh`, full set) | ≥ 10 |
| B | safe kernel (current 1.0.21 baseline) | ≥ 10 |

**Record cold vs warm for every boot.** A stall that only appears on cold boots
(SR9700 link-up after power-on is slow) needs power-cycle access to reproduce;
a harness reboot is *warm* and will not show it. Suggested discipline:
5 cold (power-cycle) + 5 warm (reboot) per run.

Per boot, record:

| field | how |
|---|---|
| cold or warm | your action (power-cycle vs `ssh $HOST reboot` / console reboot) |
| outcome | `up+route` / `up+no-route` / `watchdog-reset` / `unreachable>deadline` |
| time-to-network | how long until SSH answered, from boot start (rough stopwatch is fine; >240 s ⇒ failed boot) |
| evidence | `/data/selftest.log`, `/data/debug/*`, `/data/boots.log` pulled per §2 when anything fails |

Classification, from the device after each boot:

```sh
ssh $HOST 'tail -n 4 /data/selftest.log; tail -n 2 /data/boots.log; wayang status'
```

Rules of engagement:

- Stage bundles with `wayang update --from /data/x.wup` (verify sha256 after
  the `cat >` copy — large uploads can drop); never reboot remotely into an
  untested kernel without the safety net active (`uname -a` +
  `cat /proc/cmdline` must show `wayang.selftest=`).
- Do not run the device part of the harness without the owner's OK
  (TODO-M7-UNBLOCK rule).

## 5. What PASS means, with the safety net active

The net (landing with T2, in `configs/defconfig-intel`): `wayang.selftest=120`
window, `wayang mark-ok` only after the probe still answers at 120 s, fail →
`wayang update --fallback` → reboot to the good slot; `panic=10` + soft/hard
lockup + hung-task detectors reboot a dead kernel; watchdog petted during and
after the window; GRUB falls back to `wayang_good` after 3 boot attempts
without a mark.

Healthy boot on this device (owner-measured 2026-09-28): **SSH active within
~120–200 s, a route inside the 120 s self-test window.** **A boot with no SSH
by ~240 s counts as failed — do not sit and wait beyond that**; classify it
(`up+no-route` / `unreachable>deadline`) and move on. The T3 harness applies
the same ~240 s per-boot deadline.

| Scenario | PASS looks like | Evidence to keep |
|---|---|---|
| Boot to network | IP on the uplink, SSH answers within ~200 s, `wayang status` shows the test slot active+good | `selftest.log` line `PASS`, boots.log entry, time-to-network |
| Slow DHCP lease (the suspected failure) | Lease arrives inside the 120 s window → slot marks good; box up, just slow | `selftest.log` showing late `gw=` value + PASS; time-to-network figure |
| DHCP never arrives | Self-test FAIL at 120 s → auto-fallback to good slot, box back in ≤ ~3 min, **without human help** | `selftest.log` FAIL + fallback lines; `boots.log` slot history |
| Hard lockup (Caps Lock dead) | Watchdog/detectors reset the box ×3 → GRUB budget → good slot, back in ≤ ~5 min | `/data/debug/*` snapshots, selftest.log gap, boots.log showing 3 attempts |
| Freeze with no auto-recovery | Box has no SSH by ~240 s **and** none of the above fires within 10 min | Everything from §2 **and** a note that the net failed — this is a T2 bug, report it, do not debug inline |

PASS for the whole matrix = both kernels' failure rates indistinguishable
(e.g. both 0/10, or both ~2/10 with the same cold/warm pattern) → per the
[decision rule](TODO-M7-UNBLOCK.md#decision-rule-replaces-abc) the "freeze" is
environmental and the router block ships with the net. Only-block-stalls →
keep the block off, resume
[ROUTER-KERNEL-INTERACTION.md](ROUTER-KERNEL-INTERACTION.md) with the new
signature. Both clean → ship the block and require a cold-boot soak before
every future tag.

## 6. Result-recording templates

Append to [HARDWARE.md](HARDWARE.md) after the session:

```markdown
### HW session YYYY-MM-DD — SR9700 vs e1000e uplink (T4)

| Kernel | Boots | Cold fails | Warm fails | Signature | Net recovered? |
|---|---|---|---|---|---|
| block (bisect full set) | 10 |  |  |  |  |
| safe (1.0.21)           | 10 |  |  |  |  |

| Uplink | Boots | No-lease | Late lease (>15 s) | Same /32 kept |
|---|---|---|---|---|
| SR9700 (eth1) |  |  |  |  |
| e1000e (eth0) |  |  |  |  |

Caps Lock tests: N performed — alive N, dead N. Lease MAC-bound? yes/no/unknown.
Conclusion + data:
```

Append to [INCIDENT-1.0.13.md](INCIDENT-1.0.13.md):

```markdown
## Follow-up YYYY-MM-DD — HW session (T4 checklist)

- Freeze reproduced? yes/no — which kernel(s), how many boots.
- Caps Lock at freeze: [alive → network stall | dead → lockup] (first verified observation).
- Off the SR9700: [reproduces | vanishes | no lease (MAC-bound: …)].
- Safety-net behaviour at failure: [selftest FAIL → fallback | watchdog reset ×3 | did NOT recover: …].
- Evidence files: <paths/attachments>.
- Conclusion:
```
