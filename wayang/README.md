# `wayang` — WayangOS CLI + console HUD

The `wayang` binary is the box's updater, the runtime `net` / `wifi` HUDs, and
the interactive system console (the "command deck"). `wayang` with no
subcommand on a TTY opens the HUD; anything piped falls back to printing help,
so scripts are never surprised. See `../docs/UPDATE-DESIGN.md` for the frozen
subcommands and exit codes.

## Chrome (canonical — see `../docs/TUI-UX-REVAMP.md`)

The CLI, `wayang-fw` and `wayang-router` share one look, implemented once in the
**`wayang-tui`** crate (private git dependency, tag `v0.1.0`). `src/hud.rs` is
now a thin CLI adapter: it re-exports the crate's theme/widgets and keeps only
the CLI-specific pieces (the `Tone` severity model, the status row, the ASCII
spinner, the indeterminate progress bar, the tab-row grammar, the inline hint
row and the `▰` slot marker), so the three HUDs match **by construction**.

* **Theme**: `hud::detect()` → `wayang_tui::theme::Theme::resolve(WAYANG_OS,
  Flags::default())`, driven by `NO_COLOR` / `WAYANG_OS_COLOR` /
  `WAYANG_TUI_COLOR` / `COLORTERM`. The CLI's old `Mode::Console` blue palette
  and the OSC palette rewrite are gone — the crate's console/ansi handling is
  canonical.
* **Panel**: `wayang_tui::widgets::panel` / `panel_focused` — thin `border`,
  heavy corners `┏ ┓ ┗ ┛`, title `◢ TITLE ◣` (title `accent` + bold, chevrons
  `accent2`).
* **Caption**: `wayang_tui::widgets::caption` — `── TEXT ─────` (`border`
  lead/fill, `accent` text).
* **Keycaps / footer**: `wayang_tui::widgets::keycaps` (+ `hud::footer`, the
  CLI's one-column gutter) — `` key `` on `bar` bg + `accent` fg, dim label;
  with no `bar` (ANSI) they render as `[key]`.
* **Header**: `hud::header_bar` folds the CLI's revision label into
  `wayang_tui::widgets::Header::subtitle` so it stays on the left
  (`◢◤ WAYANG OS // DECK ▸ SYSTEM  v1.4.1`), beside the state badge.
* **Selection**: `Palette::highlight()` (reverse video when there is no
  palette); `NO_COLOR` swaps the cursor for a plain `> `.

### Focused pane

Exactly one pane owns the keyboard. `wayang_tui::widgets::panel(f, area, title,
None, focused, t)` draws the focused pane's border and title in `accent` and
prefixes the title with `▸ ` (`>` with `NO_COLOR`); unfocused panes keep the
`border`/dim title. The deck focuses the MODULES list (the preview card is dim);
each module's split focuses the list or the active tab's pane; a modal (help,
REVIEW, input, picker, jump) takes focus while it is open — REVIEW and the input
/picker modals use the crate's shared `wayang_tui::overlay::Overlay` frame.
Border colour is never the only signal.

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
