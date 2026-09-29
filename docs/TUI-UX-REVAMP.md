# TUI UX revamp — plan (dcheck · wayang-fw · wayang-router)

Status: **in progress** (2026-09-30). Scope (owner decision): the **`wayang`
CLI HUD**, **`wayang-fw`** and **`wayang-router`**. **`dcheck` is deferred** —
it stays the historical look-and-feel reference.

### Status

| HUD | repo | state |
|---|---|---|
| `wayang` CLI | `wayangos` (`wayang/src/`) | ✅ P0 `ca49082` · P1 `8b407da`/`eaa92b9`/`655c82c` · visual+focus `38b368d`/`a797448`/`faf9d4d` (206 tests) |
| `wayang-fw` | `wayang-fw` | ✅ P0 `c9dd87f` (0.4.0) · P1 `b764f78` (0.4.1) · focus ⏳ |
| `wayang-router` | `wayang-router` | ✅ P0 `a8bc823` · P1 `af4ad8b` (0.4.1) · focus `d0ee09e` (**0.4.2**) |
| `dcheck` | `dcheck` | ⏸ deferred |

**Visual spec + focused-pane highlight** (§5) are canonical here. The **shared
component library [`wayang-tui`](https://github.com/dalang-io/wayang-tui)** has
been created (scaffold); the next step extracts `theme`/`widgets`/`focus`/
`overlays` from the three products into it and has each product depend on it (so
consistency is *by construction*). A **product API** (`api` subcommand per tool)
is designed in [`docs/PRODUCT-API.md`](PRODUCT-API.md).

Released: **wayang-router v0.4.1**, **wayang-fw v0.4.1** (tags + GitHub releases +
mirror; OS pins bumped; `v0.4.2` for router's focus wave). `wayang` CLI ships
inside WayangOS (no separate tag); installed on the test box for manual testing.

## 1. Why

The HUDs are fast for power users but hostile to the two audiences we now sell
to:

* **Operators migrating from GUI products** — MikroTik **Winbox**, FortiGate
  (FortiOS GUI/CLI), Cloudflare dashboard. They expect visible menus, tabs,
  wizards, and to see *what will happen before it happens*.
* **Field agents / installers** who are not CLI-native and work over a serial
  console or SSH on a small screen.

Observed problems (from real use, 2026-09-29):

1. **Hidden side effects.** "Apply static" (address/interface) also changes the
   *primary/default* path, because the router **auto-derives the main default
   route** from the health-checked uplinks (`docs/CONFIG.md` §uplinks). A
   Winbox user expects "add address" and "add route" to be two separate acts;
   here one apply silently promotes an uplink to primary. Surprising and scary
   on a box that can cut your own SSH.
2. **Inconsistent navigation.** `←/→` switch panes in some modules (ROUTES,
   DHCP, VPN, QOS, NAT) but not module-to-module; `tab` sometimes cycles panes,
   sometimes does nothing; there is no single mental model ("left/right = where,
   up/down = what").
3. **Shortcuts are invisible.** Keys vary per screen and are only in a footer
   line and `?`; nothing marks the action keys inline, so people don't discover
   (`tab`, `v`, `space`, `J/K`, …).
4. **Forms vs code.** Some objects are form-edited, others "edit as code"
   (OSPF, uplinks, policy routes). The user cannot tell which, and a code edit
   has no in-place validation feedback.
5. **Commit semantics are subtle.** `enter`/`a` stage an *edit*; `c` commits;
   a pending commit shows `CONFIRM Ns`. That is powerful but needs to be
   *visible and teachable*, not a surprise.
6. **No search / jump / wizard.** Reaching "the DHCP pool for eth0" takes many
   keystrokes with no `/`-jump or fuzzy find; there is no "set up a WAN" flow.

## 2. Design principles

1. **One mental model.** `↑↓` = within the current list/field group; `←→` =
   between tabs/panes **everywhere**; `enter` = open/activate; `esc` = back;
   `1–9` = module; `?` = all keys. Never overload these.
2. **No hidden side effects.** An apply changes exactly what its title says; if
   it will also change the default route / primary uplink / NAT, that is a
   **separate, named step** and it is spelled out in the pre-apply summary.
3. **Preview before apply.** Every staging shows a **"what will change"**
   summary (the plan/diff lines) with `enter=stage`, `esc=cancel`; commit stays
   a deliberate second act (`c` → `y`), with the confirm window visible.
4. **Discoverable.** Action keys are shown **on the row/field** (dim keycaps)
   and in a context footer; `?` opens a full, scrollable, printable reference.
5. **Forgive.** Destructive actions arm then act (keep); every commit is
   rollback-able and the rollback window is on screen; `esc` never commits.
6. **Teach the migrant.** Offer the GUI concept next to ours ("RouterOS
   `IP > Routes` → ROUTES > STATIC") in the `?`/help pane and in wizards.

## 3. Navigation grammar (universal)

| Key | Action (every module) |
|---|---|
| `↑` `↓` | move the selection (list) / field (form) |
| `←` `→` | previous / next **tab** (or pane) — never moves between modules |
| `tab` / `shift-tab` | next / previous tab **and** field while editing |
| `PgUp` `PgDn` `Home` `End` | page / first / last in long lists |
| `enter` | open · edit · activate · confirm |
| `space` | toggle (enable, multi-select) — never commits |
| `esc` | back / close / cancel (never commits) |
| `b` | back to the command deck |
| `1–9` | jump to module N; `0` = EXIT |
| `/` | **quick jump / find** (fuzzy over modules, objects, screens) |
| `?` | help: all keys + concept map |
| `q` | quit (with a guard when a commit is pending) |
| `y` `n` | answer a pending commit (confirm / roll back) |

**Breadcrumb.** The header always shows `MODULE ▸ TAB ▸ (item)`; the footer
shows only the 3–6 keys relevant to the current context, right-aligned status.

**Tab row.** Every multi-pane module renders a visible tab row
(`STATIC │ KERNEL │ BGP │ OSPF`) with the active tab highlighted and the keys
shown; `←→` moves it. ROUTES/DHCP/VPN/QOS/NAT already have panes — give them
all the same tab row (some currently hide it).

**Command deck.** MODULES list stays, but: show the module's key on the row,
add a **RECENT/QUICK** strip (last edited objects, pending commit), and let `/`
jump.

## 4. Discoverability

* **Inline keycaps**: list rows and form fields render their action key dimmed
  at the right (e.g. `enter edit · space on/off · d delete`).
* **Context footer**: max 6 keys, the most relevant first; overflow goes to `?`.
* **`?` overlay**: a two-pane help — left = keys (global, list, form, commit),
  right = **concept map** for migrants (§7). Scrollable; `p` prints to a file
  (`/data/var/<tool>/keys.txt`) for a wall chart.
* **Empty states** say what to press ("no policies yet — `a` to add, or import
  from FortiOS: `wayang-fw import-fortios`").

## 5. Layout / chrome

```
 ┌ header: BRAND │ breadcrumb (MODULE ▸ TAB ▸ item) │ state badge │ rev ┐
 ├ tabs:   STATIC │ KERNEL │ BGP │ OSPF        (←→)                     ┤
 ├ list (top)  |  detail / target (right or bottom)  ── consistent split ┤
 ├ form when editing: FIELDS with focus order + inline validation        ┤
 └ footer: keycaps (context)                         status message ─────┘
```

Rules: one split layout (list + target) everywhere; the TARGET panel is always
the same place; a modal form replaces the list area (not the whole screen) so
context stays visible. Wide (≥120) vs narrow (80, e.g. serial) layouts defined
per screen; charts fall back to half-blocks on `TERM=linux` (already done).

### Visual spec (canonical — all three HUDs must match)

Palette (neon_dark, 24-bit; ansi/mono fallbacks; `<TOOL>_COLOR`/`NO_COLOR`/`--light`/`--mono`):
`accent (0,229,255) · accent2 (255,46,151) · dim (112,132,152) · border (28,74,92) ·
ok (57,255,136) · warn (255,176,0) · bad (255,59,59) · fg (196,210,224) · bg (10,14,20) ·
bar (18,26,38) · selection (bg 74,14,52 / fg white) · on_badge (10,14,20)`.

Chrome (one implementation):
* **Panel**: thin border in `border`; **heavy corners** `┏ ┓ ┗ ┛` in `accent`; title
  `◢ TITLE ◣` (title `accent`+bold, chevrons `accent2`); optional right-aligned title.
  The CLI's `▐ TITLE ▌` title is **non-canonical** — it must become `◢ TITLE ◣`.
* **Caption** inside a panel: `── TEXT ─────` (`border` lead/fill, `accent` text).
* **Keycaps**: ` key ` on `bar` bg + `accent` fg, label in `dim`.
* **Header**: `bar` bg — brand · **breadcrumb `MODULE ▸ TAB ▸ item`** · state badge
  (glyph **and** word, never colour alone) · revision.
* **Footer**: context keycaps + right-aligned status.
* **Selection row**: `selection` (reverse); in mono, a `>` marker + bold.
* **Badges/status**: glyph + word; mono renders `[✔ OK]`.

**Focused-pane highlight (new — every HUD).** Exactly one pane owns the keyboard at a
time; it is unmistakable without colour:
* the **focused** pane's border + title are drawn in `accent`; **unfocused** panes keep
  `border`/dim titles;
* the focused pane's title is prefixed `▸ ` (unfocused: none);
* the active **tab** already carries `▸`; a focused **list** shows the `selection` row that
  moves with `↑↓`; a focused **form** shows the field cursor;
* in mono/`--plain`, focus is `▸` + bold only (works with no colour).

Scope of "panes": the command **deck** (MODULES list focused; preview card dim) and each
module's split (main list vs TARGET/detail) and tab groups. A modal/form takes focus when
open.

## 5b. No-flash startup & cross-app handoff

**Problem (reported).** Opening a HUD flashes a blank screen first — worst on a
**light** terminal and over **SSH**. Two causes:

1. Entering the alternate screen clears it, and the first *real* frame is only
   drawn after data is sampled (proc reads, nft/wg queries, leases) → an empty
   frame shows for a beat. On a light terminal that empty frame is white; over
   SSH the latency makes it obvious.
2. **Handoff between tools** (`wayang` → FIREWALL/ROUTER, and back) briefly
   returns to the normal screen between the two TUIs → a "flashblank".

**Fix — one shared rule, so all three behave the same:**

1. **First frame is the splash.** The very first thing after entering the
   alternate screen is a *loading* frame — brand logo + `loading <tool>…` +
   spinner + the breadcrumb — drawn **before** any sampling. Never an empty alt
   screen. Data loads after (or in the background) and only then replaces it.
   Same on return from a child tool.
2. **Theme the clear.** Set the terminal background with **OSC 11** to the
   palette `bg` at startup (and restore on exit); clear with the `bg` fill, not
   `Reset`. The clear is then theme-coloured, never a white flash (light mode /
   SSH).
3. **Handoff transition.** The parent draws `▸ launching <tool>…` and keeps the
   alternate screen until the child has drawn its own splash; the child's first
   frame is its splash; the parent draws a `loading …` frame on return. Target:
   no full-screen blank at any point.
4. Ship this as a shared **`splash` / `transition` component in `wayang-tui`**
   so the three tools are identical here too.
5. The splash stays bounded and **never blocks** — it must not delay the first
   keypress handling or add startup latency.

## 5c. Terminal robustness against external output

**Problem (reported).** While the HUD owns the terminal, the OS or another
process writes to the tty (kernel printk, a service/systemd-less init log, a
child the HUD spawned, `wayang update` output) → the incremental renderer leaves
garbage on screen and the user must exit and re-enter to get a clean UI.

**Fix — one mechanism, all three tools:**

1. **Never leak child output.** Every `Command` the HUD spawns must set
   `stdout`/`stderr` to `Stdio::null()` (or pipe+capture when it reads them);
   long-running daemons are started detached with their output going to a log
   file (the init script's job), never to the shared tty.
2. **Self-healing redraw.** Force a full repaint (ratatui
   `Terminal::clear()` before the next `draw`) on: startup, `SIGWINCH`,
   **returning from a child tool/job**, and a **slow periodic tick** (≈2–5 s)
   so external junk is overwritten automatically without the user having to
   quit. Keep it cheap (repaint only when needed) to avoid flicker.
3. **Manual repaint.** Bind `Ctrl-L` (and `r` on read-only screens) to force a
   repaint.
4. Provide it in **`wayang-tui`** (a `Terminal`/repaint helper + the spawn
   helper) so the three HUDs behave identically.

This also makes the HUD survive WayangOS's boot chatter when a console HUD is
opened mid-boot.

## 6. Forms & apply semantics (fixes issue #1)

* **Stage vs commit stays**, but make it explicit and teachable:
  `edit → [ REVIEW changes ] → enter stages (candidate) → c commits → y`. The
  REVIEW step lists the exact plan lines (from `plan`), not a vague "ok".
* **Split derived effects.** Because the router owns the default route:
  * Editing/adding an interface or an `[[uplink]]` shows a line
    `▸ this makes <name> the primary path (default route) — keep? [Y/n]`
    *only* when it actually changes the primary (health/preference), and the
    user can decline (the uplink is still created, just not promoted).
  * Provide an **explicit control** `[primary]` / `[make default]` on the
    uplink row so promotion is an intentional act, matching RouterOS's separate
    `IP > Routes` default (`0.0.0.0/0`).
* **Never auto-promote silently** on add/adopt. The plan/diff already reports
  the route change; surface it in the modal too.
* **Which editor?** Every object's form header says `EDIT: form` or
  `EDIT: code (TOML)`; a form and the code view must round-trip the same object
  (`f` toggles), with validation inline for both.
* **Validation** shows the field's error inline and blocks staging; a
  `caps.rs` refusal (kernel-gated feature) is explained in plain words, with a
  link to the lab/QEMU note — not a bare "refused".

## 7. Flows to redesign (concrete)

**wayang-router**
* INTERFACES: add/adopt → pick port → mode (`dhcp|static`) → addresses →
  **REVIEW** (shows the route/default change explicitly). `v` VLAN, `space`
  enable, `d` delete stay; add `[primary]` marker on the uplink/view.
* ROUTES: keep tabs; add a **default-route row** that shows *which uplink owns
  it and why* (health/preference/weight), and `enter` explains + lets you
  change preference.
* VPN: peer config flow → show the wg-quick text with `w` write and `q` QR.
* DHCP: `s` make-static → REVIEW the lease→reservation conversion.
* COMMIT: rename the mental model in the UI: `PLAN` (what would run) vs
  `DIFF` (vs active) vs `COMMIT`; the header badge already helps.

**wayang-fw**
* POLICIES: `a` opens a **guided policy wizard**: from zone → to zone →
  service → source/dest → action → NAT → log/schedule, one step per screen,
  a live preview of the rendered `nft` rule at the end. This is the Winbox
  "Firewall > Filter" flow people know.
* NAT: separate tabs for SNAT / VIP-port-forward / IP-pool (already) with the
  same tab row + `←→`.
* DoS / OBJECTS / LOGS: tab rows + inline keycaps; OBJECTS add flow uses
  pickers (already) with `enter`/`space` explicit.
* FortiOS import: after import, land on a **REVIEW** screen listing what mapped
  and what was dropped (already reports; make it a navigable diff).

## 8. GUI migrant concept map (in `?` and wizard sidebars)

| They know (Winbox / FortiGate / Cloudflare) | Here |
|---|---|
| RouterOS `IP > Addresses` | INTERFACES (addresses on the interface) |
| RouterOS `IP > Routes` + default `0.0.0.0/0` | ROUTES, and the **uplink's default** (see #6) |
| RouterOS `IP > DHCP Server` | DHCP (pools + leases) |
| RouterOS `Interfaces > WireGuard` | VPN |
| RouterOS `IP > Firewall > Filter/NAT` | wayang-fw POLICIES / NAT |
| RouterOS `Queues` | QOS |
| RouterOS `PPP` / `L2TP` | (roadmap v0.8) |
| FortiGate `Policy & Objects > Firewall Policy` | wayang-fw POLICIES (wizard) |
| FortiGate `Network > Interfaces/Routes` | INTERFACES / ROUTES |
| FortiGate `VPN > IPsec` | IPsec (roadmap v0.4) |
| Cloudflare `DNS / WAF / Firewall rules` | wayang-fw + WAF (roadmap) |
| "commit"/"apply" in a GUI | `enter` stage → `c` commit → `y` confirm |

## 9. Shared kit (implementation notes)

* `theme.rs` and `widgets.rs` are currently **copied** between the three repos
  — extract a small shared crate (or a vendored `tui-kit`) so the navigation
  grammar, tab row, keycap hints, REVIEW modal and `?` overlay are implemented
  **once**. dcheck adopts it first to prove it.
* Keep it dependency-light (the current custom renderer over crossterm); no
  ratatui migration required, but the kit defines the widgets.
* Snapshot tests already exist (`snapshot` renders SVG/TXT) — add keymap tests
  (every screen has a tab row where it has panes; every row's key works) so the
  model cannot regress.

## 10. Phases & acceptance

* **P0 — navigation grammar + discoverability** (both products): universal
  `↑↓/←→/tab/enter/esc/1–9//?`, visible tab rows, inline keycaps, `?` overlay,
  breadcrumb. Accept: every module has a tab row iff it has panes; `←→` never
  leaves a module; `?` lists all keys.
* **P0 — apply semantics** (router first): REVIEW-before-stage with the exact
  plan; **no silent primary promotion**; explicit `[primary]`; inline caps
  explanation. Accept: adding a static WAN never changes the default route
  without an explicit confirmation line.
* **P1 — wizards & migrant help**: policy wizard (fw), WAN wizard (router),
  REVIEW-on-import, concept map everywhere. Accept: a Winbox user can set up a
  WAN and a filter policy without prior keys, guided on screen.
* **P1 — search/jump**: `/` fuzzy jump; RECENT strip on the deck.
* **P2 — shared kit extraction** + dcheck adoption; printable key chart.
* **P2 — accessibility**: high-contrast/mono themes verified; color-blind-safe
  badges (state by glyph, not only colour).

## 11. Open questions

1. Review modal: reuse the COMMIT screen's plan renderer (yes?) — one renderer,
   two entry points.
2. `[primary]` model: is it per-`[[uplink]]` preference only, or an explicit
   `[[route]] default` the user can pin? (CONFIG.md currently makes a second
   default an error when uplinks exist.)
3. Wizards: in-TUI only, or also mirrored as `wayang-router/ fw … --wizard` for
   scripts?
4. Shared kit as a new `dalang-io/wayang-tui` crate vs vendored per repo.
5. Localisation: the HUD is English; do we need Bahasa for field techs?
