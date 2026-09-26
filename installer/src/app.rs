//! Installer state machine and key handling. Drawing lives in `views`.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::disks::{self, Disk};
use crate::hud::Theme;
use crate::install::{self, Msg, Plan};
use crate::keys::{self, SshKey};
use crate::net;
use crate::sys::{self, SysInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Target,
    Network,
    Access,
    Confirm,
    Installing,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Hostname,
    Remote,
    TypeKey,
    NetIpv4Addr,
    NetIpv4Gw,
    NetIpv4Dns,
    NetIpv6Addr,
    NetIpv6Gw,
    NetIpv6Dns,
}

pub enum Modal {
    None,
    Input {
        kind: InputKind,
        buf: String,
        error: Option<String>,
    },
    Busy {
        label: String,
        rx: Receiver<Result<Vec<SshKey>, String>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Shell,
    Reboot,
    PowerOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Bad,
}

pub const WELCOME_ITEMS: [&str; 4] = ["INSTALL TO DISK", "OPEN A SHELL", "REBOOT", "POWER OFF"];
pub const ACCESS_ITEMS: [&str; 5] = [
    "HOSTNAME",
    "GITHUB / GITLAB USER",
    "IMPORT FROM USB DRIVE",
    "TYPE OR PASTE A KEY",
    "CONTINUE",
];
const ACCESS_CONTINUE: usize = 4;

pub struct App {
    pub demo: bool,
    pub t: Theme,
    pub sys: SysInfo,
    sys_at: Instant,
    pub screen: Screen,
    pub welcome_sel: usize,
    pub disks: Vec<Disk>,
    pub disk_sel: usize,
    pub target: Option<Disk>,
    pub ifaces: Vec<net::Iface>,
    pub net_sel: usize,
    pub net: net::NetChoice,
    pub hostname: String,
    pub keys: Vec<SshKey>,
    pub access_sel: usize,
    no_keys_ok: bool,
    pub modal: Modal,
    pub notice: Option<(Tone, String)>,
    pub confirm: String,
    pub step: usize,
    pub copy: f64,
    pub log: Vec<String>,
    pub error: Option<String>,
    rx: Option<Receiver<Msg>>,
    pub tick: usize,
    pub exit: Option<Exit>,
    pub missing: Vec<&'static str>,
}

impl App {
    pub fn new(demo: bool, t: Theme) -> App {
        let disks = disks::scan(demo);
        let ifaces = net::scan(demo);
        let mut app = App {
            demo,
            t,
            sys: sys::sysinfo(demo),
            sys_at: Instant::now(),
            screen: Screen::Welcome,
            welcome_sel: 0,
            disk_sel: 0,
            disks,
            target: None,
            net_sel: default_net_sel(&ifaces),
            ifaces,
            net: net::NetChoice::default(),
            hostname: "wayangos".into(),
            keys: Vec::new(),
            access_sel: 0,
            no_keys_ok: false,
            modal: Modal::None,
            notice: None,
            confirm: String::new(),
            step: 0,
            copy: 0.0,
            log: Vec::new(),
            error: None,
            rx: None,
            tick: 0,
            exit: None,
            missing: install::missing_payload(demo),
        };
        app.select_first_eligible();
        app
    }

    fn select_first_eligible(&mut self) {
        self.disk_sel = self
            .disks
            .iter()
            .position(|d| d.blocked().is_none())
            .unwrap_or(0);
    }

    fn say(&mut self, tone: Tone, text: impl Into<String>) {
        self.notice = Some((tone, text.into()));
    }

    /// Background work: install progress, key fetches, refreshed network info.
    pub fn poll(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        if self.sys_at.elapsed() > Duration::from_secs(3) && self.screen != Screen::Installing {
            self.sys = sys::sysinfo(self.demo);
            self.sys_at = Instant::now();
        }

        if let Modal::Busy { rx, .. } = &self.modal {
            match rx.try_recv() {
                Ok(Ok(new)) => {
                    let source = new.first().map(|k| k.source.clone()).unwrap_or_default();
                    let n = keys::merge(&mut self.keys, new);
                    if !self.demo {
                        keys::authorize_live(&self.keys);
                    }
                    self.modal = Modal::None;
                    self.no_keys_ok = false;
                    if n == 0 {
                        self.say(
                            Tone::Warn,
                            format!("no new keys from {source} (already added)"),
                        );
                    } else {
                        self.say(Tone::Ok, format!("added {n} key(s) from {source}"));
                    }
                }
                Ok(Err(e)) => {
                    self.modal = Modal::None;
                    self.say(Tone::Bad, e);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.modal = Modal::None,
            }
        }

        let mut finished = None;
        if let Some(rx) = &self.rx {
            loop {
                match rx.try_recv() {
                    Ok(Msg::Step(i)) => {
                        self.step = i;
                    }
                    Ok(Msg::Copy(p)) => self.copy = p,
                    Ok(Msg::Log(l)) => self.log.push(l),
                    Ok(Msg::Done) => finished = Some(None),
                    Ok(Msg::Failed(e)) => finished = Some(Some(e)),
                    Err(_) => break,
                }
            }
        }
        match finished {
            Some(None) => {
                self.rx = None;
                self.step = install::STEPS.len();
                self.screen = Screen::Done;
            }
            Some(Some(e)) => {
                self.rx = None;
                self.log.push(format!("ERROR: {e}"));
                self.error = Some(e);
                self.screen = Screen::Failed;
            }
            None => {}
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.modal, Modal::Busy { .. }) || self.rx.is_some()
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.rx.is_none() {
                self.exit = Some(Exit::Shell);
            }
            return;
        }
        if !matches!(self.modal, Modal::None) {
            self.on_modal_key(key);
            return;
        }
        self.notice = None;
        match self.screen {
            Screen::Welcome => self.on_welcome(key),
            Screen::Target => self.on_target(key),
            Screen::Network => self.on_network(key),
            Screen::Access => self.on_access(key),
            Screen::Confirm => self.on_confirm(key),
            Screen::Installing => {}
            Screen::Done => match key.code {
                KeyCode::Enter => self.exit = Some(Exit::Reboot),
                KeyCode::Char('s') => self.exit = Some(Exit::Shell),
                KeyCode::Char('p') => self.exit = Some(Exit::PowerOff),
                _ => {}
            },
            Screen::Failed => match key.code {
                KeyCode::Char('b') | KeyCode::Esc => {
                    self.rescan();
                    self.screen = Screen::Target;
                }
                KeyCode::Char('s') => self.exit = Some(Exit::Shell),
                KeyCode::Char('r') => self.exit = Some(Exit::Reboot),
                _ => {}
            },
        }
    }

    fn on_welcome(&mut self, key: KeyEvent) {
        let n = WELCOME_ITEMS.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.welcome_sel = (self.welcome_sel + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.welcome_sel = (self.welcome_sel + 1) % n,
            KeyCode::Char(c @ '1'..='4') => {
                self.welcome_sel = c as usize - '1' as usize;
                self.welcome_enter();
            }
            KeyCode::Enter => self.welcome_enter(),
            KeyCode::Char('q') => self.exit = Some(Exit::Shell),
            _ => {}
        }
    }

    fn welcome_enter(&mut self) {
        match self.welcome_sel {
            0 if !self.missing.is_empty() => self.say(
                Tone::Bad,
                format!(
                    "this image can't install: missing {}",
                    self.missing.join(", ")
                ),
            ),
            0 => {
                self.rescan();
                self.screen = Screen::Target;
            }
            1 => self.exit = Some(Exit::Shell),
            2 => self.exit = Some(Exit::Reboot),
            _ => self.exit = Some(Exit::PowerOff),
        }
    }

    fn rescan(&mut self) {
        self.disks = disks::scan(self.demo);
        self.select_first_eligible();
    }

    fn on_target(&mut self, key: KeyEvent) {
        let n = self.disks.len().max(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.disk_sel = (self.disk_sel + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.disk_sel = (self.disk_sel + 1) % n,
            KeyCode::Char('r') => {
                self.rescan();
                self.say(
                    Tone::Ok,
                    format!("rescanned: {} device(s)", self.disks.len()),
                );
            }
            KeyCode::Esc => self.screen = Screen::Welcome,
            KeyCode::Enter => match self.disks.get(self.disk_sel) {
                None => self.say(Tone::Bad, "no disk found - plug one in and press r"),
                Some(d) => match d.blocked() {
                    Some(why) => self.say(Tone::Warn, format!("{} can't be used: {why}", d.path())),
                    None => {
                        self.target = Some(d.clone());
                        self.ifaces = net::scan(self.demo);
                        self.net_sel = default_net_sel(&self.ifaces);
                        self.screen = Screen::Network;
                    }
                },
            },
            _ => {}
        }
    }

    fn on_access(&mut self, key: KeyEvent) {
        let n = ACCESS_ITEMS.len() + self.keys.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.access_sel = (self.access_sel + n - 1) % n,
            KeyCode::Down | KeyCode::Char('j') => self.access_sel = (self.access_sel + 1) % n,
            KeyCode::Esc => self.screen = Screen::Network,
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace
                if self.access_sel >= ACCESS_ITEMS.len() =>
            {
                let k = self.keys.remove(self.access_sel - ACCESS_ITEMS.len());
                self.access_sel = self.access_sel.min(n - 2);
                self.say(
                    Tone::Warn,
                    format!("removed {} {}", k.kind(), k.fingerprint()),
                );
            }
            KeyCode::Enter => match self.access_sel {
                0 => self.open_input(InputKind::Hostname, self.hostname.clone()),
                1 => self.open_input(InputKind::Remote, String::new()),
                2 => {
                    let disks = disks::scan(self.demo);
                    let demo = self.demo;
                    self.start_job("SCANNING USB DRIVES FOR .pub FILES", move || {
                        keys::scan_usb(&disks, demo)
                    });
                }
                3 => self.open_input(InputKind::TypeKey, String::new()),
                ACCESS_CONTINUE => {
                    if self.keys.is_empty() && !self.no_keys_ok {
                        self.no_keys_ok = true;
                        self.say(Tone::Warn, "no SSH key: only the local console can log in - enter again to continue anyway");
                    } else {
                        self.confirm.clear();
                        self.screen = Screen::Confirm;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn open_input(&mut self, kind: InputKind, buf: String) {
        self.modal = Modal::Input {
            kind,
            buf,
            error: None,
        };
    }

    fn on_network(&mut self, key: KeyEvent) {
        let rows = self.ifaces.len() + 1; // row 0 is AUTO
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.net_sel = (self.net_sel + rows - 1) % rows,
            KeyCode::Down | KeyCode::Char('j') => self.net_sel = (self.net_sel + 1) % rows,
            KeyCode::Esc => self.screen = Screen::Target,
            KeyCode::Char('m') => {
                self.net.mode = match self.net.mode {
                    net::Mode::Dhcp => net::Mode::Static,
                    net::Mode::Static => net::Mode::Dhcp,
                };
            }
            KeyCode::Char('f') if self.net.mode == net::Mode::Static => {
                self.net.family = self.net.family.next();
            }
            KeyCode::Char(c @ '1'..='6') if self.net.mode == net::Mode::Static => {
                let kind = match c {
                    '1' => InputKind::NetIpv4Addr,
                    '2' => InputKind::NetIpv4Gw,
                    '3' => InputKind::NetIpv4Dns,
                    '4' => InputKind::NetIpv6Addr,
                    '5' => InputKind::NetIpv6Gw,
                    _ => InputKind::NetIpv6Dns,
                };
                let buf = match kind {
                    InputKind::NetIpv4Addr => self.net.ipv4_address.clone(),
                    InputKind::NetIpv4Gw => self.net.ipv4_gateway.clone(),
                    InputKind::NetIpv4Dns => self.net.ipv4_dns.clone(),
                    InputKind::NetIpv6Addr => self.net.ipv6_address.clone(),
                    InputKind::NetIpv6Gw => self.net.ipv6_gateway.clone(),
                    _ => self.net.ipv6_dns.clone(),
                };
                self.open_input(kind, buf);
            }
            KeyCode::Enter => self.network_continue(),
            _ => {}
        }
    }

    fn network_continue(&mut self) {
        self.net.iface = if self.net_sel == 0 {
            None
        } else {
            self.ifaces.get(self.net_sel - 1).map(|i| i.name.clone())
        };
        if let Err(e) = self.network_ready() {
            self.say(Tone::Bad, e);
            return;
        }
        let summary = self.net.summary();
        match self.net.apply_live(self.demo) {
            Ok(msg) => self.say(Tone::Ok, msg),
            Err(e) => self.say(Tone::Warn, format!("{summary}: {e}")),
        }
        self.screen = Screen::Access;
    }

    fn network_ready(&self) -> Result<(), String> {
        if self.net.mode != net::Mode::Static {
            return Ok(());
        }
        if self.net.family.has_v4() && self.net.ipv4_address.is_empty() {
            return Err("set an IPv4 address (press 1), or use DHCP".into());
        }
        if self.net.family.has_v6() && self.net.ipv6_address.is_empty() {
            return Err("set an IPv6 address (press 4), or use DHCP".into());
        }
        Ok(())
    }

    fn start_job(
        &mut self,
        label: &str,
        job: impl FnOnce() -> Result<Vec<SshKey>, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(job());
        });
        self.modal = Modal::Busy {
            label: label.into(),
            rx,
        };
    }

    fn on_modal_key(&mut self, key: KeyEvent) {
        let Modal::Input { kind, buf, error } = &mut self.modal else {
            return;
        };
        let kind = *kind;
        match key.code {
            KeyCode::Esc => self.modal = Modal::None,
            KeyCode::Backspace => {
                buf.pop();
                *error = None;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if buf.len() < 16 * 1024 {
                    buf.push(c);
                }
                *error = None;
            }
            KeyCode::Enter => {
                let value = buf.trim().to_string();
                match kind {
                    InputKind::Hostname => match valid_hostname(&value) {
                        Ok(h) => {
                            self.hostname = h;
                            self.modal = Modal::None;
                        }
                        Err(e) => *error = Some(e),
                    },
                    InputKind::Remote => match keys::remote_spec(&value) {
                        Err(e) => *error = Some(e),
                        Ok((host, user)) => {
                            let demo = self.demo;
                            self.start_job(&format!("FETCHING {host}/{user}.keys"), move || {
                                keys::fetch_remote(&value, demo)
                            });
                        }
                    },
                    InputKind::TypeKey => match SshKey::parse(&value, "typed") {
                        None => {
                            *error =
                                Some("not an SSH public key (ssh-ed25519 AAAA... comment)".into())
                        }
                        Some(k) => {
                            let (tx, rx) = mpsc::channel();
                            let _ = tx.send(Ok(vec![k]));
                            self.modal = Modal::Busy {
                                label: String::new(),
                                rx,
                            };
                        }
                    },
                    InputKind::NetIpv4Addr => match net::valid_cidr(&value, false) {
                        Ok(()) => {
                            self.net.ipv4_address = value;
                            self.modal = Modal::None;
                        }
                        Err(e) => *error = Some(e),
                    },
                    InputKind::NetIpv4Gw => {
                        let r = if value.is_empty() {
                            Ok(())
                        } else {
                            net::valid_ip(&value, false)
                        };
                        match r {
                            Ok(()) => {
                                self.net.ipv4_gateway = value;
                                self.modal = Modal::None;
                            }
                            Err(e) => *error = Some(e),
                        }
                    }
                    InputKind::NetIpv4Dns => match net::valid_dns(&value, false) {
                        Ok(()) => {
                            self.net.ipv4_dns = value;
                            self.modal = Modal::None;
                        }
                        Err(e) => *error = Some(e),
                    },
                    InputKind::NetIpv6Addr => match net::valid_cidr(&value, true) {
                        Ok(()) => {
                            self.net.ipv6_address = value;
                            self.modal = Modal::None;
                        }
                        Err(e) => *error = Some(e),
                    },
                    InputKind::NetIpv6Gw => {
                        let r = if value.is_empty() {
                            Ok(())
                        } else {
                            net::valid_ip(&value, true)
                        };
                        match r {
                            Ok(()) => {
                                self.net.ipv6_gateway = value;
                                self.modal = Modal::None;
                            }
                            Err(e) => *error = Some(e),
                        }
                    }
                    InputKind::NetIpv6Dns => match net::valid_dns(&value, true) {
                        Ok(()) => {
                            self.net.ipv6_dns = value;
                            self.modal = Modal::None;
                        }
                        Err(e) => *error = Some(e),
                    },
                }
            }
            _ => {}
        }
    }

    fn on_confirm(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.screen = Screen::Access,
            KeyCode::Backspace => {
                self.confirm.pop();
            }
            KeyCode::Char(c) if self.confirm.len() < 8 => self.confirm.push(c.to_ascii_uppercase()),
            KeyCode::Enter if self.confirm == "YES" => {
                let Some(disk) = self.target.clone() else {
                    return;
                };
                let plan = Plan {
                    disk,
                    hostname: self.hostname.clone(),
                    keys: self.keys.iter().map(SshKey::line).collect(),
                    net: self.net.clone(),
                };
                self.log.clear();
                self.step = 0;
                self.copy = 0.0;
                self.rx = Some(install::start(plan, self.demo));
                self.screen = Screen::Installing;
            }
            KeyCode::Enter => self.say(
                Tone::Warn,
                "type YES (in capitals) to erase the disk and install",
            ),
            _ => {}
        }
    }

    /// Overall install progress 0.0..=1.0.
    pub fn progress(&self) -> f64 {
        let n = install::STEPS.len() as f64;
        let inner = if self.step == 4 { self.copy } else { 0.0 };
        ((self.step as f64 + inner) / n).min(1.0)
    }

    pub fn ip(&self) -> Option<String> {
        self.sys
            .net
            .first()
            .map(|(_, a)| a.split('/').next().unwrap_or(a).to_string())
    }
}

/// Network list row 0 is AUTO; wired interfaces follow. Pre-select the first
/// interface with a link, falling back to AUTO when nothing is plugged in.
fn default_net_sel(ifaces: &[net::Iface]) -> usize {
    ifaces
        .iter()
        .position(|i| i.link)
        .map(|i| i + 1)
        .unwrap_or(0)
}

fn valid_hostname(h: &str) -> Result<String, String> {
    let h = h.to_ascii_lowercase();
    let ok = !h.is_empty()
        && h.len() <= 63
        && !h.starts_with('-')
        && !h.ends_with('-')
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if ok {
        Ok(h)
    } else {
        Err("use letters, digits and '-' (1-63 chars)".into())
    }
}

/// Named setup that puts the demo app on one screen.
pub type DemoState = (&'static str, Box<dyn Fn(&mut App)>);

/// Put the demo app into each screen in turn (for `--screens`).
pub fn demo_states() -> Vec<DemoState> {
    let ed = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKDgLaeocpxjyYcVfzkVhNoYwyzzw0TqveMdqGKqXB6 alice@laptop";
    let with_keys = move |a: &mut App| {
        a.target = a.disks.first().cloned();
        a.keys = keys::parse_all(ed, "github:alice");
        a.hostname = "warung-01".into();
    };
    vec![
        ("01-welcome", Box::new(|_: &mut App| {})),
        (
            "02-target",
            Box::new(|a: &mut App| a.screen = Screen::Target),
        ),
        (
            "03-target-blocked",
            Box::new(|a: &mut App| {
                a.screen = Screen::Target;
                a.disk_sel = 3;
                a.say(Tone::Warn, "/dev/sdc can't be used: installer media");
            }),
        ),
        (
            "03b-network",
            Box::new(|a: &mut App| {
                a.target = a.disks.first().cloned();
                a.screen = Screen::Network;
                a.net_sel = default_net_sel(&a.ifaces);
                a.say(Tone::Ok, "network: eth0 dhcp");
            }),
        ),
        (
            "03c-network-static",
            Box::new(|a: &mut App| {
                a.target = a.disks.first().cloned();
                a.screen = Screen::Network;
                a.net_sel = default_net_sel(&a.ifaces);
                a.net = net::NetChoice {
                    iface: Some("eth0".into()),
                    mode: net::Mode::Static,
                    family: net::Family::Both,
                    ipv4_address: "192.168.1.50/24".into(),
                    ipv4_gateway: "192.168.1.1".into(),
                    ipv4_dns: "1.1.1.1 8.8.8.8".into(),
                    ipv6_address: "2001:db8::50/64".into(),
                    ipv6_gateway: "2001:db8::1".into(),
                    ipv6_dns: "2001:4860:4860::8888".into(),
                };
            }),
        ),
        (
            "04-access",
            Box::new(move |a: &mut App| {
                with_keys(a);
                a.screen = Screen::Access;
                a.access_sel = 5;
                a.say(Tone::Ok, "added 1 key(s) from github:alice");
            }),
        ),
        (
            "04b-access-many-keys",
            Box::new(move |a: &mut App| {
                with_keys(a);
                let k = a.keys[0].clone();
                a.keys = (1..=12)
                    .map(|n| SshKey {
                        comment: format!("user{n}@team"),
                        ..k.clone()
                    })
                    .collect();
                a.screen = Screen::Access;
                a.access_sel = ACCESS_ITEMS.len() + 10;
            }),
        ),
        (
            "05-access-input",
            Box::new(|a: &mut App| {
                a.screen = Screen::Access;
                a.target = a.disks.first().cloned();
                a.access_sel = 1;
                a.modal = Modal::Input {
                    kind: InputKind::Remote,
                    buf: "alice".into(),
                    error: None,
                };
            }),
        ),
        (
            "06-confirm",
            Box::new(move |a: &mut App| {
                with_keys(a);
                a.screen = Screen::Confirm;
                a.confirm = "YE".into();
            }),
        ),
        (
            "07-installing",
            Box::new(move |a: &mut App| {
                with_keys(a);
                a.screen = Screen::Installing;
                a.step = 4;
                a.copy = 0.62;
                a.log = vec![
                    "sfdisk /dev/nvme0n1: GPT, 512M EFI + Linux".into(),
                    "mkfs.vfat /dev/nvme0n1p1".into(),
                    "copy vmlinuz -> /boot/".into(),
                ];
            }),
        ),
        (
            "08-done",
            Box::new(move |a: &mut App| {
                with_keys(a);
                a.screen = Screen::Done;
                a.step = install::STEPS.len();
            }),
        ),
        (
            "09-failed",
            Box::new(move |a: &mut App| {
                with_keys(a);
                a.screen = Screen::Failed;
                a.step = 3;
                a.error = Some("/usr/sbin/mke2fs -F -q -t ext4 -L WAYANGDATA /dev/nvme0n1p2 failed: Device or resource busy".into());
                a.log = vec![
                    "sfdisk /dev/nvme0n1: GPT, 512M EFI + Linux".into(),
                    "mkfs.vfat /dev/nvme0n1p1".into(),
                ];
            }),
        ),
    ]
}
