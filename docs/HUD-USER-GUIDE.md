# WayangOS HUD — operator guide

The three consoles — **`wayang`** (the OS CLI), **`wayang-fw`** (firewall) and
**`wayang-router`** (router) — share one interface: the same keys, the same
chrome, the same panels. They all render through the shared component library
`wayang-tui`, so **if you learn one, you know the other two**. This guide is for
operators and field engineers, including those coming from Winbox / FortiGate /
Cloudflare.

> Design source of truth: `docs/TUI-UX-REVAMP.md`. Nothing here needs the shell
> — the HUD is fully keyboard-driven.

## 1. Opening a console

```sh
wayang            # the OS console (system, updates, network, wifi, ssh, firewall, router)
wayang-router     # the router (interfaces, routes, DHCP, VPN, QoS, BGP)
wayang-fw         # the firewall (zones, policies, NAT, objects, DoS, logs)
```

Useful flags (all three): `--demo` (sample data, touches nothing), `--screens DIR`
(render every screen to a file — for documentation/CI), `--light`, `--plain`
(ASCII glyphs — for fonts without box-drawing), `--no-splash`. Colour mode:
`NO_COLOR=1`, or `<TOOL>_COLOR=ansi|truecolor|mono` (e.g. `WAYANG_ROUTER_COLOR`),
or `WAYANG_TUI_COLOR` for any of them.

Non-interactive use is unaffected: piping or redirecting a command (e.g.
`wayang status | head`) just prints text — the HUD only starts on a real TTY.

## 2. What you see

```
 ┌ header:  BRAND · breadcrumb (MODULE ▸ TAB ▸ item) · state badge · revision ┐
 ├ tab row: STATIC │ KERNEL │ UPLINKS │ BGP/OSPF │ POLICY        (←→)          ┤
 │  the selected tab owns the whole body (full screen)                        │
 │  … and, when a row is selected, a shared DETAIL strip at the bottom        │
 └ footer:  the keys that matter right now            status message ──────────┘
```

* **Splash.** On start you see a short loading screen (logo + `loading …` +
  spinner) — never a blank flash. It disappears as soon as the first data is in.
* **Breadcrumb** always tells you where you are: `MODULE ▸ TAB ▸ item`.
* **State badge**: `✔ IN SYNC` / `▲ DRIFT` / `✖ CONFIRM 42s` / `NO CONFIG` /
  `PREVIEW` (not root). Always shown as a glyph **and** a word, so it reads with
  or without colour.
* **Tab row**: every screen that has more than one view shows it at the top.
* **Footer** shows the keys for the current screen (about six), never the whole
  key map — that lives behind `?`.

## 3. Navigation — the one model

| Key | Does |
|---|---|
| `↑` `↓` (or `j` `k`) | move the selection, or the field while editing |
| `←` `→` | **switch tab** (the selected tab fills the screen) |
| `tab` / `shift-tab` | next / previous tab (and next / previous field while editing) |
| `PgUp` `PgDn` `Home` `End` (`g` `G`) | page / first / last in long lists |
| `enter` | open · edit · **stage** (see §5) · confirm |
| `space` | toggle (enable, multi-select) — never commits |
| `esc` | back one level; **at the home deck it quits** |
| `b` | back to the home deck |
| `1`–`9` | jump to module N; `0` or the EXIT entry quits |
| `/` | quick jump — fuzzy-find a module, tab or **object** by name |
| `?` | help: every key, plus the “coming from…” concept map (§9); `p` writes it to `/data/var/<tool>/keys.txt` |
| `q` | quit (if a commit is pending it asks you to confirm/roll back first) |
| `y` / `n` | answer a pending commit (keep / roll back) |
| `Ctrl-L` (or `r`) | **repaint** the screen (see §8) |

**One tab = one full-screen view.** The selected tab owns the whole body; you
never have to look for “where the content is” on this page — it is always the
same model. If a row has details, an identical **DETAIL** strip appears at the
bottom, on every screen.

**Focus is always obvious.** Exactly one pane owns the keyboard; it is drawn in
the accent colour with a `▸` prefix, the rest is dimmed. Without colour, the `▸`
is the whole signal.

## 4. Editing: form or code

Every editor header says which you are in:
* `EDIT: form` — guided fields; pickers (`enter`) for references (interfaces,
  zones, prefix lists) so you cannot typo a name; inline validation.
* `EDIT: code (TOML)` — the whole object as config; validated the same way.

Where both exist, `f` toggles the same object between them. `check` never lets a
bad candidate stage.

## 5. Review before apply, and commit-confirm

Nothing changes silently:

1. You edit (or run a wizard) and press `enter` →
2. **REVIEW**: you are shown the *exact* effect — the rendered firewall rule,
   the route/`ip` commands, or the candidate diff. `enter` stages it, `esc`
   cancels.
3. Press `c` to **commit**; `y` keeps it, `n` rolls back. A commit left
   unanswered **rolls itself back** (safety), so a bad change cannot lock you
   out.

The router's default path is owned by the health-checked uplinks; if a change
would move the primary/default route, REVIEW says so and asks — you can decline
and still save the object. `[primary]` marks the current default; `p` promotes
one on purpose.

## 6. Cheat-sheets

**`wayang`** — `01 SYSTEM · 02 UPDATES · 03 NETWORK · 04 WIFI · 05 SSH ·
06 DCHECK · 07 FIREWALL · 08 ROUTER`. The last two hand the terminal to
`wayang-fw` / `wayang-router`. Updates are A/B (safe rollback); `wayang reset`
returns the box to defaults (with a REVIEW).

**`wayang-fw`** — `POLICIES` (`a` opens the **policy wizard**: from → to →
service → source → destination → action → NAT → log → preview of the rendered
rule), `DOS`, `NAT` (source / port-forward / pool), `OBJECTS` (addresses,
services, FQDNs), `LOGS` (windows + traffic). `i` opens the **FortiOS import
review** after `wayang-fw import-fortios`.

**`wayang-router`** — `INTERFACES`, `ROUTES` (static / kernel / **uplinks** /
BGP-OSPF / policy), `DHCP`, `VPN` (WireGuard), `QOS`, `SYSTEM`, `COMMIT`,
`HISTORY`. `a` on UPLINKS starts the **WAN wizard**; `/` jumps straight to any
object.

## 7. Colour, light and accessibility

The palette is one shared set (neon for truecolor, a 16-colour fallback for
`TERM=linux`/older terminals, mono for `NO_COLOR`). `--light` flips to the light
palette. Every state is carried by **glyph + word + weight**, so the HUD is
readable with no colour at all; the active tab and the focused pane use a `▸`
glyph, and the list cursor is a filled row (a `>` in plain/mono).

## 8. Troubleshooting

* **UI looks garbled / characters shifted.** Something else wrote to the
  terminal (a boot message, a service log). The HUD repaints itself on a slow
  tick and on any key that needs it; press **`Ctrl-L`** (or `r`) to force a
  clean repaint immediately.
* **A blank flash on open, especially over SSH / on a light terminal.** The
  first frame is the splash and the screen is cleared in the theme colour (no
  white flash). If you still see a flash, you are on an older build — update.
* **Handing off between tools** (`wayang` ↔ firewall/router) shows a short
  `▸ launching …` frame, not a blank screen.
* **Piping output** works normally (`wayang status | head`); the HUD does not
  start unless stdin and stdout are both TTYs.
* **No colours** — set `NO_COLOR=1` or `--plain`; nothing is lost.

## 9. Coming from Winbox / RouterOS, FortiGate, Cloudflare

| You know | Here |
|---|---|
| RouterOS `IP > Addresses` | ROUTER ▸ INTERFACES |
| RouterOS `IP > Routes` (default `0.0.0.0/0`) | ROUTER ▸ ROUTES, and the UPLINKS default (`[primary]`) |
| RouterOS `IP > DHCP Server` | ROUTER ▸ DHCP |
| RouterOS `Interfaces > WireGuard` | ROUTER ▸ VPN |
| RouterOS `IP > Firewall > Filter/NAT` | FW ▸ POLICIES / NAT |
| RouterOS `Queues` | ROUTER ▸ QOS |
| FortiGate `Policy & Objects > Firewall Policy` | FW ▸ POLICIES (wizard) |
| FortiGate `Network > Interfaces/Routes` | ROUTER ▸ INTERFACES / ROUTES |
| Cloudflare `DNS / WAF / Firewall rules` | FW (+ the planned WAF, see `docs/WAF.md`) |
| a GUI “Apply/Commit” | `enter` stages → `c` commits → `y` confirms |

## 10. For enterprise operators

* **Everything is config-as-code.** The HUD writes one `config.toml`; the same
  file can be reviewed in git and applied with `check → plan → commit FILE →
  confirm` from CI. The HUD and the CLI produce the same revisions.
* **Safe by construction**: REVIEW + commit-confirm + rollback + history; a
  monitored box never strands itself.
* **Fleet**: the same binaries run on every unit; configs are rendered per site
  by wayangi and shipped as signed bundles. A product **API** (`api` subcommand,
  loopback + token) is being added so an orchestrator can drive a box without
  SSH — see `docs/PRODUCT-API.md`.
