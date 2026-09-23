//! Terminal UI (ratatui). Entered for interactive use; falls back to the plain
//! text menu when stdout is not a terminal.
//!
//! UX notes: SMART reads happen on background threads (spinner while loading),
//! the list shows per-device health, report has a contextual title + position
//! indicator, there is a help overlay, mouse scrolling, and `NO_COLOR` support.

use std::io::{self, Stdout, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::{Frame, Terminal};

use crate::model::Device;
use crate::report;

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
const VERSION: &str = env!("CARGO_PKG_VERSION");

struct Palette {
    accent: Color,
    dim: Color,
    hl_bg: Color,
    hl_fg: Color,
    row_bg: Color,
    row_fg: Color,
    ok: Color,
    warn: Color,
    bad: Color,
    mono: bool,
}

impl Palette {
    fn for_theme(light: bool) -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            return Palette {
                accent: Color::Reset,
                dim: Color::Reset,
                hl_bg: Color::Reset,
                hl_fg: Color::Reset,
                row_bg: Color::Reset,
                row_fg: Color::Reset,
                ok: Color::Reset,
                warn: Color::Reset,
                bad: Color::Reset,
                mono: true,
            };
        }
        if light {
            Palette {
                accent: Color::Rgb(140, 90, 0),
                dim: Color::Rgb(80, 80, 80),
                hl_bg: Color::Rgb(230, 200, 120),
                hl_fg: Color::Black,
                row_bg: Color::Rgb(243, 234, 214),
                row_fg: Color::Rgb(120, 80, 0),
                ok: Color::Rgb(0, 120, 0),
                warn: Color::Rgb(170, 100, 0),
                bad: Color::Rgb(170, 0, 0),
                mono: false,
            }
        } else {
            Palette {
                accent: Color::Rgb(200, 148, 26),
                dim: Color::Gray,
                hl_bg: Color::Rgb(200, 148, 26),
                hl_fg: Color::Black,
                row_bg: Color::Rgb(40, 32, 16),
                row_fg: Color::Rgb(240, 200, 80),
                ok: Color::Rgb(120, 200, 120),
                warn: Color::Rgb(230, 180, 60),
                bad: Color::Rgb(240, 100, 100),
                mono: false,
            }
        }
    }

    fn severity(&self, sev: u8) -> Color {
        match sev {
            0 => self.ok,
            2 => self.warn,
            3 | 4 => self.bad,
            _ => self.dim,
        }
    }
}

#[derive(Clone)]
enum Screen {
    Menu,
    Storage,
    Report,
    Notice(String, String),
    Help,
}

struct Pending {
    rx: Receiver<Vec<String>>,
    tick: u8,
}

pub struct App {
    devices: Vec<Device>,
    demo: bool,
    light: bool,
    screen: Screen,
    help_prev: Option<Screen>,
    menu: ListState,
    table: TableState,
    report_lines: Vec<String>,
    report_dev: Option<String>,
    scroll: u16,
    view_height: u16,
    health: Vec<(String, u8)>,
    health_rx: Option<Receiver<Vec<(String, u8)>>>,
    pending: Option<Pending>,
    mouse: bool,
    status: Option<String>,
}

impl App {
    fn new(devices: Vec<Device>, light: bool, demo: bool, mouse: bool) -> Self {
        let mut menu = ListState::default();
        menu.select(Some(0));
        let mut table = TableState::default();
        if !devices.is_empty() {
            table.select(Some(0));
        }
        let mut app = App {
            health: vec![("…".to_string(), 1); devices.len()],
            devices,
            demo,
            light,
            mouse,
            status: None,
            screen: Screen::Menu,
            help_prev: None,
            menu,
            table,
            report_lines: Vec::new(),
            report_dev: None,
            scroll: 0,
            view_height: 1,
            health_rx: None,
            pending: None,
        };
        app.spawn_health();
        app
    }

    fn spawn_health(&mut self) {
        let devices = self.devices.clone();
        self.health = vec![("…".to_string(), 1); devices.len()];
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let out: Vec<(String, u8)> = devices
                .iter()
                .map(|d| {
                    report::health_summary(d)
                        .map(|h| (h.verdict.label().to_string(), h.verdict.severity()))
                        .unwrap_or_else(|| ("UNKNOWN".to_string(), 1))
                })
                .collect();
            let _ = tx.send(out);
        });
        self.health_rx = Some(rx);
    }

    fn start_report(&mut self, index: usize) {
        let Some(dev) = self.devices.get(index).cloned() else {
            return;
        };
        let path = dev.path.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(report::device_report_lines(&dev));
        });
        self.report_lines.clear();
        self.report_dev = Some(path);
        self.scroll = 0;
        self.pending = Some(Pending { rx, tick: 0 });
        self.screen = Screen::Report;
    }

    fn rescan(&mut self) {
        self.devices = reload_devices(self.demo);
        self.table = TableState::default();
        if !self.devices.is_empty() {
            self.table.select(Some(0));
        }
        self.spawn_health();
    }

    fn health_for(&self, i: usize) -> (String, u8, bool) {
        let (label, sev) = self
            .health
            .get(i)
            .cloned()
            .unwrap_or_else(|| ("…".to_string(), 1));
        (label, sev, self.health_rx.is_some())
    }

    fn open_help(&mut self) {
        self.help_prev = Some(self.screen.clone());
        self.screen = Screen::Help;
    }

    fn close_help(&mut self) {
        if let Some(prev) = self.help_prev.take() {
            self.screen = prev;
        } else {
            self.screen = Screen::Menu;
        }
    }

    fn max_scroll(&self) -> u16 {
        let visible = self.view_height.saturating_sub(2).max(1) as usize;
        self.report_lines.len().saturating_sub(visible) as u16
    }
}

/// Copy the current report to the clipboard via OSC 52.
fn copy_report(app: &mut App) {
    if app.report_lines.is_empty() {
        app.status = Some("nothing to copy yet".to_string());
        return;
    }
    let text = app.report_lines.join("\n");
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
    app.status = Some("report copied via OSC52 (terminal must support it)".to_string());
}

/// Minimal base64 encoder for the OSC 52 clipboard sequence.
fn base64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(T[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(T[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn reload_devices(demo: bool) -> Vec<Device> {
    if demo {
        return crate::enumerate::demo_devices();
    }
    let devices = crate::enumerate::list_devices();
    if devices.is_empty() && !crate::enumerate::has_sysfs() {
        crate::enumerate::demo_devices()
    } else {
        devices
    }
}

/// Run the TUI. Must be called on a real terminal.
pub fn run(devices: Vec<Device>, light: bool, demo: bool, mouse: bool) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    if mouse {
        execute!(stdout, EnableMouseCapture)?;
    }
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, devices, light, demo, mouse);

    disable_raw_mode()?;
    if mouse {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    devices: Vec<Device>,
    light: bool,
    demo: bool,
    mouse: bool,
) -> io::Result<()> {
    let mut app = App::new(devices, light, demo, mouse);
    loop {
        // Drain background results.
        if let Some(rx) = &app.health_rx {
            match rx.try_recv() {
                Ok(h) => {
                    app.health = h;
                    app.health_rx = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => app.health_rx = None,
            }
        }
        if let Some(pending) = &app.pending {
            match pending.rx.try_recv() {
                Ok(lines) => {
                    app.report_lines = lines;
                    app.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => app.pending = None,
            }
        }
        if let Some(pending) = &mut app.pending {
            pending.tick = pending.tick.wrapping_add(1);
        }

        terminal.draw(|f| draw(f, &mut app))?;

        let busy = app.pending.is_some() || app.health_rx.is_some();
        let timeout = if busy {
            Duration::from_millis(120)
        } else {
            Duration::from_millis(500)
        };

        if !event::poll(timeout)? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                app.status = None;
                if handle_key(&mut app, key.code) {
                    break;
                }
            }
            Event::Mouse(mouse) => {
                if app.mouse {
                    handle_mouse(&mut app, mouse.kind);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Returns true when the app should quit.
fn handle_key(app: &mut App, code: KeyCode) -> bool {
    if matches!(&app.screen, Screen::Help) {
        if code == KeyCode::Char('q') {
            return true;
        }
        app.close_help();
        return false;
    }
    if matches!(&app.screen, Screen::Notice(_, _)) {
        if code == KeyCode::Char('q') {
            return true;
        }
        app.screen = Screen::Menu;
        return false;
    }

    match app.screen.clone() {
        Screen::Menu => match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('?') => app.open_help(),
            KeyCode::Up | KeyCode::Char('k') => {
                let i = app.menu.selected().unwrap_or(0);
                app.menu.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = app.menu.selected().unwrap_or(0);
                app.menu.select(Some((i + 1).min(3)));
            }
            KeyCode::Home => app.menu.select(Some(0)),
            KeyCode::End => app.menu.select(Some(3)),
            KeyCode::Enter => match app.menu.selected().unwrap_or(0) {
                0 => app.screen = Screen::Storage,
                1 => app.screen = Screen::Notice("RAM".into(), "coming soon".into()),
                2 => app.screen = Screen::Notice("CPU".into(), "coming soon".into()),
                _ => return true,
            },
            _ => {}
        },
        Screen::Storage => match code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc => app.screen = Screen::Menu,
            KeyCode::Char('?') => app.open_help(),
            KeyCode::Char('r') => app.rescan(),
            KeyCode::Up | KeyCode::Char('k') => {
                let i = app.table.selected().unwrap_or(0);
                app.table.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = app.table.selected().unwrap_or(0);
                let max = app.devices.len().saturating_sub(1);
                app.table.select(Some((i + 1).min(max)));
            }
            KeyCode::Home => app.table.select(Some(0)),
            KeyCode::End => {
                let max = app.devices.len().saturating_sub(1);
                app.table.select(Some(max));
            }
            KeyCode::Enter => {
                if let Some(i) = app.table.selected() {
                    app.start_report(i);
                }
            }
            _ => {}
        },
        Screen::Report => match code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => app.screen = Screen::Storage,
            KeyCode::Char('?') => app.open_help(),
            KeyCode::Char('c') => copy_report(app),
            KeyCode::Down | KeyCode::Char('j') => app.scroll = app.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll = app.scroll.saturating_sub(1),
            KeyCode::PageDown => app.scroll = app.scroll.saturating_add(10),
            KeyCode::PageUp => app.scroll = app.scroll.saturating_sub(10),
            KeyCode::Char('g') | KeyCode::Home => app.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => app.scroll = app.max_scroll(),
            _ => {}
        },
        Screen::Help | Screen::Notice(_, _) => {}
    }
    false
}

fn handle_mouse(app: &mut App, kind: MouseEventKind) {
    match kind {
        MouseEventKind::ScrollUp => match app.screen {
            Screen::Report => app.scroll = app.scroll.saturating_sub(3),
            Screen::Storage => {
                let i = app.table.selected().unwrap_or(0);
                app.table.select(Some(i.saturating_sub(1)));
            }
            _ => {}
        },
        MouseEventKind::ScrollDown => match app.screen {
            Screen::Report => {
                let m = app.max_scroll();
                app.scroll = (app.scroll + 3).min(m);
            }
            Screen::Storage => {
                let i = app.table.selected().unwrap_or(0);
                let max = app.devices.len().saturating_sub(1);
                app.table.select(Some((i + 1).min(max)));
            }
            _ => {}
        },
        _ => {}
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    let palette = Palette::for_theme(app.light);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            "dcheck",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  {VERSION} — device health check")),
    ]))
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(title, chunks[0]);

    let screen = app.screen.clone();
    match screen {
        Screen::Menu => {
            let items: Vec<ListItem> = [
                "Storage",
                "RAM            (coming soon)",
                "CPU            (coming soon)",
                "Quit",
            ]
            .iter()
            .map(|s| ListItem::new(*s))
            .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Menu "))
                .highlight_style(highlight(&palette, 0))
                .highlight_symbol("> ");
            f.render_stateful_widget(list, chunks[1], &mut app.menu);
            hint(
                f,
                chunks[2],
                palette.dim,
                "↑/↓ move   Enter select   ? help   q quit",
            );
        }
        Screen::Storage => draw_storage(f, app, chunks[1], &palette, chunks[2]),
        Screen::Report => draw_report(f, app, chunks[1], &palette, chunks[2]),
        Screen::Notice(title, msg) => {
            let text = Text::from(vec![
                Line::from(""),
                Line::from(Span::styled(
                    title.clone(),
                    Style::default()
                        .fg(palette.accent)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(msg.clone()),
                Line::from(""),
                Line::from(Span::styled(
                    "press any key to go back",
                    Style::default().fg(palette.dim),
                )),
            ]);
            let p = Paragraph::new(text)
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::ALL).title(" Info "));
            f.render_widget(p, chunks[1]);
            hint(f, chunks[2], palette.dim, "any key back   q quit");
        }
        Screen::Help => {
            let lines: Vec<Line> = [
                "Navigation",
                "  ↑/↓ or j/k      move / scroll",
                "  PgUp / PgDn     page",
                "  g / G, Home/End top / bottom",
                "  Enter           select / open report",
                "  Esc or b        back",
                "Actions",
                "  r               rescan devices (Storage)",
                "  c               copy report (OSC52)",
                "  ?               this help",
                "  q               quit",
                "Mouse wheel scrolls lists and reports (with --mouse).",
                "Tip: text is selectable when mouse capture is off (default).",
                "Env: DCHECK_THEME, NO_COLOR, DCHECK_SMART_ARGS, DCHECK_NATIVE",
            ]
            .iter()
            .map(|l| Line::from(*l))
            .collect();
            let p = Paragraph::new(Text::from(lines))
                .block(Block::default().borders(Borders::ALL).title(" Help "))
                .wrap(Wrap { trim: false });
            f.render_widget(p, chunks[1]);
            hint(f, chunks[2], palette.dim, "any key back   q quit");
        }
    }

    // Status/toast takes over the footer until the next keypress.
    if let Some(status) = &app.status {
        hint(f, chunks[2], palette.accent, status);
    }
}

fn draw_storage(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    palette: &Palette,
    hint_area: Rect,
) {
    if app.devices.is_empty() {
        let p = Paragraph::new("No block devices found.")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Storage "));
        f.render_widget(p, area);
        hint(f, hint_area, palette.dim, "r rescan   Esc back   q quit");
        return;
    }

    let wide = area.width;
    let show_bus = wide >= 80;
    let show_mount = wide >= 95;

    let mut header = vec![Cell::from("DEVICE"), Cell::from("TYPE")];
    if show_bus {
        header.push(Cell::from("BUS"));
    }
    header.push(Cell::from("MODEL"));
    header.push(Cell::from("CAPACITY"));
    header.push(Cell::from("HEALTH"));
    if show_mount {
        header.push(Cell::from("MOUNT"));
    }
    let header = Row::new(header).style(
        Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = app
        .devices
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let (label, sev, loading) = app.health_for(i);
            let health = Cell::from(if loading {
                Span::styled(label, Style::default().fg(palette.dim))
            } else {
                Span::styled(
                    label,
                    Style::default()
                        .fg(palette.severity(sev))
                        .add_modifier(Modifier::BOLD),
                )
            });
            let mut cells = vec![Cell::from(d.path.clone()), Cell::from(d.kind.to_string())];
            if show_bus {
                cells.push(Cell::from(d.bus.to_string()));
            }
            cells.push(Cell::from(d.label()));
            cells.push(Cell::from(report::human_size(d.size_bytes)));
            cells.push(health);
            if show_mount {
                cells.push(Cell::from(report::mount_summary(d)));
            }
            Row::new(cells)
        })
        .collect();

    let mut widths = vec![Constraint::Length(14), Constraint::Length(5)];
    if show_bus {
        widths.push(Constraint::Length(7));
    }
    widths.push(Constraint::Min(16));
    widths.push(Constraint::Length(10));
    widths.push(Constraint::Length(14));
    if show_mount {
        widths.push(Constraint::Length(16));
    }

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Storage "))
        .row_highlight_style(highlight(palette, 1))
        .highlight_symbol("> ");
    f.render_stateful_widget(table, area, &mut app.table);

    let footer = if app.health_rx.is_some() {
        "reading health…   ↑/↓ move   Enter report   r rescan   ? help   q quit"
    } else {
        "↑/↓ move   Enter report   r rescan   ? help   q quit"
    };
    hint(f, hint_area, palette.dim, footer);
}

fn draw_report(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    palette: &Palette,
    hint_area: Rect,
) {
    app.view_height = area.height;
    let dev = app.report_dev.clone().unwrap_or_default();

    let title = if app.pending.is_some() {
        let tick = app.pending.as_ref().map(|p| p.tick).unwrap_or(0);
        format!(
            " Report — {dev}   loading {} ",
            SPINNER[(tick as usize) % SPINNER.len()]
        )
    } else {
        let total = app.report_lines.len();
        let pos = (app.scroll as usize + 1).min(total.max(1));
        format!(" Report — {dev}   [{pos}/{total}] ")
    };

    let body = if app.pending.is_some() {
        Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  Reading SMART for {dev}…"),
                Style::default().fg(palette.accent),
            )),
        ])
    } else {
        Text::from(app.report_lines.iter().cloned().map(Line::from).collect::<Vec<_>>())
    };

    let para = Paragraph::new(body)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    f.render_widget(para, area);
    hint(
        f,
        hint_area,
        palette.dim,
        "↑/↓ PgUp/PgDn Home/End scroll   b/Esc back   ? help   q quit",
    );
}

fn highlight(palette: &Palette, kind: u8) -> Style {
    if palette.mono {
        return Style::default().add_modifier(Modifier::REVERSED);
    }
    match kind {
        0 => Style::default()
            .fg(palette.hl_fg)
            .bg(palette.hl_bg)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().bg(palette.row_bg).fg(palette.row_fg),
    }
}

fn hint(f: &mut Frame, area: Rect, color: Color, text: &str) {
    let p = Paragraph::new(text).style(Style::default().fg(color));
    f.render_widget(p, area);
}