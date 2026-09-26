//! Interactive HUD (`wayang` with no subcommand on a TTY): the WayangOS
//! console, laid out like dcheck, wayang-fw and wayang-router: a one-row
//! header bar, the command deck (MODULES on the left, a live card for the
//! selected module on the right), a status row and keycaps.
//!
//! Modules: SYSTEM (slots, boot state), UPDATES (check / update / upgrade /
//! rollback, run in the background), NETWORK, WIFI and SSH (sub-screens), and
//! FIREWALL / ROUTER, which hand the terminal to wayang-fw / wayang-router when
//! they are installed. Number keys only jump; nothing that changes the system
//! runs without its own key, and rollback asks twice.
//!
//! Actions reuse the exact same code paths as the CLI subcommands; their
//! console output is silenced via [`crate::ui`] while the HUD is on screen.
//!
//! `wayang --demo` and `wayang --screens DIR [--size WxH]` render the screens
//! without touching the system.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Gauge, Paragraph, Wrap};
use ratatui::Frame;

use crate::cli::UpdateArgs;
pub use crate::hud::Tone;
use crate::hud::{self, Theme};
use crate::manifest::SlotMeta;
use crate::net;
use crate::netui;
use crate::paths;
use crate::slot::{self, Slot};
use crate::sshkeys;
use crate::sshkeysui;
use crate::status::{self, Status};
use crate::ui;
use crate::update;
use crate::wifiui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Module {
    System,
    Updates,
    Network,
    Wifi,
    Ssh,
    Dcheck,
    Firewall,
    Router,
}

/// Command-deck entries in order; `1`..`8` jump, `0` / EXIT quits.
pub const MODULES: [(Module, &str); 8] = [
    (Module::System, "SYSTEM"),
    (Module::Updates, "UPDATES"),
    (Module::Network, "NETWORK"),
    (Module::Wifi, "WIFI"),
    (Module::Ssh, "SSH"),
    (Module::Dcheck, "DCHECK"),
    (Module::Firewall, "FIREWALL"),
    (Module::Router, "ROUTER"),
];

/// A modal sub-screen opened from the deck.
pub enum Sub {
    Net(netui::App),
    Wifi(wifiui::App),
    Ssh(sshkeysui::App),
}

/// An installable companion app (wayang-fw, wayang-router, dcheck).
#[derive(Debug, Clone, Default)]
pub struct Companion {
    pub bin: Option<PathBuf>,
    /// `/data/etc/<dir>/config.toml`: a confirmed config exists.
    pub confirmed: bool,
    /// `pending.toml`: a commit waits for confirmation.
    pub pending: bool,
    /// The app has no config concept at all (dcheck): installed is healthy.
    pub configless: bool,
}

impl Companion {
    fn probe(name: &str, dir: &str) -> Companion {
        let data = paths::data_dir();
        let bin = [data.join("bin").join(name), PathBuf::from("/usr/bin").join(name)]
            .into_iter()
            .find(|p| p.is_file());
        let etc = data.join("etc").join(dir);
        Companion { bin, confirmed: etc.join("config.toml").is_file(), pending: etc.join("pending.toml").is_file(), configless: false }
    }

    fn tone(&self) -> Option<Tone> {
        match (&self.bin, self.pending, self.confirmed) {
            (None, _, _) => None,
            (Some(_), true, _) => Some(Tone::Bad),
            (Some(_), false, _) if self.configless => Some(Tone::Ok),
            (Some(_), false, true) => Some(Tone::Ok),
            (Some(_), false, false) => Some(Tone::Warn),
        }
    }
}

/// Everything the cards show, read once at start and on `r`.
#[derive(Debug, Clone, Default)]
pub struct Deck {
    pub host: String,
    pub uptime: String,
    pub ifaces: Vec<net::Iface>,
    pub wifi_saved: bool,
    pub fw: Companion,
    pub rt: Companion,
    pub dcheck: Companion,
    pub nft: bool,
    /// Root's authorized keys (`/data` + live), for the SSH module tone.
    pub ssh_keys: usize,
}

impl Deck {
    fn probe() -> Deck {
        let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default().trim().to_string();
        let secs: u64 = read("/proc/uptime").split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Deck {
            host: read("/proc/sys/kernel/hostname"),
            uptime: uptime(secs),
            ifaces: net::list(net::primary_iface().as_deref()),
            wifi_saved: paths::wpa_conf_file().is_file(),
            fw: Companion::probe("wayang-fw", "fw"),
            rt: Companion::probe("wayang-router", "router"),
            dcheck: Companion { configless: true, ..Companion::probe("dcheck", "dcheck") },
            nft: Path::new("/usr/sbin/nft").is_file(),
            ssh_keys: sshkeys::load().len(),
        }
    }

    fn demo() -> Deck {
        let i = |name: &str, link: bool, wireless: bool, primary: bool, ip: &[&str]| net::Iface {
            name: name.into(),
            link,
            driver: if wireless { "rtl8xxxu" } else { "e1000e" }.into(),
            mac: "52:54:00:12:34:56".into(),
            wireless,
            primary,
            ipv4: ip.iter().map(|s| s.to_string()).collect(),
        };
        Deck {
            host: "naga".into(),
            uptime: "3d 4h".into(),
            ifaces: vec![
                i("eth0", true, false, true, &["192.168.1.42/24"]),
                i("eth1", false, false, false, &[]),
                i("wlan0", false, true, false, &[]),
            ],
            wifi_saved: false,
            fw: Companion { bin: Some("/data/bin/wayang-fw".into()), confirmed: false, pending: false, configless: false },
            rt: Companion::default(),
            dcheck: Companion { bin: Some("/usr/bin/dcheck".into()), confirmed: false, pending: false, configless: true },
            nft: true,
            ssh_keys: 2,
        }
    }
}

fn uptime(s: u64) -> String {
    let (d, h, m) = (s / 86_400, s % 86_400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else {
        format!("{h}h {m:02}m")
    }
}

/// An update action running in the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Check,
    Update,
    Upgrade,
    Rollback,
    BootOther,
}

pub struct Job {
    pub label: String,
    rx: mpsc::Receiver<Result<i32, String>>,
    kind: JobKind,
    /// Byte counter fed by the bundle download; `None` when the job never
    /// fetches, so the card keeps the indeterminate bar.
    pub progress: Option<Arc<update::Progress>>,
    /// When the job started, for the elapsed-seconds readout.
    started: Instant,
}

/// A determinate progress line for the UPDATES card: `(ratio, label)`.
/// `None` while the transfer size is unknown (keep the sweeping bar).
fn job_progress(job: &Job) -> Option<(f64, String)> {
    let p = job.progress.as_ref()?;
    let pct = update::percent(p.downloaded(), p.total())?;
    let elapsed = job.started.elapsed().as_secs_f64();
    let rate = update::human_rate(update::mb_per_s(p.downloaded(), elapsed));
    Some((pct / 100.0, format!("{pct:>3.0}%  {rate}  {elapsed:.0}s")))
}

pub struct App {
    pub t: Theme,
    pub demo: bool,
    /// Deck position: 0..MODULES.len(), MODULES.len() = EXIT.
    pub sel: usize,
    pub status: Status,
    pub error: Option<String>,
    pub deck: Deck,
    pub message: Option<(Tone, String)>,
    pub job: Option<Job>,
    /// Rollback key pressed once.
    pub armed: bool,
    /// "Boot other slot" key pressed once.
    pub armed_other: bool,
    pub help: bool,
    pub exit: bool,
    /// Open sub-screen (network/wifi), if any.
    pub sub: Option<Sub>,
    /// Companion app to run full-screen next (taken by the runner).
    pub launch: Option<PathBuf>,
    pub tick: usize,
}

impl App {
    pub fn new(demo: bool) -> App {
        let (status, error) = if demo {
            (demo_status(), None)
        } else {
            match status::snapshot(None) {
                Ok(s) => (s, None),
                Err(e) => (status::unavailable(), Some(e.to_string())),
            }
        };
        App {
            t: Theme::detect(),
            demo,
            sel: 0,
            status,
            error,
            deck: if demo { Deck::demo() } else { Deck::probe() },
            message: None,
            job: None,
            armed: false,
            armed_other: false,
            help: false,
            exit: false,
            sub: None,
            launch: None,
            tick: 0,
        }
    }

    pub fn module(&self) -> Option<Module> {
        MODULES.get(self.sel).map(|m| m.0)
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.sub.is_some() {
            match self.sub.as_mut() {
                Some(Sub::Net(a)) => a.on_key(key),
                Some(Sub::Wifi(a)) => a.on_key(key),
                Some(Sub::Ssh(a)) => a.on_key(key),
                None => {}
            }
            if self.sub_exited() {
                self.sub = None;
                self.refresh();
            }
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        let armed = std::mem::take(&mut self.armed);
        let armed_other = std::mem::take(&mut self.armed_other);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.sel = (self.sel + 1).min(MODULES.len()),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('r') => {
                self.refresh();
                self.message = Some((Tone::Ok, "Refreshed.".into()));
            }
            KeyCode::Enter => self.open(self.sel),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('0') => self.exit = true,
            KeyCode::Char(c @ '1'..='8') => {
                self.sel = c as usize - '1' as usize;
                self.open(self.sel);
            }
            // update actions: only from the UPDATES card, never from a number key
            KeyCode::Char(c @ ('c' | 'u' | 'g' | 'x' | 'b')) if self.module() == Some(Module::Updates) => {
                if let Some(j) = &self.job {
                    self.message = Some((Tone::Warn, format!("{} is still running.", j.label)));
                    return;
                }
                match c {
                    'c' => self.start_job(JobKind::Check),
                    'u' => self.start_job(JobKind::Update),
                    'g' => self.start_job(JobKind::Upgrade),
                    'b' if armed_other => self.start_job(JobKind::BootOther),
                    'b' => {
                        self.armed_other = true;
                        self.message = Some((
                            Tone::Warn,
                            format!("Press b again to boot slot {} next (one-shot).", self.status.active.idle().as_str()),
                        ));
                    }
                    _ if armed => self.start_job(JobKind::Rollback),
                    _ => {
                        self.armed = true;
                        self.message = Some((
                            Tone::Warn,
                            format!("Press x again to boot slot {} next (rollback).", self.status.active.idle().as_str()),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    pub fn sub_exited(&self) -> bool {
        match &self.sub {
            Some(Sub::Net(a)) => a.exit,
            Some(Sub::Wifi(a)) => a.exit,
            Some(Sub::Ssh(a)) => a.exit,
            None => false,
        }
    }

    fn refresh(&mut self) {
        if self.demo {
            return;
        }
        match status::snapshot(None) {
            Ok(s) => {
                self.status = s;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.deck = Deck::probe();
    }

    /// Enter on a module: sub-screens open, companions launch, the rest stays
    /// on the deck (their card is the screen).
    fn open(&mut self, idx: usize) {
        let demo = self.demo;
        match MODULES.get(idx).map(|m| m.0) {
            None => self.exit = true,
            Some(Module::System) => self.refresh(),
            Some(Module::Updates) => {}
            Some(Module::Network) => self.sub = Some(Sub::Net(netui::App::new(demo))),
            Some(Module::Wifi) => self.sub = Some(Sub::Wifi(wifiui::App::new(demo))),
            Some(Module::Ssh) => self.sub = Some(Sub::Ssh(sshkeysui::App::new(demo))),
            Some(m @ (Module::Firewall | Module::Router)) => {
                let (c, name) =
                    if m == Module::Firewall { (&self.deck.fw, "wayang-fw") } else { (&self.deck.rt, "wayang-router") };
                match (&c.bin, demo) {
                    (Some(_), true) => self.message = Some((Tone::Ok, format!("demo: would open {name}"))),
                    (Some(b), false) => self.launch = Some(b.clone()),
                    (None, _) => {
                        self.message =
                            Some((Tone::Warn, format!("{name} is not installed: copy it to /data/bin/{name}")))
                    }
                }
            }
            Some(Module::Dcheck) => match (&self.deck.dcheck.bin, demo) {
                (Some(_), true) => self.message = Some((Tone::Ok, "demo: would open dcheck".into())),
                (Some(b), false) => self.launch = Some(b.clone()),
                (None, _) => {
                    self.message = Some((Tone::Warn, "dcheck is not installed (ships at /usr/bin/dcheck)".into()))
                }
            },
        }
    }

    fn start_job(&mut self, kind: JobKind) {
        let label = match kind {
            JobKind::Check => "Checking for updates",
            JobKind::Update => "Updating",
            JobKind::Upgrade => "Upgrading",
            JobKind::Rollback => "Rolling back",
            JobKind::BootOther => "Staging boot",
        };
        if self.demo {
            self.message = Some((Tone::Ok, format!("demo: {} (nothing runs)", label.to_lowercase())));
            return;
        }
        let (tx, rx) = mpsc::channel();
        // Only a fetch reports bytes; slot switches have nothing to measure.
        let progress = matches!(kind, JobKind::Update | JobKind::Upgrade).then(update::Progress::new);
        let hook = progress.clone();
        let upgrade = kind == JobKind::Upgrade;
        let args = match kind {
            JobKind::Check => UpdateArgs { check: true, ..Default::default() },
            JobKind::Rollback => UpdateArgs { rollback: true, ..Default::default() },
            JobKind::BootOther => UpdateArgs { boot_other: true, ..Default::default() },
            _ => UpdateArgs::default(),
        };
        std::thread::spawn(move || {
            ui::set_quiet(true);
            let r = update::run_with_progress(upgrade, &args, hook).map_err(|e| e.to_string());
            let _ = tx.send(r);
        });
        self.message = Some((Tone::Warn, format!("{label}…")));
        self.job = Some(Job { label: label.into(), rx, kind, progress, started: Instant::now() });
    }

    /// Picks up a finished background job.
    pub fn poll(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        let Some(job) = &self.job else { return };
        let Ok(result) = job.rx.try_recv() else { return };
        let kind = job.kind;
        self.job = None;
        self.message = Some(match result {
            Ok(0) if kind == JobKind::Check => {
                (Tone::Ok, "An update is available: u installs it into the other slot.".into())
            }
            Ok(0) if kind == JobKind::Rollback => (Tone::Warn, "Rollback staged: reboot to apply.".into()),
            Ok(0) if kind == JobKind::BootOther => {
                (Tone::Warn, "Boot to the other slot staged: reboot to apply.".into())
            }
            Ok(0) => (Tone::Ok, "Update staged in the other slot: reboot to apply.".into()),
            Ok(2) => (Tone::Ok, "Up to date: no update available.".into()),
            Ok(code) => (Tone::Bad, format!("Finished with exit code {code}.")),
            Err(e) => (Tone::Bad, e),
        });
        self.refresh();
    }

    fn system_tone(&self) -> Tone {
        if self.error.is_some() {
            Tone::Bad
        } else if self.status.attempts >= slot::ATTEMPT_LIMIT || !self.status.data {
            Tone::Warn
        } else {
            Tone::Ok
        }
    }

    /// Status symbol per module on the deck (`None` = not applicable).
    pub fn module_tone(&self, m: Module) -> Option<Tone> {
        match m {
            Module::System => Some(self.system_tone()),
            Module::Updates => match (&self.job, &self.message) {
                (Some(_), _) => Some(Tone::Warn),
                _ if self.error.is_some() => Some(Tone::Bad),
                _ => Some(Tone::Ok),
            },
            Module::Network => {
                let up = self.deck.ifaces.iter().any(|i| i.link && !i.ipv4.is_empty());
                Some(if up { Tone::Ok } else { Tone::Warn })
            }
            Module::Wifi => {
                if !self.deck.ifaces.iter().any(|i| i.wireless) {
                    None
                } else if self.deck.ifaces.iter().any(|i| i.wireless && i.link) {
                    Some(Tone::Ok)
                } else {
                    Some(Tone::Warn)
                }
            }
            Module::Firewall => self.deck.fw.tone(),
            Module::Router => self.deck.rt.tone(),
            Module::Dcheck => self.deck.dcheck.tone(),
            Module::Ssh => Some(if self.deck.ssh_keys > 0 { Tone::Ok } else { Tone::Warn }),
        }
    }
}

fn demo_status() -> Status {
    let mut s = status::unavailable();
    s.version = Some("1.4.1".into());
    s.channel = "stable".into();
    s.active = Slot::B;
    s.boot_next = Slot::B;
    s.good = Some(Slot::B);
    s.attempts = 0;
    s.data = true;
    s.backend = "grubenv".into();
    s.slots[0].meta = Some(SlotMeta {
        version: "1.3.0".into(),
        channel: "stable".into(),
        arch: "x86_64".into(),
        ..Default::default()
    });
    s.slots[1].meta = Some(SlotMeta {
        version: "1.4.1".into(),
        channel: "stable".into(),
        arch: "x86_64".into(),
        ..Default::default()
    });
    s
}

// ---- drawing -----------------------------------------------------------

pub fn draw(f: &mut Frame, app: &App, tick: usize) {
    if let Some(sub) = &app.sub {
        match sub {
            Sub::Net(a) => netui::draw(f, a, tick),
            Sub::Wifi(a) => wifiui::draw(f, a, tick),
            Sub::Ssh(a) => sshkeysui::draw(f, a, tick),
        }
        return;
    }
    let t = &app.t;
    let area = f.area();
    f.render_widget(ratatui::widgets::Block::default().style(t.base()), area);
    let [header, body, status, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(4), Constraint::Length(1), Constraint::Length(1)])
            .areas(area);

    let st = &app.status;
    let (color, sym) = t.tone(app.system_tone());
    let label = if app.error.is_some() { "UPDATER UNAVAILABLE".to_string() } else { format!("SLOT {}", st.active.as_str()) };
    let mut right = Vec::new();
    if app.demo {
        right.push(Span::styled("DEMO DATA  ", t.bold(t.warn)));
    }
    if !app.deck.host.is_empty() {
        right.push(Span::styled(format!("{} {}  ", t.g.brand, app.deck.host), t.fg(t.dim)));
    }
    right.push(hud::badge(color, sym, &label, t));
    right.push(Span::raw(" "));
    hud::header_bar(f, header, "SYSTEM CONSOLE", st.version.as_deref().unwrap_or(""), right, t);

    let wide = body.width >= 72 && body.height >= 10;
    let deck_w = if body.width >= 110 { 42 } else { 32 };
    let (left, right) = if wide {
        let [l, r] = Layout::horizontal([Constraint::Length(deck_w), Constraint::Min(30)]).areas(body);
        (l, Some(r))
    } else {
        (body, None)
    };
    draw_modules(f, left, app);
    if let Some(r) = right {
        draw_card(f, r, app, tick);
    }

    let busy = app.job.as_ref().map(|_| hud::spinner(tick));
    let msg = app.message.as_ref().map(|(tone, m)| (*tone, m.as_str()));
    hud::status_row(f, status, msg, busy, "Pick a module; ? shows every key.", t);

    let keys: Vec<(&str, &str)> = match app.module() {
        Some(Module::Updates) => vec![
            ("↑↓", "move"),
            ("c", "check"),
            ("u", "update"),
            ("g", "upgrade"),
            ("b", "boot other"),
            ("x", "rollback"),
            ("?", "help"),
            ("q", "quit"),
        ],
        _ => vec![("↑↓", "move"), ("enter", "open"), ("1-8", "jump"), ("r", "refresh"), ("?", "help"), ("q", "quit")],
    };
    let mut line = hud::keycaps(&keys, t);
    line.spans.insert(0, Span::raw(" "));
    f.render_widget(Paragraph::new(line), footer);

    if app.help {
        draw_help(f, body, app);
    }
}

fn draw_modules(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "MODULES", t);
    let mut lines: Vec<Line> = Vec::new();
    let logo = hud::logo(1, t);
    if inner.height as usize >= logo.len() + MODULES.len() + 3 && logo.iter().all(|l| l.width() <= inner.width as usize) {
        lines.extend(logo.into_iter().map(|l| l.centered()));
        lines.push(Line::from(""));
    }
    let row_w = inner.width.saturating_sub(2) as usize;
    let item = |i: usize, num: &str, name: &str, sym: Span<'static>| {
        let selected = i == app.sel;
        let pad = row_w.saturating_sub(num.len() + 1 + name.len() + 2);
        let (cur, name_style) = if selected {
            (Span::styled(t.g.cursor, t.bold(t.accent2)), t.highlight())
        } else {
            (Span::raw("  "), t.bold(t.fg))
        };
        Line::from(vec![
            cur,
            Span::styled(format!("{num} "), if selected { t.highlight() } else { t.fg(t.dim) }),
            Span::styled(format!("{name}{}", " ".repeat(pad)), name_style),
            sym,
        ])
    };
    for (i, (m, name)) in MODULES.iter().enumerate() {
        lines.push(item(i, &format!("{:02}", i + 1), name, t.sev(app.module_tone(*m))));
    }
    lines.push(item(MODULES.len(), "00", "EXIT", Span::raw("")));
    f.render_widget(Paragraph::new(lines), inner);
}

fn field(label: &str, value: impl Into<String>, t: &Theme) -> Line<'static> {
    hud::field(label, 12, vec![Span::styled(value.into(), t.fg(t.fg))], t)
}

fn hint(text: String, t: &Theme) -> Line<'static> {
    Line::from(Span::styled(text, t.fg(t.accent)))
}

fn caption(text: &str, t: &Theme) -> Line<'static> {
    Line::from(Span::styled(format!("── {text} "), t.bold(t.accent)))
}

fn status_field(tone: Option<Tone>, label: &str, t: &Theme) -> Line<'static> {
    let span = match tone {
        Some(tone) => {
            let (c, sym) = t.tone(tone);
            hud::badge(c, sym, label, t)
        }
        None => Span::styled(label.to_string(), t.fg(t.dim)),
    };
    hud::field("STATUS", 12, vec![span], t)
}

fn draw_card(f: &mut Frame, area: Rect, app: &App, tick: usize) {
    let t = &app.t;
    let arrow = if t.g.corners.is_some() { "▸" } else { ">" };
    let Some(m) = app.module() else {
        let inner = hud::panel(f, area, "EXIT", t);
        let lines = vec![
            Line::from(Span::styled("Leave the console.", t.fg(t.fg))),
            Line::from(Span::styled("Everything keeps running; `wayang` brings this back.", t.fg(t.dim))),
            Line::from(""),
            hint(format!("enter {arrow} quit"), t),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        return;
    };
    let st = &app.status;
    let d = &app.deck;
    let tone = app.module_tone(m);
    let (title, lines, gauge): (&str, Vec<Line>, Option<(usize, f64, String)>) = match m {
        Module::System => {
            let good = st.good.map(Slot::as_str).unwrap_or("-");
            let mut l = vec![
                status_field(tone, if app.error.is_some() { "UNAVAILABLE" } else { "OK" }, t),
                field("VERSION", st.version.clone().unwrap_or_else(|| "unknown".into()), t),
                field("CHANNEL", st.channel.clone(), t),
                field("HOST", format!("{} {} up {}", d.host, t.g.brand, d.uptime), t),
                field("DATA", if st.data { "/data mounted (persistent)" } else { "missing: nothing survives a reboot" }, t),
                Line::from(""),
                caption("A/B SLOTS", t),
            ];
            for si in &st.slots {
                let v = si.meta.as_ref().map(|m| m.version.as_str()).unwrap_or("-");
                let mut tags = Vec::new();
                if si.slot == st.active {
                    tags.push("running");
                }
                if Some(si.slot) == st.good {
                    tags.push("good");
                }
                if si.slot == st.boot_next {
                    tags.push("boots next");
                }
                l.push(Line::from(vec![
                    Span::styled(format!("  {} {:<3}", t.g.brand, si.slot.as_str()), t.bold(if si.slot == st.active { t.ok } else { t.fg })),
                    Span::styled(format!("{v:<10}"), t.fg(t.fg)),
                    Span::styled(tags.join(", "), t.fg(t.dim)),
                ]));
            }
            l.push(Line::from(""));
            l.push(field("BOOT", format!("next {} {} good {good} {} attempts {}", st.boot_next.as_str(), t.g.brand, t.g.brand, st.attempts), t));
            l.push(field("BACKEND", st.backend.clone(), t));
            if let Some(e) = &app.error {
                l.push(Line::from(""));
                l.push(Line::from(Span::styled(e.clone(), t.fg(t.bad))));
            }
            l.push(Line::from(""));
            l.push(hint(format!("enter {arrow} refresh"), t));
            ("SYSTEM", l, None)
        }
        Module::Updates => {
            let mut l = vec![
                status_field(tone, if app.job.is_some() { "RUNNING" } else { "READY" }, t),
                field("INSTALLED", st.version.clone().unwrap_or_else(|| "unknown".into()), t),
                field("CHANNEL", st.channel.clone(), t),
                field("TARGET", format!("slot {} (the one not running)", st.active.idle().as_str()), t),
            ];
            let mut gauge = None;
            if let Some(job) = &app.job {
                l.push(Line::from(""));
                l.push(caption("PROGRESS", t));
                match job_progress(job) {
                    // Determinate: a real Gauge row is drawn where this line is.
                    Some((ratio, label)) => gauge = Some((l.len(), ratio, label)),
                    // No byte count: the sweeping bar stays as the fallback.
                    None => l.push(Line::from(vec![
                        Span::styled(hud::progress_bar(tick, 24), t.bold(t.accent2)),
                        Span::styled(format!("  {}s", job.started.elapsed().as_secs()), t.fg(t.dim)),
                    ])),
                }
                l.push(Line::from(Span::styled(
                    format!("{} {}…", hud::spinner(tick), job.label),
                    t.fg(t.warn),
                )));
            }
            l.push(Line::from(""));
            l.push(caption("ACTIONS", t));
            for (k, what) in [
                ("c", "check the channel for a newer release"),
                ("u", "download, verify and stage it in the other slot"),
                ("g", "same, and allow a new major version"),
                ("b b", "boot the other slot next (one-shot, asks twice)"),
                ("x x", "roll back: boot the other slot next (asks twice)"),
            ] {
                l.push(Line::from(vec![
                    Span::styled(format!("  {k:<5}"), t.bold(t.accent)),
                    Span::styled(what, t.fg(t.fg)),
                ]));
            }
            l.push(Line::from(""));
            if app.job.is_none() {
                if let Some((_, m)) = &app.message {
                    l.push(field("LAST", m.clone(), t));
                }
            }
            l.push(Line::from(Span::styled(
                "Staged updates apply on the next reboot; a boot that never reaches `wayang mark-ok` falls back on its own.",
                t.fg(t.dim),
            )));
            ("UPDATES", l, gauge)
        }
        Module::Network => {
            let prim = d.ifaces.iter().find(|i| i.primary);
            let mut l = vec![
                status_field(tone, if tone == Some(Tone::Ok) { "ONLINE" } else { "NO ADDRESS" }, t),
                field(
                    "UPLINK",
                    prim.map(|i| format!("{} {}", i.name, i.ipv4.first().cloned().unwrap_or_else(|| "(no address)".into())))
                        .unwrap_or_else(|| "automatic (first wired port with a DHCP lease)".into()),
                    t,
                ),
            ];
            if d.rt.confirmed {
                l.push(field("MANAGED BY", "wayang-router (06 ROUTER)", t));
            }
            l.push(Line::from(""));
            l.push(caption("INTERFACES", t));
            for i in &d.ifaces {
                l.push(Line::from(vec![
                    Span::styled(format!("  {:<10}", i.name), t.bold(t.fg)),
                    Span::styled(if i.link { "up    " } else { "down  " }, t.fg(if i.link { t.ok } else { t.dim })),
                    Span::styled(format!("{:<18}", i.ipv4.first().cloned().unwrap_or_default()), t.fg(t.fg)),
                    Span::styled(format!("{}{}", if i.wireless { "wifi " } else { "" }, if i.primary { "primary" } else { "" }), t.fg(t.dim)),
                ]));
            }
            l.push(Line::from(""));
            l.push(hint(format!("enter {arrow} uplink: DHCP or static, IPv4/IPv6"), t));
            ("NETWORK", l, None)
        }
        Module::Wifi => {
            let radios: Vec<&net::Iface> = d.ifaces.iter().filter(|i| i.wireless).collect();
            let l = vec![
                status_field(tone, if radios.is_empty() { "NO RADIO" } else if tone == Some(Tone::Ok) { "CONNECTED" } else { "NOT CONNECTED" }, t),
                field("RADIOS", if radios.is_empty() { "none found".to_string() } else { radios.iter().map(|i| format!("{} ({})", i.name, i.driver)).collect::<Vec<_>>().join(", ") }, t),
                field("SAVED", if d.wifi_saved { "a network is saved (/data)" } else { "nothing saved" }, t),
                Line::from(""),
                hint(format!("enter {arrow} scan and connect"), t),
            ];
            ("WIFI", l, None)
        }
        Module::Dcheck => {
            let c = &d.dcheck;
            let state = if c.bin.is_some() { "INSTALLED" } else { "NOT INSTALLED" };
            let mut l = vec![
                status_field(tone, state, t),
                field("APP", "dcheck: storage health (SMART, RAID/NVMe, fake-drive checks)", t),
            ];
            if c.bin.is_some() {
                l.push(field("CONFIG", "not needed: dcheck has no config", t));
            }
            match &c.bin {
                Some(b) => {
                    l.push(field("BINARY", b.display().to_string(), t));
                    l.push(Line::from(""));
                    l.push(hint(format!("enter {arrow} open dcheck (q there comes back here)"), t));
                }
                None => {
                    l.push(Line::from(""));
                    l.push(Line::from(Span::styled("dcheck ships at /usr/bin/dcheck.", t.fg(t.dim))));
                }
            }
            ("DCHECK", l, None)
        }
        Module::Ssh => {
            let n = d.ssh_keys;
            let l = vec![
                status_field(tone, if n > 0 { "AUTHORIZED" } else { "NO KEYS" }, t),
                field("KEYS", format!("{n} authorized key(s) for root"), t),
                field("STORED", "/data/etc/ssh/authorized_keys (survives updates)", t),
                field("LIVE", "/root/.ssh/authorized_keys (this boot)", t),
                Line::from(""),
                hint(format!("enter {arrow} manage SSH keys"), t),
            ];
            ("SSH", l, None)
        }
        Module::Firewall | Module::Router => {
            let fw = m == Module::Firewall;
            let (c, name, what) = if fw {
                (&d.fw, "wayang-fw", "zones, policies, NAT (nftables), commit-confirm")
            } else {
                (&d.rt, "wayang-router", "interfaces, VLANs, bridges, routes, DHCP, commit-confirm")
            };
            let state = match (&c.bin, c.pending, c.confirmed) {
                (None, _, _) => "NOT INSTALLED",
                (Some(_), true, _) => "CONFIRM PENDING",
                (Some(_), false, true) => "ACTIVE",
                (Some(_), false, false) => "NO CONFIG",
            };
            let mut l = vec![
                status_field(tone, state, t),
                field("APP", format!("{name}: {what}"), t),
            ];
            match &c.bin {
                Some(b) => {
                    l.push(field("BINARY", b.display().to_string(), t));
                    l.push(field(
                        "CONFIG",
                        if c.confirmed { "confirmed config, applied at boot".to_string() } else { "nothing committed yet (boot leaves the box alone)".to_string() },
                        t,
                    ));
                    if fw {
                        l.push(field("ENGINE", if d.nft { "nftables ready" } else { "no nft on this OS: preview only" }, t));
                    } else if c.confirmed {
                        l.push(field("NETWORK", "interfaces are managed by the router at boot", t));
                    }
                    if c.pending {
                        l.push(Line::from(Span::styled(
                            "A commit is waiting for confirmation: open it and press y, or it rolls back.",
                            t.bold(t.bad),
                        )));
                    }
                    l.push(Line::from(""));
                    l.push(hint(format!("enter {arrow} open {name} (q there comes back here)"), t));
                }
                None => {
                    l.push(Line::from(""));
                    l.push(Line::from(Span::styled(format!("Copy {name} to /data/bin/{name} (survives updates)."), t.fg(t.dim))));
                }
            }
            (if fw { "FIREWALL" } else { "ROUTER" }, l, None)
        }
    };
    let inner = hud::panel(f, area, title, t);
    let wrap = Wrap { trim: false };
    match gauge {
        None => f.render_widget(Paragraph::new(lines).wrap(wrap), inner),
        Some((at, ratio, label)) => {
            let at = at.min(inner.height as usize) as u16;
            if at > 0 {
                let prefix: Vec<Line> = lines.iter().take(at as usize).cloned().collect();
                f.render_widget(Paragraph::new(prefix).wrap(wrap), Rect { height: at, ..inner });
            }
            if at < inner.height {
                let bar = Rect { x: inner.x, y: inner.y + at, width: inner.width, height: 1 };
                f.render_widget(
                    Gauge::default()
                        .ratio(ratio)
                        .label(label)
                        .style(t.fg(t.dim))
                        .gauge_style(t.bold(t.accent2)),
                    bar,
                );
            }
            let below = at + 1;
            if below < inner.height {
                let suffix: Vec<Line> = lines.iter().skip(at as usize).cloned().collect();
                if !suffix.is_empty() {
                    f.render_widget(
                        Paragraph::new(suffix).wrap(wrap),
                        Rect { y: inner.y + below, height: inner.height - below, ..inner },
                    );
                }
            }
        }
    }
}

fn draw_help(f: &mut Frame, body: Rect, app: &App) {
    let t = &app.t;
    let r = hud::centered(body, 66, 18);
    f.render_widget(Clear, r);
    let inner = hud::panel(f, r, "COMMAND REFERENCE", t);
    let key = |k: &str, d: &str| {
        Line::from(vec![Span::styled(format!("  {k:<12}"), t.bold(t.accent)), Span::styled(d.to_string(), t.fg(t.fg))])
    };
    let lines = vec![
        caption("DECK", t),
        key("↑ ↓  j k", "move"),
        key("enter", "open the module"),
        key("1 - 8", "jump to a module (01 system … 08 router)"),
        key("r", "refresh everything"),
        key("q  0  esc", "quit"),
        caption("UPDATES", t),
        key("c", "check for an update"),
        key("u", "update (same major version)"),
        key("g", "upgrade (may cross a major version)"),
        key("b  b", "boot the other slot next (one-shot)"),
        key("x  x", "roll back to the other slot"),
        caption("SCREENS", t),
        key("q", "back to this deck (network, wifi, ssh, fw, router)"),
        Line::from(""),
        Line::from(Span::styled("  any key closes this reference", t.fg(t.dim))),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

// ---- interactive runner ------------------------------------------------

/// Run the HUD on the real terminal.
pub fn run() -> std::io::Result<()> {
    crate::screen::run_with_launch(
        App::new(false),
        draw,
        |app| {
            app.poll();
            // Keep an open sub-screen's background work moving (net jobs,
            // wifi link refresh) so the HUD never blocks on it.
            match app.sub.as_mut() {
                Some(Sub::Net(a)) => a.poll_job(),
                Some(Sub::Wifi(a)) => a.poll_tick(),
                Some(Sub::Ssh(a)) => a.poll_job(),
                None => {}
            }
        },
        |app, key| app.on_key(key),
        |app| app.exit,
        |app| app.launch.take().map(std::process::Command::new),
        |app, res| {
            app.refresh();
            if let Err(e) = res {
                app.message = Some((Tone::Bad, format!("could not start: {e}")));
            }
        },
    )
}

// ---- demo / snapshot mode ----------------------------------------------

/// A named demo screen setup: `(name, mutate-the-app)`.
pub type DemoState = (&'static str, Box<dyn Fn(&mut App)>);

/// Handlers that put a demo app into each screen state.
pub fn demo_states() -> Vec<DemoState> {
    vec![
        ("dashboard", Box::new(|_app: &mut App| {})),
        (
            "updates",
            Box::new(|app: &mut App| {
                app.sel = 1;
                app.message = Some((Tone::Ok, "An update is available: u installs it into the other slot.".into()));
            }),
        ),
        (
            "updates-running",
            Box::new(|app: &mut App| {
                app.sel = 1;
                let (tx, rx) = mpsc::channel();
                std::mem::forget(tx);
                let progress = update::Progress::new();
                progress.set_total(8_000_000);
                progress.set(6_000_000);
                let started = Instant::now()
                    .checked_sub(std::time::Duration::from_secs(4))
                    .unwrap_or_else(Instant::now);
                app.job = Some(Job {
                    label: "Updating".into(),
                    rx,
                    kind: JobKind::Update,
                    progress: Some(progress),
                    started,
                });
                app.message = Some((Tone::Warn, "Updating…".into()));
            }),
        ),
        (
            "error",
            Box::new(|app: &mut App| {
                app.error = Some("no ESP found (set WAYANG_ESP or pass --esp DEV)".into());
                app.message = Some((Tone::Bad, "trusted keys: not found".into()));
            }),
        ),
        ("deck-network", Box::new(|app: &mut App| app.sel = 2)),
        ("deck-firewall", Box::new(|app: &mut App| app.sel = 6)),
        ("help", Box::new(|app: &mut App| app.help = true)),
        (
            "net",
            Box::new(|app: &mut App| {
                app.sub = Some(Sub::Net(netui::App::new(true)));
            }),
        ),
        (
            "net-static",
            Box::new(|app: &mut App| {
                app.sub = Some(Sub::Net(netui::demo_static()));
            }),
        ),
        (
            "wifi",
            Box::new(|app: &mut App| {
                app.sub = Some(Sub::Wifi(wifiui::App::new(true)));
            }),
        ),
        (
            "ssh",
            Box::new(|app: &mut App| {
                app.sub = Some(Sub::Ssh(sshkeysui::App::new(true)));
            }),
        ),
    ]
}

fn parse_size(size: &str) -> Result<(u16, u16), String> {
    size.split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)))
        .ok_or_else(|| format!("bad --size '{size}', want COLSxROWS"))
}

/// `wayang --demo`: print every screen to stdout.
pub fn dump_stdout(size: &str) -> Result<(), String> {
    let (w, h) = parse_size(size)?;
    for (name, setup) in demo_states() {
        let mut app = App::new(true);
        setup(&mut app);
        println!("===== {name} =====");
        print!("{}", crate::screen::render_text(&app, w, h, draw)?);
    }
    Ok(())
}

/// `wayang --screens DIR`: one text file per screen, plus an SVG (docs) when
/// `svg` is set.
pub fn dump_screens(dir: &str, size: &str, svg: bool) -> Result<(), String> {
    let (w, h) = parse_size(size)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
    for (name, setup) in demo_states() {
        let mut app = App::new(true);
        if svg {
            app.t = Theme::new(hud::Mode::Neon, &hud::FANCY);
            if let Some(s) = app.sub.as_mut() {
                set_theme(s);
            }
        }
        setup(&mut app);
        if svg {
            if let Some(s) = app.sub.as_mut() {
                set_theme(s);
            }
        }
        let path = format!("{dir}/{name}.txt");
        let text = crate::screen::render_text(&app, w, h, draw)?;
        std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))?;
        if svg {
            let path = format!("{dir}/{name}.svg");
            let doc = crate::screen::render_svg(&app, w, h, draw, &app.t)?;
            std::fs::write(&path, doc).map_err(|e| format!("{path}: {e}"))?;
        }
    }
    Ok(())
}

fn set_theme(s: &mut Sub) {
    let t = || Theme::new(hud::Mode::Neon, &hud::FANCY);
    match s {
        Sub::Net(a) => a.t = t(),
        Sub::Wifi(a) => a.t = t(),
        Sub::Ssh(a) => a.t = t(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(app: &App, w: u16, h: u16) -> Result<String, String> {
        crate::screen::render_text(app, w, h, draw)
    }

    fn key(app: &mut App, c: KeyCode) {
        app.on_key(KeyEvent::from(c));
    }

    #[test]
    fn deck_renders_like_dcheck() {
        let app = App::new(true);
        let text = render(&app, 120, 36).unwrap();
        for s in [
            "WAYANG OS",
            "SYSTEM CONSOLE",
            "MODULES",
            "01 SYSTEM",
            "02 UPDATES",
            "03 NETWORK",
            "04 WIFI",
            "05 SSH",
            "06 DCHECK",
            "07 FIREWALL",
            "08 ROUTER",
            "00 EXIT",
            "A/B SLOTS",
            "1.4.1",
            "SLOT B",
        ] {
            assert!(text.contains(s), "missing {s}:\n{text}");
        }
    }

    #[test]
    fn every_screen_renders_at_every_size() {
        for (w, h) in [(80, 24), (120, 36), (200, 60), (40, 12)] {
            for (name, setup) in demo_states() {
                let mut app = App::new(true);
                setup(&mut app);
                let text = render(&app, w, h).unwrap();
                assert!(!text.trim().is_empty(), "{name} at {w}x{h}");
            }
        }
    }

    #[test]
    fn number_keys_never_run_update_actions() {
        let mut app = App::new(true);
        for c in ['1', '2'] {
            key(&mut app, KeyCode::Char(c));
            assert!(app.job.is_none() && app.sub.is_none());
        }
        assert_eq!(app.module(), Some(Module::Updates));
    }

    #[test]
    fn rollback_asks_twice() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Char('x'));
        assert!(app.armed);
        assert!(app.message.as_ref().unwrap().1.contains("again"));
        key(&mut app, KeyCode::Down);
        assert!(!app.armed, "any other key disarms");
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Char('x'));
        key(&mut app, KeyCode::Char('x'));
        assert!(app.message.as_ref().unwrap().1.contains("rolling back"), "{:?}", app.message);
    }

    #[test]
    fn boot_other_asks_twice_then_stages() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Char('b'));
        assert!(app.armed_other);
        assert!(app.job.is_none(), "the first b only arms");
        let msg = app.message.as_ref().unwrap().1.clone();
        assert!(msg.contains("again") && msg.contains("one-shot"), "{msg}");
        // any other key disarms
        key(&mut app, KeyCode::Down);
        assert!(!app.armed_other);
        key(&mut app, KeyCode::Up);
        key(&mut app, KeyCode::Char('b'));
        key(&mut app, KeyCode::Char('b'));
        assert!(!app.armed_other);
        assert!(app.message.as_ref().unwrap().1.to_lowercase().contains("staging boot"), "{:?}", app.message);
    }

    #[test]
    fn boot_other_does_not_disturb_rollback_arming() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Char('b'));
        assert!(app.armed_other && !app.armed);
        key(&mut app, KeyCode::Char('x'));
        assert!(app.armed && !app.armed_other);
    }

    #[test]
    fn deck_opens_screens_and_back() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('3'));
        assert!(matches!(app.sub, Some(Sub::Net(_))));
        key(&mut app, KeyCode::Char('q'));
        assert!(app.sub.is_none());
        key(&mut app, KeyCode::Char('4'));
        assert!(matches!(app.sub, Some(Sub::Wifi(_))));
        key(&mut app, KeyCode::Char('q'));
        assert!(app.sub.is_none());
        key(&mut app, KeyCode::Char('5'));
        assert!(matches!(app.sub, Some(Sub::Ssh(_))));
        key(&mut app, KeyCode::Char('q'));
        assert!(app.sub.is_none());
    }

    #[test]
    fn companions_launch_or_explain() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('8'));
        assert!(app.message.as_ref().unwrap().1.contains("not installed"));
        app.demo = false;
        key(&mut app, KeyCode::Char('7'));
        assert_eq!(app.launch.as_deref(), Some(Path::new("/data/bin/wayang-fw")));
        key(&mut app, KeyCode::Char('6'));
        assert_eq!(app.launch.as_deref(), Some(Path::new("/usr/bin/dcheck")));
    }

    #[test]
    fn dcheck_is_green_when_installed_and_absent_is_none() {
        let mut app = App::new(true);
        assert_eq!(app.module_tone(Module::Dcheck), Some(Tone::Ok), "configless app reads healthy");
        app.deck.dcheck = Companion::default();
        assert_eq!(app.module_tone(Module::Dcheck), None, "absent: no symbol");
        // a config-having companion still warns without config.toml
        let fw = Companion {
            bin: Some("/data/bin/wayang-fw".into()),
            confirmed: false,
            pending: false,
            configless: false,
        };
        assert_eq!(fw.tone(), Some(Tone::Warn));
    }

    #[test]
    fn running_update_shows_determinate_gauge() {
        let mut app = App::new(true);
        app.sel = 1;
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        let progress = update::Progress::new();
        progress.set_total(8_000_000);
        progress.set(4_000_000);
        app.job = Some(Job {
            label: "Updating".into(),
            rx,
            kind: JobKind::Update,
            progress: Some(progress),
            started: Instant::now(),
        });
        let text = render(&app, 120, 36).unwrap();
        assert!(text.contains("PROGRESS"), "{text}");
        assert!(text.contains("RUNNING"), "{text}");
        assert!(text.contains("Updating"), "{text}");
        assert!(text.contains("50%"), "the gauge shows the real percent:\n{text}");
        assert!(text.contains("MB/s"), "the gauge shows the rate:\n{text}");
    }

    #[test]
    fn running_update_without_bytes_falls_back_to_the_bar() {
        let mut app = App::new(true);
        app.sel = 1;
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        app.job = Some(Job {
            label: "Checking for updates".into(),
            rx,
            kind: JobKind::Check,
            progress: None,
            started: Instant::now(),
        });
        let text = render(&app, 120, 36).unwrap();
        assert!(text.contains("PROGRESS"), "{text}");
        assert!(text.contains('█') && text.contains('░'), "indeterminate sweep is drawn:\n{text}");
        assert!(!text.contains('%'), "no percent when the total is unknown:\n{text}");
    }

    #[test]
    fn job_progress_formats_percent_rate_and_elapsed() {
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        let progress = update::Progress::new();
        progress.set_total(1_000_000);
        progress.set(250_000);
        let job = Job {
            label: "Updating".into(),
            rx,
            kind: JobKind::Update,
            progress: Some(progress),
            started: Instant::now(),
        };
        let (ratio, label) = job_progress(&job).unwrap();
        assert!((ratio - 0.25).abs() < 1e-9);
        assert!(label.contains("25%"), "{label}");
        assert!(label.contains("/s"), "{label}");
        assert!(label.ends_with('s'), "elapsed is shown: {label}");
    }

    #[test]
    fn navigation_clamps_and_quits() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Up);
        assert_eq!(app.sel, 0);
        for _ in 0..20 {
            key(&mut app, KeyCode::Down);
        }
        assert_eq!(app.sel, MODULES.len());
        key(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        key(&mut app, KeyCode::Char('q'));
        assert!(!app.help && !app.exit, "the first key only closes help");
        key(&mut app, KeyCode::Char('q'));
        assert!(app.exit);
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_size("nope").is_err());
        assert_eq!(parse_size("80x24").unwrap(), (80, 24));
    }
}
