//! Interactive HUD (`wayang` with no subcommand on a TTY): the WayangOS
//! console, laid out like dcheck, wayang-fw and wayang-router: a one-row
//! header bar, the command deck (MODULES on the left, a live card for the
//! selected module on the right), a status row and keycaps.
//!
//! Modules: SYSTEM (slots, boot state), UPDATES (check / update / upgrade /
//! rollback, run in the background), NETWORK, WIFI and SSH (sub-screens), and
//! FIREWALL / ROUTER, which hand the terminal to wayang-fw / wayang-router.
//! Number keys only jump; nothing that changes the system runs without its own
//! key, and rollback asks twice.
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
use crate::{help, input, review};
use crate::slot::{self, Slot};
use crate::sshkeys;
use crate::sshkeysui;
use crate::status::{self, Status};
use crate::ui;
use crate::update;
use crate::wifiui;

/// One command-deck card: `(title, lines, gauge)`.
type CardLines = (&'static str, Vec<Line<'static>>, Option<(usize, f64, String)>);

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
    Reset,
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
    /// The config-reset action was armed (SYSTEM card).
    pub armed_reset: bool,
    pub help: bool,
    pub help_scroll: usize,
    /// Pending REVIEW before a mutating action runs.
    pub review: Option<(Action, review::Review)>,
    /// Recently run actions (command-deck RECENT strip).
    pub recent: Vec<String>,
    /// `/` quick-jump query, if open.
    pub jump: Option<input::Input>,
    pub exit: bool,
    /// Open sub-screen (network/wifi), if any.
    pub sub: Option<Sub>,
    /// Companion app to run full-screen next (taken by the runner).
    pub launch: Option<PathBuf>,
    pub tick: usize,
}

/// A mutating action the deck can run after its REVIEW is confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Check,
    Update,
    Upgrade,
    Rollback,
    BootOther,
    Reset,
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
            armed_reset: false,
            help: false,
            help_scroll: 0,
            review: None,
            recent: Vec::new(),
            jump: None,
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
            self.absorb_sub_recent();
            if self.sub_exited() {
                self.sub = None;
                self.refresh();
            }
            return;
        }
        // REVIEW modal: enter/y confirms, esc/n/q cancels.
        if self.review.is_some() {
            let decision = self.review.as_mut().map(|(_, r)| r.on_key(key));
            match decision {
                Some(review::Decision::Confirm) => {
                    if let Some((action, _)) = self.review.take() {
                        self.run_action(action);
                    }
                }
                Some(review::Decision::Cancel) => {
                    self.review = None;
                    self.message = Some((Tone::Warn, "Cancelled: nothing changed.".into()));
                }
                _ => {}
            }
            return;
        }
        // `/` quick jump.
        if let Some(input) = self.jump.as_mut() {
            match input.on_key(key) {
                input::Outcome::Cancel => self.jump = None,
                input::Outcome::Submit(q) => {
                    self.jump = None;
                    self.jump_to(&q);
                }
                input::Outcome::None => {}
            }
            return;
        }
        // `?` help overlay (scrollable, `p` prints).
        if self.help {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(5),
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(5),
                KeyCode::Home => self.help_scroll = 0,
                KeyCode::Char('p') => {
                    self.message = Some(match help::print_keys("DECK") {
                        Ok(p) => (Tone::Ok, format!("Wrote {}", p.display())),
                        Err(e) => (Tone::Bad, e),
                    });
                }
                code if help::closes(code) => {
                    self.help = false;
                    self.help_scroll = 0;
                }
                _ => {}
            }
            return;
        }
        let armed = std::mem::take(&mut self.armed);
        let armed_other = std::mem::take(&mut self.armed_other);
        let armed_reset = std::mem::take(&mut self.armed_reset);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(MODULES.len()),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(3),
            KeyCode::PageDown => self.sel = (self.sel + 3).min(MODULES.len()),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = MODULES.len(),
            // `←→` switch tabs/panes and never leave the module; the deck is one pane.
            KeyCode::Left | KeyCode::Right => {}
            KeyCode::Tab => self.sel = (self.sel + 1).min(MODULES.len()),
            KeyCode::BackTab => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('/') => {
                self.jump = Some(input::Input::new(
                    "QUICK JUMP",
                    "Type a module or screen (fuzzy):",
                    "e.g. net, wifi, ssh, update, disk",
                ));
            }
            KeyCode::Char('r') => {
                self.refresh();
                self.message = Some((Tone::Ok, "Refreshed.".into()));
            }
            KeyCode::Enter => self.open(self.sel),
            KeyCode::Esc | KeyCode::Char('0') => self.exit = true,
            KeyCode::Char('q') => {
                if self.job.is_some() {
                    self.message =
                        Some((Tone::Warn, "An action is running; wait for it, or press 0 to leave.".into()));
                } else {
                    self.exit = true;
                }
            }
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
                let action = match c {
                    'c' => Action::Check,
                    'u' => Action::Update,
                    'g' => Action::Upgrade,
                    'x' => Action::Rollback,
                    _ => Action::BootOther,
                };
                // rollback / boot-other stay arm-then-act: the first press arms,
                // the second opens the REVIEW.
                match action {
                    Action::Rollback if !armed => {
                        self.armed = true;
                        self.message = Some((
                            Tone::Warn,
                            format!("Press x again to stage slot {} (rollback).", self.status.active.idle().as_str()),
                        ));
                        return;
                    }
                    Action::BootOther if !armed_other => {
                        self.armed_other = true;
                        self.message = Some((
                            Tone::Warn,
                            format!("Press b again to boot slot {} next (one-shot).", self.status.active.idle().as_str()),
                        ));
                        return;
                    }
                    _ => {}
                }
                self.request(action);
            }
            // SYSTEM card: reset to defaults, armed then REVIEWed.
            KeyCode::Char('x') if self.module() == Some(Module::System) => {
                if !armed_reset {
                    self.armed_reset = true;
                    self.message =
                        Some((Tone::Warn, "Press x again to reset the box's config to defaults.".into()));
                    return;
                }
                self.request(Action::Reset);
            }
            _ => {}
        }
    }

    /// Open a REVIEW for a mutating action (or run a read-only one directly).
    fn request(&mut self, action: Action) {
        if self.job.is_some() {
            self.message = Some((Tone::Warn, "An action is already running; wait for it.".into()));
            return;
        }
        if action == Action::Check {
            self.run_action(action);
            return;
        }
        let rev = self.review_for(action);
        self.review = Some((action, rev));
    }

    fn review_for(&self, action: Action) -> review::Review {
        let idle = self.status.active.idle();
        let other = idle.as_str();
        let (title, effect, note): (String, Vec<String>, Option<String>) = match action {
            Action::Check => unreachable!(),
            Action::Update => (
                "UPDATE (same major version)".into(),
                vec![
                    format!("check channel {} for a newer release", self.status.channel),
                    "download the signed bundle".into(),
                    "verify its signature and manifest".into(),
                    format!("stage it in slot {other}; it boots on the next reboot"),
                ],
                Some("A boot that never reaches `wayang mark-ok` falls back on its own.".into()),
            ),
            Action::Upgrade => (
                "UPGRADE (may cross a major version)".into(),
                vec![
                    format!("check channel {} for the newest release", self.status.channel),
                    "download the signed bundle".into(),
                    "verify its signature and manifest".into(),
                    format!("stage it in slot {other}; it boots on the next reboot"),
                ],
                Some("A boot that never reaches `wayang mark-ok` falls back on its own.".into()),
            ),
            Action::Rollback => (
                format!("ROLLBACK to slot {other}"),
                vec![
                    format!("stage slot {other} to boot next"),
                    "the running slot's data is untouched".into(),
                    "revert by booting the other slot again".into(),
                ],
                Some("This changes which slot boots next.".into()),
            ),
            Action::BootOther => (
                format!("BOOT slot {other} next (one-shot)"),
                vec![
                    format!("set a one-shot boot to slot {other}"),
                    "the following boot returns to the normal order".into(),
                ],
                Some("This changes which slot boots next, once.".into()),
            ),
            Action::Reset => (
                "RESET CONFIG to defaults".into(),
                vec![
                    "remove /data/etc/router (wayang-router config)".into(),
                    "remove /data/etc/fw (wayang-fw config)".into(),
                    "remove /data/etc/wayangi (EdgeRouter token/state)".into(),
                    "remove /data/etc/network (uplink choice)".into(),
                    "clear any pending OS update (boot the running slot)".into(),
                ],
                Some("Kept: /data/bin, /data/var (history) and the SSH keys.".into()),
            ),
        };
        let mut r = review::Review::new(title, effect);
        if let Some(n) = note {
            r = r.note(n);
        }
        r
    }

    /// Run a confirmed action.
    fn run_action(&mut self, action: Action) {
        let label = match action {
            Action::Check => "Checking for updates",
            Action::Update => "Updating",
            Action::Upgrade => "Upgrading",
            Action::Rollback => "Rolling back",
            Action::BootOther => "Staging boot",
            Action::Reset => "Resetting config",
        };
        self.record(label);
        match action {
            Action::Reset => self.start_reset_job(),
            Action::Check => self.start_job(JobKind::Check),
            Action::Update => self.start_job(JobKind::Update),
            Action::Upgrade => self.start_job(JobKind::Upgrade),
            Action::Rollback => self.start_job(JobKind::Rollback),
            Action::BootOther => self.start_job(JobKind::BootOther),
        }
    }

    /// Record an action in the command-deck RECENT strip (newest first).
    fn record(&mut self, what: impl Into<String>) {
        self.recent.insert(0, what.into());
        self.recent.truncate(5);
    }

    /// Pull actions a sub-screen recorded (net apply, wifi connect, ssh
    /// add/remove) into the deck's RECENT strip. The sub keeps them oldest
    /// first; the deck keeps newest first.
    fn absorb_sub_recent(&mut self) {
        let items = match self.sub.as_mut() {
            Some(Sub::Net(a)) => a.take_recent(),
            Some(Sub::Wifi(a)) => a.take_recent(),
            Some(Sub::Ssh(a)) => a.take_recent(),
            None => Vec::new(),
        };
        for what in items.into_iter().rev() {
            self.record(what);
        }
    }

    /// `/` quick jump: fuzzy over the module names plus a few aliases.
    fn jump_to(&mut self, query: &str) {
        let hits = match_modules(query);
        match hits.first().copied() {
            Some(i) => {
                self.sel = i;
                self.open(i);
            }
            None => {
                if !query.trim().is_empty() {
                    self.message = Some((Tone::Warn, format!("No module matches '{query}'.")));
                }
            }
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

    // ---- refresh ---------------------------------------------------------

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
            JobKind::Reset => "Resetting config",
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

    /// Reset the box's config in the background (shares the CLI's implementation).
    fn start_reset_job(&mut self) {
        if self.demo {
            self.message = Some((Tone::Ok, "demo: resetting config (nothing runs)".into()));
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            ui::set_quiet(true);
            crate::reset_config();
            let r: Result<i32, String> = Ok(0);
            let _ = tx.send(r);
        });
        self.message = Some((Tone::Warn, "Resetting config…".into()));
        self.job = Some(Job {
            label: "Resetting config".into(),
            rx,
            kind: JobKind::Reset,
            progress: None,
            started: Instant::now(),
        });
    }

    /// Picks up a finished background job.
    pub fn poll(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        let Some(job) = &self.job else { return };
        let Ok(result) = job.rx.try_recv() else { return };
        let kind = job.kind;
        self.job = None;
        let msg = match result {
            Ok(0) if kind == JobKind::Check => {
                (Tone::Ok, "An update is available: u installs it into the other slot.".into())
            }
            Ok(0) if kind == JobKind::Rollback => (Tone::Warn, "Rollback staged: reboot to apply.".into()),
            Ok(0) if kind == JobKind::BootOther => {
                (Tone::Warn, "Boot to the other slot staged: reboot to apply.".into())
            }
            Ok(0) if kind == JobKind::Reset => (Tone::Ok, "Config reset to defaults. Reboot to apply.".into()),
            Ok(0) => (Tone::Ok, "Update staged in the other slot: reboot to apply.".into()),
            Ok(2) => (Tone::Ok, "Up to date: no update available.".into()),
            Ok(code) => (Tone::Bad, format!("Finished with exit code {code}.")),
            Err(e) => (Tone::Bad, e),
        };
        self.record(msg.1.clone());
        self.message = Some(msg);
        self.refresh();
    }

    /// A staged update is visible: the next boot is not the running slot.
    pub fn pending_update(&self) -> bool {
        self.status.boot_next != self.status.active
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
    let crumb = match app.module() {
        Some(m) => {
            let name = MODULES.iter().find(|(mm, _)| *mm == m).map(|(_, n)| *n).unwrap_or("");
            t.breadcrumb(&["COMMAND DECK", name])
        }
        None => "COMMAND DECK".to_string(),
    };
    // The demo header must not show a real-looking release; the SYSTEM card
    // still shows the sample versions, clearly marked DEMO DATA.
    let ver = if app.demo { "demo" } else { st.version.as_deref().unwrap_or("") };
    hud::header_bar(f, header, &crumb, ver, right, t);

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

    let busy = app.job.is_some().then(|| hud::spinner(tick));
    let msg = app.message.as_ref().map(|(tone, m)| (*tone, m.as_str()));
    let idle = if app.pending_update() {
        "A staged update boots next; u replaces it, x rolls back."
    } else {
        "Pick a module; ? shows every key."
    };
    hud::status_row(f, status, msg, busy, idle, t);

    // Context footer: at most six, most relevant first.
    let keys: Vec<(&str, &str)> = match app.module() {
        Some(Module::Updates) => vec![
            ("u", "update"),
            ("g", "upgrade"),
            ("c", "check"),
            ("x", "rollback"),
            ("b", "boot other"),
            ("?", "help"),
        ],
        Some(Module::System) => vec![
            ("enter", "refresh"),
            ("x", "reset"),
            ("1-8", "jump"),
            ("/", "find"),
            ("?", "help"),
            ("q", "quit"),
        ],
        _ => vec![
            ("↑↓", "move"),
            ("enter", "open"),
            ("1-8", "jump"),
            ("/", "find"),
            ("?", "help"),
            ("q", "quit"),
        ],
    };
    let mut line = hud::keycaps(&keys, t);
    line.spans.insert(0, Span::raw(" "));
    f.render_widget(Paragraph::new(line), footer);

    if app.help {
        help::draw(f, body, t, "DECK", app.help_scroll);
    }
    if let Some((_, rev)) = &app.review {
        review::draw(f, body, t, rev);
    }
    if let Some(jump) = &app.jump {
        input::draw(f, body, t, jump, tick);
        draw_jump_hits(f, body, jump, t);
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
            (Span::styled(t.cursor(), t.bold(t.accent2)), t.highlight())
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
    // RECENT / QUICK strip: the last actions plus any staged update.
    if inner.height as usize > lines.len() + 3 {
        lines.push(Line::from(""));
        lines.push(hud::caption("RECENT", inner.width, t));
        if app.pending_update() {
            lines.push(Line::from(Span::styled(
                format!("{} pending update staged", t.g.warn),
                t.bold(t.warn),
            )));
        }
        for r in app.recent.iter().take(2) {
            lines.push(Line::from(Span::styled(format!("  {}", t.clip(r, row_w)), t.fg(t.dim))));
        }
        if app.recent.is_empty() && !app.pending_update() {
            lines.push(Line::from(Span::styled("  nothing yet — / to find", t.fg(t.dim))));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn field(label: &str, value: impl Into<String>, t: &Theme) -> Line<'static> {
    hud::field(label, 12, vec![Span::styled(value.into(), t.fg(t.fg))], t)
}

fn hint(text: String, t: &Theme) -> Line<'static> {
    Line::from(Span::styled(text, t.fg(t.accent)))
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
        // The deck's preview card never owns the keyboard: MODULES does.
        let inner = hud::panel_focus(f, area, "EXIT", false, t);
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
    // Inner width of the panel drawn at the end (borders take two columns), for
    // the `── TEXT ─────` captions built while the lines are assembled.
    let cap_w = area.width.saturating_sub(2);
    let (title, lines, gauge): CardLines = match m {
        Module::System => {
            let good = st.good.map(Slot::as_str).unwrap_or("-");
            let mut l = vec![
                status_field(tone, if app.error.is_some() { "UNAVAILABLE" } else { "OK" }, t),
                field("VERSION", st.version.clone().unwrap_or_else(|| "unknown".into()), t),
                field("CHANNEL", st.channel.clone(), t),
                field("HOST", format!("{} {} up {}", d.host, t.g.brand, d.uptime), t),
                field("DATA", if st.data { "/data mounted (persistent)" } else { "missing: nothing survives a reboot" }, t),
                Line::from(""),
                hud::caption("A/B SLOTS", cap_w, t),
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
                l.push(hud::caption("PROGRESS", cap_w, t));
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
            l.push(hud::caption("ACTIONS", cap_w, t));
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
            l.push(hud::caption("INTERFACES", cap_w, t));
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
    let inner = hud::panel_focus(f, area, title, false, t);
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

fn module_alias(m: Module) -> &'static str {
    match m {
        Module::System => "system slots boot",
        Module::Updates => "update upgrade rollback channel",
        Module::Network => "net network uplink ip interface",
        Module::Wifi => "wifi wireless ssid wlan",
        Module::Ssh => "ssh key authorized",
        Module::Dcheck => "dcheck disk storage health",
        Module::Firewall => "firewall fw nft policy",
        Module::Router => "router route dhcp vlan",
    }
}

/// Fuzzy module matches for a `/` query (number, name or alias).
fn match_modules(q: &str) -> Vec<usize> {
    let q = q.trim().to_ascii_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    if let Ok(n) = q.parse::<usize>() {
        if (1..=MODULES.len()).contains(&n) {
            return vec![n - 1];
        }
    }
    MODULES
        .iter()
        .enumerate()
        .filter(|(_, (m, name))| {
            let hay = format!("{} {}", name.to_ascii_lowercase(), module_alias(*m));
            hay.contains(&q) || hay.split_whitespace().any(|w| w.starts_with(&q))
        })
        .map(|(i, _)| i)
        .collect()
}

/// The matched destinations under the `/` input box.
fn draw_jump_hits(f: &mut Frame, body: Rect, jump: &input::Input, t: &Theme) {
    let hits = match_modules(&jump.buf);
    if hits.is_empty() {
        return;
    }
    let ir = hud::centered(body, 76, 9);
    let h = (hits.len() as u16 + 2).min(8).min(body.bottom().saturating_sub(ir.y + 9));
    if h < 3 {
        return;
    }
    let rect = Rect { x: ir.x, y: ir.y + 9, width: ir.width, height: h };
    f.render_widget(Clear, rect);
    let inner = hud::panel(f, rect, "JUMP TO", t);
    let mut lines = Vec::new();
    for i in hits.iter().take(inner.height as usize) {
        let (_, name) = MODULES[*i];
        lines.push(Line::from(vec![
            Span::styled(format!("  {:02} ", i + 1), t.fg(t.dim)),
            Span::styled(name.to_string(), t.bold(t.fg)),
            Span::styled("   enter opens it", t.fg(t.dim)),
        ]));
    }
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
            "review-update",
            Box::new(|app: &mut App| {
                app.sel = 1;
                app.request(Action::Update);
            }),
        ),
        (
            "review-reset",
            Box::new(|app: &mut App| {
                app.request(Action::Reset);
            }),
        ),
        (
            "recent",
            Box::new(|app: &mut App| {
                app.recent = vec!["Updating".into(), "Checking for updates".into()];
            }),
        ),
        (
            "jump",
            Box::new(|app: &mut App| {
                app.jump = Some(input::Input::new("QUICK JUMP", "Type a module or screen (fuzzy):", ""));
                if let Some(j) = app.jump.as_mut() {
                    j.buf = "net".into();
                }
            }),
        ),
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
            "wifi-country",
            Box::new(|app: &mut App| {
                app.sub = Some(Sub::Wifi(wifiui::demo_country()));
            }),
        ),
        (
            "ssh",
            Box::new(|app: &mut App| {
                app.sub = Some(Sub::Ssh(sshkeysui::App::new(true)));
            }),
        ),
        (
            "ssh-add",
            Box::new(|app: &mut App| {
                app.sub = Some(Sub::Ssh(sshkeysui::demo_add()));
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
    // Deterministic theme so snapshots never depend on the caller's terminal.
    let mode = if svg { hud::Mode::Neon } else { hud::Mode::Ansi };
    for (name, setup) in demo_states() {
        let mut app = App::new(true);
        app.t = Theme::new(mode, &hud::FANCY);
        setup(&mut app);
        if let Some(s) = app.sub.as_mut() {
            set_theme_with(s, mode);
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

fn set_theme_with(s: &mut Sub, mode: hud::Mode) {
    let t = || Theme::new(mode, &hud::FANCY);
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
            "COMMAND DECK",
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
    fn focused_deck_pane_and_dim_card() {
        // The MODULES list owns the keyboard; the preview card is unfocused.
        let app = App::new(true);
        let text = render(&app, 120, 36).unwrap();
        assert!(text.contains("◢ ▸ MODULES ◣"), "{text}");
        assert!(text.contains("◢ SYSTEM ◣"), "the card is dim (no marker):\n{text}");
        assert!(!text.contains("◢ ▸ SYSTEM ◣"), "{text}");
    }

    #[test]
    fn sub_screen_focus_follows_the_active_pane() {
        let mut app = App::new(true);
        app.sub = Some(Sub::Net(netui::App::new(true)));
        let text = render(&app, 110, 34).unwrap();
        assert!(text.contains("◢ ▸ INTERFACES ◣"), "{text}");
        assert!(!text.contains("◢ ▸ MODE"), "the config pane is unfocused:\n{text}");
        // → moves the keyboard to CONFIG; the mode/address pane lights up.
        app.on_key(KeyEvent::from(KeyCode::Right));
        let text = render(&app, 110, 34).unwrap();
        assert!(text.contains("◢ ▸ MODE — EDIT: form ◣"), "{text}");
        assert!(!text.contains("◢ ▸ INTERFACES ◣"), "{text}");
    }

    #[test]
    fn focus_marker_survives_no_colour() {
        let mut app = App::new(true);
        app.t = Theme::new(hud::Mode::Mono, &hud::FANCY);
        let text = render(&app, 120, 36).unwrap();
        assert!(text.contains("◢ > MODULES ◣"), "mono marks focus with `>`:\n{text}");
        assert!(!text.contains("◢ > SYSTEM ◣"), "the card stays unfocused:\n{text}");
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
    fn rollback_arms_then_reviews_then_runs() {
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
        assert!(app.review.is_some(), "the second x opens REVIEW, nothing runs yet");
        assert!(app.job.is_none());
        key(&mut app, KeyCode::Enter);
        assert!(app.review.is_none());
        assert!(app.message.as_ref().unwrap().1.contains("rolling back"), "{:?}", app.message);
    }

    #[test]
    fn boot_other_arms_then_reviews_then_stages() {
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
        assert!(app.review.is_some(), "the second b opens REVIEW");
        key(&mut app, KeyCode::Enter);
        assert!(app.message.as_ref().unwrap().1.to_lowercase().contains("staging boot"), "{:?}", app.message);
    }

    #[test]
    fn review_can_be_cancelled_without_running() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('2'));
        key(&mut app, KeyCode::Char('u'));
        let (_, rev) = app.review.as_ref().expect("u opens REVIEW");
        assert!(rev.effect.iter().any(|l| l.contains("stage it in slot")), "{:?}", rev.effect);
        key(&mut app, KeyCode::Esc);
        assert!(app.review.is_none() && app.job.is_none(), "esc cancels; nothing runs");
        assert!(app.message.as_ref().unwrap().1.to_lowercase().contains("nothing changed"));
        // check does not need a review
        key(&mut app, KeyCode::Char('c'));
        assert!(app.review.is_none());
        assert!(app.message.as_ref().unwrap().1.to_lowercase().contains("checking") || app.job.is_some());
    }

    #[test]
    fn reset_arms_reviews_and_never_runs_in_demo() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('1'));
        key(&mut app, KeyCode::Char('x'));
        assert!(app.armed_reset, "first x arms");
        assert!(app.review.is_none());
        key(&mut app, KeyCode::Char('x'));
        assert!(app.review.is_some(), "second x opens REVIEW");
        key(&mut app, KeyCode::Enter);
        assert!(app.job.is_none(), "demo never runs a job");
        assert!(app.message.as_ref().unwrap().1.to_lowercase().contains("resetting config"));
    }

    #[test]
    fn arrows_switch_panes_and_never_leave_the_deck() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Left);
        assert!(!app.exit && app.sel == 0, "←→ never leave or move the deck selection");
        key(&mut app, KeyCode::End);
        assert_eq!(app.sel, MODULES.len());
        key(&mut app, KeyCode::Home);
        assert_eq!(app.sel, 0);
        key(&mut app, KeyCode::PageDown);
        assert_eq!(app.sel, 3);
    }

    #[test]
    fn q_is_guarded_while_a_job_runs() {
        let mut app = App::new(true);
        app.sel = 1;
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        app.job = Some(Job {
            label: "Updating".into(),
            rx,
            kind: JobKind::Update,
            progress: None,
            started: Instant::now(),
        });
        key(&mut app, KeyCode::Char('q'));
        assert!(!app.exit, "q warns instead of quitting mid-action");
        assert!(app.message.as_ref().unwrap().1.contains("running"));
    }

    #[test]
    fn committed_screens_match_the_renderer() {
        // Guards the checked-in snapshots; regenerate with:
        //   cargo run -- --screens screens --size 110x34
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("screens");
        if !dir.is_dir() {
            return;
        }
        for (name, setup) in demo_states() {
            let path = dir.join(format!("{name}.txt"));
            if !path.is_file() {
                continue;
            }
            let mut app = App::new(true);
            app.t = Theme::new(hud::Mode::Ansi, &hud::FANCY);
            setup(&mut app);
            if let Some(s) = app.sub.as_mut() {
                set_theme_with(s, hud::Mode::Ansi);
            }
            let text = render(&app, 110, 34).unwrap();
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert_eq!(text, on_disk, "screens/{name}.txt is stale; regenerate with `--screens screens`");
        }
    }

    #[test]
    fn multi_pane_screens_have_a_tab_row_and_arrows_stay() {
        let mut app = App::new(true);
        for (num, tab) in [('3', "INTERFACES"), ('4', "ACCESS POINTS"), ('5', "AUTHORIZED KEYS")] {
            key(&mut app, KeyCode::Char(num));
            let text = render(&app, 120, 36).unwrap();
            assert!(text.contains(tab), "module {num} shows its tab row:\n{text}");
            key(&mut app, KeyCode::Right);
            key(&mut app, KeyCode::Left);
            assert!(!app.exit && app.sub.is_some(), "arrows stay inside module {num}");
            key(&mut app, KeyCode::Char('b'));
            assert!(app.sub.is_none() && !app.exit, "b returns to the deck");
        }
    }

    #[test]
    fn slash_jumps_to_a_module_and_help_scrolls() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('/'));
        assert!(app.jump.is_some());
        for c in "wifi".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Enter);
        assert!(app.jump.is_none());
        assert!(matches!(app.sub, Some(Sub::Wifi(_))), "jump opens WIFI");
        key(&mut app, KeyCode::Char('q')); // back to the deck
        key(&mut app, KeyCode::Char('?'));
        assert!(app.help);
        key(&mut app, KeyCode::Down);
        assert!(app.help, "scroll keys keep help open");
        assert_eq!(app.help_scroll, 1);
        key(&mut app, KeyCode::Char('?'));
        assert!(!app.help, "? closes it");
    }

    #[test]
    fn sub_screen_actions_echo_into_recent() {
        // A net apply inside the NETWORK sub-screen must show in the deck strip.
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('3'));
        key(&mut app, KeyCode::Char('a'));
        assert!(app.recent.is_empty(), "nothing recorded before the confirm");
        key(&mut app, KeyCode::Enter);
        assert!(
            app.recent.iter().any(|r| r.starts_with("Network: applied")),
            "{:?}",
            app.recent
        );
        // leave the net screen (its demo job may still be running) and open SSH
        app.sub = None;
        key(&mut app, KeyCode::Char('5'));
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Enter); // paste a public key
        for c in "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKDgLaeocpxjyYcVfzkVhNoYwyzzw0TqveMdqGKqXB6 mentee@box".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        key(&mut app, KeyCode::Enter); // -> REVIEW
        key(&mut app, KeyCode::Enter); // confirm
        assert!(
            app.recent.iter().any(|r| r.starts_with("SSH: authorized")),
            "{:?}",
            app.recent
        );
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
