# `wayang` — WayangOS CLI + console HUD

The `wayang` binary is the box's updater, the runtime `net` / `wifi` HUDs, and
the interactive system console (the "command deck"). `wayang` with no
subcommand on a TTY opens the HUD; anything piped falls back to printing help,
so scripts are never surprised. See `../docs/UPDATE-DESIGN.md` for the frozen
subcommands and exit codes.

## Chrome (canonical — see `../docs/TUI-UX-REVAMP.md`)

The CLI, `wayang-fw` and `wayang-router` share one look. The implementation is
`src/hud.rs`:

* **Panel**: thin `border`, heavy corners `┏ ┓ ┗ ┛` in `accent`, title
  `◢ TITLE ◣` (title `accent` + bold, chevrons `accent2`). The old cp437
  `▐ TITLE ▌` title is gone; the kernel console uses the same chrome now.
* **Caption**: `── TEXT ─────` (`border` lead/fill, `accent` text) via
  `hud::caption`.
* **Keycaps**: `` key `` on `bar` bg + `accent` fg, dim label; with no `bar`
  (ANSI) they render as `[key]`.
* **Header**: `bar` bg — brand · breadcrumb `MODULE ▸ TAB ▸ item` · state badge
  (glyph and word) · revision.
* **Selection**: the `selection` style (reverse video when there is no palette);
  `NO_COLOR` swaps the cursor for a plain `> `.

### Focused pane

Exactly one pane owns the keyboard. `hud::panel_focus(f, area, title, focused, t)`
draws the focused pane's border and title in `accent` and prefixes the title
with `▸ ` (`>` with `NO_COLOR`); unfocused panes keep the `border`/dim title.
The deck focuses the MODULES list (the preview card is dim); each module's
split focuses the list or the active tab's pane; a modal (help, REVIEW, input,
picker, jump) takes focus while it is open. Border colour is never the only
signal.

## Version

`wayang --version` prints `wayang <crate-semver> (<build-marker>)`, e.g.
`wayang 0.1.0 (ux-p1)`. The crate semver is left alone — the OS product version
lives in `/etc/wayang/version`. The marker is `version::BUILD_MARKER`.

`wayang --demo` and `wayang --screens DIR [--svg] [--size WxH]` render every
screen from sample data without touching the system. Demo output is labelled
`DEMO DATA` and the header shows `demo`, never a fake release.

## Screens

`wayang/screens/*.txt` are checked-in snapshots rendered with

```sh
cargo run -- --screens screens --size 110x34
```

and kept in sync by `tui::tests::committed_screens_match_the_renderer`.

## Build

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

Static musl release (on the build host):

```sh
cargo build --release --target x86_64-unknown-linux-musl
```
