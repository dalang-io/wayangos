# WayangOS ARM64 A/B update slots (M7)

Design and runbook for `wayang update` / `wayang upgrade` / `wayang update
--rollback` on **Raspberry Pi 3** and **Orange Pi Zero 2W**.

This document is subordinate to **[`UPDATE-DESIGN.md`](UPDATE-DESIGN.md)**, which
freezes the interfaces. If anything here disagrees with the frozen spec, the
spec wins. The updater state backend (`wayang/src/state.rs`, `slot.rs`,
`staging.rs`) is owned by agent **A/F**; this document and
[`scripts/build-arm64-image.sh`](../scripts/build-arm64-image.sh) are owned by
the M7 boot work.

Status: **design + tooling only.** Nothing here has run on real hardware. See
[Untested / assumptions](#untested--assumptions).

---

## 1. The ARM model in one paragraph

On ARM there is no GRUB and no UEFI ESP. The **boot partition** is a small
FAT32 partition (recommended label `WAYANGBOOT`, 512 MiB) that the board ROM /
firmware / U-Boot reads. Just like x86, each update installs a complete
`vmlinuz` + `initramfs.img` pair into the **idle slot** (`A/` or `B/`); the
rootfs never changes in place. The only difference is *who switches slots*:

| | x86 (frozen) | ARM (this doc) |
|---|---|---|
| Boot selector | GRUB (`grub.cfg`) | RPi firmware `config.txt` / U-Boot |
| Slot files | `<ESP>/A|B/…` | `<boot>/A|B/…` |
| Fallback state | `<ESP>/grub/grubenv` | `<boot>/wayang/vars` |
| State keys | `wayang_slot`, `wayang_good`, `wayang_attempts` (+ `wayang_prev`) | identical |
| Fallback mechanism | GRUB reads attempt count | RPi `tryboot` one-shot / U-Boot `bootcount` |

`wayang` resolves the backend exactly as frozen (§"ARM64 slots (M7)" in the
design): `grub/grubenv` if present, else `wayang/vars`, else a clear error. The
same keys drive both. The kernel filename inside a slot is **always
`vmlinuz`** on every board, because that is what `wayang/src/staging.rs` writes;
`--kernel Image` in the builder just names the *input* artifact.

`wayang` maintains state only:

- `stage` → writes the idle slot, then `wayang_prev=<active>`,
  `wayang_slot=<target>`, `wayang_attempts=0`.
- `mark-ok` → `wayang_good=<active>`, `wayang_slot=<active>`, `wayang_attempts=0`.
- `--rollback` → swap to `wayang_prev` (or `wayang_good`), reset attempts.

Everything below is the board-side glue that **consumes** `wayang/vars` and
actually selects the slot.

---

## 2. Boot partition layout (shared)

```
WAYANGBOOT (FAT32, 512 MiB)
├── <firmware/U-Boot from the board>      # board-specific, not touched by wayang
├── config.txt / extlinux/ …              # board boot config (§3, §4)
├── A/
│   ├── vmlinuz
│   ├── initramfs.img
│   ├── dtb/<board>.dtb                   # optional, explicit
│   └── overlays/…                        # optional (RPi)
├── B/
│   ├── vmlinuz
│   ├── initramfs.img
│   └── dtb/<board>.dtb
├── var/
│   ├── meta-A.json                       # written by wayang staging
│   └── meta-B.json
└── wayang/
    └── vars                              # fallback state (§5)
```

At runtime the running system sees the boot partition mounted at `/boot`, so
the state file is `/boot/wayang/vars`. The builder writes it to
`<out>/wayang/vars`, i.e. relative to the partition root.

Both slots are 20–35 MiB; two slots plus firmware fit comfortably in 512 MiB.
`wayang` must refuse to stage when the idle slot cannot fit (x86 rule, applies
unchanged).

---

## 3. Raspberry Pi 3

### 3.1 Layout and prompt

Pi 3 boot flow: ROM → `bootcode.bin` → `start.elf`/`fixup.dat` (root) → reads
`config.txt` → loads kernel/initramfs/dtb. `os_prefix` applies to the files
loaded *after* `config.txt` (kernel, initramfs, dtb, overlays), so the firmware
blobs stay at the root and only the slot payload is prefixed:

```
/config.txt              # default (last known good)
/tryboot.txt             # one-shot candidate
/bootcode.bin
/start.elf
/fixup.dat
/A/vmlinuz
/A/initramfs.img
/A/dtb/bcm2710-rpi-3-b.dtb
/A/overlays/…
/B/…
/wayang/vars
```

### 3.2 `config.txt` (default / known-good slot)

```ini
# WayangOS / Raspberry Pi 3
arm_64bit=1
enable_uart=1
os_prefix=A/
kernel=vmlinuz
initramfs initramfs.img followkernel
# device_tree is normally auto-selected; set it only if you ship a custom dtb:
#device_tree=bcm2710-rpi-3-b.dtb
```

### 3.3 `tryboot.txt` (candidate slot)

Same file with `os_prefix=B/`. The Raspberry Pi bootloader supports a
**one-shot** `tryboot` reboot: the OS calls `reboot "0 tryboot"`, the firmware
then reads `tryboot.txt` instead of `config.txt` for that boot. If the candidate
never reaches userspace the flag is not renewed, so the next reset falls back to
`config.txt` (the known-good slot). That is the anti-brick primitive.

```ini
# WayangOS / Raspberry Pi 3 — one-shot trial slot
arm_64bit=1
enable_uart=1
os_prefix=B/
kernel=vmlinuz
initramfs initramfs.img followkernel
```

### 3.4 How the slot is selected from `wayang/vars`

The Pi firmware cannot parse `wayang/vars`. The bridge is a tiny privileged
helper, `wayang-arm-sync` (to live in the rootfs, agent B/F), invoked at the end
of a successful `wayang update --reboot` and at shutdown:

1. read `/boot/wayang/vars`;
2. write `config.txt` with `os_prefix=<wayang_good>/`;
3. write `tryboot.txt` with `os_prefix=<wayang_slot>/`;
4. `reboot "0 tryboot"` (or plain `reboot` if the target equals good).

On the candidate boot, `rcS` runs `wayang mark-ok`, which sets
`wayang_good=<active>` and `wayang_attempts=0`. The next sync promotes that slot
into `config.txt`. If the candidate fails, the one-shot flag has already been
consumed and the following reset returns to `config.txt` → good slot, and
`wayang_attempts` still records the failed trial for `wayang status`.

> Contract note: `wayang` itself only writes `wayang/vars`; it does **not**
> edit `config.txt`. The `wayang-arm-sync` helper is the ARM analogue of GRUB's
> `grubenv` consumer. It is not implemented in this milestone.

Alternative if Pi 3 firmware lacks `tryboot`: use `autoboot.txt` with a
`[tryboot]` section, or put U-Boot in the Pi 3 boot chain and reuse the U-Boot
selector from §4 (the design doc explicitly allows "a small bootloader script").

---

## 4. Orange Pi Zero 2W

Allwinner H618 boot flow: BootROM → SPL (`boot0`) → U-Boot (`u-boot.itb`) →
reads a boot script / `extlinux` from the FAT boot partition → kernel. U-Boot
can read files, compare integers and write files, so it consumes `wayang/vars`
directly.

### 4.1 `extlinux/extlinux.cfg` (manual / distro-boot path)

Stock U-Boot `sysboot` looks for `extlinux/extlinux.conf`; the M7 spec names the
file `extlinux.cfg`. The builder writes **both** with identical content.

```
DEFAULT wayang-A
TIMEOUT 1
PROMPT 0
MENU TITLE WayangOS A/B

LABEL wayang-A
    LINUX ../A/vmlinuz
    INITRD ../A/initramfs.img
    FDT ../A/dtb/sun50i-h618-orangepi-zero2w.dtb
    APPEND console=ttyS0,115200 root=/dev/ram0 rw quiet

LABEL wayang-B
    LINUX ../B/vmlinuz
    INITRD ../B/initramfs.img
    FDT ../B/dtb/sun50i-h618-orangepi-zero2w.dtb
    APPEND console=ttyS0,115200 root=/dev/ram0 rw quiet
```

`DEFAULT` is static here; the automatic selector below is preferred because it
also persists the attempt counter.

### 4.2 `boot.cmd` / `boot.scr` (automatic selector)

Compile with `mkimage -T script -C none -n 'WayangOS A/B' -d boot.cmd boot.scr`
and place `boot.scr` at the FAT root. It loads `wayang/vars` (plain
`key=value`), imports it with U-Boot's text `env import -t`, honours the
attempt budget, and loads the chosen slot:

```sh
setenv bootpart 1
setenv scriptaddr 0x42000000
setenv wayang_slot A
setenv wayang_good A
setenv wayang_attempts 0
setenv wayang_dtb sun50i-h618-orangepi-zero2w.dtb

if load mmc 0:${bootpart} ${scriptaddr} wayang/vars; then
    env import -t ${scriptaddr} ${filesize}
    if test -n "${wayang_attempts}" && test ${wayang_attempts} -ge 3; then
        setenv wayang_slot ${wayang_good}
    fi
fi

load mmc 0:${bootpart} ${kernel_addr_r} ${wayang_slot}/vmlinuz
load mmc 0:${bootpart} ${ramdisk_addr_r} ${wayang_slot}/initramfs.img
setenv ramdisk_size ${filesize}
load mmc 0:${bootpart} ${fdt_addr_r} ${wayang_slot}/dtb/${wayang_dtb}
booti ${kernel_addr_r} ${ramdisk_addr_r}:${ramdisk_size} ${fdt_addr_r}
```

Attempt counting on this board is best handled by U-Boot's built-in boot counter
rather than rewriting the text file from the bootloader:

```
# in the U-Boot environment:
setenv bootlimit 3
setenv altbootcmd 'setenv wayang_slot ${wayang_good}; run wayang_boot'
```

Requires `CONFIG_BOOTCOUNT_LIMIT=y` and a bootcounter backend. `bootcount` is
incremented by U-Boot before boot; when it exceeds `bootlimit`, `altbootcmd`
runs instead of `bootcmd` and forces the good slot. `wayang mark-ok` /
`--rollback` still edit `wayang/vars`, which the script reads on every boot.

> If your U-Boot has no bootcounter backend, the fallback is: boot the candidate
> once; if it fails to reach `rcS`, the board must be power-cycled manually, or
> `wayang update --rollback` run from a rescue environment. Real automatic
> fallback needs the bootcounter.

### 4.3 `wayang/vars`

```
wayang_slot=A
wayang_good=A
wayang_attempts=0
```

`wayang` may also add `wayang_prev` (see `staging.rs`); the boot glue only needs
the three frozen keys.

---

## 5. Fallback state file (`wayang/vars`)

Plain text, one `key=value` per line, `\n`-terminated, no padding (unlike the
1024-byte GRUB block). `#` comments and blank lines are ignored; a malformed
line is an error. Keys:

| Key | Meaning | Values |
|---|---|---|
| `wayang_slot` | slot to boot next | `A` \| `B` |
| `wayang_good` | last slot that reached `mark-ok` | `A` \| `B` |
| `wayang_attempts` | failed boots of `wayang_slot` so far | integer |
| `wayang_prev` | slot before the current stage (extra, used by rollback) | `A` \| `B` |

Semantics are identical to the GRUB design:

- choose `wayang_good` when `wayang_attempts >= 3`, otherwise `wayang_slot`;
- `update`/`upgrade` stage → `wayang_slot=<target>`, `attempts=0`;
- `mark-ok` → `wayang_good=<active>`, `wayang_slot=<active>`, `attempts=0`;
- `--rollback` → `wayang_slot=<prev or good>`, `attempts=0`.

The builder creates `wayang/vars` if missing (defaults `A`, `A`, `0`) and
otherwise **merges** any missing key, so re-running it over an existing image
never clobbers state.

---

## 6. Runbook: build a bootable A/B SD image

### 6.1 Assemble the boot partition tree

```sh
# Raspberry Pi 3 (kernel built as Image, but installed as vmlinuz)
./scripts/build-arm64-image.sh \
    --board rpi3 --slot A \
    --kernel  out/Image \
    --initramfs out/initramfs.img \
    --dtb     out/bcm2710-rpi-3-b.dtb \
    --out     build/sd-boot

# Orange Pi Zero 2W (produces extlinux/ + boot.cmd, tries mkimage)
./scripts/build-arm64-image.sh \
    --board orangepi --slot A \
    --kernel  out/Image \
    --initramfs out/initramfs.img \
    --dtb     out/sun50i-h618-orangepi-zero2w.dtb \
    --out     build/sd-boot
```

This writes `build/sd-boot/<slot>/{vmlinuz,initramfs.img,dtb/}`, the board boot
config, and `build/sd-boot/wayang/vars`. Run it again with `--slot B` to stage
the second slot; the board config is regenerated and `tryboot.txt` /
`LABEL wayang-B` appear because `B/` now exists.

The builder is intentionally dumb about "good" vs "trial": it writes
`config.txt` (`os_prefix`) for the slot you asked for, and `tryboot.txt` for the
other slot when present. The `wayang-arm-sync` helper (§3.4) establishes the
runtime relationship (`config.txt` = `wayang_good`, `tryboot.txt` =
`wayang_slot`). `wayang/vars` is created once and only merged on re-runs, so
existing state is never clobbered.

### 6.2 Optional: produce a FAT image

If `mkfs.vfat`/`mkfs.fat` and `mtools` are installed (or you are root with
`losetup`), add `--img`:

```sh
./scripts/build-arm64-image.sh ... --out build/sd-boot \
    --img build/WAYANGBOOT.img --size 512
```

Otherwise the script prints exactly what is missing and leaves the directory
tree for you to copy onto a prepared partition.

### 6.3 Prepare the card

1. Partition (msdos table, or GPT with a `0x0c`-type first partition):
   - p1: 512 MiB FAT32, label `WAYANGBOOT` — boot partition;
   - p2: rest, ext4, label `WAYANGDATA` — `/data`.
2. Populate p1 with:
   - the board firmware / U-Boot (RPi: `bootcode.bin`, `start.elf`, `fixup.dat`;
     OPi: `boot0` at the raw SD offset, `u-boot.itb`/`u-boot-sunxi-with-spl.bin`
     as the board requires);
   - the `build/sd-boot` tree (slots, boot config, `wayang/vars`).
3. On RPi, `config.txt` must be at the FAT root; `os_prefix` is relative to it.
   On OPi, ensure U-Boot is configured to run `boot.scr` (or `sysboot`).

### 6.4 On-board update flow

```sh
wayang status                       # active slot, good slot, attempts, version
wayang update --check               # online lookup for the arm64 channel
wayang update --from /media/usb/wayang-1.4.1-arm64.wup
wayang status                       # wayang_slot now points at the idle slot
# reboot:
#   x86:  GRUB reads grubenv and boots wayang_slot
#   RPi:  wayang-arm-sync rewrites config.txt/tryboot.txt, reboot "0 tryboot"
#   OPi:  U-Boot boot.cmd imports wayang/vars and loads wayang_slot
# after the new slot reaches userspace, rcS runs:
wayang mark-ok                      # wayang_good=<active>, attempts=0
```

### 6.5 Rollback

```sh
wayang update --rollback [--reboot]
```

This sets `wayang_slot=<previous|good>` and `attempts=0`. The next boot picks
that slot through the board glue above. On RPi the sync helper must run before
the reboot so `config.txt` reflects the swap.

---

## 7. Testing checklist (needs real hardware)

Boot/fallback:

- [ ] Fresh image boots slot A; `wayang status` shows `A`, `good=A`, `attempts=0`.
- [ ] `wayang update --from <b.wup>` stages `B/vmlinuz` + `B/initramfs.img`;
      `wayang/vars` gets `wayang_slot=B`, `wayang_prev=A`, `attempts=0`.
- [ ] Reboot boots B; `mark-ok` sets `good=B`, `attempts=0`.
- [ ] Rollback from B boots A and leaves `/data` intact.
- [ ] Corrupt/remove `B/vmlinuz` (or its initramfs) → board falls back to A:
      - RPi: `tryboot` one-shot consumed once, then `config.txt` → A;
      - OPi: `bootcount` reaches `bootlimit`, `altbootcmd` forces A.
- [ ] Power-cut during staging leaves the active slot bootable (FAT has no
      journal).
- [ ] `wayang status` on ARM reports backend `vars` (not `grubenv`).
- [ ] `wayang update` refuses when the idle slot will not fit on the FAT.

Builder/tooling:

- [ ] `shellcheck scripts/build-arm64-image.sh` clean; `bash -n` clean.
- [ ] Both boards produce the expected tree; `tryboot.txt` only when the other
      slot exists; re-run does not clobber `wayang/vars`.
- [ ] `--img` path when tools present, and the clean skip when they are not.
- [ ] `mkimage` builds a bootable `boot.scr` when installed.

---

## 8. Where x86 and ARM differ

- **Selector:** GRUB script vs RPi firmware / U-Boot. The state *keys* and the
  `wayang` CLI are identical, so `wayang status`/`update`/`--rollback` behave the
  same; only the materialisation of the choice differs.
- **State backend:** `<boot>/grub/grubenv` (1024-byte GRUB block) vs
  `<boot>/wayang/vars` (plain text). `wayang/src/state.rs` picks grubenv first.
- **Kernel file name:** `<slot>/vmlinuz` everywhere (the ARM builder renames the
  input `Image`).
- **DTB/overlays:** ARM slots carry a `dtb/` (and RPi `overlays/`) directory;
  x86 has none.
- **Attempt counter:** x86 GRUB can edit `grubenv` itself (`save_env`); RPi
  firmware cannot edit files, and OPi needs `bootcount`/`altbootcmd` or an init
  hook. `wayang_attempts` is still authoritative in `wayang/vars`.
- **No UEFI/ESP, no `EFI/BOOT/BOOTX64.EFI`, no `grub.cfg`** on ARM.

---

## 9. Untested / assumptions

Things that must be validated on hardware before M7 is "done":

1. **`os_prefix` semantics on Pi 3 firmware.** Assumed to prefix only the files
   loaded after `config.txt` (kernel/initramfs/dtb/overlays), leaving firmware
   blobs at the root. Verify with the actual firmware revision.
2. **`tryboot` on Pi 3.** `tryboot` is documented mainly for Pi 4/CM4/5; Pi 3
   support is **unverified**. If absent, switch to `autoboot.txt [tryboot]` or
   U-Boot on Pi 3.
3. **`wayang-arm-sync` helper does not exist yet.** It is the required bridge
   between `wayang/vars` and `config.txt`/`tryboot.txt` and is owned by the
   rootfs/init work (agent B/F). Without it, RPi does not consume `wayang/vars`.
4. **U-Boot `env import -t ${scriptaddr} ${filesize}`** must be confirmed for
   the specific U-Boot build on the Orange Pi Zero 2W (vendor U-Boots differ;
   `scriptaddr` may be undefined, `env import` options may be trimmed).
5. **`bootcount`/`bootlimit`/`altbootcmd`** depends on
   `CONFIG_BOOTCOUNT_LIMIT=y` and a writable counter backend; the vendor
   U-Boot may not enable it. Manual rollback is always available.
6. **FAT tooling matrix.** `mkfs.vfat`/`mkfs.fat` + `mtools` on Linux CI; macOS
   lacks them, so `--img` falls back to a directory tree. The loop-mount path
   needs root.
7. **Secure boot / signed firmware** is out of scope; nothing here is verified
   by the board ROM. Bundle signature verification still happens in `wayang`
   before staging.
8. **Partition table**: the script only produces the boot *contents*; creating
   `p1`/`p2`, flashing U-Boot and the rootfs is manual until a board image
   builder exists.