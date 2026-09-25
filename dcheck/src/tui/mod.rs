//! Terminal UI (ratatui) — a sci-fi "HUD" over the same data as the text
//! reports. Entered for interactive use; `main` falls back to the plain text
//! menu when stdout is not a terminal.
//!
//! - `theme`: neon truecolor / ANSI / mono palettes and Unicode/ASCII glyphs.
//! - `widgets`: bracket panels, line gauges, badges, keycaps.
//! - `views`: splash, command deck, storage, report, RAM, CPU, recovery,
//!   capacity test, help overlay.
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
const MENU_ITEMS: usize = 5;

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
    /// Motherboard: identity, firmware, devices, sensors, BMC log.
    Board,
    /// Can deleted files still be recovered? (read-only)
    Recover,
    /// Capacity test in free space (writes test files; asks first).
    Verify,
    /// Deleted files of a disk with a block map; recover to another disk.
    Undelete,
}

enum UndelMsg {
    Scanned(Result<crate::undelete::Scan, String>),
    Recovered(Vec<Result<(String, u64), String>>),
}

/// Background messages of the recovery screen: the assessment first, the
/// (slower) disk map after it.
enum RecoverMsg {
    Gathered(Result<crate::recover::Gathered, String>),
    Map(Result<crate::recover::DiskMap, String>),
}

enum VerifyMsg {
    Progress(&'static str, u64, u64),
    Done(Result<(crate::verify::Outcome, Option<String>), String>),
}

/// The capacity-test flow: plan → (y) running → result.
enum VerifyState {
    Idle,
    Plan {
        plan: Result<crate::verify::Plan, String>,
        full: bool,
    },
    Running {
        plan: crate::verify::Plan,
        total: u64,
        phase: &'static str,
        done: u64,
        of: u64,
        started: Instant,
        stopping: bool,
        rx: Receiver<VerifyMsg>,
    },
    Done {
        code: i32,
    },
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

    /// Health of a list row: a virtual disk without SMART is VIRTUAL, not
    /// UNKNOWN.
    fn for_device(d: &Device, m: Option<&(SmartData, Health)>) -> Self {
        match m {
            None if crate::virt::is_virtual_disk(d) => DevHealth { label: "VIRTUAL".into(), sev: 0, life: None, temp: None },
            _ => DevHealth::from_metrics(m),
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
    board: Option<crate::board::BoardInfo>,
    board_lines: Vec<String>,
    board_rx: Option<Receiver<crate::board::BoardInfo>>,

    /// Device of the recovery / capacity screens, and the screen to go
    /// back to.
    tool_dev: Option<usize>,
    tool_back: Screen,
    recover: Option<crate::recover::Gathered>,
    recover_map: Option<Result<crate::recover::DiskMap, String>>,
    recover_lines: Vec<String>,
    recover_rx: Option<Receiver<RecoverMsg>>,
    verify: VerifyState,
    verify_lines: Vec<String>,

    /// Deleted-files screen: scan, selection, marks, destination prompt,
    /// and the result of the last recovery.
    undel: Option<crate::undelete::Scan>,
    undel_src: String,
    undel_rx: Option<Receiver<UndelMsg>>,
    undel_table: TableState,
    undel_marked: std::collections::BTreeSet<usize>,
    undel_prompt: Option<String>,
    undel_log: Vec<String>,
    undel_error: Option<String>,

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
            board: None,
            board_lines: Vec::new(),
            board_rx: None,
            tool_dev: None,
            tool_back: Screen::Storage,
            recover: None,
            recover_map: None,
            recover_lines: Vec::new(),
            recover_rx: None,
            verify: VerifyState::Idle,
            verify_lines: Vec::new(),
            undel: None,
            undel_src: String::new(),
            undel_rx: None,
            undel_table: TableState::default(),
            undel_marked: std::collections::BTreeSet::new(),
            undel_prompt: None,
            undel_log: Vec::new(),
            undel_error: None,
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
        self.spawn_board();
    }

    fn spawn_board(&mut self) {
        let demo = self.demo;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(if demo { crate::board::demo() } else { crate::board::read() });
        });
        self.board_rx = Some(rx);
    }

    fn open_board(&mut self) {
        if self.board_rx.is_none() && self.board.is_none() {
            self.spawn_board();
        }
        self.scroll = 0;
        self.screen = Screen::Board;
    }

    fn spawn_health(&mut self) {
        let devices = self.devices.clone();
        self.health = vec![DevHealth::pending(); devices.len()];
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let out: Vec<DevHealth> = report::metrics_all(&devices)
                .iter()
                .zip(&devices)
                .map(|(m, d)| DevHealth::for_device(d, m.as_ref()))
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

    /// Device for `u` / `v`: the open report, else the selected row.
    fn tool_target(&self) -> Option<usize> {
        match self.screen {
            Screen::Report => self.report_dev,
            _ => self.table.selected(),
        }
    }

    fn open_recover(&mut self, index: usize) {
        let Some(dev) = self.devices.get(index).cloned() else {
            return;
        };
        if dev.failure.is_some() {
            self.status = Some("no disk to examine on a dead port".into());
            return;
        }
        self.tool_back = self.screen;
        self.tool_dev = Some(index);
        self.recover = None;
        self.recover_map = None;
        self.recover_lines.clear();
        let demo = self.demo;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if demo {
                let (g, m) = crate::recover::demo(&dev);
                let _ = tx.send(RecoverMsg::Gathered(Ok(g)));
                std::thread::sleep(Duration::from_millis(600));
                let _ = tx.send(RecoverMsg::Map(Ok(m)));
                return;
            }
            let g = crate::recover::gather(&dev.path);
            let map = match &g {
                Ok(g) if crate::native::is_root() => Some(crate::recover::sample_map(g, 512)),
                Ok(_) => Some(Err("run dcheck as root to map where the disk still holds data".into())),
                Err(_) => None,
            };
            let _ = tx.send(RecoverMsg::Gathered(g));
            if let Some(m) = map {
                let _ = tx.send(RecoverMsg::Map(m));
            }
        });
        self.recover_rx = Some(rx);
        self.scroll = 0;
        self.screen = Screen::Recover;
    }

    /// Scan the disk of the recovery screen for deleted files (read-only).
    fn open_undelete(&mut self) {
        let Some(dev) = self.tool_dev.and_then(|i| self.devices.get(i)).cloned() else {
            return;
        };
        self.undel = None;
        self.undel_error = None;
        self.undel_log.clear();
        self.undel_marked.clear();
        self.undel_prompt = None;
        self.undel_table = TableState::default();
        self.undel_src = dev.path.clone();
        let demo = self.demo;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = if demo {
                std::thread::sleep(Duration::from_millis(400));
                Ok(crate::undelete::demo_scan(dev.size_bytes))
            } else {
                open_source(&dev.path).map(|s| crate::undelete::scan(&*s))
            };
            let _ = tx.send(UndelMsg::Scanned(r));
        });
        self.undel_rx = Some(rx);
        self.screen = Screen::Undelete;
    }

    /// Write the marked files (or the selected one) into `dir`.
    fn start_undelete_write(&mut self, dir: String) {
        let Some(scan) = &self.undel else { return };
        let mut pick: Vec<usize> = self.undel_marked.iter().copied().collect();
        if pick.is_empty() {
            pick.extend(self.undel_table.selected());
        }
        let files: Vec<crate::undelete::Deleted> = pick.iter().filter_map(|i| scan.files.get(*i).cloned()).collect();
        if files.is_empty() {
            return;
        }
        let path = std::path::PathBuf::from(dir.trim());
        if self.demo {
            self.undel_log = files
                .iter()
                .map(|f| format!("(demo, nothing written) {}/{}", path.display(), f.path))
                .collect();
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&path)
            .map_err(|e| format!("cannot create {}: {e}", path.display()))
            .and_then(|_| crate::undelete::check_destination(&self.undel_src, &path))
        {
            self.status = Some(e);
            return;
        }
        let src = self.undel_src.clone();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = match open_source(&src) {
                Ok(s) => {
                    let refs: Vec<&crate::undelete::Deleted> = files.iter().collect();
                    crate::undelete::recover_to(&*s, &refs, &path)
                }
                Err(e) => vec![Err(e)],
            };
            let _ = tx.send(UndelMsg::Recovered(r));
        });
        self.undel_rx = Some(rx);
        self.status = Some("recovering…".into());
    }

    fn open_verify(&mut self, index: usize) {
        if matches!(self.verify, VerifyState::Running { .. }) {
            self.screen = Screen::Verify;
            return;
        }
        let Some(dev) = self.devices.get(index) else {
            return;
        };
        let plan = if dev.failure.is_some() {
            Err("no disk to test on a dead port".into())
        } else if self.demo {
            // The demo "Flash Disk" plays a counterfeit stick.
            Ok(crate::verify::demo_plan(dev, dev.bus == crate::model::Bus::Usb))
        } else {
            crate::verify::plan(dev, None)
        };
        self.tool_back = self.screen;
        self.tool_dev = Some(index);
        self.verify = VerifyState::Plan { plan, full: false };
        self.verify_lines.clear();
        self.scroll = 0;
        self.screen = Screen::Verify;
    }

    /// Size the test would write with the current choice.
    fn verify_total(plan: &crate::verify::Plan, full: bool) -> u64 {
        if full || plan.simulated.is_some() {
            plan.room
        } else {
            plan.room.min(crate::verify::QUICK)
        }
    }

    fn start_verify(&mut self) {
        let VerifyState::Plan { plan: Ok(plan), full } = &self.verify else {
            return;
        };
        let (plan, total) = (plan.clone(), Self::verify_total(plan, *full));
        crate::verify::reset_stop();
        let (tx, rx) = mpsc::channel();
        let p2 = plan.clone();
        std::thread::spawn(move || {
            let mut last = Instant::now() - Duration::from_secs(1);
            let tx2 = tx.clone();
            let r = crate::verify::run_plan(&p2, total, &mut |ph, d, t| {
                if last.elapsed() >= Duration::from_millis(100) || d >= t {
                    last = Instant::now();
                    let _ = tx2.send(VerifyMsg::Progress(ph, d, t));
                }
            });
            let _ = tx.send(VerifyMsg::Done(r));
        });
        self.verify = VerifyState::Running {
            plan,
            total,
            phase: "writing",
            done: 0,
            of: total,
            started: Instant::now(),
            stopping: false,
            rx,
        };
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
                        if let Some(d) = self.devices.get(i) {
                            *row = DevHealth::for_device(d, self.report_metrics.as_ref());
                        }
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
        if let Some(b) = poll(&mut self.board_rx) {
            self.board_lines = report::board_report_lines(&b);
            self.board = Some(b);
        }
        self.drain_recover();
        self.drain_verify();
        match poll(&mut self.undel_rx) {
            Some(UndelMsg::Scanned(Ok(s))) => {
                if !s.files.is_empty() {
                    self.undel_table.select(Some(0));
                }
                self.undel = Some(s);
            }
            Some(UndelMsg::Scanned(Err(e))) => self.undel_error = Some(e),
            Some(UndelMsg::Recovered(r)) => {
                let ok = r.iter().filter(|x| x.is_ok()).count();
                self.status = Some(format!("{ok} of {} file(s) recovered", r.len()));
                self.undel_log = r
                    .into_iter()
                    .map(|x| match x {
                        Ok((p, n)) => format!("recovered  {p}  ({})", crate::report::human_size_bin(n)),
                        Err(e) => format!("failed     {e}"),
                    })
                    .collect();
            }
            None => {}
        }
    }

    fn drain_recover(&mut self) {
        let Some(rx) = &self.recover_rx else { return };
        loop {
            match rx.try_recv() {
                Ok(RecoverMsg::Gathered(Ok(g))) => {
                    self.recover_lines = crate::recover::report_lines(&g);
                    self.recover = Some(g);
                }
                Ok(RecoverMsg::Gathered(Err(e))) => {
                    self.recover_lines = vec![format!("  cannot assess this disk: {e}")];
                }
                Ok(RecoverMsg::Map(m)) => {
                    if let Ok(m) = &m {
                        self.recover_lines.extend(crate::recover::share_lines(m));
                    }
                    self.recover_map = Some(m);
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.recover_rx = None;
                    return;
                }
            }
        }
    }

    fn drain_verify(&mut self) {
        let mut finished = None;
        if let VerifyState::Running { rx, phase, done, of, .. } = &mut self.verify {
            loop {
                match rx.try_recv() {
                    Ok(VerifyMsg::Progress(ph, d, t)) => {
                        *phase = ph;
                        *done = d;
                        *of = t;
                    }
                    Ok(VerifyMsg::Done(r)) => {
                        finished = Some(r);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = Some(Err("the test stopped unexpectedly".into()));
                        break;
                    }
                }
            }
        }
        let Some(r) = finished else { return };
        let VerifyState::Running { plan, .. } = &self.verify else { return };
        let target = if plan.simulated.is_some() {
            format!("{} (simulated demo drive)", plan.device)
        } else {
            format!("{} (free space, test files)", plan.base)
        };
        let (lines, code) = match r {
            Ok((o, cleanup)) => {
                let (mut lines, code) = crate::verify::outcome_lines(&target, &o, plan.simulated.is_some());
                if let Some(e) = cleanup {
                    lines.push(format!("  Warning      : {e}"));
                }
                (lines, code)
            }
            Err(e) => (vec![format!("  Result       : ERROR — {e}")], 1),
        };
        self.verify_lines = lines;
        self.verify = VerifyState::Done { code };
        self.scroll = 0;
    }

    fn busy(&self) -> bool {
        self.health_rx.is_some()
            || self.report_rx.is_some()
            || self.ram_rx.is_some()
            || self.cpu_rx.is_some()
            || self.board_rx.is_some()
            || self.recover_rx.is_some()
            || self.undel_rx.is_some()
            || matches!(self.verify, VerifyState::Running { .. })
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

    fn board_sev(&self) -> (&'static str, u8) {
        self.board.as_ref().map(|b| b.verdict()).unwrap_or(("UNKNOWN", 1))
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
            Screen::Board => self.board_lines.len(),
            Screen::Recover => self.recover_lines.len(),
            Screen::Verify => self.verify_lines.len(),
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

/// Open a device or image read-only for undelete.
fn open_source(path: &str) -> Result<Box<dyn crate::undelete::Source + Send>, String> {
    #[cfg(unix)]
    {
        crate::undelete::FileSource::open(path)
            .map(|s| Box::new(s) as Box<dyn crate::undelete::Source + Send>)
            .map_err(|e| format!("cannot open {path}: {e}"))
    }
    #[cfg(not(unix))]
    {
        Err(format!("cannot open {path}: not supported on this platform"))
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

/// Keys of the deleted-files screen (and its destination prompt).
fn handle_undelete_key(app: &mut App, code: KeyCode) {
    if !app.undel_log.is_empty() {
        app.undel_log.clear();
        return;
    }
    if let Some(input) = &mut app.undel_prompt {
        match code {
            KeyCode::Esc => app.undel_prompt = None,
            KeyCode::Enter => {
                let dir = input.clone();
                app.undel_prompt = None;
                if !dir.trim().is_empty() {
                    app.start_undelete_write(dir);
                }
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) => input.push(c),
            _ => {}
        }
        return;
    }
    let n = app.undel.as_ref().map_or(0, |s| s.files.len());
    match code {
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => app.screen = Screen::Recover,
        KeyCode::Up | KeyCode::Char('k') => {
            let i = app.undel_table.selected().unwrap_or(0);
            app.undel_table.select(Some(i.saturating_sub(1)));
        }
        KeyCode::Down | KeyCode::Char('j') if n > 0 => {
            let i = app.undel_table.selected().unwrap_or(0);
            app.undel_table.select(Some((i + 1).min(n - 1)));
        }
        KeyCode::Char(' ') => {
            if let Some(i) = app.undel_table.selected() {
                if !app.undel_marked.remove(&i) {
                    app.undel_marked.insert(i);
                }
                if i + 1 < n {
                    app.undel_table.select(Some(i + 1));
                }
            }
        }
        KeyCode::Char('a') => {
            if let Some(s) = &app.undel {
                let intact: Vec<usize> = (0..n).filter(|i| s.files[*i].state == crate::undelete::State::Intact).collect();
                if intact.iter().all(|i| app.undel_marked.contains(i)) {
                    app.undel_marked.clear();
                } else {
                    app.undel_marked.extend(intact);
                }
            }
        }
        KeyCode::Char('w') | KeyCode::Enter if n > 0 && app.undel_rx.is_none() => {
            app.undel_prompt = Some(String::new());
        }
        KeyCode::Char('c') => copy_current(app),
        _ => {}
    }
}

/// Keys of the capacity-test flow.
fn handle_verify_key(app: &mut App, code: KeyCode) {
    match &mut app.verify {
        VerifyState::Plan { plan, full } => match code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') | KeyCode::Char('n') => {
                app.verify = VerifyState::Idle;
                app.screen = app.tool_back;
            }
            KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') | KeyCode::Char('k') => {
                match plan {
                    Ok(p) if p.system.is_some() && !*full => {
                        app.status = Some("full test is not offered on a system disk (use the CLI with --full)".into());
                    }
                    Ok(_) => *full = !*full,
                    Err(_) => {}
                }
            }
            KeyCode::Char('y') | KeyCode::Char('Y') if plan.is_ok() => app.start_verify(),
            KeyCode::Enter if plan.is_ok() => {
                app.status = Some("press y to start the test (it writes test files)".into());
            }
            _ => {}
        },
        VerifyState::Running { stopping, .. } => {
            if matches!(code, KeyCode::Esc | KeyCode::Char('b')) && !*stopping {
                *stopping = true;
                crate::verify::request_stop();
                app.status = Some("stopping — test files are removed".into());
            }
        }
        VerifyState::Done { .. } | VerifyState::Idle => match code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => {
                app.verify = VerifyState::Idle;
                app.screen = app.tool_back;
            }
            KeyCode::Char('c') => copy_current(app),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_by(1),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_by(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => app.scroll_by(10),
            KeyCode::PageUp => app.scroll_by(-10),
            _ => {}
        },
    }
}

/// Returns true when the app should quit.
fn handle_key(app: &mut App, code: KeyCode) -> bool {
    if app.screen == Screen::Splash {
        app.screen = Screen::Menu;
        return false;
    }
    if app.help {
        if code == KeyCode::Char('q') && !matches!(app.verify, VerifyState::Running { .. }) {
            return true;
        }
        app.help = false;
        return false;
    }
    let testing = matches!(app.verify, VerifyState::Running { .. });
    // Typing a destination: every key goes to the prompt.
    if app.screen == Screen::Undelete && app.undel_prompt.is_some() {
        handle_undelete_key(app, code);
        return false;
    }
    match code {
        KeyCode::Char('q') if testing => {
            app.status = Some("a capacity test is running — esc stops it first".into());
            return false;
        }
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
            KeyCode::Char('4') => app.open_board(),
            KeyCode::Enter => match app.menu.selected().unwrap_or(0) {
                0 => app.screen = Screen::Storage,
                1 => app.open_ram(),
                2 => app.open_cpu(),
                3 => app.open_board(),
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
            KeyCode::Char('u') => {
                if let Some(i) = app.tool_target() {
                    app.open_recover(i);
                }
            }
            KeyCode::Char('v') => {
                if let Some(i) = app.tool_target() {
                    app.open_verify(i);
                }
            }
            _ => {}
        },
        Screen::Verify => handle_verify_key(app, code),
        Screen::Undelete => handle_undelete_key(app, code),
        Screen::Report | Screen::Ram | Screen::Cpu | Screen::Board | Screen::Recover => match code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('b') => {
                app.screen = match app.screen {
                    Screen::Report => Screen::Storage,
                    Screen::Recover => app.tool_back,
                    _ => Screen::Menu,
                };
            }
            KeyCode::Char('d') if app.screen == Screen::Recover => app.open_undelete(),
            KeyCode::Char('u') if app.screen == Screen::Report => {
                if let Some(i) = app.tool_target() {
                    app.open_recover(i);
                }
            }
            KeyCode::Char('v') if app.screen == Screen::Report => {
                if let Some(i) = app.tool_target() {
                    app.open_verify(i);
                }
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
                Screen::Board => {
                    app.spawn_board();
                }
                Screen::Recover => {
                    if let Some(i) = app.tool_dev {
                        let back = app.tool_back;
                        app.open_recover(i);
                        app.tool_back = back;
                    }
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
        Screen::Report | Screen::Ram | Screen::Cpu | Screen::Board | Screen::Recover | Screen::Verify => app.scroll_by(delta * 3),
        Screen::Undelete => {
            let n = app.undel.as_ref().map_or(0, |s| s.files.len()) as i32;
            let i = app.undel_table.selected().unwrap_or(0) as i32 + delta;
            app.undel_table.select(Some(i.clamp(0, (n - 1).max(0)) as usize));
        }
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
        Screen::Board => app.board_lines.join("\n"),
        Screen::Recover => app.recover_lines.join("\n"),
        Screen::Verify => app.verify_lines.join("\n"),
        Screen::Undelete => app.undel.as_ref().map(|s| crate::undelete::scan_lines(s).join("\n")).unwrap_or_default(),
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
