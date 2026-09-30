# HUD deploy & release runbook

How to build, install and release the three consoles — `wayang`,
`wayang-fw`, `wayang-router` — **onto a box, without an OS release**, and how to
cut a proper release. Cross-repo; see `AGENTS.md` for the OS rules.

## 0. TL;DR

```sh
# build on the build box (macOS musl link fails locally)
rsync -az --exclude .git --exclude target <repo>/ root@10.0.0.251:/tmp/hud/<name>/
ssh root@10.0.0.251 'cd /tmp/hud/<name> && PATH=$HOME/.cargo/bin:$PATH \
  cargo build --release --target x86_64-unknown-linux-musl'

# install on the box (persistent /data/bin wins over the baked /usr/bin)
cat <bin> | ssh root@<box> 'cat > /data/bin/.new && chmod 0755 /data/bin/.new \
  && mv -f /data/bin/.new /data/bin/<tool> && ln -sf /data/bin/<tool> /usr/bin/<tool>'
```

## 1. What persists where

| Path | Lives | Notes |
|---|---|---|
| `/usr/bin/<tool>` | **tmpfs** (image) | replaced at every boot from the image |
| `/data/bin/<tool>` | **ext4 `/data`** | survives updates **and** reboots |
| `/usr/bin/<tool>` → symlink | tmpfs | `init.d` re-links `wayang-router`/`wayang-fw` from `/data/bin` at boot |

So: for **`wayang-router` and `wayang-fw`**, dropping the binary in `/data/bin`
is enough — `init.d` prefers it and symlinks it on every boot. For the
**`wayang` CLI** nothing auto-links `/data/bin/wayang`; a `/usr/bin/wayang`
symlink is lost on reboot (re-create it, or ship via an OS release — §5).

## 2. Build

* Build on **`root@10.0.0.251`** (has cargo + the `x86_64-unknown-linux-musl`
  target). Cross-building from macOS fails to link (no musl linker).
* The products depend on the private crate `wayang-tui` via a **git tag**
  (`wayang-tui = { git = "https://github.com/dalang-io/wayang-tui", tag = "vX" }`);
  the build box can fetch it (SSH + HTTPS). Never vendor it.
* `PATH=$HOME/.cargo/bin:$PATH cargo build --release --target x86_64-unknown-linux-musl`.
* A `libc::time_t` deprecation **warning** on the musl target is pre-existing and
  harmless (host `clippy` is unaffected).

## 3. Install on the box

* **Never overwrite a running binary in place** — you get `Text file busy`.
  Write to a temp file, then `mv -f` (atomic; the running process keeps the old
  inode, the next start uses the new one).
* Reach the box directly (`ssh root@163.128.55.4`) or via the hub
  (`ssh dell-jkt` → `ssh root@10.99.130.5`). **No `sftp-server`** on the box —
  copy with `ssh 'cat > FILE' < FILE`.
* Verify after install:

```sh
ssh root@<box> 'sha256sum /data/bin/wayang-router /data/bin/wayang-fw /data/bin/wayang
  wayang-router --version; wayang-fw --version; wayang --version
  wayang-router snapshot /tmp/s >/dev/null && ls /tmp/s | wc -l'
```

## 4. Cut a release (router / fw tool repos)

1. Bump `Cargo.toml` (+ `Cargo.lock` via `cargo check`) — patch for fixes, minor
   for features (e.g. `0.4.3 → 0.5.0`). Commit + push.
2. Build musl (§2) and copy the artifact with the release name:
   `wayang-router-vX.Y.Z-x86_64-unknown-linux-musl` / `wayang-fw-vX.Y.Z-…`.
3. Tag + push + GitHub release (owner-approved; tagging `v*` is the release):

```sh
cd <repo> && git tag -a vX.Y.Z -m "…" && git push origin vX.Y.Z
gh release create vX.Y.Z --repo dalang-io/<repo> --title vX.Y.Z --notes "…" <asset>
```

4. **Publish to the tool mirror** (served at `https://wayang.dalang.io/edge/tools/`):

```sh
ssh root@10.0.0.251 'cp <built> /root/wayang.dalang.io/public/edge/tools/<asset>'
curl -fsSL https://wayang.dalang.io/edge/tools/<asset> | shasum -a 256   # matches
```

5. **Bump the OS pin** so the next image bakes it —
   `wayangos/scripts/build-wayang-{router,fw}.sh`: `VERSION` + `SHA256` (of the
   published binary). Commit + push. (`BUILD_DIR=/tmp/x bash scripts/…` validates
   the fetch + checksum locally.)
6. Install on the test box for manual testing (§3).

## 5. Shipping to all devices (OS release)

The `wayang` CLI is part of the OS image, so it ships with a **WayangOS release**:
tag `vX` in `dalang-io/wayangos` → `release.yml`/`installer-iso.yml` build the ISO
+ signed `.wup` and publish the update channel (every device sees it). **Ask the
owner before tagging.** For a single box, install/upégrade from `/data/bin` (§3)
instead.

## 6. Rollback

* `wayang-fw`/`wayang-router`: reinstall the previous binary to `/data/bin`
  (init prefers it), or restore `<tool>.prev` if the deploy script kept one.
* `wayang` CLI: re-symlink `/usr/bin/wayang` to the previous `/data/bin/wayang`.
* OS updates are A/B — `wayang update --rollback` (or boot the other slot).
* The DB/config is separate: `wayang reset` restores default configs (with a
  REVIEW); nothing here touches `/data/etc`.
