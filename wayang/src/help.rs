//! The `?` help overlay: every key, grouped (global / list / form / confirm),
//! next to the GUI-migrant concept map (TUI-UX-REVAMP §8). Scrollable, and `p`
//! writes the same reference to `/data/var/wayang/keys.txt` for a wall chart.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::hud::{self, Theme};
use crate::paths;

/// Global keys, valid on every screen.
pub const GLOBAL: &[(&str, &str)] = &[
    ("↑ ↓ j k", "move the selection / field"),
    ("← →", "previous / next tab or pane"),
    (
        "tab / shift-tab",
        "next / previous tab (and field while editing)",
    ),
    ("PgUp PgDn", "page a long list"),
    ("Home End", "first / last item"),
    ("enter", "open · edit · activate · confirm"),
    ("space", "toggle (never commits)"),
    ("esc", "back / close / cancel"),
    ("b", "back to the command deck"),
    ("1 - 8", "jump to module 01 system … 08 router"),
    ("0", "exit"),
    ("/", "quick jump: a module, or an item on this screen"),
    ("?", "this reference"),
    ("q", "quit / back (guarded while an action is pending)"),
];

/// Actions that act on the current list row.
pub const LIST: &[(&str, &str)] = &[
    ("enter", "open / edit the selected row"),
    ("a", "add a new object"),
    ("d / delete", "remove the selected object"),
    ("r", "reload from disk / re-scan"),
    ("u / d", "link up / down (network)"),
    ("space", "toggle enable (never commits)"),
];

/// Actions while editing a form or a text field.
pub const FORM: &[(&str, &str)] = &[
    ("enter", "submit the field"),
    ("tab / shift-tab", "next / previous field"),
    ("esc", "cancel the edit (nothing is written)"),
    ("backspace", "delete a character"),
];

/// The REVIEW / commit step before anything mutates the box.
pub const CONFIRM: &[(&str, &str)] = &[
    ("enter / y", "confirm and apply"),
    ("esc / n / q", "cancel (nothing changes)"),
    ("↑ ↓", "scroll the plan"),
];

/// "They know (Winbox / FortiGate / Cloudflare) → Here" (TUI-UX-REVAMP §8).
pub const CONCEPT: &[(&str, &str)] = &[
    (
        "RouterOS IP > Addresses",
        "NETWORK (addresses on the interface)",
    ),
    (
        "RouterOS IP > Routes / default",
        "ROUTES, and the uplink's default",
    ),
    ("RouterOS IP > DHCP Server", "DHCP (pools + leases)"),
    ("RouterOS Interfaces > WireGuard", "VPN"),
    ("RouterOS IP > Firewall Filter/NAT", "FIREWALL → wayang-fw"),
    ("RouterOS Queues", "QOS"),
    (
        "FortiGate Policy & Objects > Policy",
        "FIREWALL → wayang-fw POLICIES",
    ),
    ("FortiGate Network > Interfaces", "NETWORK / ROUTER"),
    ("Cloudflare DNS / WAF / rules", "wayang-fw + WAF (roadmap)"),
    ("GUI commit / apply", "enter = REVIEW → confirm"),
];

fn group(title: &str, items: &[(&str, &str)], width: u16, t: &Theme) -> Vec<Line<'static>> {
    let mut out = vec![hud::caption(title, width, t)];
    for (k, d) in items {
        out.push(Line::from(vec![
            Span::raw(format!("  {k:<16}")),
            Span::raw(d.to_string()),
        ]));
    }
    out.push(Line::from(""));
    out
}

/// The key reference as plain text (for `p` → keys.txt).
pub fn keys_text(screen: &str) -> String {
    let mut s = format!("WAYANG OS — key reference ({screen})\n\n");
    for (title, items) in [
        ("GLOBAL", GLOBAL),
        ("LIST", LIST),
        ("FORM", FORM),
        ("CONFIRM", CONFIRM),
    ] {
        s.push_str(&format!("{title}\n"));
        for (k, d) in items {
            s.push_str(&format!("  {k:<16} {d}\n"));
        }
        s.push('\n');
    }
    s.push_str("CONCEPT MAP (Winbox / FortiGate / Cloudflare -> here)\n");
    for (they, here) in CONCEPT {
        s.push_str(&format!("  {they:<34} -> {here}\n"));
    }
    s
}

/// Write the reference to `/data/var/wayang/keys.txt`.
pub fn print_keys(screen: &str) -> Result<std::path::PathBuf, String> {
    let dir = paths::data_var_dir().join("wayang");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("keys.txt");
    std::fs::write(&path, keys_text(screen)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Draw the overlay centered in `body`; `scroll` scrolls the key column.
pub fn draw(f: &mut Frame, body: Rect, t: &Theme, screen: &str, scroll: usize) {
    let r = hud::centered(body, 96, 22);
    f.render_widget(ratatui::widgets::Clear, r);
    let inner = hud::panel_focused(
        f,
        r,
        &format!("HELP — {screen} — KEYS & CONCEPT MAP"),
        None,
        t,
    );
    // Reserve the last row for the footer so it never overwrites content.
    let content = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let cols =
        Layout::horizontal([Constraint::Percentage(52), Constraint::Percentage(48)]).split(content);

    let mut keys: Vec<Line> = Vec::new();
    for (title, items) in [
        ("GLOBAL", GLOBAL),
        ("LIST", LIST),
        ("FORM", FORM),
        ("CONFIRM", CONFIRM),
    ] {
        for l in group(title, items, cols[0].width, t) {
            keys.push(l);
        }
    }
    let room = cols[0].height as usize;
    let start = scroll.min(keys.len().saturating_sub(1));
    let end = (start + room).min(keys.len());
    let left: Vec<Line> = keys[start..end].to_vec();
    f.render_widget(Paragraph::new(left).wrap(Wrap { trim: false }), cols[0]);

    let mut right = vec![
        Line::from(Span::styled("THEY KNOW", t.palette.bold(t.palette.accent))),
        Line::from(""),
    ];
    for (they, here) in CONCEPT {
        right.push(Line::from(Span::styled(
            they.to_string(),
            t.palette.fg(t.palette.dim),
        )));
        right.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{} ", t.ui.arrow()),
                t.palette.fg(t.palette.accent2),
            ),
            Span::styled(here.to_string(), t.palette.fg(t.palette.fg)),
        ]));
    }
    f.render_widget(Paragraph::new(right).wrap(Wrap { trim: false }), cols[1]);

    if inner.height >= 2 {
        let footer = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        let hint = Line::from(Span::styled(
            format!(
                "↑↓ scroll   p print to {}",
                paths::data_var_dir().join("wayang/keys.txt").display()
            ),
            t.palette.fg(t.palette.dim),
        ));
        f.render_widget(Paragraph::new(hint), footer);
    }
}

/// Whether `key` closes the overlay (anything but scroll / print).
pub fn closes(code: ratatui::crossterm::event::KeyCode) -> bool {
    use ratatui::crossterm::event::KeyCode;
    !matches!(
        code,
        KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Char('p')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_text_has_every_group_and_the_map() {
        let s = keys_text("DECK");
        for want in [
            "GLOBAL",
            "LIST",
            "FORM",
            "CONFIRM",
            "CONCEPT MAP",
            "Winbox",
            "REVIEW",
        ] {
            assert!(s.contains(want), "missing {want}:\n{s}");
        }
    }

    #[test]
    fn scroll_keys_do_not_close() {
        use ratatui::crossterm::event::KeyCode;
        assert!(!closes(KeyCode::Down));
        assert!(!closes(KeyCode::Char('p')));
        assert!(closes(KeyCode::Esc));
        assert!(closes(KeyCode::Char('x')));
    }
}
