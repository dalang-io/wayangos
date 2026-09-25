//! Screens. Layout: header bar, step tracker, body panels, notice, keycaps.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use crate::app::{App, InputKind, Modal, Screen, Tone, ACCESS_ITEMS, WELCOME_ITEMS};
use crate::hud::{self, Theme};
use crate::install::STEPS;
use crate::sys::human;

const MAX_W: u16 = 120;
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn draw(f: &mut Frame, app: &App) {
    let t = &app.t;
    let area = f.area();
    f.render_widget(Block::default().style(t.base()), area);

    let [header, steps, body, notice, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if app.screen == Screen::Welcome { 0 } else { 2 }),
        Constraint::Min(8),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, app, header);
    if app.screen != Screen::Welcome {
        draw_steps(f, app, steps);
    }
    // the big logo needs room, and the welcome height depends on it
    let scale = if body.width >= 76 && body.height >= 24 {
        2
    } else {
        1
    };
    let body = hud::centered(body, MAX_W, screen_height(app, scale).min(body.height));
    match app.screen {
        Screen::Welcome => welcome(f, app, body, scale),
        Screen::Target => target(f, app, body),
        Screen::Access => access(f, app, body),
        Screen::Confirm => confirm(f, app, body),
        Screen::Installing => installing(f, app, body),
        Screen::Done => done(f, app, body),
        Screen::Failed => failed(f, app, body),
    }
    if let Some((tone, text)) = &app.notice {
        let (c, sym) = tone_style(t, *tone);
        let line = Line::from(vec![
            Span::styled(format!(" {sym} "), t.bold(c)),
            Span::styled(text.clone(), t.fg(c)),
        ]);
        f.render_widget(Paragraph::new(line), notice);
    }
    f.render_widget(
        Paragraph::new(hud::keycaps(&footer_keys(app), t)).style(t.base()),
        footer,
    );
    draw_modal(f, app, area);
}

/// Rows a screen needs; it is centred in whatever is left.
fn screen_height(app: &App, scale: u16) -> u16 {
    let steps = STEPS.len() as u16;
    match app.screen {
        // logo + tagline, then the SYSTEM panel: 5 fields + one per address
        Screen::Welcome => 2 * scale + 3 + 7 + app.sys.net.len().max(1) as u16,
        Screen::Target => {
            let parts = app
                .disks
                .iter()
                .map(|d| d.parts.len().max(1))
                .max()
                .unwrap_or(1) as u16;
            (app.disks.len() as u16 + 3).max(6) + parts + 7
        }
        Screen::Access => 20,
        Screen::Confirm | Screen::Done => 20,
        Screen::Installing | Screen::Failed => steps + 8 + 12,
    }
}

fn tone_style(t: &Theme, tone: Tone) -> (ratatui::style::Color, &'static str) {
    match tone {
        Tone::Ok => (t.ok, t.g.ok),
        Tone::Warn => (t.warn, t.g.warn),
        Tone::Bad => (t.bad, t.g.bad),
    }
}

fn footer_keys(app: &App) -> Vec<(&'static str, &'static str)> {
    if let Modal::Input { .. } = app.modal {
        return vec![("enter", "ok"), ("esc", "cancel")];
    }
    match app.screen {
        Screen::Welcome => vec![
            ("↑↓", "nav"),
            ("enter", "select"),
            ("1-4", "jump"),
            ("q", "shell"),
        ],
        Screen::Target => vec![
            ("↑↓", "select"),
            ("enter", "use this disk"),
            ("r", "rescan"),
            ("esc", "back"),
        ],
        Screen::Access => vec![
            ("↑↓", "nav"),
            ("enter", "select"),
            ("d", "remove key"),
            ("esc", "back"),
        ],
        Screen::Confirm => vec![("YES", "+ enter to install"), ("esc", "back")],
        Screen::Installing => vec![("··", "please wait - do not power off")],
        Screen::Done => vec![("enter", "reboot"), ("s", "shell"), ("p", "power off")],
        Screen::Failed => vec![("b", "back to disks"), ("s", "shell"), ("r", "reboot")],
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let bar = t.bar.map(|c| Style::default().bg(c)).unwrap_or_default();
    // ꦮꦪꦁ only where the font has it: not on the kernel console
    let aksara = if t.g.aksara { "ꦮꦪꦁ " } else { "" };
    let left = Line::from(vec![
        Span::styled(format!(" {} ", t.g.brand), t.bold(t.accent2)),
        Span::styled(format!("{aksara}WAYANGOS"), t.bold(t.accent)),
        Span::styled(" // SYSTEM INSTALLER", t.fg(t.dim)),
        Span::styled(format!("  v{VERSION}"), t.fg(t.dim)),
    ]);
    let net = match app.sys.net.first() {
        Some((i, a)) => format!("{i} {a}"),
        None => "no network".into(),
    };
    let right = Line::from(vec![
        Span::styled(
            if app.sys.uefi { "UEFI" } else { "BIOS" },
            t.fg(if app.sys.uefi { t.ok } else { t.warn }),
        ),
        Span::styled(format!(" {} ", t.g.dot), t.fg(t.dim)),
        Span::styled(
            format!("{net} "),
            t.fg(if app.sys.net.is_empty() { t.warn } else { t.fg }),
        ),
    ]);
    f.render_widget(Paragraph::new(left).style(bar), area);
    f.render_widget(
        Paragraph::new(right).alignment(Alignment::Right).style(bar),
        area,
    );
}

/// `01 TARGET ── 02 ACCESS ── 03 CONFIRM ── 04 INSTALL ── 05 DONE`
fn draw_steps(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let current = match app.screen {
        Screen::Target => 0,
        Screen::Access => 1,
        Screen::Confirm => 2,
        Screen::Installing | Screen::Failed => 3,
        _ => 4,
    };
    let mut spans = vec![Span::raw(" ")];
    for (i, name) in ["TARGET", "ACCESS", "CONFIRM", "INSTALL", "DONE"]
        .iter()
        .enumerate()
    {
        if i > 0 {
            spans.push(Span::styled(
                " ──── ",
                t.fg(if i <= current { t.accent } else { t.border }),
            ));
        }
        let failed = app.screen == Screen::Failed && i == current;
        let (num, style) = if failed {
            (t.g.bad.to_string(), t.bold(t.bad))
        } else if i < current || app.screen == Screen::Done {
            (t.g.ok.to_string(), t.bold(t.ok))
        } else if i == current {
            (format!("{:02}", i + 1), t.bold(t.accent2))
        } else {
            (format!("{:02}", i + 1), t.fg(t.dim))
        };
        spans.push(Span::styled(format!("{num} "), style));
        spans.push(Span::styled(
            name.to_string(),
            if i == current {
                t.bold(t.accent)
            } else {
                style
            },
        ));
    }
    let area = Rect {
        y: area.y + 1,
        height: 1,
        ..area
    };
    f.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Center),
        area,
    );
}

// ---- screens -----------------------------------------------------------

fn welcome(f: &mut Frame, app: &App, area: Rect, scale: u16) {
    let t = &app.t;
    let logo = hud::logo(scale as usize, t);
    let logo_h = logo.len() as u16 + 3;
    let [top, bottom] =
        Layout::vertical([Constraint::Length(logo_h), Constraint::Min(6)]).areas(area);

    let mut lines = vec![Line::raw("")];
    lines.extend(logo);
    lines.push(Line::from(Span::styled(
        "THE SHADOW THAT POWERS THE MACHINE",
        t.fg(t.dim),
    )));
    f.render_widget(
        Paragraph::new(Text::from(lines)).alignment(Alignment::Center),
        top,
    );

    let [menu, info] =
        Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]).areas(bottom);
    let inner = hud::panel(f, menu, "INSTALLER", None, t);
    let items: Vec<Line> = WELCOME_ITEMS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            menu_line(
                t,
                i == app.welcome_sel,
                &format!("{:02}", i + 1),
                name,
                inner.width,
            )
        })
        .collect();
    f.render_widget(Paragraph::new(Text::from(items)), pad(inner));

    let inner = hud::panel(f, info, "SYSTEM", None, t);
    let w = 11;
    let mut lines = vec![
        hud::field(
            "CPU",
            w,
            vec![Span::styled(
                t.clip(
                    &app.sys.cpu,
                    inner.width.saturating_sub(w as u16 + 2) as usize,
                ),
                t.fg(t.fg),
            )],
            t,
        ),
        hud::field(
            "MEMORY",
            w,
            vec![Span::styled(human_mem(app.sys.mem_bytes), t.fg(t.fg))],
            t,
        ),
        hud::field(
            "FIRMWARE",
            w,
            if app.sys.uefi {
                vec![Span::styled(format!("{} UEFI", t.g.ok), t.bold(t.ok))]
            } else {
                vec![Span::styled(
                    format!("{} BIOS - the installed disk boots UEFI only", t.g.warn),
                    t.bold(t.warn),
                )]
            },
            t,
        ),
    ];
    if app.sys.net.is_empty() {
        lines.push(hud::field(
            "NETWORK",
            w,
            vec![Span::styled(
                format!("{} no address yet", t.g.warn),
                t.fg(t.warn),
            )],
            t,
        ));
    }
    for (i, (iface, addr)) in app.sys.net.iter().enumerate() {
        let label = if i == 0 { "NETWORK" } else { "" };
        lines.push(hud::field(
            label,
            w,
            vec![
                Span::styled(format!("{iface}  "), t.fg(t.dim)),
                Span::styled(addr.clone(), t.bold(t.fg)),
            ],
            t,
        ));
    }
    let eligible = app.disks.iter().filter(|d| d.blocked().is_none()).count();
    lines.push(hud::field(
        "STORAGE",
        w,
        vec![Span::styled(
            format!(
                "{} device(s) {} {eligible} installable",
                app.disks.len(),
                t.g.dot
            ),
            t.fg(t.fg),
        )],
        t,
    ));
    lines.push(hud::field(
        "IMAGE",
        w,
        if app.missing.is_empty() {
            vec![Span::styled(format!("{} ready", t.g.ok), t.fg(t.ok))]
        } else {
            vec![Span::styled(
                format!("{} missing {}", t.g.bad, app.missing.join(", ")),
                t.fg(t.bad),
            )]
        },
        t,
    ));
    f.render_widget(Paragraph::new(Text::from(lines)), pad(inner));
}

fn human_mem(b: u64) -> String {
    format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)
}

fn menu_line(t: &Theme, selected: bool, num: &str, name: &str, width: u16) -> Line<'static> {
    let text = format!("{}{num}  {name}", if selected { t.g.cursor } else { "  " });
    let text = format!("{text:<w$}", w = width.saturating_sub(2) as usize);
    if selected {
        Line::from(Span::styled(text, t.highlight()))
    } else {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(num.to_string(), t.fg(t.dim)),
            Span::styled(format!("  {name}"), t.bold(t.fg)),
        ])
    }
}

fn pad(r: Rect) -> Rect {
    Rect {
        x: r.x + 1,
        width: r.width.saturating_sub(2),
        ..r
    }
}

fn target(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let parts = app
        .disks
        .get(app.disk_sel)
        .map_or(1, |d| d.parts.len().max(1)) as u16;
    let detail_h = (parts + 7).min(area.height / 2);
    let [list, detail] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(detail_h)]).areas(area);

    let right = Line::from(Span::styled(
        format!(" {} device(s) ", app.disks.len()),
        t.fg(t.dim),
    ));
    let inner = hud::panel(f, list, "STORAGE ARRAY", Some(right), t);
    let header = Row::new(
        ["DEVICE", "TYPE", "MODEL", "SIZE", "CONTENTS"]
            .map(|h| Cell::from(h).style(t.bold(t.accent))),
    );
    let rows: Vec<Row> = app
        .disks
        .iter()
        .map(|d| {
            let blocked = d.blocked().is_some();
            let dim = |s: Style| if blocked { t.fg(t.dim) } else { s };
            let contents = d.contents();
            let contents_style = if blocked {
                t.fg(t.dim)
            } else if d.is_empty() {
                t.fg(t.ok)
            } else {
                t.fg(t.warn)
            };
            Row::new(vec![
                Cell::from(d.path()).style(dim(t.bold(t.fg))),
                Cell::from(d.kind()).style(dim(t.fg(t.accent))),
                Cell::from(d.model.clone()).style(dim(t.fg(t.fg))),
                Cell::from(Line::from(human(d.size)).alignment(Alignment::Right))
                    .style(dim(t.bold(t.fg))),
                Cell::from(contents).style(contents_style),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(12),
            Constraint::Fill(3),
            Constraint::Length(8),
            Constraint::Fill(2),
        ],
    )
    .header(header)
    .column_spacing(2)
    .row_highlight_style(t.highlight())
    .highlight_symbol(t.g.cursor);
    let mut state = TableState::default().with_selected(Some(app.disk_sel));
    f.render_stateful_widget(table, inner, &mut state);
    if app.disks.is_empty() {
        let msg = Paragraph::new(Span::styled(
            "no disks detected - attach one and press r",
            t.fg(t.warn),
        ))
        .alignment(Alignment::Center);
        f.render_widget(
            msg,
            Rect {
                y: inner.y + 2,
                height: 1,
                ..inner
            },
        );
    }

    let Some(d) = app.disks.get(app.disk_sel) else {
        return;
    };
    let inner = hud::panel(
        f,
        detail,
        &format!("TARGET {} {}", t.g.arrow, d.path()),
        None,
        t,
    );
    let w = 10;
    let mut lines = vec![
        hud::field(
            "MODEL",
            w,
            vec![
                Span::styled(d.model.clone(), t.bold(t.fg)),
                Span::styled(format!("  {}  {}", t.g.dot, d.kind()), t.fg(t.accent)),
                Span::styled(format!("  {}  {}", t.g.dot, human(d.size)), t.bold(t.fg)),
            ],
            t,
        ),
        hud::field(
            "SERIAL",
            w,
            vec![Span::styled(
                d.serial.clone().unwrap_or_else(|| "-".into()),
                t.fg(t.fg),
            )],
            t,
        ),
    ];
    if d.parts.is_empty() {
        let what = match (&d.whole.kind, &d.whole.label) {
            (None, _) => "no partitions, no filesystem".to_string(),
            (Some(k), Some(l)) => format!("{k} filesystem '{l}' on the whole disk"),
            (Some(k), None) => format!("{k} filesystem on the whole disk"),
        };
        lines.push(hud::field(
            "CONTENTS",
            w,
            vec![Span::styled(what, t.fg(t.fg))],
            t,
        ));
    } else {
        for (i, p) in d.parts.iter().enumerate() {
            let label = if i == 0 { "CONTENTS" } else { "" };
            let fs = p.fs.kind.clone().unwrap_or_else(|| "raw".into());
            let name =
                p.fs.label
                    .clone()
                    .map(|l| format!("'{l}'"))
                    .unwrap_or_default();
            lines.push(hud::field(
                label,
                w,
                vec![
                    Span::styled(format!("{} {:<12}", t.g.bullet, p.name), t.fg(t.dim)),
                    Span::styled(format!("{:>8}  ", human(p.size)), t.fg(t.fg)),
                    Span::styled(format!("{fs:<9}"), t.fg(t.accent)),
                    Span::styled(name, t.fg(t.fg)),
                ],
                t,
            ));
        }
    }
    let verdict = match d.blocked() {
        Some(why) => Span::styled(format!("{} not selectable: {why}", t.g.bad), t.bold(t.bad)),
        None if d.is_empty() => Span::styled(
            format!("{} empty disk - ready to install", t.g.ok),
            t.bold(t.ok),
        ),
        None => Span::styled(
            format!("{} everything above will be ERASED", t.g.warn),
            t.bold(t.warn),
        ),
    };
    lines.push(Line::raw(""));
    lines.push(Line::from(verdict));
    f.render_widget(Paragraph::new(Text::from(lines)), pad(inner));
}

fn access(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let [menu, keys] =
        Layout::horizontal([Constraint::Length(44), Constraint::Min(30)]).areas(area);

    let inner = hud::panel(f, menu, "ACCESS", None, t);
    let mut lines = Vec::new();
    for (i, name) in ACCESS_ITEMS.iter().enumerate() {
        if i == ACCESS_ITEMS.len() - 1 {
            lines.push(Line::raw(""));
        }
        let label = if i == 0 {
            format!("{name}  {}", app.hostname)
        } else {
            name.to_string()
        };
        let num = if i == ACCESS_ITEMS.len() - 1 {
            t.g.arrow.to_string()
        } else {
            format!("{:02}", i + 1)
        };
        lines.push(menu_line(t, app.access_sel == i, &num, &label, inner.width));
        if i == 0 {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "  ADD SSH KEYS FOR root",
                t.fg(t.dim),
            )));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  Password logins are disabled:",
        t.fg(t.dim),
    )));
    lines.push(Line::from(Span::styled(
        "  only these keys can log in over SSH.",
        t.fg(t.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), pad(inner));

    let right = Line::from(Span::styled(
        format!(" {} key(s) ", app.keys.len()),
        t.fg(t.dim),
    ));
    let inner = hud::panel(f, keys, "AUTHORIZED KEYS", Some(right), t);
    let mut lines = Vec::new();
    if app.keys.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("{} none yet", t.g.warn),
            t.bold(t.warn),
        )));
        lines.push(Line::from(Span::styled(
            "fetch them from GitHub/GitLab, a USB stick,",
            t.fg(t.dim),
        )));
        lines.push(Line::from(Span::styled(
            "or paste one. Each box can use its own keys.",
            t.fg(t.dim),
        )));
    }
    // 2 rows per key; scroll so the selected one stays in view
    let fits = (inner.height.saturating_sub(2) / 2).max(1) as usize;
    let sel_key = app.access_sel.checked_sub(ACCESS_ITEMS.len());
    let first = sel_key.map_or(0, |s| (s + 1).saturating_sub(fits));
    if first > 0 {
        lines.push(Line::from(Span::styled(
            format!("  {} {first} more above", t.g.bullet),
            t.fg(t.dim),
        )));
    }
    for (i, k) in app.keys.iter().enumerate().skip(first).take(fits) {
        let selected = sel_key == Some(i);
        let head = format!(
            "{}{:<10} {}",
            if selected { t.g.cursor } else { "  " },
            k.kind(),
            k.fingerprint()
        );
        let head = format!("{head:<w$}", w = inner.width.saturating_sub(2) as usize);
        lines.push(Line::from(Span::styled(
            head,
            if selected {
                t.highlight()
            } else {
                t.bold(t.fg)
            },
        )));
        let comment = if k.comment.is_empty() {
            "(no comment)".to_string()
        } else {
            k.comment.clone()
        };
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(t.clip(&comment, 28), t.fg(t.accent)),
            Span::styled(format!("  {} {}", t.g.dot, k.source), t.fg(t.dim)),
        ]));
    }
    if !app.keys.is_empty() {
        if let Some(ip) = app.ip() {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("these keys work now too: ", t.fg(t.dim)),
                Span::styled(format!("ssh root@{ip}"), t.bold(t.accent)),
            ]));
        }
    }
    f.render_widget(Paragraph::new(Text::from(lines)), pad(inner));
}

fn confirm(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let Some(d) = &app.target else { return };
    let inner = hud::panel(f, hud::centered(area, 90, 20), "FINAL CHECK", None, t);
    let w = 11;
    let keys = if app.keys.is_empty() {
        Span::styled(
            format!("{} none - console login only", t.g.warn),
            t.bold(t.warn),
        )
    } else {
        Span::styled(
            format!("{} key(s): {}", app.keys.len(), key_names(app)),
            t.fg(t.fg),
        )
    };
    let mut lines = vec![
        hud::field(
            "TARGET",
            w,
            vec![
                Span::styled(d.path(), t.bold(t.accent)),
                Span::styled(
                    format!(
                        "  {}  {}  {}  {}",
                        d.model,
                        t.g.dot,
                        human(d.size),
                        d.kind()
                    ),
                    t.fg(t.fg),
                ),
            ],
            t,
        ),
        hud::field(
            "ERASES",
            w,
            vec![Span::styled(
                d.contents(),
                t.fg(if d.is_empty() { t.ok } else { t.warn }),
            )],
            t,
        ),
        hud::field(
            "HOSTNAME",
            w,
            vec![Span::styled(app.hostname.clone(), t.bold(t.fg))],
            t,
        ),
        hud::field("SSH", w, vec![keys], t),
        Line::raw(""),
        hud::caption("NEW LAYOUT", inner.width.saturating_sub(2), t),
        hud::field(
            "",
            w,
            vec![
                Span::styled(format!("{:<16}", d.part_path(1)), t.fg(t.fg)),
                Span::styled(
                    "512 MiB  FAT32  WAYANGBOOT  boot loader + system",
                    t.fg(t.dim),
                ),
            ],
            t,
        ),
        hud::field(
            "",
            w,
            vec![
                Span::styled(format!("{:<16}", d.part_path(2)), t.fg(t.fg)),
                Span::styled(
                    format!(
                        "{:<7}  EXT4   WAYANGDATA  /data (kept across reboots)",
                        human(d.size.saturating_sub(537_000_000))
                    ),
                    t.fg(t.dim),
                ),
            ],
            t,
        ),
        Line::raw(""),
        Line::from(Span::styled(
            format!(
                "{} EVERYTHING ON {} WILL BE DESTROYED. THIS CANNOT BE UNDONE.",
                t.g.warn,
                d.path()
            ),
            t.bold(t.bad),
        )),
        Line::raw(""),
    ];
    let cursor = blink(app);
    let typed_ok = app.confirm == "YES";
    lines.push(Line::from(vec![
        Span::styled("TYPE ", t.fg(t.dim)),
        Span::styled("YES", t.bold(t.accent2)),
        Span::styled(" TO INSTALL  ", t.fg(t.dim)),
        Span::styled(format!("{} ", t.g.arrow), t.fg(t.accent)),
        Span::styled(
            app.confirm.clone(),
            t.bold(if typed_ok { t.ok } else { t.fg }),
        ),
        Span::styled(cursor, t.fg(t.accent)),
    ]));
    if typed_ok {
        lines.push(Line::from(Span::styled(
            "press ENTER to begin",
            t.bold(t.ok),
        )));
    }
    f.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        pad(inner),
    );
}

/// Text cursor, blinking with the poll ticks.
fn blink(app: &App) -> &'static str {
    if (app.tick / 5).is_multiple_of(2) {
        "█"
    } else {
        " "
    }
}

fn key_names(app: &App) -> String {
    let names: Vec<&str> = app
        .keys
        .iter()
        .map(|k| {
            if k.comment.is_empty() {
                k.kind()
            } else {
                k.comment.as_str()
            }
        })
        .collect();
    app.t.clip(&names.join(", "), 50)
}

fn step_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let t = &app.t;
    let failed = app.screen == Screen::Failed;
    STEPS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let (sym, style) = if i < app.step {
                (t.g.ok.to_string(), t.bold(t.ok))
            } else if i == app.step && failed {
                (t.g.bad.to_string(), t.bold(t.bad))
            } else if i == app.step && app.step < STEPS.len() {
                (
                    t.g.spinner[app.tick / 2 % t.g.spinner.len()].to_string(),
                    t.bold(t.accent2),
                )
            } else {
                (t.g.todo.to_string(), t.fg(t.dim))
            };
            let mut spans = vec![
                Span::styled(format!(" {sym}  "), style),
                Span::styled(
                    format!("{:02}  {name:<20}", i + 1),
                    if i == app.step {
                        t.bold(t.accent)
                    } else {
                        style
                    },
                ),
            ];
            if i == 4 && app.step == 4 && !failed {
                let cells = (width as usize).saturating_sub(36).clamp(8, 40);
                spans.extend(hud::gauge(app.copy, cells, t.accent, t));
                spans.push(Span::styled(
                    format!(" {:>3.0}%", app.copy * 100.0),
                    t.bold(t.fg),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn installing(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let Some(d) = &app.target else { return };
    let [top, log] = Layout::vertical([
        Constraint::Length(STEPS.len() as u16 + 6),
        Constraint::Min(4),
    ])
    .areas(area);
    let inner = hud::panel(
        f,
        top,
        &format!("INSTALLING {} {}", t.g.arrow, d.path()),
        None,
        t,
    );
    let mut lines = step_lines(app, inner.width);
    lines.push(Line::raw(""));
    let cells = (inner.width as usize).saturating_sub(22).clamp(10, 70);
    let mut total = vec![Span::styled(" PROGRESS  ", t.fg(t.dim))];
    total.extend(hud::gauge(app.progress(), cells, t.accent2, t));
    total.push(Span::styled(
        format!(" {:>3.0}%", app.progress() * 100.0),
        t.bold(t.fg),
    ));
    lines.push(Line::from(total));
    f.render_widget(Paragraph::new(Text::from(lines)), pad(inner));
    log_panel(f, app, log);
}

fn log_panel(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let inner = hud::panel(f, area, "LOG", None, t);
    let n = inner.height as usize;
    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(app.log.len().saturating_sub(n))
        .map(|l| {
            let style = if l.starts_with("ERROR") {
                t.fg(t.bad)
            } else {
                t.fg(t.dim)
            };
            Line::from(vec![
                Span::styled(format!("{} ", t.g.bullet), t.fg(t.border)),
                Span::styled(l.clone(), style),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(Text::from(lines)), pad(inner));
}

fn done(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let Some(d) = &app.target else { return };
    let inner = hud::panel(f, hud::centered(area, 84, 18), "INSTALL COMPLETE", None, t);
    let mut lines = vec![
        Line::raw(""),
        Line::from(hud::badge(t.ok, t.g.ok, "WAYANGOS INSTALLED", t)).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(vec![
            Span::styled("on ", t.fg(t.dim)),
            Span::styled(d.path(), t.bold(t.accent)),
            Span::styled(format!("  {}  {}", d.model, t.g.dot), t.fg(t.fg)),
            Span::styled(format!(" hostname {}", app.hostname), t.fg(t.fg)),
        ])
        .alignment(Alignment::Center),
        Line::raw(""),
        hud::caption("NEXT", inner.width.saturating_sub(2), t),
        Line::from(vec![
            Span::styled(" 1  ", t.bold(t.accent2)),
            Span::styled("Remove the USB stick.", t.bold(t.fg)),
        ]),
        Line::from(vec![
            Span::styled(" 2  ", t.bold(t.accent2)),
            Span::styled("Press ", t.fg(t.fg)),
            Span::styled("ENTER", t.bold(t.accent)),
            Span::styled(" to reboot into WayangOS.", t.fg(t.fg)),
        ]),
    ];
    let ssh = match (app.keys.is_empty(), app.ip()) {
        (true, _) => vec![Span::styled(
            "No SSH keys: log in on the console, then run wayang-addkey.",
            t.fg(t.warn),
        )],
        (false, Some(ip)) => vec![
            Span::styled("From your computer: ", t.fg(t.fg)),
            Span::styled(format!("ssh root@{ip}"), t.bold(t.accent)),
            Span::styled(" (the address may change)", t.fg(t.dim)),
        ],
        (false, None) => vec![Span::styled(
            "Then connect with ssh root@<its address>.",
            t.fg(t.fg),
        )],
    };
    let mut row = vec![Span::styled(" 3  ", t.bold(t.accent2))];
    row.extend(ssh);
    lines.push(Line::from(row));
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(" More keys later: ", t.fg(t.dim)),
        Span::styled("wayang-addkey github:USER", t.fg(t.accent)),
    ]));
    lines.push(Line::from(Span::styled(
        " The disk boots UEFI only, with Secure Boot off.",
        t.fg(t.dim),
    )));
    f.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        pad(inner),
    );
}

fn failed(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    let [top, log] = Layout::vertical([
        Constraint::Length(STEPS.len() as u16 + 7),
        Constraint::Min(4),
    ])
    .areas(area);
    let inner = hud::panel(f, top, "INSTALL FAILED", None, t);
    let mut lines = step_lines(app, inner.width);
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{} {}", t.g.bad, app.error.clone().unwrap_or_default()),
        t.bold(t.bad),
    )));
    lines.push(Line::from(Span::styled(
        "The disk may be partly written. Fix the cause and install again.",
        t.fg(t.dim),
    )));
    f.render_widget(
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
        pad(inner),
    );
    log_panel(f, app, log);
}

fn draw_modal(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.t;
    match &app.modal {
        Modal::None => {}
        Modal::Busy { label, .. } => {
            let r = hud::centered(area, 64, 5);
            f.render_widget(Clear, r);
            let inner = hud::panel(f, r, "WORKING", None, t);
            let spin = t.g.spinner[app.tick / 2 % t.g.spinner.len()];
            let line = Line::from(vec![
                Span::styled(format!("{spin}  "), t.bold(t.accent2)),
                Span::styled(label.clone(), t.bold(t.fg)),
            ]);
            f.render_widget(
                Paragraph::new(vec![Line::raw(""), line]).alignment(Alignment::Center),
                inner,
            );
        }
        Modal::Input { kind, buf, error } => {
            let (title, prompt, hint) = match kind {
                InputKind::Hostname => (
                    "HOSTNAME",
                    "Name of this box on the network:",
                    "letters, digits and '-'",
                ),
                InputKind::Remote => (
                    "FETCH KEYS",
                    "GitHub user name (or gitlab:NAME):",
                    "downloads https://github.com/NAME.keys - needs network",
                ),
                InputKind::TypeKey => (
                    "PASTE A KEY",
                    "One public key line:",
                    "ssh-ed25519 AAAA... you@laptop (the .pub file, never the private key)",
                ),
            };
            let r = hud::centered(area, 76, 9);
            f.render_widget(Clear, r);
            let inner = hud::panel(f, r, title, None, t);
            let room = inner.width.saturating_sub(6) as usize;
            let shown: String = {
                let n = buf.chars().count();
                buf.chars().skip(n.saturating_sub(room)).collect()
            };
            let cursor = blink(app);
            let field = Line::from(vec![
                Span::styled(format!("{} ", t.g.arrow), t.bold(t.accent2)),
                Span::styled(shown, t.bold(t.fg).add_modifier(Modifier::UNDERLINED)),
                Span::styled(cursor, t.fg(t.accent)),
            ]);
            let msg = match error {
                Some(e) => Line::from(Span::styled(format!("{} {e}", t.g.bad), t.fg(t.bad))),
                None => Line::from(Span::styled(hint.to_string(), t.fg(t.dim))),
            };
            let lines = vec![
                Line::raw(""),
                Line::from(Span::styled(prompt.to_string(), t.fg(t.fg))),
                Line::raw(""),
                field,
                Line::raw(""),
                msg,
            ];
            f.render_widget(Paragraph::new(lines), pad(inner));
        }
    }
}
