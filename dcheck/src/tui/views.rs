//! Screens: splash, command deck (menu), storage array, device report
//! dashboard, memory, processor, and the help overlay.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, List, ListItem, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use super::theme::{Palette, Ui};
use super::widgets as w;
use super::{App, Screen};
use crate::cpu::CpuInfo;
use crate::health::Health;
use crate::model::Device;
use crate::ram::RamInfo;
use crate::report::{human_size, human_size_bin, mount_summary};
use crate::smartctl::SmartData;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Width of the label column in vitals panels.
const LW: usize = 11;
/// Width reserved for a gauge's value text.
const VW: usize = 15;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(Block::default().style(app.pal.base()), area);

    if app.screen == Screen::Splash {
        splash(f, app, area);
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, app, header);
    match app.screen {
        Screen::Menu => menu(f, app, body),
        Screen::Storage => storage(f, app, body),
        Screen::Report => report_view(f, app, body),
        Screen::Ram => ram_view(f, app, body),
        Screen::Cpu => cpu_view(f, app, body),
        Screen::Splash => {}
    }
    draw_footer(f, app, footer);
    if app.help {
        help(f, app, body);
    }
}

// ─── chrome ──────────────────────────────────────────────────────────────────

fn severity_label(sev: u8) -> &'static str {
    match sev {
        0 => "OK",
        2 => "MONITOR",
        3 => "BACK UP",
        4 => "REPLACE",
        _ => "UNKNOWN",
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let (p, ui) = (&app.pal, app.ui);
    let bar = match p.bar {
        Some(bg) => p.base().bg(bg),
        None => p.base(),
    };
    f.render_widget(Block::default().style(bar), area);

    let mut left = Vec::new();
    if !ui.plain {
        left.push(Span::styled(" ◢◤ ", p.fg(p.accent2)));
    } else {
        left.push(Span::raw(" "));
    }
    left.push(Span::styled("DCHECK", p.bold(p.accent)));

    let (label, sev, scanning, ctx) = match app.screen {
        Screen::Ram => {
            let (l, s) = app.ram_sev();
            (l, s, app.ram_rx.is_some(), "MEMORY".to_string())
        }
        Screen::Cpu => {
            let (l, s) = app.cpu_sev();
            (l, s, app.cpu_rx.is_some(), "PROCESSOR".to_string())
        }
        _ => {
            let s = app.worst_device();
            (
                severity_label(s),
                s,
                app.health_rx.is_some(),
                format!("{} DEV", app.devices.len()),
            )
        }
    };
    let mut right = Vec::new();
    if app.demo {
        right.push(Span::styled("DEMO DATA  ", p.bold(p.warn)));
    }
    let host_sym = if ui.plain { "@" } else { "◈" };
    let host = Span::styled(format!("{host_sym} {}  ", app.host), p.fg(p.dim));
    if scanning {
        right.push(Span::styled(
            format!("{} SCANNING", ui.spinner(app.tick)),
            p.bold(p.accent),
        ));
    } else {
        right.push(w::badge(sev, label, p, ui));
    }
    right.push(Span::styled(format!(" {ctx} "), p.fg(p.dim)));

    // Drop optional pieces (host, subtitle, version) until everything fits.
    let width = area.width as usize;
    let used = |spans: &[Span]| spans.iter().map(|s| s.width()).sum::<usize>();
    let host_at = if app.demo { 1 } else { 0 };
    if used(&left) + used(&right) + host.width() < width {
        right.insert(host_at, host);
    }
    let subtitle = Span::styled(" // DEVICE HEALTH SYSTEM", p.fg(p.dim));
    let version = Span::styled(format!("  v{VERSION}"), p.fg(p.dim));
    if used(&left) + used(&right) + subtitle.width() + version.width() < width {
        left.push(subtitle);
    }
    if used(&left) + used(&right) + version.width() < width {
        left.push(version);
    }

    let right = Line::from(right);
    let rw = (right.width() as u16).min(area.width);
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(rw)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)).style(bar), l);
    f.render_widget(Paragraph::new(right).alignment(Alignment::Right).style(bar), r);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let (p, ui) = (&app.pal, app.ui);
    let nav = if ui.plain { "j/k" } else { "↑↓" };
    let items: &[(&str, &str)] = match app.screen {
        Screen::Menu => &[(nav, "nav"), ("enter", "open"), ("1-3", "jump"), ("?", "help"), ("q", "quit")],
        Screen::Storage => &[
            (nav, "select"),
            ("enter", "report"),
            ("r", "rescan"),
            ("esc", "back"),
            ("?", "help"),
            ("q", "quit"),
        ],
        _ => &[
            (nav, "scroll"),
            ("pgup/dn", "page"),
            ("r", "refresh"),
            ("c", "copy"),
            ("esc", "back"),
            ("q", "quit"),
        ],
    };
    let mut line = w::keycaps(items, p);
    line.spans.insert(0, Span::raw(" "));
    f.render_widget(Paragraph::new(line).style(p.base()), area);
    if let Some(status) = &app.status {
        let msg = Line::from(Span::styled(format!("{} {status} ", ui.arrow()), p.bold(p.accent2)));
        f.render_widget(Paragraph::new(msg).alignment(Alignment::Right), area);
    }
}

// ─── splash ─────────────────────────────────────────────────────────────────

fn splash(f: &mut Frame, app: &App, area: Rect) {
    let (p, ui) = (&app.pal, app.ui);
    let elapsed = app.started.elapsed().as_millis() as u64;
    let steps: [(&str, String); 4] = [
        ("SYSFS / PROC BUS", "LINKED".into()),
        ("BLOCK DEVICES", format!("{} FOUND", app.devices.len())),
        ("SMART ENGINE", if app.demo { "SIMULATED" } else { "ARMED" }.into()),
        ("TELEMETRY", "ONLINE".into()),
    ];
    let shown = ((elapsed / 100) as usize + 1).min(steps.len());

    let mut lines = w::logo_lines(ui, p);
    lines.push(Line::from(Span::styled(
        format!("DEVICE HEALTH SYSTEM  v{VERSION}"),
        p.fg(p.dim),
    )));
    lines.push(Line::from(""));
    for (name, state) in steps.iter().take(shown) {
        lines.push(Line::from(vec![
            Span::styled("[ OK ] ", p.bold(p.ok)),
            Span::styled(format!("{:.<24}", format!("{name} ")), p.fg(p.dim)),
            Span::styled(format!(" {state:>9}"), p.bold(p.accent)),
        ]));
    }
    for _ in shown..steps.len() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(""));
    let pct = (elapsed as f64 / super::SPLASH.as_millis() as f64 * 100.0).min(100.0);
    lines.push(w::gauge("BOOT  ", 6, pct, 30, p.accent, &format!("{pct:>3.0}%"), p, ui));

    let h = lines.len() as u16;
    let r = w::centered(area, 46, h);
    f.render_widget(Paragraph::new(Text::from(lines)).alignment(Alignment::Center), r);
}

// ─── menu / command deck ────────────────────────────────────────────────────

fn menu(f: &mut Frame, app: &mut App, area: Rect) {
    let wide = area.width >= 72 && area.height >= 10;
    let (left, right) = if wide {
        let [l, r] = Layout::horizontal([Constraint::Length(32), Constraint::Min(30)]).areas(area);
        (l, Some(r))
    } else {
        (area, None)
    };

    let (p, ui) = (app.pal.clone(), app.ui);
    let inner = w::panel(f, left, "MODULES", None, &p, ui);

    let logo = w::logo_lines(ui, &p);
    let logo_h = if inner.height as usize >= logo.len() + 7 {
        logo.len() as u16 + 1
    } else {
        0
    };
    let [logo_area, list_area] =
        Layout::vertical([Constraint::Length(logo_h), Constraint::Min(1)]).areas(inner);
    if logo_h > 0 {
        f.render_widget(
            Paragraph::new(Text::from(logo)).alignment(Alignment::Center),
            logo_area,
        );
    }

    let (_, ram_sev) = app.ram_sev();
    let (_, cpu_sev) = app.cpu_sev();
    // (number, name, (severity, loading)) — EXIT has no status.
    type Entry = (&'static str, &'static str, Option<(u8, bool)>);
    let entries: [Entry; 4] = [
        ("01", "STORAGE", Some((app.worst_device(), app.health_rx.is_some()))),
        ("02", "MEMORY", Some((ram_sev, app.ram_rx.is_some()))),
        ("03", "PROCESSOR", Some((cpu_sev, app.cpu_rx.is_some()))),
        ("00", "EXIT", None),
    ];
    let row_w = list_area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = entries
        .iter()
        .map(|(num, name, state)| {
            let mut spans = vec![
                Span::styled(format!("{num} "), p.fg(p.dim)),
                Span::styled(name.to_string(), p.bold(p.fg)),
            ];
            if let Some((sev, loading)) = state {
                let (sym, color) = if *loading {
                    (ui.spinner(app.tick), p.accent)
                } else {
                    (ui.sym(*sev), p.severity(*sev))
                };
                let pad = row_w.saturating_sub(num.len() + 1 + name.len() + 2);
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(sym, p.bold(color)));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .style(p.base())
        .highlight_style(p.highlight())
        .highlight_symbol(ui.cursor());
    f.render_stateful_widget(list, list_area, &mut app.menu);

    if let Some(r) = right {
        match app.menu.selected().unwrap_or(0) {
            0 => storage_card(f, app, r),
            1 => ram_card(f, app, r),
            2 => cpu_card(f, app, r),
            _ => exit_card(f, app, r),
        }
    }
}

fn scanning_line(app: &App, what: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("{} {what}", app.ui.spinner(app.tick)),
        app.pal.bold(app.pal.accent),
    ))
}

fn storage_card(f: &mut Frame, app: &App, area: Rect) {
    let (p, ui) = (&app.pal, app.ui);
    let right = Line::from(Span::styled(format!(" {} ATTACHED ", app.devices.len()), p.fg(p.dim)));
    let inner = w::panel(f, area, "STORAGE ARRAY", Some(right), p, ui);
    let mut lines = Vec::new();
    if app.health_rx.is_some() {
        lines.push(w::field(
            "STATUS",
            LW,
            vec![Span::styled(format!("{} SCANNING", ui.spinner(app.tick)), p.bold(p.accent))],
            p,
        ));
    } else {
        let s = app.worst_device();
        lines.push(w::field("STATUS", LW - 1, vec![w::badge(s, severity_label(s), p, ui)], p));
    }
    let mounted = app.devices.iter().filter(|d| mount_summary(d) != "-").count();
    let total: u64 = app.devices.iter().map(|d| d.size_bytes).sum();
    lines.push(w::field(
        "DEVICES",
        LW,
        vec![Span::styled(
            format!("{} attached {} {} mounted", app.devices.len(), ui.dot(), mounted),
            p.fg(p.fg),
        )],
        p,
    ));
    lines.push(w::field("CAPACITY", LW, vec![Span::styled(format!("{} raw", human_size(total)), p.fg(p.fg))], p));
    lines.push(Line::from(""));
    lines.push(w::caption("UNITS", inner.width, p, ui));
    let room = (inner.height as usize).saturating_sub(lines.len() + 2);
    for (i, d) in app.devices.iter().enumerate().take(room) {
        let h = app.health.get(i).cloned().unwrap_or_else(super::DevHealth::pending);
        let mut spans = vec![
            Span::styled(format!("{} ", ui.arrow()), p.fg(p.accent)),
            Span::styled(format!("{:<10}", w::clip(&d.name, 10, ui)), p.bold(p.fg)),
            Span::styled(format!(" {:<5}", d.kind.to_string()), p.fg(p.dim)),
            Span::styled(format!("{:>9}  ", human_size(d.size_bytes)), p.fg(p.fg)),
        ];
        if app.health_rx.is_some() {
            spans.push(Span::styled(ui.spinner(app.tick), p.fg(p.accent)));
        } else {
            spans.push(w::status(h.sev, &h.label, p, ui));
        }
        lines.push(Line::from(spans));
    }
    if app.devices.len() > room {
        lines.push(Line::from(Span::styled(
            format!("  +{} more", app.devices.len() - room),
            p.fg(p.dim),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("enter {} open storage array", ui.arrow()),
        p.fg(p.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn ram_card(f: &mut Frame, app: &App, area: Rect) {
    let (p, ui) = (&app.pal, app.ui);
    let inner = w::panel(f, area, "MEMORY BANK", None, p, ui);
    let mut lines = match &app.ram {
        Some(r) => ram_vitals(r, p, ui, inner.width),
        None => vec![scanning_line(app, "READING MEMORY")],
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!("enter {} memory diagnostics", ui.arrow()), p.fg(p.dim))));
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }), inner);
}

fn cpu_card(f: &mut Frame, app: &App, area: Rect) {
    let (p, ui) = (&app.pal, app.ui);
    let inner = w::panel(f, area, "PROCESSOR", None, p, ui);
    let mut lines = match &app.cpu {
        Some(c) => cpu_vitals(c, p, ui, inner.width, 2),
        None => vec![scanning_line(app, "READING PROCESSOR")],
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(format!("enter {} processor diagnostics", ui.arrow()), p.fg(p.dim))));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn exit_card(f: &mut Frame, app: &App, area: Rect) {
    let (p, ui) = (&app.pal, app.ui);
    let inner = w::panel(f, area, "SESSION", None, p, ui);
    let lines = vec![
        Line::from(Span::styled("Terminate the dcheck session.", p.fg(p.fg))),
        Line::from(""),
        Line::from(Span::styled(format!("enter {} exit", ui.arrow()), p.fg(p.dim))),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

// ─── vitals builders ────────────────────────────────────────────────────────

fn text(s: impl Into<String>, p: &Palette) -> Vec<Span<'static>> {
    vec![Span::styled(s.into(), p.fg(p.fg))]
}

fn temp_gauge(t: i64, warn: i64, cells: usize, p: &Palette, ui: Ui) -> Line<'static> {
    temp_gauge_range(t, None, warn, cells, p, ui)
}

/// Temperature gauge; `range` = lifetime (min, max) shown next to the value.
fn temp_gauge_range(
    t: i64,
    range: Option<(i64, i64)>,
    warn: i64,
    cells: usize,
    p: &Palette,
    ui: Ui,
) -> Line<'static> {
    let pct = t as f64 / (warn + 20).max(1) as f64 * 100.0;
    let color = p.level(t as f64, (warn - 10) as f64, warn as f64);
    let value = match range {
        Some((lo, hi)) => format!("{t}{} ({lo}–{hi})", ui.degrees()),
        None => format!("{t}{}", ui.degrees()),
    };
    w::gauge("TEMP", LW, pct, cells, color, &value, p, ui)
}

fn ram_vitals(r: &RamInfo, p: &Palette, ui: Ui, width: u16) -> Vec<Line<'static>> {
    let cells = w::gauge_cells_for(width, LW, VW);
    let (label, sev) = r.verdict();
    let mut lines = vec![w::field("STATUS", LW - 1, vec![w::badge(sev, label, p, ui)], p)];
    let used = r.used_percent();
    lines.push(w::gauge(
        "USED",
        LW,
        used,
        cells,
        p.level(used, 75.0, 90.0),
        &format!("{}/{}", human_size_bin(r.used_bytes()), human_size_bin(r.total_bytes)),
        p,
        ui,
    ));
    if r.swap_total_bytes > 0 {
        let su = r.swap_total_bytes.saturating_sub(r.swap_free_bytes);
        let pct = su as f64 * 100.0 / r.swap_total_bytes as f64;
        lines.push(w::gauge(
            "SWAP",
            LW,
            pct,
            cells,
            p.level(pct, 50.0, 80.0),
            &format!("{}/{}", human_size_bin(su), human_size_bin(r.swap_total_bytes)),
            p,
            ui,
        ));
    } else {
        lines.push(w::field("SWAP", LW, vec![Span::styled("none", p.fg(p.dim))], p));
    }
    let mut kind = Vec::new();
    if let Some(t) = r.memory_type() {
        kind.push(t);
    }
    if let Some(s) = r.modules.iter().find_map(|m| m.configured_mts.or(m.speed_mts)) {
        kind.push(format!("{s} MT/s"));
    }
    if !kind.is_empty() {
        lines.push(w::field("TYPE", LW, text(kind.join(&format!(" {} ", ui.dot())), p), p));
    }
    let used_all = r.populated();
    let total = (r.slots_total as usize).max(used_all);
    if total > 0 {
        let (full, empty) = ui.slot_cells();
        let used = used_all.min(total);
        let room = (width as usize).saturating_sub(LW + 14);
        let spaced = total * 2 <= room;
        let cap = if spaced { total } else { total.min(room) };
        let sep = if spaced { " " } else { "" };
        let mut spans = Vec::new();
        for i in 0..cap {
            let (g, c) = if i < used { (full, p.accent) } else { (empty, p.track) };
            spans.push(Span::styled(format!("{g}{sep}"), p.bold(c)));
        }
        let listed = if used != r.modules.len() {
            format!(" (fw lists {})", r.modules.len())
        } else {
            String::new()
        };
        spans.push(Span::styled(format!(" {used}/{total} populated{listed}"), p.fg(p.dim)));
        lines.push(w::field("SLOTS", LW, spans, p));
    }
    let ecc_color = if r.ecc_uncorrectable > 0 {
        p.bad
    } else if r.ecc_correctable > 0 {
        p.warn
    } else {
        p.fg
    };
    lines.push(w::field(
        "ECC",
        LW,
        vec![Span::styled(
            format!(
                "{} corrected {} {} uncorrectable",
                r.ecc_correctable,
                ui.dot(),
                r.ecc_uncorrectable
            ),
            p.fg(ecc_color),
        )],
        p,
    ));
    if let Some(t) = r.ram_temp_c {
        lines.push(temp_gauge(t, crate::config::load().temp_warn_c, cells, p, ui));
    }
    for n in r.notes() {
        lines.push(Line::from(Span::styled(format!("{} {n}", ui.bullet()), p.fg(p.dim))));
    }
    lines
}

fn cpu_vitals(
    c: &CpuInfo,
    p: &Palette,
    ui: Ui,
    width: u16,
    grid_rows: usize,
) -> Vec<Line<'static>> {
    let cells = w::gauge_cells_for(width, LW, VW);
    let health = c.health();
    let (label, sev) = (health.label, health.severity);
    // Gauge scale: the hottest sensor's own limit (not the disk threshold).
    let hottest = c.sensors.iter().max_by_key(|s| s.temp_c);
    let warn = crate::config::load()
        .cpu_temp_warn_c
        .or_else(|| hottest.and_then(|s| s.high_c.or(s.crit_c.map(|c| c - 10))))
        .unwrap_or(crate::cpu::DEFAULT_CPU_WARN_C);
    let mut lines = vec![w::field("STATUS", LW - 1, vec![w::badge(sev, label, p, ui)], p)];
    let model = if c.model.is_empty() { "-" } else { &c.model };
    let room = (width as usize).saturating_sub(LW);
    lines.push(w::field("MODEL", LW, text(w::clip(model, room, ui), p), p));
    lines.push(w::field(
        "TOPOLOGY",
        LW,
        text(
            format!(
                "{} socket {d} {} cores {d} {} threads",
                c.sockets,
                c.cores,
                c.threads,
                d = ui.dot()
            ),
            p,
        ),
        p,
    ));
    if let Some(l) = c.load1 {
        let pct = l / c.threads.max(1) as f64 * 100.0;
        lines.push(w::gauge("LOAD", LW, pct, cells, p.level(pct, 70.0, 90.0), &format!("{l:.2} (1m)"), p, ui));
    }
    if let Some(t) = c.temp_c {
        let mut g = temp_gauge(t, warn, cells, p, ui);
        if let Some(s) = hottest.filter(|_| c.sensors.len() > 1) {
            let short = s.label.replace("Package id ", "socket ");
            g.spans.push(Span::styled(format!(" {short}"), p.fg(p.dim)));
        }
        lines.push(g);
    }
    match (c.mhz, c.max_mhz) {
        (Some(cur), Some(max)) if max > 0.0 => lines.push(w::gauge(
            "CLOCK",
            LW,
            cur / max * 100.0,
            cells,
            p.accent,
            &format!("{cur:.0}/{max:.0} MHz"),
            p,
            ui,
        )),
        (Some(cur), _) => lines.push(w::field("CLOCK", LW, text(format!("{cur:.0} MHz"), p), p)),
        (None, Some(max)) => lines.push(w::field("CLOCK", LW, text(format!("max {max:.0} MHz"), p), p)),
        _ => {}
    }
    if let Some(kb) = c.cache_kb {
        let cache = if kb >= 1024 { format!("{} MB", kb / 1024) } else { format!("{kb} KB") };
        lines.push(w::field("CACHE", LW, text(cache, p), p));
    }
    if c.threads > 0 && grid_rows > 0 {
        let (full, empty) = ui.slot_cells();
        let busy = c.load1.map(|l| l.round() as usize).unwrap_or(0).min(c.threads as usize);
        let per_row = ((width as usize).saturating_sub(LW) / 9 * 8).max(8);
        let total = c.threads as usize;
        let shown = total.min(per_row * grid_rows);
        for row in 0..shown.div_ceil(per_row) {
            let mut spans = Vec::new();
            for i in row * per_row..((row + 1) * per_row).min(shown) {
                if i > row * per_row && i % 8 == 0 {
                    spans.push(Span::raw(" "));
                }
                let (g, col) = if i < busy { (full, p.warn) } else { (empty, p.accent) };
                spans.push(Span::styled(g, p.fg(col)));
            }
            if row == 0 && shown < total {
                spans.push(Span::styled(format!(" +{}", total - shown), p.fg(p.dim)));
            }
            lines.push(w::field(if row == 0 { "THREADS" } else { "" }, LW, spans, p));
        }
    }
    let issue_color = if sev >= 3 { p.bad } else { p.warn };
    for i in &health.issues {
        lines.push(Line::from(Span::styled(format!("{} {i}", ui.sym(sev.max(2))), p.fg(issue_color))));
    }
    for n in &health.notes {
        lines.push(Line::from(Span::styled(format!("{} {n}", ui.bullet()), p.fg(p.dim))));
    }
    lines
}

fn report_vitals(app: &App, width: u16) -> Vec<Line<'static>> {
    let (p, ui) = (&app.pal, app.ui);
    let Some((s, h)) = &app.report_metrics else {
        let mut lines = vec![
            w::field("VERDICT", LW - 1, vec![w::badge(1, "UNKNOWN", p, ui)], p),
            Line::from(""),
            Line::from(Span::styled("SMART telemetry unavailable", p.bold(p.warn))),
        ];
        if let Some(d) = app.report_dev.and_then(|i| app.devices.get(i)) {
            let (why, hint) = crate::report::smart_unavailable(d);
            lines.push(Line::from(Span::styled(format!("{} {why}", ui.bullet()), p.fg(p.fg))));
            if let Some(h) = hint {
                lines.push(Line::from(Span::styled(format!("{} {h}", ui.bullet()), p.fg(p.dim))));
            }
        }
        return lines;
    };
    let cells = w::gauge_cells_for(width, LW, VW);
    let sev = h.verdict.severity();
    let mut lines = vec![
        w::field("VERDICT", LW - 1, vec![w::badge(sev, h.verdict.label(), p, ui)], p),
        w::field("CONFIDENCE", LW, text(h.confidence.label(), p), p),
    ];
    if !s.source.is_empty() {
        lines.push(w::field("SOURCE", LW, text(s.source.clone(), p), p));
    }
    lines.push(Line::from(""));
    life_gauges(&mut lines, s, h, app.temp_warn, cells, p, ui);

    let mut po = Vec::new();
    if let Some(poh) = s.power_on_hours {
        po.push(format!("{poh} h"));
    }
    if let Some(c) = s.power_cycles {
        po.push(format!("{c} cycles"));
    }
    if !po.is_empty() {
        lines.push(w::field("POWER-ON", LW, text(po.join(&format!(" {} ", ui.dot())), p), p));
    }
    match s.manufactured {
        Some((y, wk)) => {
            let age = crate::report::age_years(y, wk)
                .map(|a| format!(" {} {a:.1} y old", ui.dot()))
                .unwrap_or_default();
            lines.push(w::field("MADE", LW, text(format!("{y} week {wk}{age}"), p), p));
        }
        None => lines.push(w::field(
            "MADE",
            LW,
            vec![Span::styled("not stored by the drive", p.fg(p.dim))],
            p,
        )),
    }
    if let Some(poh) = s.power_on_hours {
        lines.push(w::field(
            "IN SERVICE",
            LW,
            text(format!("{:.1} y powered on @24/7", poh as f64 / (365.0 * 24.0)), p),
            p,
        ));
    }
    let mut cycles = Vec::new();
    if let (Some(a), Some(r)) = (s.power_cycles, s.rated_start_stop) {
        cycles.push(format!("start {}/{}", compact(a), compact(r)));
    }
    if let (Some(a), Some(r)) = (s.load_unload, s.rated_load_unload) {
        cycles.push(format!("load {}/{}", compact(a), compact(r)));
    }
    if !cycles.is_empty() {
        lines.push(w::field("CYCLES", LW, text(cycles.join(&format!(" {} ", ui.dot())), p), p));
    }
    if let (Some(hours), Some(used), Some(what)) = (h.design_hours, h.design_life_used, h.design_limit) {
        lines.push(w::field(
            "DESIGN",
            LW,
            vec![
                Span::styled(format!("{:.1} y @24/7 ", hours as f64 / (365.0 * 24.0)), p.fg(p.fg)),
                Span::styled(format!("({hours} h, assumed)"), p.fg(p.dim)),
            ],
            p,
        ));
        lines.push(w::field("", LW, vec![Span::styled(format!("{used}% used ({what})"), p.fg(p.dim))], p));
    }
    let est = match (h.remaining_poh, h.overdue_poh) {
        (Some(0), Some(over)) => vec![Span::styled(
            format!("0 {} {:.1}y past rated life", ui.dot(), over as f64 / (365.0 * 24.0)),
            p.bold(p.warn),
        )],
        _ => match crate::report::life_left(h) {
            Some(t) => vec![Span::styled(t.replace("  |  ", &format!(" {} ", ui.dot())), p.fg(p.fg))],
            None => vec![Span::styled("unknown", p.fg(p.dim))],
        },
    };
    lines.push(w::field("EST. LIFE", LW, est, p));
    if h.design_hours.is_none() {
        if let Some(basis) = h.life_basis {
            lines.push(w::field("", LW, vec![Span::styled(format!("({basis})"), p.fg(p.dim))], p));
        }
    }

    lines.push(Line::from(""));
    lines.push(w::caption("ALERTS", width, p, ui));
    if h.issues.is_empty() && h.notes.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{} no anomalies detected", ui.sym(0)),
            p.fg(p.ok),
        )));
    }
    let issue_color = if sev >= 3 { p.bad } else { p.warn };
    for i in &h.issues {
        lines.push(Line::from(Span::styled(
            format!("{} {i}", ui.sym(sev.max(2))),
            p.fg(issue_color),
        )));
    }
    for n in &h.notes {
        lines.push(Line::from(Span::styled(format!("{} {n}", ui.bullet()), p.fg(p.dim))));
    }
    lines
}

/// 8664 -> "8664", 50000 -> "50k", 1500000 -> "1.5M".
fn compact(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => {
            if n.is_multiple_of(1000) {
                format!("{}k", n / 1000)
            } else {
                format!("{:.1}k", n as f64 / 1000.0)
            }
        }
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

fn life_gauges(
    lines: &mut Vec<Line<'static>>,
    s: &SmartData,
    h: &Health,
    warn: i64,
    cells: usize,
    p: &Palette,
    ui: Ui,
) {
    match h.wear_used_percent.or(h.design_life_used) {
        Some(used) => {
            let left = 100u64.saturating_sub(used) as f64;
            let color = if left < 20.0 {
                p.bad
            } else if left < 50.0 {
                p.warn
            } else {
                p.ok
            };
            lines.push(w::gauge("LIFE", LW, left, cells, color, &format!("{left:.0}% left"), p, ui));
        }
        None => lines.push(w::field("LIFE", LW, vec![Span::styled("n/a", p.fg(p.dim))], p)),
    }
    if let Some(t) = s.temperature_c {
        let range = s.temp_min_c.zip(s.temp_max_c);
        lines.push(temp_gauge_range(t, range, warn, cells, p, ui));
    }
    match (h.tbw_bytes, h.rated_tbw_bytes) {
        (Some(tbw), Some(rated)) if rated > 0 => {
            let pct = tbw as f64 * 100.0 / rated as f64;
            lines.push(w::gauge(
                "ENDURANCE",
                LW,
                pct,
                cells,
                p.level(pct, 70.0, 90.0),
                &format!("{}/{}", human_size(tbw), human_size(rated)),
                p,
                ui,
            ));
        }
        (Some(tbw), _) => lines.push(w::field("WRITTEN", LW, text(human_size(tbw), p), p)),
        _ => {}
    }
}

// ─── log pane ───────────────────────────────────────────────────────────────

/// Colour a plain report line: section headers, `label : value` rows, verdicts.
fn colorize_line(line: &str, p: &Palette) -> Line<'static> {
    let trimmed = line.trim_start();
    if line.starts_with('▚') || line.starts_with("▐ ") || line.starts_with('[') {
        return Line::from(Span::styled(line.to_string(), p.bold(p.accent)));
    }
    if trimmed.starts_with('─') || trimmed.starts_with("----") {
        return Line::from(Span::styled(line.to_string(), p.fg(p.border)));
    }
    let value_color = |v: &str| {
        if v.contains("FAILED") || v.contains("REPLACE") || v.contains("BACK UP") || v.contains("FAIL") {
            p.bad
        } else if v.contains("MONITOR") || v.contains("warn") {
            p.warn
        } else if v.trim() == "OK" || v.trim() == "passed" {
            p.ok
        } else {
            p.fg
        }
    };
    if let Some(idx) = line.find(": ") {
        if line.starts_with("  ") && !line[..idx].trim().is_empty() && idx < 20 {
            let (label, value) = line.split_at(idx + 2);
            return Line::from(vec![
                Span::styled(label.to_string(), p.fg(p.dim)),
                Span::styled(value.to_string(), p.fg(value_color(value))),
            ]);
        }
    }
    Line::from(Span::styled(line.to_string(), p.fg(value_color(line))))
}

/// Rows `lines` occupy when wrapped at `width`.
fn wrapped_rows(lines: &[String], width: u16) -> usize {
    let w = width.max(1) as usize;
    lines
        .iter()
        .map(|l| l.chars().count().max(1).div_ceil(w))
        .sum()
}

fn log_pane(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    title: &str,
    loading: bool,
    lines: Vec<String>,
) {
    let (p, ui) = (app.pal.clone(), app.ui);
    let right = if loading {
        Line::from(Span::styled(format!(" {} LOADING ", ui.spinner(app.tick)), p.bold(p.accent)))
    } else {
        let rows = wrapped_rows(&lines, area.width.saturating_sub(2));
        let pos = (app.scroll as usize + 1).min(rows.max(1));
        Line::from(Span::styled(format!(" {pos}/{rows} "), p.fg(p.dim)))
    };
    let inner = w::panel(f, area, title, Some(right), &p, ui);
    app.view_height = inner.height;
    app.log_rows = wrapped_rows(&lines, inner.width);
    app.scroll = app.scroll.min(app.max_scroll());
    let body: Vec<Line> = lines.iter().map(|l| colorize_line(l, &p)).collect();
    f.render_widget(
        Paragraph::new(Text::from(body))
            .wrap(Wrap { trim: false })
            .scroll((app.scroll, 0)),
        inner,
    );
}

// ─── storage ────────────────────────────────────────────────────────────────

fn storage(f: &mut Frame, app: &mut App, area: Rect) {
    let (p, ui) = (app.pal.clone(), app.ui);
    if app.devices.is_empty() {
        let inner = w::panel(f, area, "STORAGE ARRAY", None, &p, ui);
        let msg = vec![
            Line::from(""),
            Line::from(Span::styled("NO BLOCK DEVICES DETECTED", p.bold(p.warn))),
            Line::from(Span::styled("press r to rescan", p.fg(p.dim))),
        ];
        f.render_widget(Paragraph::new(Text::from(msg)).alignment(Alignment::Center), inner);
        return;
    }

    let detail_h = if area.height >= 16 { 5 } else { 0 };
    let [table_area, detail_area] =
        Layout::vertical([Constraint::Min(4), Constraint::Length(detail_h)]).areas(area);

    let right = if app.health_rx.is_some() {
        Line::from(Span::styled(format!(" {} SCANNING ", ui.spinner(app.tick)), p.bold(p.accent)))
    } else {
        Line::from(Span::styled(format!(" {} UNITS ", app.devices.len()), p.fg(p.dim)))
    };
    let inner = w::panel(f, table_area, "STORAGE ARRAY", Some(right), &p, ui);

    let wd = inner.width;
    let show_bus = wd >= 78;
    let show_temp = wd >= 96;
    let show_life = wd >= 104;
    let show_mount = wd >= 120;

    let mut header = vec!["DEVICE", "TYPE"];
    let mut widths = vec![Constraint::Length(14), Constraint::Length(5)];
    if show_bus {
        header.push("BUS");
        widths.push(Constraint::Length(6));
    }
    header.push("MODEL");
    widths.push(Constraint::Min(12));
    header.push("CAPACITY");
    widths.push(Constraint::Length(9));
    if show_life {
        header.push("LIFE");
        widths.push(Constraint::Length(15));
    }
    if show_temp {
        header.push("TEMP");
        widths.push(Constraint::Length(6));
    }
    header.push("HEALTH");
    widths.push(Constraint::Length(14));
    if show_mount {
        header.push("MOUNT");
        widths.push(Constraint::Length(14));
    }
    let header = Row::new(header.into_iter().map(Cell::from)).style(p.bold(p.accent));

    let loading = app.health_rx.is_some();
    let rows: Vec<Row> = app
        .devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let h = app.health.get(i).cloned().unwrap_or_else(super::DevHealth::pending);
            let mut cells = vec![
                Cell::from(Span::styled(d.path.clone(), p.bold(p.fg))),
                Cell::from(Span::styled(d.kind.to_string(), p.fg(p.dim))),
            ];
            if show_bus {
                cells.push(Cell::from(Span::styled(d.bus.to_string(), p.fg(p.dim))));
            }
            cells.push(Cell::from(d.label()));
            cells.push(Cell::from(Line::from(human_size(d.size_bytes)).right_aligned()));
            if show_life {
                cells.push(Cell::from(match h.life {
                    Some(l) => {
                        let color = if l < 20 {
                            p.bad
                        } else if l < 50 {
                            p.warn
                        } else {
                            p.ok
                        };
                        let mut g = w::gauge("", 0, l as f64, 8, color, &format!("{l:>3}%"), &p, ui);
                        g.spans.remove(0);
                        g
                    }
                    None => Line::from(Span::styled(if ui.plain { "-" } else { "—" }, p.fg(p.dim))),
                }));
            }
            if show_temp {
                cells.push(Cell::from(match h.temp {
                    Some(t) => Span::styled(
                        format!("{t}{}", ui.degrees()),
                        p.fg(p.level(t as f64, (app.temp_warn - 10) as f64, app.temp_warn as f64)),
                    ),
                    None => Span::styled(if ui.plain { "-" } else { "—" }, p.fg(p.dim)),
                }));
            }
            cells.push(Cell::from(if loading {
                Span::styled(ui.spinner(app.tick), p.fg(p.accent))
            } else {
                w::status(h.sev, &h.label, &p, ui)
            }));
            if show_mount {
                cells.push(Cell::from(Span::styled(mount_summary(d), p.fg(p.dim))));
            }
            Row::new(cells)
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .style(p.base())
        .column_spacing(1)
        .row_highlight_style(p.highlight())
        .highlight_symbol(ui.cursor());
    f.render_stateful_widget(table, inner, &mut app.table);

    if detail_h > 0 {
        if let Some(d) = app.table.selected().and_then(|i| app.devices.get(i)) {
            device_detail(f, d, detail_area, &p, ui);
        }
    }
}

fn device_detail(f: &mut Frame, d: &Device, area: Rect, p: &Palette, ui: Ui) {
    let title = format!("TARGET {} {}", ui.arrow(), d.path);
    let inner = w::panel(f, area, &title, None, p, ui);
    let dot = format!(" {} ", ui.dot());
    let mut ident = vec![d.label()];
    if let Some(s) = &d.serial {
        ident.push(format!("S/N {s}"));
    }
    if let Some(fw) = &d.firmware {
        ident.push(format!("FW {fw}"));
    }
    ident.push(format!("{} {}", d.bus, human_size(d.size_bytes)));
    let parts = if d.partitions.is_empty() {
        "no partitions".to_string()
    } else {
        d.partitions
            .iter()
            .map(|pt| {
                let name = pt.path.rsplit('/').next().unwrap_or(&pt.path);
                let fs = pt.filesystem.as_deref().unwrap_or("raw");
                let at = pt.mountpoint.as_deref().unwrap_or("unmounted");
                format!("{name} {} {fs} {} {at}", human_size(pt.size_bytes), ui.arrow())
            })
            .collect::<Vec<_>>()
            .join(&dot)
    };
    let room = inner.width as usize;
    let lines = vec![
        Line::from(Span::styled(w::clip(&ident.join(&dot), room, ui), p.fg(p.fg))),
        Line::from(Span::styled(w::clip(&parts, room, ui), p.fg(p.dim))),
        Line::from(Span::styled(
            format!("enter {} full diagnostic report", ui.arrow()),
            p.fg(p.accent),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

// ─── report dashboard ───────────────────────────────────────────────────────

fn report_view(f: &mut Frame, app: &mut App, area: Rect) {
    let (p, ui) = (app.pal.clone(), app.ui);
    let wide = area.width >= 100;
    let (vit_area, log_area) = if wide {
        let [a, b] = Layout::horizontal([Constraint::Length(46), Constraint::Min(40)]).areas(area);
        (a, b)
    } else {
        // Size the vitals panel to its content so alerts stay visible.
        let need = if app.report_rx.is_some() {
            1
        } else {
            report_vitals(app, area.width.saturating_sub(2)).len() as u16
        };
        let vh = (need + 2).min(area.height.saturating_sub(5)).max(3);
        let [a, b] = Layout::vertical([Constraint::Length(vh), Constraint::Min(3)]).areas(area);
        (a, b)
    };
    let path = app
        .report_dev
        .and_then(|i| app.devices.get(i))
        .map(|d| d.path.clone())
        .unwrap_or_default();
    let loading = app.report_rx.is_some();

    let inner = w::panel(f, vit_area, "VITALS", None, &p, ui);
    let lines = if loading {
        vec![scanning_line(app, "READING SMART TELEMETRY")]
    } else {
        report_vitals(app, inner.width)
    };
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }), inner);

    let title = format!("TELEMETRY LOG {} {path}", ui.arrow());
    let lines = app.report_lines.clone();
    log_pane(f, app, log_area, &title, loading, lines);
}

// ─── memory / processor ─────────────────────────────────────────────────────

/// Rows `lines` take when wrapped at `width` cells.
fn display_rows(lines: &[Line], width: u16) -> usize {
    let w = width.max(1) as usize;
    lines.iter().map(|l| l.width().max(1).div_ceil(w)).sum()
}

fn dashboard_split(area: Rect, lines: usize) -> (Rect, Rect) {
    let h = (lines as u16 + 2).min(area.height.saturating_sub(4)).max(3);
    let [a, b] = Layout::vertical([Constraint::Length(h), Constraint::Min(3)]).areas(area);
    (a, b)
}

fn ram_view(f: &mut Frame, app: &mut App, area: Rect) {
    let (p, ui) = (app.pal.clone(), app.ui);
    let loading = app.ram_rx.is_some();
    let lines = match (&app.ram, loading) {
        (Some(r), _) => ram_vitals(r, &p, ui, area.width.saturating_sub(2)),
        (None, _) => vec![scanning_line(app, "READING MEMORY")],
    };
    let rows = display_rows(&lines, area.width.saturating_sub(2));
    let (top, bottom) = dashboard_split(area, rows);
    let inner = w::panel(f, top, "MEMORY BANK", None, &p, ui);
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }), inner);
    let log = app.ram_lines.clone();
    log_pane(f, app, bottom, "MEMORY LOG", loading && app.ram.is_none(), log);
}

fn cpu_view(f: &mut Frame, app: &mut App, area: Rect) {
    let (p, ui) = (app.pal.clone(), app.ui);
    let loading = app.cpu_rx.is_some();
    let lines = match &app.cpu {
        Some(c) => cpu_vitals(c, &p, ui, area.width.saturating_sub(2), 4),
        None => vec![scanning_line(app, "READING PROCESSOR")],
    };
    let (top, bottom) = dashboard_split(area, lines.len());
    let inner = w::panel(f, top, "PROCESSOR CORE", None, &p, ui);
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
    let log = app.cpu_lines.clone();
    log_pane(f, app, bottom, "PROCESSOR LOG", loading && app.cpu.is_none(), log);
}

// ─── help overlay ───────────────────────────────────────────────────────────

fn help(f: &mut Frame, app: &App, body: Rect) {
    let (p, ui) = (&app.pal, app.ui);
    let r = w::centered(body, 64, 17);
    f.render_widget(Clear, r);
    let right = Line::from(Span::styled(" any key closes ", p.fg(p.dim)));
    let inner = w::panel(f, r, "COMMAND REFERENCE", Some(right), p, ui);
    let nav = if ui.plain { "up/dn j/k" } else { "↑ ↓  j k" };
    let key = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<16}"), p.bold(p.accent)),
            Span::styled(d.to_string(), p.fg(p.fg)),
        ])
    };
    let lines = vec![
        w::caption("NAVIGATION", inner.width, p, ui),
        key(nav, "move / scroll"),
        key("pgup pgdn space", "page"),
        key("g G  home end", "top / bottom"),
        key("enter", "open"),
        key("esc  b", "back"),
        key("1 2 3", "jump to storage / memory / processor"),
        w::caption("ACTIONS", inner.width, p, ui),
        key("r", "rescan devices / refresh reading"),
        key("c", "copy log to clipboard (OSC 52)"),
        key("?", "this reference"),
        key("q", "quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  colour: DCHECK_COLOR=truecolor|ansi, NO_COLOR, --light",
            p.fg(p.dim),
        )),
        Line::from(Span::styled(
            "  glyphs: --plain   splash: DCHECK_NO_SPLASH   mouse: --mouse",
            p.fg(p.dim),
        )),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).style(Style::default()), inner);
}
