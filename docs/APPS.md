# Apps

What WayangOS ships and what is only advertised. The website promotes more apps
than this repository contains source for — this is the source of truth.

Status legend: **in-repo** (source here), **external** (source in another repo,
pulled at build time), **web-only** (page exists, no code here), **deprecated**.

## Promoted on the website

[`landing-page/apps.html`](../landing-page/apps.html) lists 10 apps (each with a
page under `landing-page/apps/`, all marked *Preview*) plus one *Coming Soon*:

| App | Web page | Source in this repo | Status |
|---|---|---|---|
| **WayangPOS** | `apps/pos.html` | — | external — `dalang-io/wayang-pos` (private) |
| **WayangViewer** | `apps/viewer.html` | `scripts/deprecated/fbviewer.c` | deprecated |
| **dcheck** | `apps/dcheck.html` | `dcheck/` (build artifacts only) | external — `dalang-io/dcheck` |
| WayangDCIEM | `apps/dciem.html` | — | web-only |
| WayangGates | `apps/gates.html` | — | web-only |
| WayangMAP | `apps/map.html` | — | web-only |
| WayangZeroClient | `apps/zeroclient.html` | — | web-only |
| WayangExplorer | `apps/explorer.html` | — | web-only |
| WayangTOP | `apps/top.html` | — | web-only |
| WayangMediaPlayer | `apps/mediaplayer.html` | — | web-only |
| WayangKiosk | — (Coming Soon) | — | web-only |

Count: **11 promoted, ~2 with source here** (POS + Viewer/deprecated), plus
**dcheck** from a separate repository.

## Build / fetch

- **WayangPOS** — `scripts/build-pos.sh` (binary) and `scripts/build-pos-iso.sh`
  (bootable ISO). Source: private repo `dalang-io/wayang-pos`, fetched at
  `POS_REF` (default `v3.2.2`) into `$BUILD/wayang-pos/` (direct framebuffer,
  evdev, SQLite).
- **dcheck** — `scripts/fetch-dcheck.sh` downloads the signed release binary
  (x86_64-unknown-linux-musl) from `https://wayang.dalang.io/dcheck` into
  `$BUILD/dcheck/dcheck`; `build-rootfs.sh` installs it to `/usr/bin/dcheck`.
  So dcheck ships **by default** in every image even though its source lives
  elsewhere.

## System components (not marketed apps)

These are part of the OS, not the app catalogue:

| Component | Path | Role |
|---|---|---|
| `wayang` | `wayang/` | updater/CLI: `version`, `status`, `update`, `verify`, `net`, `wifi`, … |
| `wayang-installer` | `installer/` | USB installer TUI (ratatui) |
| userspace init | `userspace/`, `scripts/build-rootfs.sh` | BusyBox init, network, sshd, splash |

## Adding a new app

1. Add the source under a top-level directory (e.g. `myapp/`).
2. Add a build script (`scripts/build-myapp.sh`) that outputs a **static** binary
   (the rootfs has no dynamic loader for app libs) and wire it into
   `scripts/ci-build.sh` before `build-rootfs.sh`.
3. Install it in `scripts/build-rootfs.sh` (`install -m 755 … "$ROOTFS/usr/bin/…"`).
4. Add the page under `landing-page/apps/` and a card in `apps.html`.
