//! `wayang net` HUD screen: pick an interface, choose DHCP/Static, edit the
//! static fields and Apply. Writes `/data/etc/network/{primary,config}` and
//! brings the choice up immediately (see `crate::net`).

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crate::hud::{self, Theme};
use crate::input::{self, Input, Outcome};
use crate::net::{self, Family, Mode, NetChoice};
use crate::tui::Tone;
use crate::{help, review};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    V4Addr,
    V4Gw,
    V4Dns,
    V6Addr,
    V6Gw,
    V6Dns,
}

/// A row in the CONFIG pane's focus order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Mode,
    Family,
    Field(Field),
}

/// A mutation awaiting REVIEW.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Apply,
    Link(bool),
    Dhcp,
    Also,
}

/// The visible tab row; `←→`/`tab` moves between them.
const TABS: [&str; 2] = ["INTERFACES", "CONFIG"];

pub struct App {
    pub t: Theme,
    pub demo: bool,
    pub ifaces: Vec<net::Iface>,
    pub sel: usize,
    /// Interfaces in the persisted `also` list (leased address-only at boot).
    pub also: Vec<String>,
    pub choice: NetChoice,
    /// Active tab/pane: 0 = INTERFACES, 1 = CONFIG.
    pub pane: usize,
    /// Focused row within the CONFIG pane.
    pub field_sel: usize,
    input: Option<(Field, Input)>,
    /// `/` quick-jump query over the interface list, if open.
    jump: Option<Input>,
    pub message: Option<(Tone, String)>,
    /// Background operation (apply / link up-down / dhcp); polled each tick so
    /// the HUD never blocks on the network.
    job: Option<Receiver<Result<String, String>>>,
    /// Pending REVIEW before a mutation runs.
    review: Option<(Pending, review::Review)>,
    /// Actions run here since the deck last drained them (RECENT strip).
    recent: Vec<String>,
    pub help: bool,
    pub help_scroll: usize,
    pub exit: bool,
}

impl App {
    pub fn new(demo: bool) -> App {
        let (ifaces, primary) = if demo {
            (demo_ifaces(), Some("eth0".to_string()))
        } else {
            let primary = net::primary_iface();
            (net::list(primary.as_deref()), primary)
        };
        let also = if demo {
            vec!["enp0s20u1".to_string()]
        } else {
            net::read_also()
        };
        let mut choice = NetChoice::default();
        if let Some(p) = &primary {
            choice.iface = Some(p.clone());
        }
        let sel = primary
            .as_ref()
            .and_then(|p| ifaces.iter().position(|i| &i.name == p))
            .unwrap_or(0);
        App {
            t: hud::detect(),
            demo,
            ifaces,
            sel,
            also,
            choice,
            pane: 0,
            field_sel: 0,
            input: None,
            jump: None,
            message: None,
            job: None,
            review: None,
            recent: Vec::new(),
            help: false,
            help_scroll: 0,
            exit: false,
        }
    }

    /// Record a mutating action so the deck's RECENT strip shows it.
    fn record(&mut self, what: impl Into<String>) {
        self.recent.push(what.into());
    }

    /// Drain the actions recorded since the last call (oldest first).
    pub fn take_recent(&mut self) -> Vec<String> {
        std::mem::take(&mut self.recent)
    }

    fn refresh(&mut self) {
        let primary = net::primary_iface();
        let keep = self.ifaces.get(self.sel).map(|i| i.name.clone());
        self.ifaces = net::list(primary.as_deref());
        if !self.demo {
            self.also = net::read_also();
        }
        self.sel = keep
            .and_then(|n| self.ifaces.iter().position(|i| i.name == n))
            .unwrap_or(0);
    }

    fn open_field(&mut self, f: Field) {
        let (title, prompt, hint, value) = match f {
            Field::V4Addr => (
                "IPv4 ADDRESS",
                "Address with prefix, e.g. 192.168.1.50/24:",
                "the /prefix is required",
                self.choice.ipv4_address.clone(),
            ),
            Field::V4Gw => (
                "IPv4 GATEWAY",
                "Default gateway (optional):",
                "e.g. 192.168.1.1",
                self.choice.ipv4_gateway.clone(),
            ),
            Field::V4Dns => (
                "IPv4 DNS",
                "Nameservers, space separated (optional):",
                "e.g. 1.1.1.1 8.8.8.8",
                self.choice.ipv4_dns.clone(),
            ),
            Field::V6Addr => (
                "IPv6 ADDRESS",
                "Address with prefix, e.g. 2001:db8::50/64:",
                "the /prefix is required",
                self.choice.ipv6_address.clone(),
            ),
            Field::V6Gw => (
                "IPv6 GATEWAY",
                "Default gateway (optional):",
                "e.g. 2001:db8::1",
                self.choice.ipv6_gateway.clone(),
            ),
            Field::V6Dns => (
                "IPv6 DNS",
                "Nameservers, space separated (optional):",
                "e.g. 2001:4860:4860::8888",
                self.choice.ipv6_dns.clone(),
            ),
        };
        self.input = Some((f, Input::new(title, prompt, hint).value(value)));
    }

    /// Rows in the CONFIG pane's focus order for the current choice.
    fn rows(&self) -> Vec<Row> {
        let mut r = vec![Row::Mode];
        if self.choice.mode == Mode::Static {
            r.push(Row::Family);
            if self.choice.family.has_v4() {
                r.extend([Row::Field(Field::V4Addr), Row::Field(Field::V4Gw), Row::Field(Field::V4Dns)]);
            }
            if self.choice.family.has_v6() {
                r.extend([Row::Field(Field::V6Addr), Row::Field(Field::V6Gw), Row::Field(Field::V6Dns)]);
            }
        }
        r
    }

    fn move_sel(&mut self, delta: isize) {
        if self.pane == 0 {
            let n = self.ifaces.len();
            if n == 0 {
                return;
            }
            self.sel = ((self.sel as isize + delta).rem_euclid(n as isize)) as usize;
        } else {
            let n = self.rows().len();
            if n == 0 {
                return;
            }
            self.field_sel = ((self.field_sel as isize + delta).clamp(0, n as isize - 1)) as usize;
        }
    }

    fn toggle_mode(&mut self) {
        self.choice.mode = match self.choice.mode {
            Mode::Dhcp => Mode::Static,
            Mode::Static => Mode::Dhcp,
        };
    }

    fn cycle_family(&mut self) {
        if self.choice.mode == Mode::Static {
            self.choice.family = self.choice.family.next();
            let n = self.rows().len();
            self.field_sel = self.field_sel.min(n.saturating_sub(1));
        }
    }

    /// Enter on a CONFIG row: toggle the mode, cycle the family, or edit a field.
    fn activate_row(&mut self) {
        match self.rows().get(self.field_sel).copied() {
            Some(Row::Mode) => self.toggle_mode(),
            Some(Row::Family) => self.cycle_family(),
            Some(Row::Field(f)) => self.open_field(f),
            None => {}
        }
    }

    /// `space`: toggle the selected mode/family row (never commits).
    fn toggle_row(&mut self) {
        match self.rows().get(self.field_sel).copied() {
            Some(Row::Mode) => self.toggle_mode(),
            Some(Row::Family) => self.cycle_family(),
            _ => {}
        }
    }

    fn request_review(&mut self, pending: Pending, title: String, effect: Vec<String>, note: Option<String>) {
        let mut r = review::Review::new(title, effect);
        if let Some(n) = note {
            r = r.note(n);
        }
        self.review = Some((pending, r));
    }

    /// Apply opens a REVIEW that names the default-route/primary change.
    fn request_apply(&mut self) {
        let Some(iface) = self.ifaces.get(self.sel) else {
            self.message = Some((Tone::Bad, "No interface to apply.".into()));
            return;
        };
        self.choice.iface = Some(iface.name.clone());
        if let Err(e) = self.choice.validate() {
            self.message = Some((Tone::Bad, e));
            return;
        }
        let name = iface.name.clone();
        let current = self.ifaces.iter().find(|i| i.primary).map(|i| i.name.clone());
        let mut effect = vec![
            format!("write /data/etc/network/primary  ->  {name}"),
            format!("write /data/etc/network/config  ({})", self.choice.summary()),
        ];
        match self.choice.mode {
            Mode::Dhcp => effect.push(format!("run udhcpc on {name} for a lease")),
            Mode::Static => {
                if self.choice.family.has_v4() {
                    effect.push(format!("add {} dev {name}", self.choice.ipv4_address));
                }
                if self.choice.family.has_v6() {
                    effect.push(format!("add {} dev {name}", self.choice.ipv6_address));
                }
            }
        }
        effect.push("replace the default route and /etc/resolv.conf".into());
        let note = Some(match &current {
            Some(c) if *c != name => {
                format!("This promotes {name} to the primary uplink (default route); {c} is demoted.")
            }
            _ => format!("This makes {name} the primary uplink at boot."),
        });
        self.request_review(Pending::Apply, format!("APPLY network on {name}"), effect, note);
    }

    fn request_pending(&mut self, pending: Pending) {
        let Some(iface) = self.selected() else {
            self.message = Some((Tone::Bad, "No interface selected.".into()));
            return;
        };
        let (title, effect, note) = match pending {
            Pending::Link(up) => (
                format!("LINK {} {}", iface, if up { "up" } else { "down" }),
                vec![format!("ip link set {iface} {}", if up { "up" } else { "down" })],
                Some("The link only; the address and primary are unchanged.".into()),
            ),
            Pending::Dhcp => (
                format!("DHCP lease on {iface}"),
                vec![
                    format!("bring {iface} up and run udhcpc once"),
                    "keep the current primary pinned (address only)".into(),
                ],
                None,
            ),
            Pending::Also => {
                let on = !net::is_also(&self.also, &iface);
                (
                    format!("ALSO-LEASE {iface} at boot: {}", if on { "on" } else { "off" }),
                    vec![format!("{} {iface} in /data/etc/network/also", if on { "add" } else { "remove" })],
                    None,
                )
            }
            Pending::Apply => unreachable!(),
        };
        self.request_review(pending, title, effect, note);
    }

    fn run_pending(&mut self, pending: Pending) {
        match pending {
            Pending::Apply => self.apply(),
            Pending::Link(up) => self.link(up),
            Pending::Dhcp => self.dhcp(),
            Pending::Also => self.toggle_also(),
        }
    }

    /// Candidate strings for the `/` quick jump (read-only: selects an iface).
    fn jump_targets(&self) -> Vec<String> {
        self.ifaces
            .iter()
            .map(|i| {
                let ips = i.ipv4.join(" ");
                if ips.is_empty() {
                    format!("{} {}", i.name, i.driver)
                } else {
                    format!("{} {} {}", i.name, i.driver, ips)
                }
            })
            .collect()
    }

    fn open_jump(&mut self) {
        self.jump = Some(Input::new(
            "QUICK JUMP",
            "Type an interface to select (fuzzy):",
            "e.g. eth0, wlan, 192.168",
        ));
    }

    fn submit_jump(&mut self, q: &str) {
        let targets = self.jump_targets();
        match input::fuzzy_matches(q, &targets).first().copied() {
            Some(i) => {
                self.sel = i;
                self.pane = 0;
                let name = self.ifaces[i].name.clone();
                self.message = Some((Tone::Ok, format!("Jumped to {name}.")));
            }
            None if !q.trim().is_empty() => {
                self.message = Some((Tone::Warn, format!("No interface matches '{q}'.")));
            }
            None => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.review.is_some() {
            let decision = self.review.as_mut().map(|(_, r)| r.on_key(key));
            match decision {
                Some(review::Decision::Confirm) => {
                    if let Some((p, _)) = self.review.take() {
                        self.run_pending(p);
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
        if self.input.is_some() {
            self.on_input_key(key);
            return;
        }
        if self.jump.is_some() {
            let outcome = self.jump.as_mut().map(|i| i.on_key(key));
            match outcome {
                Some(Outcome::Cancel) => self.jump = None,
                Some(Outcome::Submit(q)) => {
                    self.jump = None;
                    self.submit_jump(&q);
                }
                _ => {}
            }
            return;
        }
        if self.help {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll = self.help_scroll.saturating_add(1),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(5),
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(5),
                KeyCode::Home => self.help_scroll = 0,
                KeyCode::Char('p') => {
                    self.message = Some(match help::print_keys("NETWORK") {
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
        // While a background job runs, only the result matters; don't let a
        // stray key queue a conflicting operation or drop the pending result.
        if self.job.is_some() {
            self.message = Some((Tone::Warn, "An action is running; wait for it to finish.".into()));
            return;
        }
        match key.code {
            // `←→` / tab switch panes; they never leave the module.
            KeyCode::Left | KeyCode::BackTab => self.pane = self.pane.saturating_sub(1),
            KeyCode::Right | KeyCode::Tab => self.pane = (self.pane + 1).min(TABS.len() - 1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-3),
            KeyCode::PageDown => self.move_sel(3),
            KeyCode::Home => {
                if self.pane == 0 {
                    self.sel = 0;
                } else {
                    self.field_sel = 0;
                }
            }
            KeyCode::End => {
                if self.pane == 0 {
                    self.sel = self.ifaces.len().saturating_sub(1);
                } else {
                    self.field_sel = self.rows().len().saturating_sub(1);
                }
            }
            KeyCode::Enter => {
                if self.pane == 0 {
                    self.pane = 1;
                    self.field_sel = 0;
                } else {
                    self.activate_row();
                }
            }
            KeyCode::Char(' ') => self.toggle_row(),
            KeyCode::Char('m') => self.toggle_mode(),
            KeyCode::Char('f') => self.cycle_family(),
            KeyCode::Char(c @ '1'..='6') if self.choice.mode == Mode::Static => {
                let f = match c {
                    '1' => Field::V4Addr,
                    '2' => Field::V4Gw,
                    '3' => Field::V4Dns,
                    '4' => Field::V6Addr,
                    '5' => Field::V6Gw,
                    _ => Field::V6Dns,
                };
                let active = match f {
                    Field::V4Addr | Field::V4Gw | Field::V4Dns => self.choice.family.has_v4(),
                    _ => self.choice.family.has_v6(),
                };
                if active {
                    self.open_field(f);
                }
            }
            KeyCode::Char('r') => {
                self.refresh();
                self.message = Some((Tone::Ok, "Interfaces refreshed.".into()));
            }
            KeyCode::Char('u') => self.request_pending(Pending::Link(true)),
            KeyCode::Char('d') => self.request_pending(Pending::Link(false)),
            KeyCode::Char('h') => self.request_pending(Pending::Dhcp),
            KeyCode::Char('l') => self.request_pending(Pending::Also),
            KeyCode::Char('a') => self.request_apply(),
            KeyCode::Char('/') => self.open_jump(),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Esc => {
                if self.pane > 0 {
                    self.pane -= 1;
                } else {
                    self.exit = true;
                }
            }
            KeyCode::Char('q') | KeyCode::Char('b') => self.exit = true,
            _ => {}
        }
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        let Some((field, input)) = self.input.as_mut() else { return };
        let field = *field;
        match input.on_key(key) {
            Outcome::None => {}
            Outcome::Cancel => self.input = None,
            Outcome::Submit(value) => {
                let check = match field {
                    Field::V4Addr => net::valid_cidr(&value, false),
                    Field::V4Gw if value.is_empty() => Ok(()),
                    Field::V4Gw => net::valid_ip(&value, false),
                    Field::V4Dns => net::valid_dns(&value, false),
                    Field::V6Addr => net::valid_cidr(&value, true),
                    Field::V6Gw if value.is_empty() => Ok(()),
                    Field::V6Gw => net::valid_ip(&value, true),
                    Field::V6Dns => net::valid_dns(&value, true),
                };
                match check {
                    Err(e) => {
                        if let Some((_, i)) = self.input.as_mut() {
                            i.error = Some(e);
                        }
                    }
                    Ok(()) => {
                        match field {
                            Field::V4Addr => self.choice.ipv4_address = value,
                            Field::V4Gw => self.choice.ipv4_gateway = value,
                            Field::V4Dns => self.choice.ipv4_dns = value,
                            Field::V6Addr => self.choice.ipv6_address = value,
                            Field::V6Gw => self.choice.ipv6_gateway = value,
                            Field::V6Dns => self.choice.ipv6_dns = value,
                        }
                        self.input = None;
                    }
                }
            }
        }
    }

    fn selected(&self) -> Option<String> {
        self.ifaces.get(self.sel).map(|i| i.name.clone())
    }

    fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Run `f` on a worker thread; the result is picked up by [`Self::poll_job`].
    fn start_job(&mut self, label: &str, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        if self.busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.job = Some(rx);
        self.message = Some((Tone::Warn, format!("{label}…")));
    }

    /// Called once per UI tick: finish a background job when it reports back.
    pub fn poll_job(&mut self) {
        let Some(rx) = &self.job else { return };
        match rx.try_recv() {
            Ok(res) => {
                self.job = None;
                match res {
                    Ok(msg) => self.message = Some((Tone::Ok, msg)),
                    Err(e) => self.message = Some((Tone::Bad, e)),
                }
                self.refresh();
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.job = None,
        }
    }

    fn link(&mut self, up: bool) {
        let Some(name) = self.selected() else {
            self.message = Some((Tone::Bad, "No interface selected.".into()));
            return;
        };
        let state = if up { "up" } else { "down" };
        self.record(format!("Network: {name} link {state}"));
        self.start_job(&format!("{name}: link {state}"), move || net::set_link(&name, up));
    }

    /// Lease this interface without making it the primary uplink.
    fn dhcp(&mut self) {
        let Some(name) = self.selected() else {
            self.message = Some((Tone::Bad, "No interface selected.".into()));
            return;
        };
        let demo = self.demo;
        self.record(format!("Network: {name} dhcp lease"));
        self.start_job(&format!("{name}: dhcp"), move || net::dhcp_now(&name, demo));
    }

    /// Toggle "also lease at boot" for the selected interface (persisted).
    fn toggle_also(&mut self) {
        let Some(name) = self.selected() else {
            self.message = Some((Tone::Bad, "No interface selected.".into()));
            return;
        };
        let demo = self.demo;
        self.record(format!("Network: {name} also-lease"));
        self.start_job(&format!("{name}: also lease"), move || net::toggle_also(&name, demo));
    }

    fn apply(&mut self) {
        let Some(iface) = self.ifaces.get(self.sel) else {
            self.message = Some((Tone::Bad, "No interface to apply.".into()));
            return;
        };
        self.choice.iface = Some(iface.name.clone());
        if let Err(e) = self.choice.validate() {
            self.message = Some((Tone::Bad, e));
            return;
        }
        let name = iface.name.clone();
        let choice = self.choice.clone();
        let demo = self.demo;
        let label = choice.summary();
        self.record(format!("Network: applied {name} ({label})"));
        self.start_job(&label, move || choice.apply(demo));
    }
}

fn demo_ifaces() -> Vec<net::Iface> {
    vec![
        net::Iface {
            name: "eth0".into(),
            link: true,
            driver: "e1000e".into(),
            mac: "52:54:00:12:34:56".into(),
            wireless: false,
            primary: true,
            ipv4: vec!["192.168.1.42/24".into()],
        },
        net::Iface {
            name: "wlan0".into(),
            link: false,
            driver: "rtl8xxxu".into(),
            mac: "00:e0:4c:68:01:23".into(),
            wireless: true,
            primary: false,
            ipv4: Vec::new(),
        },
        net::Iface {
            name: "enp0s20u1".into(),
            link: false,
            driver: "r8152".into(),
            mac: "00:e0:4c:68:01:24".into(),
            wireless: false,
            primary: false,
            ipv4: Vec::new(),
        },
    ]
}

/// Demo app pre-set to the static form so snapshots exercise the fields.
pub fn demo_static() -> App {
    let mut app = App::new(true);
    app.choice = NetChoice {
        iface: Some("eth0".into()),
        mode: Mode::Static,
        family: Family::Both,
        ipv4_address: "192.168.1.50/24".into(),
        ipv4_gateway: "192.168.1.1".into(),
        ipv4_dns: "1.1.1.1 8.8.8.8".into(),
        ipv6_address: "2001:db8::50/64".into(),
        ipv6_gateway: "2001:db8::1".into(),
        ipv6_dns: "2001:4860:4860::8888".into(),
    };
    app.message = Some((Tone::Ok, "Ready to apply.".into()));
    app
}

// ---- drawing -----------------------------------------------------------

pub fn draw(f: &mut Frame, app: &App, tick: usize) {
    let t = &app.t;
    let area = f.area();
    f.render_widget(ratatui::widgets::Block::default().style(t.palette.base()), area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, rows[0], app);
    hud::tab_row(f, rows[1], &TABS, app.pane, t);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(rows[2]);
    draw_ifaces(f, cols[0], app);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(6)])
        .split(cols[1]);
    draw_mode(f, right[0], app);
    draw_fields(f, right[1], app);
    draw_result(f, rows[3], app, tick);

    // Context footer: at most six, most relevant first.
    let keys: Vec<(&str, &str)> = if app.pane == 0 {
        vec![("↑↓", "pick"), ("/", "find"), ("a", "apply"), ("u/d", "link"), ("?", "help"), ("q", "back")]
    } else {
        vec![("↑↓", "field"), ("/", "find"), ("enter", "edit"), ("space", "toggle"), ("a", "apply"), ("q", "back")]
    };
    hud::footer(f, rows[4], &keys, t);

    if let Some((_, input)) = &app.input {
        input::draw(f, area, t, input, tick);
    }
    if app.help {
        help::draw(f, area, t, "NETWORK", app.help_scroll);
    }
    if let Some((_, rev)) = &app.review {
        review::draw(f, area, t, rev);
    }
    if let Some(jump) = &app.jump {
        input::draw(f, area, t, jump, tick);
        let targets = app.jump_targets();
        let hits: Vec<String> = input::fuzzy_matches(&jump.buf, &targets)
            .into_iter()
            .map(|i| targets[i].clone())
            .collect();
        input::draw_hits(f, area, t, "JUMP TO", &hits);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let item = app.selected().unwrap_or_else(|| app.choice.summary());
    let crumb = t.breadcrumb(&["NETWORK", TABS[app.pane.min(1)], &item]);
    let right = vec![Span::styled("persists /data/etc/network  ", t.palette.fg(t.palette.dim))];
    hud::header_bar(f, area, &crumb, "", right, t);
}

fn iface_line(iface: &net::Iface, selected: bool, also: bool, t: &Theme) -> Line<'static> {
    let link = if iface.link { "up" } else { "down" };
    let ip = iface.ipv4.first().map(String::as_str).unwrap_or("-");
    let tag = if iface.wireless { "~" } else { " " };
    let primary = if iface.primary { "*" } else { " " };
    let boot = if also { "+" } else { " " };
    if selected {
        let text = format!(
            "{} {:<2}{:<10} {:<4} {:<8} {:<17} {:<15} {} {}",
            t.selection_mark().trim_end(),
            tag,
            iface.name,
            link,
            iface.driver,
            iface.mac,
            ip,
            primary,
            boot
        );
        Line::from(Span::styled(text, t.palette.highlight()))
    } else {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{tag} "), t.palette.fg(t.palette.accent2)),
            Span::styled(format!("{:<10}", iface.name), t.palette.bold(t.palette.fg)),
            Span::styled(format!("{:<4} ", link), t.palette.fg(if iface.link { t.palette.ok } else { t.palette.warn })),
            Span::styled(format!("{:<8} ", iface.driver), t.palette.fg(t.palette.accent)),
            Span::styled(format!("{:<17} ", iface.mac), t.palette.fg(t.palette.dim)),
            Span::styled(format!("{ip:<15} "), t.palette.fg(t.palette.fg)),
            Span::styled(primary.to_string(), t.palette.bold(t.palette.accent2)),
            Span::styled(format!(" {boot}"), t.palette.bold(t.palette.accent)),
        ])
    }
}

fn draw_ifaces(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    // The list owns the keyboard while the INTERFACES tab is active.
    let inner = hud::panel(f, area, "INTERFACES", None, app.pane == 0, t);
    let mut lines = vec![Line::from(Span::styled(
        format!("  W {:<10} {:<4} {:<8} {:<17} {:<15} P A", "IFACE", "LINK", "DRIVER", "MAC", "IPV4"),
        t.palette.fg(t.palette.dim),
    ))];
    if app.ifaces.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("{} no interface found in /sys/class/net", t.ui.sym(2)),
            t.palette.fg(t.palette.warn),
        )));
    }
    for (i, iface) in app.ifaces.iter().enumerate() {
        lines.push(iface_line(iface, i == app.sel, net::is_also(&app.also, &iface.name), t));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{} wireless   * current primary   + also lease at boot", t.ui.sym(2)),
        t.palette.fg(t.palette.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn net_row_focus(app: &App, row: Row) -> bool {
    app.pane == 1 && app.rows().get(app.field_sel).copied() == Some(row)
}

fn draw_mode(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    // On the CONFIG tab the MODE panel is focused while a mode/family row is
    // selected; a field row focuses the ADDRESS panel instead.
    let focused = matches!(app.rows().get(app.field_sel), Some(Row::Mode | Row::Family));
    let inner = hud::panel(f, area, "MODE — EDIT: form", None, app.pane == 1 && focused, t);
    let mut lines = Vec::new();
    let row_w = inner.width.saturating_sub(1) as usize;
    for m in [Mode::Dhcp, Mode::Static] {
        let sel = app.choice.mode == m;
        let note = match m {
            Mode::Dhcp => "automatic lease (udhcpc)",
            Mode::Static => "fixed address, gateway and DNS",
        };
        let focused = sel && net_row_focus(app, Row::Mode);
        let cur = if focused { t.selection_mark() } else { "  " };
        let hint = if focused { "space toggle" } else { "" };
        lines.push(hud::line_with_hint(
            vec![
                Span::styled(cur.to_string(), t.palette.bold(t.palette.accent2)),
                Span::styled(format!("{:<8}", m.label()), if sel { t.palette.bold(t.palette.ok) } else { t.palette.fg(t.palette.fg) }),
                Span::styled(note, t.palette.fg(t.palette.dim)),
            ],
            hint,
            row_w,
            t,
        ));
    }
    if app.choice.mode == Mode::Static {
        lines.push(Line::raw(""));
        let focused = net_row_focus(app, Row::Family);
        let cur = if focused { t.selection_mark() } else { "  " };
        let mut fam = vec![
            Span::styled(cur.to_string(), t.palette.bold(t.palette.accent2)),
            Span::styled("FAMILY  ", t.palette.fg(t.palette.dim)),
        ];
        for famly in [Family::Ipv4, Family::Ipv6, Family::Both] {
            let sel = app.choice.family == famly;
            fam.push(Span::styled(
                format!("{} ", famly.label()),
                if sel { t.palette.bold(t.palette.accent2) } else { t.palette.fg(t.palette.dim) },
            ));
        }
        lines.push(hud::line_with_hint(fam, if focused { "space cycle" } else { "" }, row_w, t));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("↑↓ field · space toggle · a apply", t.palette.fg(t.palette.dim))));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_fields(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let focused = matches!(app.rows().get(app.field_sel), Some(Row::Field(_)));
    let inner = hud::panel(f, area, "ADDRESS — EDIT: form", None, app.pane == 1 && focused, t);
    let mut lines = Vec::new();
    let row_w = inner.width.saturating_sub(1) as usize;
    if app.choice.mode == Mode::Dhcp {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(format!("{} DHCP needs no fields", t.ui.sym(0)), t.palette.bold(t.palette.ok))));
        lines.push(Line::from(Span::styled("the interface gets address, route and DNS", t.palette.fg(t.palette.dim))));
        lines.push(Line::from(Span::styled("from the network automatically.", t.palette.fg(t.palette.dim))));
    } else {
        if app.choice.family.has_v4() {
            lines.push(net_field(t, "1", "ADDRESS", &app.choice.ipv4_address, "192.168.1.50/24", net_row_focus(app, Row::Field(Field::V4Addr)), row_w));
            lines.push(net_field(t, "2", "GATEWAY", &app.choice.ipv4_gateway, "192.168.1.1", net_row_focus(app, Row::Field(Field::V4Gw)), row_w));
            lines.push(net_field(t, "3", "DNS", &app.choice.ipv4_dns, "1.1.1.1 8.8.8.8", net_row_focus(app, Row::Field(Field::V4Dns)), row_w));
        }
        if app.choice.family.has_v6() {
            if app.choice.family.has_v4() {
                lines.push(Line::raw(""));
            }
            lines.push(net_field(t, "4", "ADDRESS", &app.choice.ipv6_address, "2001:db8::50/64", net_row_focus(app, Row::Field(Field::V6Addr)), row_w));
            lines.push(net_field(t, "5", "GATEWAY", &app.choice.ipv6_gateway, "2001:db8::1", net_row_focus(app, Row::Field(Field::V6Gw)), row_w));
            lines.push(net_field(t, "6", "DNS", &app.choice.ipv6_dns, "2001:4860:4860::8888", net_row_focus(app, Row::Field(Field::V6Dns)), row_w));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("gateway and DNS are optional", t.palette.fg(t.palette.dim))));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn net_field(t: &Theme, n: &str, label: &str, value: &str, hint: &str, focused: bool, row_w: usize) -> Line<'static> {
    let shown = if value.is_empty() {
        Span::styled(format!("{hint}  (press {n})"), t.palette.fg(t.palette.dim))
    } else {
        Span::styled(value.to_string(), t.palette.bold(t.palette.fg))
    };
    let cur = if focused { t.selection_mark() } else { "  " };
    let left = vec![
        Span::styled(cur.to_string(), t.palette.bold(t.palette.accent2)),
        Span::styled(format!("{n} {label:<8}"), if focused { t.palette.bold(t.palette.accent) } else { t.palette.fg(t.palette.dim) }),
        shown,
    ];
    hud::line_with_hint(left, if focused { "enter edit" } else { "" }, row_w, t)
}

fn draw_result(f: &mut Frame, area: Rect, app: &App, tick: usize) {
    let msg = app.message.as_ref().map(|(tone, m)| (*tone, m.as_str()));
    hud::status_row(f, area, msg, app.busy().then(|| hud::spinner(tick)), "↑↓ pick, enter config, a apply.", &app.t);
}

/// Run the screen on the real terminal.
pub fn run() -> std::io::Result<()> {
    let app = App::new(false);
    let theme = app.t.clone();
    crate::screen::run(
        app,
        &theme,
        draw,
        |app| app.poll_job(),
        |app, key| app.on_key(key),
        |app| app.exit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_renders_interfaces_and_fields() {
        let app = demo_static();
        let text = crate::screen::render_text(&app, 100, 32, draw).unwrap();
        assert!(text.contains("NETWORK"));
        assert!(text.contains("INTERFACES"));
        assert!(text.contains("eth0"));
        assert!(text.contains("wlan0"));
        assert!(text.contains("192.168.1.50/24"));
        assert!(text.contains("BOTH"));
    }

    #[test]
    fn dhcp_here_is_demo_safe_and_reviewed() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('h')));
        assert!(app.review.is_some(), "dhcp opens REVIEW first");
        assert!(!app.busy(), "nothing runs before the confirm");
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.busy(), "confirm starts a background job");
        for _ in 0..200 {
            app.poll_job();
            if !app.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!app.busy());
        assert!(matches!(app.message, Some((Tone::Ok, _))));
    }

    #[test]
    fn also_toggle_is_demo_safe_and_reviewed() {
        let mut app = App::new(true);
        assert!(net::is_also(&app.also, "enp0s20u1"), "demo shows an also entry");
        app.on_key(KeyEvent::from(KeyCode::Char('l')));
        assert!(app.review.is_some(), "also toggle opens REVIEW");
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.busy(), "also toggle starts a background job");
        for _ in 0..200 {
            app.poll_job();
            if !app.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!app.busy());
        assert!(matches!(app.message, Some((Tone::Ok, _))));
    }

    #[test]
    fn apply_review_names_the_primary_promotion_and_can_be_cancelled() {
        let mut app = App::new(true);
        // pick the non-primary interface, apply: the review must say it is promoted
        app.sel = app.ifaces.iter().position(|i| !i.primary).unwrap();
        app.on_key(KeyEvent::from(KeyCode::Char('a')));
        let (_, rev) = app.review.as_ref().expect("a opens REVIEW");
        assert!(rev.note.as_ref().unwrap().contains("primary uplink"), "{:?}", rev.note);
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.review.is_none() && !app.busy(), "esc cancels; nothing runs");
    }

    #[test]
    fn mutations_are_recorded_for_the_deck() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('h'))); // dhcp REVIEW
        assert!(app.take_recent().is_empty(), "nothing recorded before the confirm");
        app.on_key(KeyEvent::from(KeyCode::Enter));
        let r = app.take_recent();
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].starts_with("Network: eth0 dhcp"), "{r:?}");
        assert!(app.take_recent().is_empty(), "drained once");
    }

    #[test]
    fn slash_jump_selects_an_interface() {
        let mut app = App::new(true);
        app.sel = 0;
        app.on_key(KeyEvent::from(KeyCode::Char('/')));
        assert!(app.jump.is_some());
        for c in "enp0s20u1".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.jump.is_none());
        assert_eq!(app.ifaces[app.sel].name, "enp0s20u1", "jump selects the match");
        assert_eq!(app.pane, 0, "jump is read-only and lands on the list");
    }

    #[test]
    fn mode_and_family_toggle() {
        let mut app = App::new(true);
        assert_eq!(app.choice.mode, Mode::Dhcp);
        app.on_key(KeyEvent::from(KeyCode::Char('m')));
        assert_eq!(app.choice.mode, Mode::Static);
        assert_eq!(app.choice.family, Family::Ipv4);
        app.on_key(KeyEvent::from(KeyCode::Char('f')));
        assert_eq!(app.choice.family, Family::Ipv6);
    }

    #[test]
    fn editing_a_field_validates() {
        let mut app = App::new(true);
        app.choice.mode = Mode::Static;
        app.choice.family = Family::Ipv4;
        app.on_key(KeyEvent::from(KeyCode::Char('1')));
        assert!(app.input.is_some());
        // invalid: no prefix
        for c in "192.168.1.9".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.input.is_some(), "invalid value keeps the modal open");
        assert!(app.input.as_ref().unwrap().1.error.is_some());
        // fix by clearing and entering a valid CIDR
        for _ in 0..12 {
            app.on_key(KeyEvent::from(KeyCode::Backspace));
        }
        for c in "10.0.0.2/24".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.input.is_none());
        assert_eq!(app.choice.ipv4_address, "10.0.0.2/24");
    }

    #[test]
    fn apply_in_demo_does_not_touch_system() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('a')));
        assert!(app.review.is_some());
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.busy());
        for _ in 0..200 {
            app.poll_job();
            if !app.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(app.message, Some((Tone::Ok, _))));
    }

    #[test]
    fn arrows_switch_panes_and_esc_returns_to_the_list() {
        let mut app = App::new(true);
        assert_eq!(app.pane, 0);
        app.on_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.pane, 1);
        app.on_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(app.pane, 0);
        app.on_key(KeyEvent::from(KeyCode::Right));
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.pane, 0, "esc steps back to the list, not out of the module");
        assert!(!app.exit);
        app.on_key(KeyEvent::from(KeyCode::Char('b')));
        assert!(app.exit, "b returns to the deck");
    }
}
