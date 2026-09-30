# WayangOS Kernel Configs

Configs for the WayangOS Linux kernel. The base kernel version is **7.2.7**
(x86_64); the real-time flavor uses the **6.19.3-rt1** PREEMPT_RT tree.

## Config format

A config file is either a **full `.config`** or a small **fragment**:

- If the first line is `# WAYANG_BASE: <name>`, the build script loads
  `configs/<name>` as the base, appends the rest of this file, then runs
  `make olddefconfig` to resolve dependencies.
- `# WAYANG_BASE: defconfig` is special: it runs `make ARCH=$ARCH defconfig`
  (used by the ARM64 fragments).
- No marker → the whole file is used verbatim (this is how `defconfig-qemu`
  works).

Fragments are intentional: they stay readable and are resolved by
`olddefconfig`, so we don't duplicate a 5000-line config for every GPU.

## Available configs

| Config | Arch | Base | Purpose |
|--------|------|------|---------|
| `defconfig-qemu` | x86_64 | full config | QEMU/VM testing (recommended starting point) |
| `defconfig-intel` | x86_64 | `defconfig-qemu` | Intel i915 integrated GPU |
| `defconfig-amd` | x86_64 | `defconfig-qemu` | AMD GPU (amdgpu + radeon) |
| `defconfig-nvidia` | x86_64 | `defconfig-qemu` | NVIDIA open driver (nouveau) |
| `defconfig-rt` | x86_64 | `defconfig-qemu` | PREEMPT_RT real-time (needs RT-patched tree) |
| `defconfig-wifi` | x86_64 | `defconfig-qemu` | Common USB WiFi drivers (needs firmware + wpa_supplicant in the rootfs) |
| `defconfig-arm64-rpi3` | arm64 | `defconfig` | Raspberry Pi 3 (BCM2837, VC4/V3D) |
| `defconfig-arm64-orangepi-zero2w` | arm64 | `defconfig` | Orange Pi Zero 2W (Allwinner H618, Panfrost) |
| `defconfig-waf-inline` | x86_64 | `defconfig-qemu` | **Staged, not built by anything.** L7 interception (TPROXY + NFQUEUE) for WAF mode B; see below |

### WAF mode B, transparent inline (`defconfig-waf-inline`)

Lets a later `wayang-waf` phase inspect traffic **in the path** (bump-in-the-wire)
instead of only as a reverse proxy. Design: `wayang-fw/docs/WAF.md` §3 and §10
(P2). WAF **mode A needs no kernel option at all** and is the recommended first
phase; this fragment exists only for P2.

**It is not in any build pipeline.** No script and no workflow builds it, and it
must not reach an image until it has had the same treatment the router block got
(docs/ROUTER-KERNEL-BISECT.md): per-option bisect, then a boot-count soak, then a
cold-boot soak. The precedent is not theoretical — 1.0.13 and 1.0.20 both froze
the device when a large kernel block went in, and the router block was only
re-enabled in 1.0.27 after ~30 clean boots plus a cold-boot soak. AGENTS.md
"Current state" records the 1.0.27 soak ("~30 clean block boots + cold-boot
PASS"). Nine kernel symbols are not worth a bricked kiosk.

**Cost: +20 KiB of `bzImage` (measured).** Built on `linux-7.2.7` on
`root@10.0.0.251` and diffed against the resolved `defconfig-intel`:

| kernel | `bzImage` | delta |
|---|---|---|
| `defconfig-intel` (current device kernel) | 16.31 MiB | — |
| **+ this fragment** | **16.33 MiB** | **+20 KiB (+0.12%)** |
| + the same plus XDP/eBPF (deliberately *not* included) | 16.94 MiB | +640 KiB (+3.8%) |

That last row is the point: enabling XDP/eBPF would cost **32×** more than the
whole rest of this fragment, and nothing in P2 uses it.

**Building it.** The base is `defconfig-qemu`, not `defconfig-intel` — see the
fragment's header. `build-kernel.sh` copies the base **verbatim** and resolves
only one level, so `WAYANG_BASE: defconfig-intel` yields a kernel with no
networking at all (`CONFIG_NET`/`PCI`/`MODULES`/`SMP` all `n`). Flatten first:

```bash
{ head -n 1 configs/defconfig-intel
  tail -n +2 configs/defconfig-intel
  tail -n +2 configs/defconfig-waf-inline; } > /tmp/defconfig-waf-inline
cp /tmp/defconfig-waf-inline configs/
./scripts/build-kernel.sh defconfig-waf-inline
```

**Symbol caveats worth knowing** (all verified in the 7.2.7 tree):

- There is **no `CONFIG_NF_TPROXY`** and **no `CONFIG_NFQUEUE`** — those are
  pre-4.18 names. 7.2.7 uses `NF_TPROXY_IPV4`/`NF_TPROXY_IPV6`,
  `NETFILTER_NETLINK_QUEUE` and `NFT_QUEUE`.
- `NETFILTER_XT_TARGET_TPROXY` silently vanishes unless `IP_NF_MANGLE` **or**
  `NFT_COMPAT` is set. The fragment sets `NFT_COMPAT`; the alternative,
  `IP_NF_MANGLE`, is gated behind the deprecated `IP_NF_IPTABLES_LEGACY`.
- `XT_MATCH_SOCKET` is `NETFILTER_XT_MATCH_SOCKET` and does **not** depend on
  `NET_SOCKETS` — that symbol no longer exists in 7.2.7.
- There is no `CONFIG_NET_XDP` in 7.2.7 (generic XDP is unconditional; support is
  per-driver). What is actually off in the base is `BPF_SYSCALL` and `BPF_JIT`.

**RT caveat (unverified).** All of the above was verified against **linux-7.2.7**
only. The RT editions build **6.19.3-rt1**, and that tree is not present on the
build box, so these symbols have **not** been checked there. The netfilter
TPROXY/NFQUEUE symbols predate 6.19 by a wide margin and are very likely
identical, but treat that as unconfirmed: resolve this fragment against the RT
tree before using it for an RT edition.

### Real-time (`defconfig-rt`)

`CONFIG_PREEMPT_RT` only exists in an RT-patched kernel tree. Fetch it first:

```bash
KERNEL_FLAVOR=rt ./scripts/fetch-sources.sh   # Linux 6.19.3-rt1 → $BUILD/linux-6.19.3-rt1
./scripts/build-kernel.sh defconfig-rt
```

`build-kernel.sh` automatically uses `$BUILD/linux-6.19.3-rt1` for any config
whose name contains `rt`.

## Building

```bash
# x86_64 (default)
./scripts/build-kernel.sh defconfig-qemu
./scripts/build-kernel.sh defconfig-intel
./scripts/build-kernel.sh defconfig-rt

# ARM64 (cross-compile)
ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- ./scripts/build-kernel.sh defconfig-arm64-rpi3
ARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- ./scripts/build-kernel.sh defconfig-arm64-orangepi-zero2w
```

Output: `$BUILD_DIR/bzImage-<config>` (x86_64) or `$BUILD_DIR/Image-<config>`
(arm64). See [`../BUILDING.md`](../BUILDING.md) for prerequisites and the full
pipeline.

## Creating a new config

Start from an existing one and add only what changes:

```bash
cp configs/defconfig-intel configs/defconfig-mybox
# edit the options you need, then:
./scripts/build-kernel.sh defconfig-mybox
```

## Status / caveats

- `defconfig-qemu` is the only config confirmed booting in QEMU with working
  PS/2 keyboard input.
- `defconfig-waf-inline` has been resolved through `olddefconfig` and measured
  on `linux-7.2.7`, but **has never been booted** on QEMU or on hardware. It is
  a staged option list, not a supported config.
- The GPU and ARM64 fragments enable the right drivers, but they have **not
  been re-verified on real hardware** from this repo yet. Treat them as the
  reproducible starting point for the released editions.
- NVIDIA uses the open `nouveau` driver, not the proprietary blob.
- RISC-V is roadmap only (no config yet).

## Common pitfalls

1. **Custom minimal configs break keyboard input in QEMU.** The i8042 PS/2
   controller and AT keyboard driver are required. `defconfig-qemu` includes
   them; hand-tuned minimal configs often don't. Required options:
   ```
   CONFIG_SERIO=y
   CONFIG_SERIO_I8042=y
   CONFIG_KEYBOARD_ATKBD=y
   CONFIG_INPUT_EVDEV=y
   ```
2. **`olddefconfig` can drop options whose dependencies are unmet.** If a GPU
   option disappears after building, enable its parent (`CONFIG_DRM`,
   `CONFIG_FB`, platform `CONFIG_ARCH_*`, etc.) in the fragment. The failure is
   silent — the option simply does not appear in the resolved `.config`, which
   is why you must diff the result rather than assume:
   ```bash
   cp configs/defconfig-qemu .config && tail -n +2 configs/defconfig-FRAGMENT >> .config
   make olddefconfig && git diff --no-index .config .config.resolved
   ```
3. **A fragment cannot be based on another fragment.** `build-kernel.sh` copies
   the base file verbatim and resolves one level only, so
   `# WAYANG_BASE: defconfig-intel` builds a kernel with networking off. Only
   `defconfig-qemu` (or `defconfig`) is a usable base; flatten by hand as in
   `defconfig-waf-inline` above.
