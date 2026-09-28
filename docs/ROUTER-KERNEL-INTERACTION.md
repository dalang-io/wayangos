# Router-kernel interaction lockup — culprit research

Research-only deliverable (2026-09-28). **No config was changed and nothing was
committed.** This file ranks the most likely causes of the 1.0.13 / 1.0.20
lockup (all router groups pass individually, the accumulated set freezes) and
proposes the concrete hardware tests to run next. Context and ground truth:
[docs/INCIDENT-1.0.13.md](INCIDENT-1.0.13.md) and
[docs/ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md). Harness:
`scripts/bisect-router-opts.sh`.

## Symptom, and what it implies

Kernel **7.2.7** on an Intel **i3-7100 (Kaby Lake, 2C/4T)** + 200-series PCH,
UEFI. After the shell prompt the box **hard-freezes**: the framebuffer keeps its
last frame, the **USB keyboard** (usb 1-8) and the **USB SR9700 uplink**
(usb 1-5) both die, no SSH, no self-reboot, and **no kernel output at all**
(no soft/hard lockup, no hung task, no oops/panic) — same shape as CachyOS
issue #1056 on a Kaby Lake-R machine.

Two candidate mechanisms fit "keyboard + USB NIC dead together, nothing
logged":

1. **A hard CPU lockup** (both CPUs wedged, e.g. an atomic-context deadlock or
   an infinite loop with IRQs/BH off). This silently kills every USB interrupt
   on the shared xHCI controller.
2. **A soft lockup / workqueue storm** that saturates both cores (the i3-7100
   has 2 cores) so USB interrupt handling is starved. This can also fail to
   produce a backtrace if it happens with BH-disabled spinlocks.

Because the groups pass alone but fail together, the trigger is almost
certainly code that only runs when two subsystems are *both* present, or a
timing/memory-layout change that exposes a latent mainline bug. The freeze
window is around `/etc/init.d/network` bring-up, so the **network path is the
prime suspect area**, with crypto a close second.

## Ranked hypotheses

Confidence is intentionally calibrated; the evidence base is thin for this exact
config combination, so treat this as an ordered test plan, not a verdict.

### H1 (most likely) — Boot-time netdev + IPv6 addrconf / l3mdev (VRF)

**Claim.** The accumulated set creates `dummy0`, `bond0`, `ifb0/1`, `gre0`,
`gretap0`, `erspan0`, `tunl0` at boot, and `NET_VRF`/`NET_L3_MASTER_DEV` plus
`IPV6_MULTIPLE_TABLES`/`IPV6_SUBTREES` change how IPv6 autoconfiguration and
FIB lookups walk those devices. The interaction is between the **boot-netdev
groups** (e, possibly a/g/f) and the **VRF/policy-routing group** (c) — each side
alone has nothing to exercise the other.

**Evidence.**
- `NET_VRF` *depends on* `IPV6_MULTIPLE_TABLES` (and `NET_L3_MASTER_DEV`), so
  enabling VRF silently drags IPv6 multi-table lookup into the picture — the
  exact dependency the bisect notes flagged:
  <https://github.com/Xilinx/linux-xlnx/blob/master/drivers/net/Kconfig>.
- The l3mdev design deliberately changes IPv6 link-local/multicast/addrconf
  behaviour for devices in an L3 domain; `vrf_link_scope_lookup` and the
  "link-local addresses are not added to l3mdev devices" behaviour are core to
  it: <https://netdevconf.info/1.2/papers/ahern-what-is-l3mdev-paper.pdf>,
  <https://docs.kernel.org/networking/vrf.html>.
- **Boot stuck at IPv6 `ADDRCONF(NETDEV_CHANGE): … link becomes ready`** is a
  recurring real-world hang on bridges/multi-netdev systems:
  <https://forum.banana-pi.org/t/bpi-r3-boot-stuck-on-ipv6-addrconf-netdev-change-br-wan-link-becomes-ready/19224>.
- A representative syzbot workqueue-lockup dump for the WireGuard subsystem is
  dominated by exactly these work items — `addrconf_dad_work`,
  `mld_ifc_work`, `mld_dad_work`, `macvlan_process_broadcast` — i.e. IPv6
  autoconfig on virtual devices is a known pressure point:
  <https://syzkaller.appspot.com/bug?extid=d0d2f1a65f45b319d25d>.
- The network init script adds an on-link `/32` gateway route, which exercises
  FIB insertion while all these devices exist; with VRF/l3mdev +
  `IPV6_SUBTREES` that is new code (with the VRF group) touching new devices
  (with the netdev groups).

**Weakness.** No single upstream report pins exactly VRF + dummy/bond IPv6
addrconf to a hard freeze; this is inference from code reachability.

**Test.** Pairwise: `vrf-multipath` × `dummy-bonding` first, then
`vrf-multipath` × `veth-macvlan-tun`, then `vrf-multipath` × `gre-ipip`.
See "Test plan".

---

### H2 — x86 kernel-mode FPU in softirq / `*_ARCH` SIMD crypto

**Claim.** The x86 crypto `*_ARCH` libraries select SIMD asm that now runs
kernel-mode FPU (SSE/AVX) even in softirq. On 7.x this made FPU-in-softirq
reliable by replacing `preempt_disable()` with `local_bh_disable()` in
`kernel_fpu_begin()` (commit `d02198550423`). That same change is documented to
turn an x86/EFI fault during a runtime call into a **hard system freeze**
(`panic("Fatal exception in interrupt")`). Enabling WireGuard/ESP/tunnels is
what pulls the SIMD crypto into the receive/softirq and workqueue paths.

**Evidence.**
- CVE-2026-46290, verbatim root cause, fix and "unrecoverable hang" wording:
  <https://nvd.nist.gov/vuln/detail/CVE-2026-46290>,
  <https://github.com/advisories/GHSA-qr7g-p698-q8wm>,
  <https://launchpad.net/bugs/cve/CVE-2026-46290>.
- The originating patchset (make kernel-mode FPU reliable in softirqs, then
  drop the x86 "crypto SIMD helper" fallback):
  <https://lkml.org/lkml/2025/2/19/2093>,
  <https://lkml.org/lkml/2025/3/4/2101>,
  <https://lkml.iu.edu/hypermail/linux/kernel/2504.0/03145.html>,
  <https://www.phoronix.com/news/Linux-x86-Crypt-Drop-Fallback>.
- Design discussion of `local_bh_enable()` running softirqs and why FPU/BH is
  delicate: <https://lkml.iu.edu/2505.2/02516.html>.
- By 7.1 the crypto *library* and its arch optimisations are the norm and use
  the generic C fallback only when the `_ARCH` option is off:
  <https://lwn.net/Articles/1077427>.
- WireGuard's decrypt/encrypt workers are a recurring source of prio-high
  lockups/soft lockups/stalls, including inside
  `chacha20poly1305_crypt_sg_inplace`:
  <https://syzkaller.appspot.com/upstream/s/wireguard>,
  <https://syzkaller.appspot.com/bug?extid=27a6e390f71f14f4fecf>,
  <https://syzkaller.appspot.com/bug?extid=d0d2f1a65f45b319d25d>.
- WireGuard also holds RTNL around `napi_disable()` and has hung real
  production nodes: <https://blog.cloudflare.com/searching-for-the-cause-of-hung-tasks-in-the-linux-kernel/>.

**Weakness.** The syzbot crypto lockups are **arm64** and only with WireGuard
traffic; the EFI freeze is about EFI runtime faults, not crypto throughput. On
this box WireGuard is built-in but *not configured*, so the SIMD path may not
run at boot — unless a crypto self-test does. **Check the resolved `.config`
for `CONFIG_CRYPTO_SELFTEST` and `CONFIG_CRYPTO_MANAGER_EXTRA_TESTS` before
trusting this one.**

**Test.** Build the accumulated set with every `*_ARCH` crypto option forced
`# … is not set` (generic C). One boot. This is the recommended first toggle
(see below).

---

### H3 — WireGuard + UDP tunnel / GRO interaction

**Claim.** `WIREGUARD` selects `NET_UDP_TUNNEL`; `NET_IPGRE`/`NET_UDP_TUNNEL`
plus WireGuard's GRO (`napi_gro_receive`) and the `act_mirred`/`ifb` path are all
new together. A receive-side interaction between WireGuard and (VXLAN/GRE/FOU)
GRO has bitten real deployments.

**Evidence.**
- WireGuard selects `NET_UDP_TUNNEL`, `DST_CACHE`, `CRYPTO_LIB_*`:
  <https://github.com/Xilinx/linux-xlnx/blob/master/drivers/net/Kconfig>.
- WireGuard + VXLAN + GRO, decrypted overlay dropped on the GRO path
  (`napi_gro_receive` → `dev_gro_receive` → `inet_gro_receive`):
  <https://github.com/cilium/cilium/issues/48990>.
- WireGuard receive had to be fixed for `napi_gro_receive` semantics
  (GRO_DROP handling): <https://git.zx2c4.com/wireguard-linux/>.
- `udp_tunnel`/`ip6_udp_tunnel` are used by WireGuard, VXLAN, GENEVE, etc.,
  and the `NET_UDP_TUNNEL` option is what links them:
  <https://linuxera.org/extending-vxlan-across-nodes-with-wireguard/>.

**Weakness.** Same as H2: mostly needs live traffic; at boot there is none.

**Test.** Pairwise `wireguard` × `gre-ipip`, and `wireguard` × `qos`.

---

### H4 — bonding + GRE/tunnel type confusion (probably already fixed)

**Claim.** Enslaving a non-Ethernet device (GRE) to a bond makes
`bond_setup_by_slave()` copy the slave's `header_ops`, so `dev_hard_header()`
calls `ipgre_header()` with `netdev_priv()` pointing at `struct bonding` →
type confusion → `BUG()`/panic. This is the textbook (e) × (f) interaction:
"dummy + GRE + bond".

**Evidence.**
- CVE-2026-43456 (mainline fix in **7.0**; stable 6.12.78 / 6.18.19 / 6.19.9):
  <https://www.sentinelone.com/vulnerability-database/cve-2026-43456>,
  <https://access.redhat.com/security/cve/cve-2026-43456>,
  <https://www.penligent.ai/hackinglabs/cve-2026-43456>.
- Related bonding/XDP `WARN_ON` class: CVE-2025-22105,
  <https://lore.kernel.org/linux-cve-announce/2025041622-CVE-2025-22105-afef@gregkh/T>.

**Weakness.** 7.2.7 is newer than the 7.0 fix, so it should already contain
CVE-2026-43456. Include it only to confirm the fix is present and to run the
exact reproducer; do **not** expect this to be the cause.

**Test.** Verify the pair `dummy-bonding` × `gre-ipip` with the stock
reproducer (`ip link add … type bond/gre` + enslave + send) — if it does **not**
BUG, this H is ruled out.

---

### H5 — Mainline 7.2.x Kaby Lake hard hang (kernel-version, not options)

**Claim.** 7.2.x has a **silent total hard hang on Kaby Lake**: last frame kept,
keyboard fully dead (Caps Lock LED does not respond), no lockup detector output,
no pstore, hard power-off required. The router options may only change timing /
memory layout enough to expose it.

**Evidence.**
- CachyOS issue #1056 (i5-8350U, Kaby Lake-R, 7.2.4–7.2.6; LTS 6.18 clean):
  <https://github.com/CachyOS/linux-cachyos/issues/1056>.
- Related "silent hard lockup on 7.2.x", also 7.x-bound:
  <https://github.com/CachyOS/linux-cachyos/issues/1045>.
- 7.2 release notes / regressions (DRM scheduler FAIR policy reverted;
  scheduler fixes for hangs):
  <https://www.phoronix.com/news/Linux-7.0-Sched-Fixes-Perf-Hang>.

**Weakness.** 1.0.12 and 1.0.14 run the **same 7.2.7** kernel on this box and
are stable; and 1.0.13/1.0.20 freeze deterministically, which argues for a
config-dependent path rather than this random race. Still worth ruling out with
a kernel-version A/B.

**Test.** Boot the **same accumulated config** on a 6.18/7.1 kernel (or stock
distro kernel with the block) and soak. If it also freezes, this becomes H1.

---

### H6 — IFB / `sch_*` / `act_mirred` boot-time interaction (weak)

No boot-time evidence. `IFB` creates `ifb0/1` at boot and `NET_SCH_INGRESS` +
`NET_ACT_MIRRED` + `NET_REDIRECT` are known recursion hazards, but only once
`tc` installs rules — which nothing does at boot here. Keep it in the pairwise
matrix because it is cheap to test, but do not start here.

---

## Answers to the four questions

**1. x86 arch crypto asm (SSE/AVX/AVX2/AVX-512) lockups on Skylake/Kaby Lake.**
There is **no Skylake/Kaby-Lake-specific errata or CPU-feature-detection bug
specific to CHACHA20/POLY1305/CURVE25519 asm** that I could find. What *is* real
and recent is the **FPU-in-softirq rework** (`d02198550423`), whose known failure
mode is an x86 **hard freeze**, and the removal of the x86 "crypto SIMD helper"
fallback, which makes that FPU path universal. See H2 and its citations. The
WireGuard crypto workers (`wg_packet_{en,de}crypt_worker`, incl.
`chacha20poly1305_crypt_sg_inplace`) do show up in prio-high syzbot
lockup/soft-lockup/stall reports, but so far on **arm64**.

**2. Known interactions among the groups.**
- **WireGuard + GRO/UDP tunnels:** yes — `WIREGUARD` selects `NET_UDP_TUNNEL`;
  WireGuard/VXLAN/GRO receive-path reports exist (H3).
- **NET_IPGRE / NET_UDP_TUNNEL + something:** the concrete documented one is
  **bond enslaving GRE** = CVE-2026-43456 (H4, fixed in 7.0). GRE + UDP-tunnel
  GRO sharing with WireGuard is plausible (H3).
- **VRF + IPV6_MULTIPLE_TABLES:** not an optional combination — VRF literally
  *depends on* `IPV6_MULTIPLE_TABLES` + `NET_L3_MASTER_DEV`; VRF also has its own
  IPv6 link-local/multicast FIB path (H1).
- **IFB/sch_* built-in at boot:** creates `ifb0/1`; no documented boot lockup,
  but `act_mirred` is a known recursion source once rules exist (H6).
- **DUMMY/BONDING at boot:** `dummy0`/`bond0` via initcalls; the documented
  danger is the GRE-bond type confusion (H4).
- **XFRM + ESP:** ESP runs in the NET_RX softirq, i.e. exactly the context the
  FPU change targets; no separate xfrm-specific freeze found. (H2)

**3. Many boot netdevs + WireGuard/VXLAN on this PCH/CPU.** No PCH- or
sr9700-specific report. The mechanisms that could tie it to this box are:
(a) only 2 cores, so a soft-lockup/workqueue storm starves the shared xHCI
(keyboard + uplink both on `0000:00:14.0`), and (b) IPv6 addrconf/MLD pressure
from the boot netdevs (H1). Both are consistent with the observed simultaneous
USB deaths.

**4. Is disabling the `*_ARCH` crypto options a known workaround?** There is no
upstream report that names it as a *published* workaround, but it is the
supported way to force the generic C implementation (the arch optimisation is
only used when the `_ARCH` symbol is on; see the LWN/patchset citations). The
exact symbols to drop:

```
# force generic C (checksum/addr/cipher asm off)
CONFIG_CRYPTO_LIB_CHACHA_ARCH
CONFIG_CRYPTO_LIB_POLY1305_ARCH
CONFIG_CRYPTO_LIB_CURVE25519_ARCH
CONFIG_CRYPTO_LIB_BLAKE2S_ARCH
CONFIG_CRYPTO_LIB_GF128HASH_ARCH
CONFIG_CRYPTO_LIB_AES_ARCH
CONFIG_CRYPTO_LIB_SHA1_ARCH
CONFIG_CRYPTO_LIB_SHA256_ARCH
CONFIG_CRYPTO_LIB_SHA512_ARCH
# arch glue algorithm modules (if present in the resolved config)
CONFIG_CRYPTO_CHACHA20_X86_64
CONFIG_CRYPTO_POLY1305_X86_64
CONFIG_CRYPTO_CURVE25519_X86       # or _X86_64 — confirm in .config
CONFIG_CRYPTO_BLAKE2S_X86
# CONFIG_CRYPTO_AES_NI_INTEL is already off in defconfig-qemu
```

In the fragment use `# CONFIG_…_ARCH is not set` lines (plain `=n` is not
enough if something `select`s it), then **read the resolved `.config`** and fail
the build if any `_ARCH` survived — `olddefconfig` can silently re-select them
(ROUTER-KERNEL-BISECT.md §"Interpreting and folding back", item 2).

---

## Test plan (to replace guesswork)

### Step 0 — make the failure observable (do this first, cheap)

Right now the box logs nothing. Without a trace, every hypothesis is a guess.

- Add a built-in cmdline / GRUB edit:
  `nmi_watchdog=1 hardlockup_panic=1 softlockup_panic=1 hung_task_panic=1
  panic=10` plus `kernel.hardlockup_all_cpu_backtrace=1` and
  `kernel.softlockup_all_cpu_backtrace=1` via sysctl in rcS (the safety net in
  the bisect harness already does most of this — see ROUTER-KERNEL-BISECT.md).
- Capture to a console that survives a halted kernel: a **serial port**
  (`console=ttyS0,115200`) or **netconsole** to the build box, plus
  **pstore/ramoops** on the `/data` disk. Keep a `while :; do dmesg …; done`
  logger (the `wayang.debug` path) writing `dmesg`/`/proc/interrupts`/`loadavg`
  to `/data/debug/`.
- Record `cat /proc/interrupts` and `loadavg` at 1 s so a soft-lockup/IRQ storm
  is distinguishable from a hard lockup.

### Step 1 — recommended first option test

**Build the accumulated block with all `*_ARCH` crypto set to `n` (generic C),
keep everything else and the watchdog, boot supervised.** One boot, directly
tests H2 and is the cheapest stand-alone toggle. Outcomes:

- **Survives** → H2 confirmed (an interaction between the SIMD crypto path and
  the accumulated network/crypto config). Then bisect the crypto side: re-add
  only `CRYPTO_LIB_CHACHA_ARCH`+`POLY1305_ARCH`, then `CURVE25519_ARCH`, etc.
- **Still freezes** → drop H2 and go to Step 2 (net path).

### Step 2 — cumulative re-add (find the minimal failing subset)

The harness already builds one group per boot. Run this order, one supervised
boot each, so the freeze is attributable:

```
baseline → +veth-macvlan-tun → +wireguard → +vrf-multipath → +ipsec
         → +dummy-bonding → +gre-ipip → +qos
```

The first boot that freezes names the group that, combined with the
already-passing prefix, triggers it. Then halve that group's options and repeat.

### Step 3 — pairwise matrix (if cumulative is ambiguous)

Cross the most-suspect groups against the boot-netdev groups, cheapest first:

| pair | tests |
|---|---|
| `vrf-multipath` × `dummy-bonding` | H1, H4 |
| `vrf-multipath` × `veth-macvlan-tun` | H1 (macvlan l3s) |
| `wireguard` × `gre-ipip` | H3 |
| `wireguard` × `qos` | H3 (GRO + tc/ifb) |
| `dummy-bonding` × `gre-ipip` | H4 (stock CVE-2026-43456 reproducer) |

### Step 4 — kernel-version A/B

Same accumulated config on a 6.18 or 7.1 kernel. Distinguishes H5 from H1–H3.

### Extra probes that fit the existing tooling

- **Rule USB out of the equation:** unplug the SR9700 and use `eth0` (e1000e)
  for the uplink while booting the accumulated set. If the keyboard survives,
  the trigger is in the USB-NIC + new-net path, not a generic CPU lockup
  (INCIDENT-1.0.13.md, "Remove USB from the equation").
- **QEMU lab kernel:** `scripts/build-lab-kernel.sh` builds the full block for
  QEMU; use it to pre-screen the pairwise and cumulative sets before each
  hardware boot. It has not reproduced the freeze so far — treat QEMU passes as
  necessary but not sufficient.

## Bottom line

Most likely an **interaction on the network path at boot** (H1: boot-created
netdevs × IPv6/VRF l3mdev), with the **x86 SIMD-crypto / FPU-in-softirq** change
as the strongest single mechanism that can produce a *silent hard freeze*
(H2). First move: instrument the freeze, then boot the accumulated set with
`*_ARCH` crypto forced off; if that survives, H2 is it, otherwise run the
cumulative re-add to isolate the netdev interaction.

## Sources

- Kernel docs / Kconfig: VRF <https://docs.kernel.org/networking/vrf.html>;
  l3mdev paper <https://netdevconf.info/1.2/papers/ahern-what-is-l3mdev-paper.pdf>;
  netdev lifetime <https://docs.kernel.org/networking/netdevices.html>;
  VRF/NET_VRF deps and WIREGUARD selects
  <https://github.com/Xilinx/linux-xlnx/blob/master/drivers/net/Kconfig>.
- FPU-softirq / crypto: CVE-2026-46290
  <https://nvd.nist.gov/vuln/detail/CVE-2026-46290> ·
  <https://github.com/advisories/GHSA-qr7g-p698-q8wm> ·
  <https://launchpad.net/bugs/cve/CVE-2026-46290>; patchset
  <https://lkml.org/lkml/2025/2/19/2093> · <https://lkml.org/lkml/2025/3/4/2101> ·
  <https://lkml.iu.edu/hypermail/linux/kernel/2504.0/03145.html>;
  <https://www.phoronix.com/news/Linux-x86-Crypt-Drop-Fallback>;
  <https://lkml.iu.edu/2505.2/02516.html>;
  <https://lwn.net/Articles/1077427>.
- WireGuard: syzbot dashboard
  <https://syzkaller.appspot.com/upstream/s/wireguard> and bug pages
  <https://syzkaller.appspot.com/bug?extid=d0d2f1a65f45b319d25d>,
  <https://syzkaller.appspot.com/bug?extid=27a6e390f71f14f4fecf>;
  RTNL/napi_disable hung tasks
  <https://blog.cloudflare.com/searching-for-the-cause-of-hung-tasks-in-the-linux-kernel/>;
  VXLAN+WireGuard GRO <https://github.com/cilium/cilium/issues/48990>.
- Bonding/GRE: CVE-2026-43456
  <https://www.sentinelone.com/vulnerability-database/cve-2026-43456> ·
  <https://access.redhat.com/security/cve/cve-2026-43456> ·
  <https://www.penligent.ai/hackinglabs/cve-2026-43456>; bonding/XDP
  <https://lore.kernel.org/linux-cve-announce/2025041622-CVE-2025-22105-afef@gregkh/T>.
- Kaby Lake 7.2.x hard hang
  <https://github.com/CachyOS/linux-cachyos/issues/1056> ·
  <https://github.com/CachyOS/linux-cachyos/issues/1045>.
- Boot-time IPv6 netdev hang
  <https://forum.banana-pi.org/t/bpi-r3-boot-stuck-on-ipv6-addrconf-netdev-change-br-wan-link-becomes-ready/19224>;
  IPv6 multipath divide-by-zero CVE-2026-89790
  <https://www.sentinelone.com/vulnerability-database/cve-2026-89790>;
  IPv6 route-walk UAF
  <https://linuxsecurity.com/features/ipv6-security-route-listing-freed-kernel-memory>.
- Repo ground truth: [INCIDENT-1.0.13.md](INCIDENT-1.0.13.md),
  [ROUTER-KERNEL-BISECT.md](ROUTER-KERNEL-BISECT.md),
  [HANDOVER.md](HANDOVER.md), `configs/defconfig-{intel,qemu}`.