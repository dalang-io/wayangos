# userspace/ (legacy reference)

These init scripts are kept for **reference only** and are **not used by the
build**. The active initramfs is generated inline by
[`../scripts/build-rootfs.sh`](../scripts/build-rootfs.sh), which writes
`/etc/inittab`, `/etc/init.d/rcS`, and the service scripts directly.

- `init` — early PID 1 experiment (getty on serial)
- `inittab` — BusyBox init table
- `rcS` — early system initialization

Do not edit these expecting the built image to change. If you want to make the
build consume this directory, refactor `build-rootfs.sh` to install from here
and remove the inline heredocs.
