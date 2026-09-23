//! HUD building blocks: bracket panels, line gauges, badges, keycaps, logo.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use super::theme::{Palette, Ui};

/// Two-row half-block logotype (fancy) or a one-line ASCII title (plain).
pub fn logo(ui: Ui) -> &'static [&'static str] {
    if ui.plain {
        &["== D C H E C K =="]
    } else {
        &["█▀▄ █▀▀ █ █ █▀▀ █▀▀ █▄▀", "█▄▀ █▄▄ █▀█ ██▄ █▄▄ █ █"]
    }
}

pub fn logo_lines(ui: Ui, p: &Palette) -> Vec<Line<'static>> {
    logo(ui)
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let color = if i == 0 { p.accent } else { p.accent2 };
            Line::from(Span::styled(*row, p.bold(color)))
        })
        .collect()
}

/// Draw a HUD panel (thin frame, heavy accent corners, `◢ TITLE ◣`) and
/// return its inner area. `right` is an optional right-aligned title.
pub fn panel(
    f: &mut Frame,
    area: Rect,
    title: &str,
    right: Option<Line<'static>>,
    p: &Palette,
    ui: Ui,
) -> Rect {
    let title_line = if ui.plain {
        Line::from(Span::styled(format!("[ {title} ]"), p.bold(p.accent)))
    } else {
        Line::from(vec![
            Span::styled("◢ ", p.fg(p.accent2)),
            Span::styled(title.to_string(), p.bold(p.accent)),
            Span::styled(" ◣", p.fg(p.accent2)),
        ])
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_set(ui.border())
        .border_style(p.fg(p.border))
        .style(p.base())
        .title(title_line);
    if let Some(r) = right {
        block = block.title_top(r.right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some([tl, tr, bl, br]) = ui.corners() {
        if area.width >= 2 && area.height >= 2 {
            let (x0, y0) = (area.x, area.y);
            let (x1, y1) = (area.right() - 1, area.bottom() - 1);
            let style = p.fg(p.accent);
            let buf = f.buffer_mut();
            for (x, y, sym) in [(x0, y0, tl), (x1, y0, tr), (x0, y1, bl), (x1, y1, br)] {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_symbol(sym).set_style(style);
                }
            }
        }
    }
    inner
}

/// Coloured status chip, e.g. ` ✔ OK ` on green. Mono: `[OK]`.
pub fn badge(sev: u8, label: &str, p: &Palette, ui: Ui) -> Span<'static> {
    if p.is_mono() {
        return Span::styled(
            format!("[{} {label}]", ui.sym(sev)),
            Style::default().add_modifier(Modifier::BOLD),
        );
    }
    Span::styled(
        format!(" {} {label} ", ui.sym(sev)),
        Style::default()
            .bg(p.severity(sev))
            .fg(p.on_badge)
            .add_modifier(Modifier::BOLD),
    )
}

/// Status text without a background, e.g. `✔ OK` in green.
pub fn status(sev: u8, label: &str, p: &Palette, ui: Ui) -> Span<'static> {
    Span::styled(format!("{} {label}", ui.sym(sev)), p.bold(p.severity(sev)))
}

/// Footer key hints: ` ↑↓ ` keycaps followed by dim labels.
pub fn keycaps(items: &[(&str, &str)], p: &Palette) -> Line<'static> {
    let mut spans = Vec::new();
    for (key, label) in items {
        match p.bar {
            Some(bg) => spans.push(Span::styled(
                format!(" {key} "),
                Style::default().bg(bg).fg(p.accent).add_modifier(Modifier::BOLD),
            )),
            None => spans.push(Span::styled(format!("[{key}]"), p.bold(p.accent))),
        }
        spans.push(Span::styled(format!(" {label}  "), p.fg(p.dim)));
    }
    Line::from(spans)
}

/// A labelled line gauge: `LABEL     ━━━━━━━──── 62%`.
#[allow(clippy::too_many_arguments)]
pub fn gauge(
    label: &str,
    label_w: usize,
    percent: f64,
    cells: usize,
    color: Color,
    value: &str,
    p: &Palette,
    ui: Ui,
) -> Line<'static> {
    let (full, empty) = ui.gauge_cells(p.is_mono());
    let pct = if percent.is_finite() { percent.clamp(0.0, 100.0) } else { 0.0 };
    let filled = ((pct / 100.0) * cells as f64).round() as usize;
    let filled = filled.min(cells);
    Line::from(vec![
        Span::styled(format!("{label:<label_w$}"), p.fg(p.dim)),
        Span::styled(full.repeat(filled), p.bold(color)),
        Span::styled(empty.repeat(cells - filled), p.fg(p.track)),
        Span::styled(format!(" {value}"), p.bold(p.fg)),
    ])
}

/// Gauge cell count that fits `width` next to a label and a value.
pub fn gauge_cells_for(width: u16, label_w: usize, value_w: usize) -> usize {
    (width as usize)
        .saturating_sub(label_w + value_w + 1)
        .clamp(4, 32)
}

/// `LABEL     value` row.
pub fn field(label: &str, label_w: usize, value: Vec<Span<'static>>, p: &Palette) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{label:<label_w$}"), p.fg(p.dim))];
    spans.extend(value);
    Line::from(spans)
}

/// Section caption inside a panel, e.g. `── ALERTS ─────`.
pub fn caption(text: &str, width: u16, p: &Palette, ui: Ui) -> Line<'static> {
    let lead = if ui.plain { "-- " } else { "── " };
    let fill = if ui.plain { "-" } else { "─" };
    let used = text.chars().count() + 4;
    let rest = (width as usize).saturating_sub(used);
    Line::from(vec![
        Span::styled(lead, p.fg(p.border)),
        Span::styled(text.to_string(), p.bold(p.accent)),
        Span::styled(format!(" {}", fill.repeat(rest)), p.fg(p.border)),
    ])
}

/// Centered sub-rectangle of at most `w` x `h`.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Truncate to `max` display chars with an ellipsis (`~` when plain).
pub fn clip(s: &str, max: usize, ui: Ui) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else if max == 0 {
        String::new()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push(if ui.plain { '~' } else { '…' });
        out
    }
}
