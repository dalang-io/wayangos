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
    Block, BorderType, Borders, Cell, List, ListItem, ListState, Paragraph, Row, Table, TableState,
    Wrap,
};
use ratatui::{Frame, Terminal};

use crate::model::Device;
use crate::report;

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Colours use the terminal's ANSI palette so dcheck blends with the user's
/// terminal/OpenCode theme instead of hardcoding RGB that can clash.
struct Palette {
    accent: Color,
    dim: Color,
    ok: Color,
    warn: Color,
    bad: Color,
}

impl Palette {
    fn for_theme(light: bool) -> Self {
        if std::env::var_os("NO_COLOR").is_some() {
            return Palette {
                accent: Color::Reset,
                dim: Color::Reset,
                ok: Color::Reset,
                warn: Color::Reset,
                bad: Color::Reset,
            };
        }
        // ANSI names are mapped by the terminal theme, so both light and dark
        // terminals (and OpenCode) stay consistent.
        Palette {
            accent: if light { Color::Blue } else { Color::Cyan },
            dim: Color::DarkGray,
            ok: Color::Green,
            warn: Color::Yellow,
            bad: Color::Red,
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

/// Glyph set: fancy (Unicode box/tech) or plain ASCII for limited fonts.
#[derive(Clone, Copy)]
struct Ui {
    plain: bool,
}

impl Ui {
    fn border(self) -> BorderType {
        if self.plain {
            BorderType::Plain
        } else {
            BorderType::Double
        }
    }

    fn cursor(self) -> &'static str {
        if self.plain {
            "> "
        } else {
            "» "
        }
    }

    fn sep(self) -> &'static str {
        if self.plain {
            " | "
        } else {
            " · "
        }
    }

    fn sym(self, sev: u8) -> &'static str {
        match (self.plain, sev) {
            (false, 0) => "✔",
            (false, 2) => "!",
            (false, 3) | (false, 4) => "✖",
            (false, _) => "?",
            (true, 0) => "",
            (true, 2) => "!",
            (true, 3) | (true, 4) => "X",
            (true, _) => "?",
        }
    }

    /// Status badge, e.g. `✔ OK` (fancy) or `OK` (plain, no symbol).
    fn badge(self, sev: u8, label: &str) -> String {
        let s = self.sym(sev);
        if s.is_empty() {
            label.to_string()
        } else {
            format!("{s} {label}")
        }
    }

    fn panel<'a>(self, title: &str, accent: Color) -> Block<'a> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(self.border())
            .title(Span::styled(
                format!(" {title} "),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ))
    }
}

#[derive(Clone)]
enum Screen {
    Menu,
    Storage,
    Report,
    Ram,
    Cpu,
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
    plain: bool,
    status: Option<String>,
    ram_lines: Vec<String>,
    ram_rx: Option<Receiver<Vec<String>>>,
    cpu_lines: Vec<String>,
    cpu_rx: Option<Receiver<Vec<String>>>,
}

impl App {
    fn new(devices: Vec<Device>, light: bool, demo: bool, mouse: bool, plain: bool) -> Self {
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
            plain,
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
            ram_lines: Vec::new(),
            ram_rx: None,
            cpu_lines: Vec::new(),
            cpu_rx: None,
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

    fn start_ram(&mut self) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(report::ram_report_lines(&crate::ram::read()));
        });
        self.ram_lines.clear();
        self.scroll = 0;
        self.ram_rx = Some(rx);
        self.screen = Screen::Ram;
    }

    fn start_cpu(&mut self) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(report::cpu_report_lines(&crate::cpu::read()));
        });
        self.cpu_lines.clear();
        self.scroll = 0;
        self.cpu_rx = Some(rx);
        self.screen = Screen::Cpu;
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
        max_scroll_for(self.report_lines.len(), self.view_height)
    }

    fn worst_severity(&self) -> u8 {
        self.health.iter().map(|(_, s)| *s).max().unwrap_or(1)
    }
}

fn max_scroll_for(lines: usize, view_height: u16) -> u16 {
    let visible = view_height.saturating_sub(2).max(1) as usize;
    lines.saturating_sub(visible) as u16
}

/// Copy the current screen's content to the clipboard via OSC 52.
fn copy_current(app: &mut App) {
    let text = match &app.screen {
        Screen::Report => app.report_lines.join("\n"),
        Screen::Ram => app.ram_lines.join("\n"),
        Screen::Cpu => app.cpu_lines.join("\n"),
        _ => {
            app.status = Some("nothing to copy".to_string());
            return;
        }
    };
    if text.trim().is_empty() {
        app.status = Some("nothing to copy yet".to_string());
        return;
    }
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = io::stdout();
    let _ = out.write_all(seq.as_bytes());
    let _ = out.flush();
    app.status = Some("copied via OSC52 (terminal must support it)".to_string());
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
pub fn run(devices: Vec<Device>, light: bool, demo: bool, mouse: bool, plain: bool) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    if mouse {
        execute!(stdout, EnableMouseCapture)?;
    }
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, devices, light, demo, mouse, plain);

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
    plain: bool,
) -> io::Result<()> {
    let mut app = App::new(devices, light, demo, mouse, plain);
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
        if let Some(rx) = &app.ram_rx {
            match rx.try_recv() {
                Ok(lines) => {
                    app.ram_lines = lines;
                    app.ram_rx = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => app.ram_rx = None,
            }
        }
        if let Some(rx) = &app.cpu_rx {
            match rx.try_recv() {
                Ok(lines) => {
                    app.cpu_lines = lines;
                    app.cpu_rx = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => app.cpu_rx = None,
            }
        }

        terminal.draw(|f| draw(f, &mut app))?;

        let busy = app.pending.is_some()
            || app.health_rx.is_some()
            || app.ram_rx.is_some()
            || app.cpu_rx.is_some();
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
                1 => app.start_ram(),
                2 => app.start_cpu(),
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
            KeyCode::Char('c') => copy_current(app),
            KeyCode::Down | KeyCode::Char('j') => app.scroll = app.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll = app.scroll.saturating_sub(1),
            KeyCode::PageDown => app.scroll = app.scroll.saturating_add(10),
            KeyCode::PageUp => app.scroll = app.scroll.saturating_sub(10),
            KeyCode::Char('g') | KeyCode::Home => app.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => app.scroll = app.max_scroll(),
            _ => {}
        },
        Screen::Ram => {
            if handle_simple(app, code, true) {
                return true;
            }
        }
        Screen::Cpu => {
            if handle_simple(app, code, false) {
                return true;
            }
        }
        Screen::Help => {}
    }
    false
}

/// Shared key handling for the RAM/CPU screens (scroll + copy + back).
fn handle_simple(app: &mut App, code: KeyCode, is_ram: bool) -> bool {
    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => app.screen = Screen::Menu,
        KeyCode::Char('?') => app.open_help(),
        KeyCode::Char('c') => copy_current(app),
        KeyCode::Down | KeyCode::Char('j') => app.scroll = app.scroll.saturating_add(1),
        KeyCode::Up | KeyCode::Char('k') => app.scroll = app.scroll.saturating_sub(1),
        KeyCode::PageDown => app.scroll = app.scroll.saturating_add(10),
        KeyCode::PageUp => app.scroll = app.scroll.saturating_sub(10),
        KeyCode::Char('g') | KeyCode::Home => app.scroll = 0,
        KeyCode::Char('G') | KeyCode::End => {
            let len = if is_ram {
                app.ram_lines.len()
            } else {
                app.cpu_lines.len()
            };
            app.scroll = max_scroll_for(len, app.view_height);
        }
        _ => {}
    }
    false
}

fn handle_mouse(app: &mut App, kind: MouseEventKind) {
    match kind {
        MouseEventKind::ScrollUp => match app.screen {
            Screen::Report | Screen::Ram | Screen::Cpu => {
                app.scroll = app.scroll.saturating_sub(3)
            }
            Screen::Storage => {
                let i = app.table.selected().unwrap_or(0);
                app.table.select(Some(i.saturating_sub(1)));
            }
            _ => {}
        },
        MouseEventKind::ScrollDown => match app.screen {
            Screen::Report | Screen::Ram | Screen::Cpu => {
                let len = match app.screen {
                    Screen::Ram => app.ram_lines.len(),
                    Screen::Cpu => app.cpu_lines.len(),
                    _ => app.report_lines.len(),
                };
                app.scroll = (app.scroll + 3).min(max_scroll_for(len, app.view_height));
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
    let ui = Ui { plain: app.plain };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    // Header: logotype left, live status chip right.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(ui.border());
    let inner = block.inner(chunks[0]);
    f.render_widget(block, chunks[0]);
    let sev = app.worst_severity();
    let chip = if app.health_rx.is_some() {
        "SCANNING…".to_string()
    } else {
        format!(
            "{}   {} DEVICES",
            ui.badge(sev, severity_label(sev)),
            app.devices.len()
        )
    };
    let chip_w = (chip.chars().count() as u16 + 1).min(inner.width);
    let hcols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(1), Constraint::Length(chip_w)])
        .split(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if ui.plain { "dcheck" } else { "◆ dcheck" },
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {VERSION}")),
        ])),
        hcols[0],
    );
    f.render_widget(
        Paragraph::new(chip)
            .alignment(Alignment::Right)
            .style(
                Style::default()
                    .fg(palette.severity(sev))
                    .add_modifier(Modifier::BOLD),
            ),
        hcols[1],
    );

    let screen = app.screen.clone();
    match screen {
        Screen::Menu => {
            let items: Vec<ListItem> = ["Storage", "RAM", "CPU", "Quit"]
                .iter()
                .map(|s| ListItem::new(*s))
                .collect();
            let list = List::new(items)
                .block(ui.panel("COMMAND", palette.accent))
                .highlight_style(highlight(&palette, 0))
                .highlight_symbol(ui.cursor());
            f.render_stateful_widget(list, chunks[1], &mut app.menu);
            hint(
                f,
                chunks[2],
                palette.dim,
                &format!(
                    "↑/↓ move{}Enter select{}? help{}q quit",
                    ui.sep(),
                    ui.sep(),
                    ui.sep()
                ),
            );
        }
        Screen::Storage => draw_storage(f, app, chunks[1], &palette, chunks[2], ui),
        Screen::Report => draw_report(f, app, chunks[1], &palette, chunks[2], ui),
        Screen::Ram => draw_simple(f, app, chunks[1], chunks[2], &palette, "RAM", true, ui),
        Screen::Cpu => draw_simple(f, app, chunks[1], chunks[2], &palette, "CPU", false, ui),
        Screen::Help => {
            let lines: Vec<Line> = [
                "NAVIGATION",
                "  ↑/↓ or j/k       move / scroll",
                "  PgUp / PgDn      page",
                "  g / G, Home/End  top / bottom",
                "  Enter            select / open",
                "  Esc or b         back",
                "ACTIONS",
                "  r                rescan (Storage)",
                "  c                copy screen (OSC52)",
                "  ?                this help",
                "  q                quit",
                "",
                "Mouse wheel scrolls (--mouse). Text is selectable by default.",
                "Env: DCHECK_THEME, NO_COLOR, DCHECK_PLAIN, DCHECK_SMART_ARGS, DCHECK_NATIVE",
            ]
            .iter()
            .map(|l| Line::from(*l))
            .collect();
            let p = Paragraph::new(Text::from(lines))
                .block(ui.panel("HELP", palette.accent))
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

/// Colourise a plain report line for the TUI (headers, verdicts).
fn colorize_line(line: &str, p: &Palette) -> Line<'static> {
    let t = line.to_string();
    let style = if line.starts_with('▚') || line.starts_with("▐ ") || line.starts_with('[') {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else if line.contains("FAILED")
        || line.contains("REPLACE")
        || line.contains('✖')
        || line.contains("BACK UP")
    {
        Style::default().fg(p.bad).add_modifier(Modifier::BOLD)
    } else if line.contains("MONITOR") || line.contains("! ") {
        Style::default().fg(p.warn)
    } else {
        Style::default()
    };
    Line::from(Span::styled(t, style))
}

fn severity_label(sev: u8) -> &'static str {
    match sev {
        0 => "OK",
        2 => "MONITOR",
        3 => "BACKUP",
        4 => "REPLACE",
        _ => "UNKNOWN",
    }
}

fn draw_storage(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    palette: &Palette,
    hint_area: Rect,
    ui: Ui,
) {
    if app.devices.is_empty() {
        let p = Paragraph::new("No block devices found.")
            .alignment(Alignment::Center)
            .block(ui.panel("STORAGE", palette.accent));
        f.render_widget(p, area);
        hint(
            f,
            hint_area,
            palette.dim,
            &format!("r rescan{}Esc back{}q quit", ui.sep(), ui.sep()),
        );
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
                    ui.badge(sev, &label),
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
    widths.push(Constraint::Length(16));
    if show_mount {
        widths.push(Constraint::Length(16));
    }

    let table = Table::new(rows, widths)
        .header(header)
        .block(ui.panel("STORAGE", palette.accent))
        .row_highlight_style(highlight(palette, 1))
        .highlight_symbol(ui.cursor());
    f.render_stateful_widget(table, area, &mut app.table);

    let footer = if app.health_rx.is_some() {
        format!("SCANNING…{}↑/↓ move{}Enter report{}r rescan{}q quit", ui.sep(), ui.sep(), ui.sep(), ui.sep())
    } else {
        format!("↑/↓ move{}Enter report{}r rescan{}? help{}q quit", ui.sep(), ui.sep(), ui.sep(), ui.sep())
    };
    hint(f, hint_area, palette.dim, &footer);
}

fn draw_report(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    palette: &Palette,
    hint_area: Rect,
    ui: Ui,
) {
    app.view_height = area.height;
    let dev = app.report_dev.clone().unwrap_or_default();
    let arrow = if ui.plain { ">" } else { "▸" };

    let title = if app.pending.is_some() {
        let tick = app.pending.as_ref().map(|p| p.tick).unwrap_or(0);
        format!(
            "REPORT {arrow} {dev}   SCANNING {}",
            SPINNER[(tick as usize) % SPINNER.len()]
        )
    } else {
        let total = app.report_lines.len();
        let pos = (app.scroll as usize + 1).min(total.max(1));
        format!("REPORT {arrow} {dev}   [{pos}/{total}]")
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
        let lines: Vec<Line> = app
            .report_lines
            .iter()
            .map(|l| colorize_line(l, palette))
            .collect();
        Text::from(lines)
    };

    let para = Paragraph::new(body)
        .block(ui.panel(&title, palette.accent))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    f.render_widget(para, area);
    hint(
        f,
        hint_area,
        palette.dim,
        &format!(
            "↑/↓ PgUp/PgDn Home/End scroll{}c copy{}b/Esc back{}q quit",
            ui.sep(),
            ui.sep(),
            ui.sep()
        ),
    );
}

fn draw_simple(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    hint_area: Rect,
    palette: &Palette,
    name: &str,
    is_ram: bool,
    ui: Ui,
) {
    app.view_height = area.height;
    let loading = if is_ram {
        app.ram_rx.is_some()
    } else {
        app.cpu_rx.is_some()
    };
    let lines: &[String] = if is_ram { &app.ram_lines } else { &app.cpu_lines };
    let title = if loading {
        format!("{name}   SCANNING…")
    } else {
        format!(
            "{name}   [{}/{}]",
            (app.scroll as usize + 1).min(lines.len().max(1)),
            lines.len()
        )
    };
    let body = if loading {
        Text::from(Span::styled(
            format!("  Reading {name}…"),
            Style::default().fg(palette.accent),
        ))
    } else if lines.is_empty() {
        Text::from("  no data")
    } else {
        Text::from(lines.iter().cloned().map(Line::from).collect::<Vec<_>>())
    };
    let para = Paragraph::new(body)
        .block(ui.panel(&title, palette.accent))
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    f.render_widget(para, area);
    hint(
        f,
        hint_area,
        palette.dim,
        &format!(
            "↑/↓ PgUp/PgDn scroll{}c copy{}b/Esc back{}q quit",
            ui.sep(),
            ui.sep(),
            ui.sep()
        ),
    );
}

fn highlight(_palette: &Palette, kind: u8) -> Style {
    // REVERSED inverts the terminal's own colours, so selection stays readable
    // on any light/dark theme without hardcoded backgrounds.
    if kind == 0 {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED)
    }
}

fn hint(f: &mut Frame, area: Rect, color: Color, text: &str) {
    let p = Paragraph::new(text).style(Style::default().fg(color));
    f.render_widget(p, area);
}