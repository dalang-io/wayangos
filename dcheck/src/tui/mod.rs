//! Terminal UI (ratatui) — a sci-fi "HUD" over the same data as the text
//! reports. Entered for interactive use; `main` falls back to the plain text
//! menu when stdout is not a terminal.
//!
//! - `theme`: neon truecolor / ANSI / mono palettes and Unicode/ASCII glyphs.
//! - `widgets`: bracket panels, line gauges, badges, keycaps.
//! - `views`: splash, command deck, storage, report, RAM, CPU, help overlay.
//!
//! All hardware reads run on background threads; the UI only redraws on input,
//! while something is loading, or during the (≤0.5 s, skippable) splash.

pub mod snapshot;
mod theme;
mod views;
mod widgets;

use std::io::{self, Stdout, Write};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::widgets::{ListState, TableState};
use ratatui::Terminal;

use crate::cpu::CpuInfo;
use crate::health::Health;
use crate::model::Device;
use crate::ram::RamInfo;
use crate::report;
use crate::smartctl::SmartData;

pub use theme::{ColorMode, Palette, Ui};

const SPLASH: Duration = Duration::from_millis(500);
const MENU_ITEMS: usize = 4;

/// TUI options resolved by `main` from flags, env and config.
pub struct Options {
    pub light: bool,
    pub demo: bool,
    pub mouse: bool,
    pub plain: bool,
    pub transparent: bool,
    pub splash: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Splash,
    Menu,
    Storage,
    Report,
    Ram,
    Cpu,
}

/// Health summary for one row of the storage list.
#[derive(Clone, Debug)]
struct DevHealth {
    label: String,
    sev: u8,
    /// Remaining life in percent (100 - wear used), when reported.
    life: Option<u64>,
    temp: Option<i64>,
}

impl DevHealth {
    fn pending() -> Self {
        DevHealth {
            label: "…".into(),
            sev: 1,
            life: None,
            temp: None,
        }
    }

    fn from_metrics(m: Option<&(SmartData, Health)>) -> Self {
        match m {
            Some((s, h)) => DevHealth {
                label: h.verdict.label().to_string(),
                sev: h.verdict.severity(),
                life: h
                    .wear_used_percent
                    .or(h.design_life_used)
                    .map(|w| 100u64.saturating_sub(w)),
                temp: s.temperature_c,
            },
            None => DevHealth {
                label: "UNKNOWN".into(),
                sev: 1,
                life: None,
                temp: None,
            },
        }
    }
}

type Metrics = Option<(SmartData, Health)>;

struct App {
    devices: Vec<Device>,
    demo: bool,
    mouse: bool,
    pal: Palette,
    ui: Ui,
    host: String,
    temp_warn: i64,

    screen: Screen,
    help: bool,
    started: Instant,
    tick: u8,
    status: Option<String>,

    menu: ListState,
    table: TableState,
    health: Vec<DevHealth>,
    health_rx: Option<Receiver<Vec<DevHealth>>>,

    report_dev: Option<usize>,
    report_lines: Vec<String>,
    report_metrics: Metrics,
    report_rx: Option<Receiver<(Vec<String>, Metrics)>>,

    ram: Option<RamInfo>,
    ram_lines: Vec<String>,
    ram_rx: Option<Receiver<RamInfo>>,
    cpu: Option<CpuInfo>,
    cpu_lines: Vec<String>,
    cpu_rx: Option<Receiver<CpuInfo>>,

    /// Scroll offset of the active log pane, its last drawn height, and the
    /// rows its (wrapped) content occupied.
    scroll: u16,
    view_height: u16,
    log_rows: usize,
}

impl App {
    fn new(devices: Vec<Device>, pal: Palette, ui: Ui, demo: bool, mouse: bool, splash: bool) -> Self {
        let mut menu = ListState::default();
        menu.select(Some(0));
        let mut table = TableState::default();
        if !devices.is_empty() {
            table.select(Some(0));
        }
        App {
            health: vec![DevHealth::pending(); devices.len()],
            devices,
            demo,
            mouse,
            pal,
            ui,
            host: hostname(),
            temp_warn: crate::config::load().temp_warn_c,
            screen: if splash { Screen::Splash } else { Screen::Menu },
            help: false,
            started: Instant::now(),
            tick: 0,
            status: None,
            menu,
            table,
            health_rx: None,
            report_dev: None,
            report_lines: Vec::new(),
            report_metrics: None,
            report_rx: None,
            ram: None,
            ram_lines: Vec::new(),
            ram_rx: None,
            cpu: None,
            cpu_lines: Vec::new(),
            cpu_rx: None,
            scroll: 0,
            view_height: 1,
            log_rows: 0,
        }
    }

    /// Kick off every background read (device health, RAM, CPU).
    fn spawn_all(&mut self) {
        self.spawn_health();
        self.spawn_ram();
        self.spawn_cpu();
    }

    fn spawn_health(&mut self) {
        let devices = self.devices.clone();
        self.health = vec![DevHealth::pending(); devices.len()];
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let out: Vec<DevHealth> = report::metrics_all(&devices)
                .iter()
                .map(|m| DevHealth::from_metrics(m.as_ref()))
                .collect();
            let _ = tx.send(out);
        });
        self.health_rx = Some(rx);
    }

    fn spawn_ram(&mut self) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::ram::read());
        });
        self.ram_rx = Some(rx);
    }

    fn spawn_cpu(&mut self) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::cpu::read());
        });
        self.cpu_rx = Some(rx);
    }

    fn start_report(&mut self, index: usize) {
        let Some(dev) = self.devices.get(index).cloned() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(report::device_report(&dev));
        });
        self.report_dev = Some(index);
        self.report_lines.clear();
        self.report_metrics = None;
        self.report_rx = Some(rx);
        self.scroll = 0;
        self.screen = Screen::Report;
    }

    fn open_ram(&mut self) {
        if self.ram_rx.is_none() {
            self.spawn_ram();
        }
        self.scroll = 0;
        self.screen = Screen::Ram;
    }

    fn open_cpu(&mut self) {
        if self.cpu_rx.is_none() {
            self.spawn_cpu();
        }
        self.scroll = 0;
        self.screen = Screen::Cpu;
    }

    fn rescan(&mut self) {
        for d in &self.devices {
            crate::cache::invalidate(d);
        }
        self.devices = reload_devices(self.demo);
        self.table = TableState::default();
        if !self.devices.is_empty() {
            self.table.select(Some(0));
        }
        self.spawn_health();
        self.status = Some("rescanning devices".into());
    }

    /// Collect finished background reads.
    fn drain(&mut self) {
        if let Some(h) = poll(&mut self.health_rx) {
            self.health = h;
        }
        if let Some((lines, metrics)) = poll(&mut self.report_rx) {
            self.report_lines = lines;
            self.report_metrics = metrics;
            // Keep the list row in sync with the fresher read.
            if let Some(i) = self.report_dev {
                if let Some(row) = self.health.get_mut(i) {
                    if self.health_rx.is_none() {
                        *row = DevHealth::from_metrics(self.report_metrics.as_ref());
                    }
                }
            }
        }
        if let Some(r) = poll(&mut self.ram_rx) {
            self.ram_lines = report::ram_report_lines(&r);
            self.ram = Some(r);
        }
        if let Some(c) = poll(&mut self.cpu_rx) {
            self.cpu_lines = report::cpu_report_lines(&c);
            self.cpu = Some(c);
        }
    }

    fn busy(&self) -> bool {
        self.health_rx.is_some()
            || self.report_rx.is_some()
            || self.ram_rx.is_some()
            || self.cpu_rx.is_some()
    }

    fn ram_sev(&self) -> (&'static str, u8) {
        self.ram.as_ref().map(|r| r.verdict()).unwrap_or(("UNKNOWN", 1))
    }

    fn cpu_sev(&self) -> (&'static str, u8) {
        self.cpu
            .as_ref()
            .map(|c| c.verdict())
            .unwrap_or(("UNKNOWN", 1))
    }

    fn worst_device(&self) -> u8 {
        self.health.iter().map(|h| h.sev).max().unwrap_or(1)
    }

    /// Lines of the log pane on the current screen.
    fn log_len(&self) -> usize {
        match self.screen {
            Screen::Report => self.report_lines.len(),
            Screen::Ram => self.ram_lines.len(),
            Screen::Cpu => self.cpu_lines.len(),
            _ => 0,
        }
    }

    fn max_scroll(&self) -> u16 {
        max_scroll_for(self.log_rows.max(self.log_len()), self.view_height)
    }

    fn scroll_by(&mut self, delta: i32) {
        let next = (self.scroll as i32 + delta).clamp(0, self.max_scroll() as i32);
        self.scroll = next as u16;
    }
}

fn poll<T>(rx: &mut Option<Receiver<T>>) -> Option<T> {
    let r = rx.as_ref()?;
    match r.try_recv() {
        Ok(v) => {
            *rx = None;
            Some(v)
        }
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            *rx = None;
            None
        }
    }
}

fn max_scroll_for(lines: usize, view_height: u16) -> u16 {
    lines.saturating_sub(view_height.max(1) as usize) as u16
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

/// Short host name for the header.
fn hostname() -> String {
    #[cfg(unix)]
    {
        extern "C" {
            fn gethostname(name: *mut std::ffi::c_char, len: usize) -> std::ffi::c_int;
        }
        let mut buf = [0u8; 256];
        let rc = unsafe { gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        if rc == 0 {
            let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
            let name = String::from_utf8_lossy(&buf[..end]);
            let short = name.split('.').next().unwrap_or("").trim();
            if !short.is_empty() {
                return short.to_string();
            }
        }
    }
    "localhost".to_string()
}

/// Run the TUI. Must be called on a real terminal.
pub fn run(devices: Vec<Device>, opts: Options) -> io::Result<()> {
    let pal = Palette::new(ColorMode::detect(), opts.light, opts.transparent);
    let ui = Ui { plain: opts.plain };
    let splash = opts.splash && !opts.plain;
    let mut app = App::new(devices, pal, ui, opts.demo, opts.mouse, splash);
    app.spawn_all();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    if opts.mouse {
        execute!(stdout, EnableMouseCapture)?;
    }
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    if opts.mouse {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        app.drain();
        if app.screen == Screen::Splash && app.started.elapsed() >= SPLASH {
            app.screen = Screen::Menu;
        }
        terminal.draw(|f| views::draw(f, app))?;

        let timeout = if app.screen == Screen::Splash {
            Duration::from_millis(40)
        } else if app.busy() {
            Duration::from_millis(120)
        } else {
            Duration::from_millis(500)
        };
        if !event::poll(timeout)? {
            app.tick = app.tick.wrapping_add(1);
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                app.status = None;
                if handle_key(app, key.code) {
                    break;
                }
            }
            Event::Mouse(m) if app.mouse => handle_mouse(app, m.kind),
            _ => {}
        }
    }
    Ok(())
}

/// Returns true when the app should quit.
fn handle_key(app: &mut App, code: KeyCode) -> bool {
    if app.screen == Screen::Splash {
        app.screen = Screen::Menu;
        return false;
    }
    if app.help {
        if code == KeyCode::Char('q') {
            return true;
        }
        app.help = false;
        return false;
    }
    match code {
        KeyCode::Char('q') => return true,
        KeyCode::Char('?') => {
            app.help = true;
            return false;
        }
        _ => {}
    }
    match app.screen {
        Screen::Splash => {}
        Screen::Menu => match code {
            KeyCode::Esc => return true,
            KeyCode::Up | KeyCode::Char('k') => {
                let i = app.menu.selected().unwrap_or(0);
                app.menu.select(Some(i.saturating_sub(1)));
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let i = app.menu.selected().unwrap_or(0);
                app.menu.select(Some((i + 1).min(MENU_ITEMS - 1)));
            }
            KeyCode::Home | KeyCode::Char('g') => app.menu.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => app.menu.select(Some(MENU_ITEMS - 1)),
            KeyCode::Char('1') => app.screen = Screen::Storage,
            KeyCode::Char('2') => app.open_ram(),
            KeyCode::Char('3') => app.open_cpu(),
            KeyCode::Enter => match app.menu.selected().unwrap_or(0) {
                0 => app.screen = Screen::Storage,
                1 => app.open_ram(),
                2 => app.open_cpu(),
                _ => return true,
            },
            _ => {}
        },
        Screen::Storage => match code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => app.screen = Screen::Menu,
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
            KeyCode::Home | KeyCode::Char('g') => app.table.select(Some(0)),
            KeyCode::End | KeyCode::Char('G') => {
                app.table.select(Some(app.devices.len().saturating_sub(1)))
            }
            KeyCode::Enter => {
                if let Some(i) = app.table.selected() {
                    app.start_report(i);
                }
            }
            _ => {}
        },
        Screen::Report | Screen::Ram | Screen::Cpu => match code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => {
                app.screen = if app.screen == Screen::Report {
                    Screen::Storage
                } else {
                    Screen::Menu
                };
            }
            KeyCode::Char('c') => copy_current(app),
            KeyCode::Char('r') => match app.screen {
                Screen::Report => {
                    if let Some(i) = app.report_dev {
                        if let Some(d) = app.devices.get(i) {
                            crate::cache::invalidate(d);
                        }
                        app.start_report(i);
                    }
                }
                Screen::Ram => {
                    app.spawn_ram();
                }
                _ => {
                    app.spawn_cpu();
                }
            },
            KeyCode::Down | KeyCode::Char('j') => app.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => app.scroll_by(10),
            KeyCode::PageUp => app.scroll_by(-10),
            KeyCode::Home | KeyCode::Char('g') => app.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => app.scroll = app.max_scroll(),
            _ => {}
        },
    }
    false
}

fn handle_mouse(app: &mut App, kind: MouseEventKind) {
    let delta = match kind {
        MouseEventKind::ScrollUp => -1,
        MouseEventKind::ScrollDown => 1,
        _ => return,
    };
    match app.screen {
        Screen::Report | Screen::Ram | Screen::Cpu => app.scroll_by(delta * 3),
        Screen::Storage => {
            let i = app.table.selected().unwrap_or(0) as i32 + delta;
            let max = app.devices.len().saturating_sub(1) as i32;
            app.table.select(Some(i.clamp(0, max) as usize));
        }
        Screen::Menu => {
            let i = app.menu.selected().unwrap_or(0) as i32 + delta;
            app.menu.select(Some(i.clamp(0, MENU_ITEMS as i32 - 1) as usize));
        }
        Screen::Splash => {}
    }
}

/// Copy the current log to the clipboard via OSC 52.
fn copy_current(app: &mut App) {
    let text = match app.screen {
        Screen::Report => app.report_lines.join("\n"),
        Screen::Ram => app.ram_lines.join("\n"),
        Screen::Cpu => app.cpu_lines.join("\n"),
        _ => String::new(),
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
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests;
