//! Look & feel for the `wayang` CLI HUD.
//!
//! The shared chrome — theme/palette/glyphs, bracket panels, captions, keycaps,
//! badges, fields and the pixel logo — now comes from the `wayang-tui` crate
//! (`wayang_tui::theme` / `wayang_tui::widgets`), so the CLI renders through
//! the *same* code as wayang-fw / wayang-router and matches them **by
//! construction** (canonical spec: `docs/TUI-UX-REVAMP.md` §5). The old
//! `Mode::Console` blue palette and the OSC palette rewrite are gone: the
//! crate's console/ansi handling is canonical.
//!
//! What stays here is genuinely CLI-specific and is marked as such:
//!
//! * [`Tone`] — the three-way severity model the deck/sub-screens use (the
//!   crate speaks in `u8` severities, mapped in [`Tone::sev`]).
//! * [`status_row`] — the one-row spinner + badge + idle-hint line.
//! * [`spinner`] / [`progress_bar`] — the ASCII spinner and the sweeping
//!   indeterminate bar (the kernel console font has them; the crate's spinner
//!   is braille and would change the checked-in screens).
//! * [`tab_row`] — the `◢ ACTIVE ◣ │ dim` tab grammar with its `←→` hint.
//! * [`line_with_hint`] — a left line plus a right-aligned dim hint.
//! * [`brand`] — the `▰` A/B slot / installed marker.
//! * [`header_bar`] / [`footer`] — thin adapters that keep the CLI's exact
//!   layout (revision on the *left*; a one-column footer gutter) over the
//!   crate's [`header`](wayang_tui::widgets::header) and keycaps.
//!
//! Everything else is re-exported so call sites keep reading `hud::…`.

use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

// Product identity, palette, glyphs and the resolved theme come from the shared
// crate; re-exported so the CLI's call sites keep reading `hud::…`.
#[allow(unused_imports)]
pub use wayang_tui::theme::{App, ColorMode, Flags, Palette, Theme, Ui, WAYANG_OS};
// The CLI uses the focused / unfocused panel, caption, keycaps, field, badge
// and the `Header` builder; the rest are available through the same crate and
// re-exported for parity with the shared module map.
#[allow(unused_imports)]
pub use wayang_tui::widgets::{
    badge, caption, centered, field, gauge, gauge_cells_for, header, keycaps, logo, logo_lines,
    panel, panel_focused, selection_row, status, Header,
};

// Focus rings (`wayang_tui::focus`) and the modal frame (`wayang_tui::overlay`)
// are used by the shared HUDs; the CLI keeps its own modal composition for now
// but stays one import away.
#[allow(unused_imports)]
pub use wayang_tui::{focus, overlay};

/// Resolve the CLI theme from the environment. The CLI has no display flags of
/// its own, so this is the crate's default `Flags` plus `NO_COLOR` /
/// `WAYANG_OS_COLOR` / `WAYANG_TUI_COLOR` / `COLORTERM`.
pub fn detect() -> Theme {
    Theme::resolve(WAYANG_OS, Flags::default())
}

/// Outcome colour of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Bad,
}

impl Tone {
    /// Severity index understood by the shared palette and glyph set
    /// (0 ok, 2 warn, 3 bad).
    pub const fn sev(self) -> u8 {
        match self {
            Tone::Ok => 0,
            Tone::Warn => 2,
            Tone::Bad => 3,
        }
    }
}

/// Module status symbol + colour; `None` = not applicable (a dim `·`).
pub fn sev(t: &Theme, tone: Option<Tone>) -> Span<'static> {
    match tone {
        Some(tone) => {
            let s = tone.sev();
            Span::styled(t.ui.sym(s), t.palette.bold(t.palette.severity(s)))
        }
        None => Span::styled(t.ui.dot(), t.palette.fg(t.palette.dim)),
    }
}

/// The A/B slot / installed marker (`▰`; `#` with `--plain`).
pub fn brand(t: &Theme) -> &'static str {
    if t.is_plain() {
        "#"
    } else {
        "▰"
    }
}

/// The one-row header bar. The shared [`header`] renderer draws it, but the CLI
/// keeps its revision label on the *left*, next to the section
/// (` ◢◤ WAYANG OS // DECK ▸ SYSTEM  v1.4.1`), where the other HUDs place it on
/// the right — so it is folded into [`Header::subtitle`]. `right` is pre-built
/// (for the deck it already carries the slot badge).
pub fn header_bar(
    f: &mut Frame,
    area: Rect,
    section: &str,
    version: &str,
    right: Vec<Span<'static>>,
    t: &Theme,
) {
    let rev = if version.is_empty() {
        String::new()
    } else if version == "demo" {
        "  demo".to_string()
    } else {
        format!("  v{version}")
    };
    let subtitle = format!("{section}{rev}");
    let hdr = Header {
        subtitle: (!subtitle.is_empty()).then_some(subtitle.as_str()),
        right,
        ..Header::default()
    };
    header(f, area, &hdr, t);
}

/// One-row footer: the shared [`keycaps`] with the CLI's leading gutter.
pub fn footer(f: &mut Frame, area: Rect, keys: &[(&str, &str)], t: &Theme) {
    let mut line = keycaps(keys, t);
    line.spans.insert(0, Span::raw(" "));
    f.render_widget(Paragraph::new(line), area);
}

/// One status row above the keycaps: spinner while busy, then the last message
/// as a badge + text, or a dim hint when there is none.
pub fn status_row(
    f: &mut Frame,
    area: Rect,
    message: Option<(Tone, &str)>,
    busy: Option<&str>,
    idle: &str,
    t: &Theme,
) {
    let mut spans = vec![Span::raw(" ")];
    if let Some(sp) = busy {
        spans.push(Span::styled(
            format!("{sp} "),
            t.palette.bold(t.palette.accent2),
        ));
    }
    match message {
        Some((tone, text)) => {
            let label = match tone {
                Tone::Ok => "OK",
                Tone::Warn => "NOTE",
                Tone::Bad => "ERROR",
            };
            spans.push(badge(tone.sev(), label, t));
            spans.push(Span::styled(
                format!(" {text}"),
                t.palette.fg(t.palette.fg),
            ));
        }
        None => spans.push(Span::styled(
            idle.to_string(),
            t.palette.fg(t.palette.dim),
        )),
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Spinner frame for a tick (ASCII: works on the console font too).
pub fn spinner(tick: usize) -> &'static str {
    ["|", "/", "-", "\\"][tick % 4]
}

/// Indeterminate progress bar: a short block that sweeps back and forth across
/// `width` cells, driven by `tick`. Rendered with half/block glyphs so it reads
/// on the console font as well as truecolour terminals.
pub fn progress_bar(tick: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let seg = 4.min(width);
    let span = width - seg;
    let period = if span == 0 { 1 } else { span * 2 };
    let p = tick % period;
    let start = if span == 0 || p <= span { p.min(span) } else { period - p };
    (0..width)
        .map(|i| if i >= start && i < start + seg { '█' } else { '░' })
        .collect()
}

/// Visible tab row: the active tab is highlighted, the rest dim; `←→` moves it.
/// Returns the line so callers can place it (the caller owns the area).
pub fn tab_row_line(tabs: &[&str], active: usize, t: &Theme) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    let mark = t.focus_mark();
    for (i, name) in tabs.iter().enumerate() {
        if i == active {
            // Glyph + reverse/selection, never colour alone (`NO_COLOR` keeps `>`).
            spans.push(Span::styled(format!(" {mark} {name} "), t.palette.highlight()));
        } else {
            spans.push(Span::styled(format!("  {name}  "), t.palette.fg(t.palette.dim)));
        }
        if i + 1 < tabs.len() {
            spans.push(Span::styled("│", t.palette.fg(t.palette.border)));
        }
    }
    spans.push(Span::styled(
        format!("  {} ←→", t.focus_mark()),
        t.palette.fg(t.palette.dim),
    ));
    Line::from(spans)
}

/// Draw a one-row tab row in `area`.
pub fn tab_row(f: &mut Frame, area: Rect, tabs: &[&str], active: usize, t: &Theme) {
    f.render_widget(Paragraph::new(tab_row_line(tabs, active, t)), area);
}

/// Left spans followed by a dim, right-aligned inline keycap hint. On narrow
/// terminals (`left + hint` does not fit) the hint is dropped, never wrapped.
pub fn line_with_hint(
    left: Vec<Span<'static>>,
    hint: &str,
    width: usize,
    t: &Theme,
) -> Line<'static> {
    let lw: usize = left.iter().map(|s| s.width()).sum();
    let hw = hint.chars().count();
    if hint.is_empty() || lw + hw + 2 > width {
        return Line::from(left);
    }
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(lw + hw))));
    spans.push(Span::styled(hint.to_string(), t.palette.fg(t.palette.dim)));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ansi() -> Theme {
        Theme::from_env(WAYANG_OS, Flags::default(), false, Some("ansi"), None)
    }

    fn mono() -> Theme {
        Theme::from_env(WAYANG_OS, Flags::default(), true, None, None)
    }

    fn plain() -> Theme {
        Theme::from_env(
            WAYANG_OS,
            Flags {
                plain: true,
                ..Flags::default()
            },
            false,
            Some("ansi"),
            None,
        )
    }

    #[test]
    fn tone_maps_to_severity() {
        assert_eq!(Tone::Ok.sev(), 0);
        assert_eq!(Tone::Warn.sev(), 2);
        assert_eq!(Tone::Bad.sev(), 3);
    }

    #[test]
    fn sev_uses_the_shared_glyphs() {
        let t = ansi();
        assert_eq!(sev(&t, Some(Tone::Ok)).content, "✔");
        assert_eq!(sev(&t, Some(Tone::Warn)).content, "▲");
        assert_eq!(sev(&t, Some(Tone::Bad)).content, "✖");
        // Not applicable: the dim `·` and no colour.
        let none = sev(&t, None);
        assert_eq!(none.content, "·");
        // Mono keeps a glyph (never colour alone).
        assert_eq!(sev(&mono(), Some(Tone::Bad)).content, "✖");
    }

    #[test]
    fn brand_is_the_slot_marker() {
        assert_eq!(brand(&ansi()), "▰");
        assert_eq!(brand(&mono()), "▰", "mono is not --plain");
    }

    #[test]
    fn logo_has_two_rows_and_blocks() {
        let rows: Vec<String> = ansi().logo_lines().iter().map(|l| l.to_string()).collect();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains('█') || rows[0].contains('▀'), "{rows:?}");
    }

    #[test]
    fn header_bar_keeps_the_version_next_to_the_section() {
        // The CLI shows `// SECTION  vX` on the left, not on the right.
        let t = ansi();
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 1)).unwrap();
        term.draw(|f| {
            let area = f.area();
            header_bar(f, area, "COMMAND DECK ▸ SYSTEM", "1.4.1", vec![Span::raw("right")], &t);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..100).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(text.contains("◢◤ WAYANG OS // COMMAND DECK ▸ SYSTEM  v1.4.1"), "{text:?}");
        assert!(text.trim_end().ends_with("right"), "{text:?}");
        // `demo` is a label, never a fake `vdemo`.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 1)).unwrap();
        term.draw(|f| {
            let area = f.area();
            header_bar(f, area, "COMMAND DECK", "demo", Vec::new(), &t);
        })
        .unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..100).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(text.contains(" COMMAND DECK  demo"), "{text:?}");
        assert!(!text.contains("vdemo"), "{text:?}");
    }

    #[test]
    fn focus_is_a_glyph_not_colour() {
        // Fancy and mono both mark focus with `▸`; only `--plain` uses a plain
        // `>`, so focus never survives on colour alone.
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 3)).unwrap();
        term.draw(|f| {
            panel_focused(f, f.area(), "TARGET", None, &ansi());
        })
        .unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..3)
            .flat_map(|y| (0..30).map(move |x| buf[(x, y)].symbol().to_string()))
            .collect();
        assert!(text.contains("▸ TARGET"), "{text}");

        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 3)).unwrap();
        term.draw(|f| {
            panel_focused(f, f.area(), "TARGET", None, &mono());
        })
        .unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..3)
            .flat_map(|y| (0..30).map(move |x| buf[(x, y)].symbol().to_string()))
            .collect();
        assert!(text.contains("▸ TARGET"), "mono keeps `▸`: {text}");

        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 3)).unwrap();
        term.draw(|f| {
            panel_focused(f, f.area(), "TARGET", None, &plain());
        })
        .unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..3)
            .flat_map(|y| (0..30).map(move |x| buf[(x, y)].symbol().to_string()))
            .collect();
        assert!(text.contains("[ > TARGET ]"), "--plain uses `>`: {text}");
    }

    #[test]
    fn progress_bar_sweeps() {
        let a = progress_bar(0, 10);
        assert_eq!(a.chars().count(), 10);
        assert!(a.contains('█') && a.contains('░'));
        assert_ne!(a, progress_bar(3, 10), "the block moves with the tick");
        assert_eq!(progress_bar(0, 0), "");
    }

    #[test]
    fn tab_row_marks_the_active_tab_with_a_glyph() {
        let line = tab_row_line(&["A", "B"], 0, &mono()).to_string();
        assert!(line.contains("▸ A"), "mono keeps `▸`: {line:?}");
        assert!(line.contains("←→"), "{line:?}");
    }
}
