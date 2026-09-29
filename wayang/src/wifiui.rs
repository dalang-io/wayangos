//! `wayang wifi` HUD screen: list wireless interfaces, scan with `iw`, pick an
//! SSID, enter the passphrase (and optional country) and Connect. Persists
//! `/data/etc/wpa_supplicant.conf` and pins the interface as primary.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::hud::{self, Theme};
use crate::input::{self, Input, Outcome, Pick, Picker};
use crate::sys;
use crate::tui::Tone;
use crate::wifi;
use crate::{help, review};

/// Visible tab row; `←→`/`tab` moves between the panes.
const TABS: [&str; 3] = ["ACCESS POINTS", "WIRELESS IFACE", "DETAILS"];

/// A mutation awaiting REVIEW.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    /// Connect to `ssid` on `iface` with `psk`.
    Connect { iface: String, ssid: String, psk: String },
}

pub struct App {
    pub t: Theme,
    pub demo: bool,
    pub ifaces: Vec<wifi::WifiIface>,
    /// Association state parallel to `ifaces` (refreshed periodically).
    pub links: Vec<wifi::LinkStatus>,
    pub iface_sel: usize,
    pub bss: Vec<wifi::Bss>,
    pub bss_sel: usize,
    pub country: String,
    /// Active tab/pane: 0 = ACCESS POINTS, 1 = IFACE, 2 = DETAILS.
    pub pane: usize,
    input: Option<Input>,
    picker: Option<Picker>,
    /// `/` quick-jump query over APs and interfaces, if open.
    jump: Option<Input>,
    /// Pending REVIEW before a connect runs.
    review: Option<(Pending, review::Review)>,
    /// Actions run here since the deck last drained them (RECENT strip).
    recent: Vec<String>,
    pub help: bool,
    pub help_scroll: usize,
    pub message: Option<(Tone, String)>,
    tick: usize,
    pub exit: bool,
}

impl App {
    pub fn new(demo: bool) -> App {
        let ifaces = if demo { demo_ifaces() } else { wifi::ifaces() };
        let mut app = App {
            t: Theme::detect(),
            demo,
            ifaces,
            links: Vec::new(),
            iface_sel: 0,
            bss: Vec::new(),
            bss_sel: 0,
            country: String::new(),
            pane: 0,
            input: None,
            picker: None,
            jump: None,
            review: None,
            recent: Vec::new(),
            help: false,
            help_scroll: 0,
            message: None,
            tick: 0,
            exit: false,
        };
        app.refresh_links();
        if demo {
            app.bss = wifi::parse_scan(DEMO_SCAN);
            app.message = Some((Tone::Ok, "Scan: 3 networks.".into()));
        } else if app.ifaces.is_empty() {
            app.message = Some((
                Tone::Bad,
                "No wireless interface. See docs/NETWORK.md wifi prerequisites.".into(),
            ));
        } else if !sys::which("iw") {
            app.message = Some((Tone::Warn, "`iw` not installed; scanning unavailable.".into()));
        } else {
            app.message = Some((Tone::Ok, "Press s to scan.".into()));
        }
        app
    }

    fn selected_iface(&self) -> Option<&wifi::WifiIface> {
        self.ifaces.get(self.iface_sel)
    }

    fn refresh_links(&mut self) {
        if self.demo {
            self.links = self.ifaces.iter().map(|_| wifi::LinkStatus::default()).collect();
            return;
        }
        self.links = self.ifaces.iter().map(|i| wifi::link_status(&i.name)).collect();
    }

    /// Called once per UI tick: refresh association state periodically (the
    /// link comes up a moment after `connect` returns).
    pub fn poll_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 10 == 0 {
            self.refresh_links();
        }
    }

    fn selected_link(&self) -> wifi::LinkStatus {
        self.links.get(self.iface_sel).cloned().unwrap_or_default()
    }

    fn scan(&mut self) {
        let Some(iface) = self.selected_iface() else {
            self.message = Some((Tone::Bad, "No wireless interface to scan.".into()));
            return;
        };
        let name = iface.name.clone();
        match wifi::scan(&name) {
            Ok(list) => {
                let n = list.len();
                self.bss = list;
                self.bss_sel = 0;
                self.message = Some((Tone::Ok, format!("Scan on {name}: {n} network(s).")));
            }
            Err(e) => {
                self.bss.clear();
                self.message = Some((Tone::Bad, e));
            }
        }
    }

    fn open_passphrase(&mut self) {
        let Some(bss) = self.bss.get(self.bss_sel) else {
            self.message = Some((Tone::Bad, "Scan and pick a network first.".into()));
            return;
        };
        self.input = Some(
            Input::new(
                format!("PASSPHRASE — {}", bss.ssid),
                "WPA-PSK passphrase:",
                "8-63 characters (or 64 hex); hidden as you type",
            )
            .masked(),
        );
    }

    fn open_country(&mut self) {
        let mut items = vec!["(not set)".to_string()];
        items.extend(wifi::REGIONS.iter().map(|(code, name)| format!("{code}  {name}")));
        self.picker = Some(Picker::new(
            "COUNTRY",
            "Regulatory domain for the wireless radio:",
            items,
        ));
        if let Some(i) = wifi::REGIONS.iter().position(|(c, _)| *c == self.country) {
            if let Some(p) = self.picker.as_mut() {
                p.sel = i + 1;
            }
        }
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        let outcome = match self.picker.as_mut() {
            Some(p) => p.on_key(key),
            None => return,
        };
        match outcome {
            Pick::None => {}
            Pick::Cancel => self.picker = None,
            Pick::Choose(i) => {
                self.country = if i == 0 { String::new() } else { wifi::REGIONS[i - 1].0.to_string() };
                self.picker = None;
                self.message = Some((
                    Tone::Ok,
                    if self.country.is_empty() {
                        "Country: not set.".into()
                    } else {
                        format!("Country: {}.", self.country)
                    },
                ));
            }
        }
    }

    fn request_connect(&mut self, psk: &str) {
        let (Some(iface), Some(bss)) = (self.selected_iface(), self.bss.get(self.bss_sel)) else {
            self.message = Some((Tone::Bad, "Scan and pick a network first.".into()));
            return;
        };
        let iface = iface.name.clone();
        let ssid = bss.ssid.clone();
        let effect = vec![
            format!("write /data/etc/wpa_supplicant.conf for \"{ssid}\""),
            format!("bring {iface} up and run wpa_supplicant"),
            format!("run udhcpc on {iface}"),
            format!("pin {iface} as the primary uplink (dhcp)"),
        ];
        let mut r = review::Review::new(format!("CONNECT to \"{ssid}\" on {iface}"), effect);
        if !self.country.is_empty() {
            r = r.note(format!("Regulatory domain is set to {}.", self.country));
        }
        self.review = Some((Pending::Connect { iface, ssid, psk: psk.to_string() }, r));
    }

    /// Record a mutating action so the deck's RECENT strip shows it.
    fn record(&mut self, what: impl Into<String>) {
        self.recent.push(what.into());
    }

    /// Drain the actions recorded since the last call (oldest first).
    pub fn take_recent(&mut self) -> Vec<String> {
        std::mem::take(&mut self.recent)
    }

    fn connect_now(&mut self, iface: &str, ssid: &str, psk: &str) {
        self.record(format!("Wifi: connected \"{ssid}\" on {iface}"));
        let country = if self.country.is_empty() { None } else { Some(self.country.as_str()) };
        match wifi::connect(iface, ssid, psk, country, self.demo) {
            Ok(msg) => {
                self.message = Some((Tone::Ok, msg));
                self.refresh_links();
            }
            Err(e) => self.message = Some((Tone::Bad, e)),
        }
    }

    /// `/` quick-jump candidates: access points first (pane 0), then the
    /// wireless interfaces (pane 1). Read-only: selecting never connects.
    fn jump_targets(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .bss
            .iter()
            .map(|b| {
                let sig = b.signal.map(|s| format!("{s}dBm")).unwrap_or_else(|| "--".into());
                format!("{} {} {}", b.ssid, b.security, sig)
            })
            .collect();
        v.extend(self.ifaces.iter().map(|i| format!("{} {}", i.name, i.driver)));
        v
    }

    fn open_jump(&mut self) {
        self.jump = Some(Input::new(
            "QUICK JUMP",
            "Type an access point or interface (fuzzy):",
            "e.g. CoffeeShop, wlan0",
        ));
    }

    fn submit_jump(&mut self, q: &str) {
        let targets = self.jump_targets();
        let bss_n = self.bss.len();
        match input::fuzzy_matches(q, &targets).first().copied() {
            Some(i) if i < bss_n => {
                self.bss_sel = i;
                self.pane = 0;
                let ssid = self.bss[i].ssid.clone();
                self.message = Some((Tone::Ok, format!("Jumped to \"{ssid}\".")));
            }
            Some(i) => {
                self.iface_sel = i - bss_n;
                self.pane = 1;
                let name = self.ifaces[i - bss_n].name.clone();
                self.message = Some((Tone::Ok, format!("Jumped to {name}.")));
            }
            None if !q.trim().is_empty() => {
                self.message = Some((Tone::Warn, format!("No access point or interface matches '{q}'.")));
            }
            None => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.review.is_some() {
            let decision = self.review.as_mut().map(|(_, r)| r.on_key(key));
            match decision {
                Some(review::Decision::Confirm) => {
                    if let Some((Pending::Connect { iface, ssid, psk }, _)) = self.review.take() {
                        self.connect_now(&iface, &ssid, &psk);
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
        if self.picker.is_some() {
            self.on_picker_key(key);
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
                    self.message = Some(match help::print_keys("WIFI") {
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
        match key.code {
            // `←→` / tab switch panes; they never leave the module.
            KeyCode::Left | KeyCode::BackTab => self.pane = self.pane.saturating_sub(1),
            KeyCode::Right | KeyCode::Tab => self.pane = (self.pane + 1).min(TABS.len() - 1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Home => match self.pane {
                0 => self.bss_sel = 0,
                1 => self.iface_sel = 0,
                _ => {}
            },
            KeyCode::End => match self.pane {
                0 => self.bss_sel = self.bss.len().saturating_sub(1),
                1 => self.iface_sel = self.ifaces.len().saturating_sub(1),
                _ => {}
            },
            // enter activates the active pane: connect, pick iface, or country.
            KeyCode::Enter => match self.pane {
                0 => self.open_passphrase(),
                1 => self.cycle_iface(),
                _ => self.open_country(),
            },
            KeyCode::Char('i') => self.cycle_iface(),
            KeyCode::Char('s') | KeyCode::Char('r') => self.scan(),
            KeyCode::Char('c') => self.open_country(),
            KeyCode::Char('w') => self.open_passphrase(),
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

    fn move_sel(&mut self, delta: isize) {
        match self.pane {
            0 => {
                let n = self.bss.len();
                if n > 0 {
                    self.bss_sel = ((self.bss_sel as isize + delta).rem_euclid(n as isize)) as usize;
                }
            }
            1 => {
                let n = self.ifaces.len();
                if n > 0 {
                    self.iface_sel = ((self.iface_sel as isize + delta).rem_euclid(n as isize)) as usize;
                }
            }
            _ => {}
        }
    }

    fn cycle_iface(&mut self) {
        if self.ifaces.is_empty() {
            return;
        }
        self.iface_sel = (self.iface_sel + 1) % self.ifaces.len();
        self.bss.clear();
        self.bss_sel = 0;
        self.message = Some((Tone::Ok, format!("Interface: {}", self.ifaces[self.iface_sel].name)));
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        let Some(input) = self.input.as_mut() else { return };
        match input.on_key(key) {
            Outcome::None => {}
            Outcome::Cancel => self.input = None,
            Outcome::Submit(value) => match wifi::valid_credentials("placeholder", &value) {
                Err(e) => {
                    if let Some(i) = self.input.as_mut() {
                        i.error = Some(e);
                    }
                }
                Ok(()) => {
                    self.input = None;
                    self.request_connect(&value);
                }
            },
        }
    }
}

const DEMO_SCAN: &str = "\
BSS 00:11:22:33:44:55(on wlan0)
\tsignal: -45.00 dBm
\tcapability: ESS Privacy (0x0411)
\tSSID: CoffeeShop
\tRSN:\t* Version: 1

BSS 66:77:88:99:aa:bb(on wlan0)
\tsignal: -72.30 dBm
\tcapability: ESS (0x0401)
\tSSID: OpenCafe

BSS aa:bb:cc:dd:ee:ff(on wlan0)
\tsignal: -60.10 dBm
\tcapability: ESS Privacy (0x0411)
\tSSID: OldRouter
\tWPA:\t* Version: 1
";

fn demo_ifaces() -> Vec<wifi::WifiIface> {
    vec![wifi::WifiIface {
        name: "wlan0".into(),
        driver: "rtl8xxxu".into(),
        mac: "00:e0:4c:68:01:23".into(),
    }]
}

/// Demo app with the country picker open, for snapshots.
pub fn demo_country() -> App {
    let mut app = App::new(true);
    app.open_country();
    app
}

// ---- drawing -----------------------------------------------------------

pub fn draw(f: &mut Frame, app: &App, tick: usize) {
    let t = &app.t;
    let area = f.area();
    f.render_widget(ratatui::widgets::Block::default().style(t.base()), area);

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
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(rows[2]);
    draw_bss(f, cols[0], app);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(6)])
        .split(cols[1]);
    draw_ifaces(f, right[0], app);
    draw_details(f, right[1], app);
    draw_result(f, rows[3], app);

    let keys: Vec<(&str, &str)> = match app.pane {
        0 => vec![("↑↓", "pick"), ("/", "find"), ("enter", "connect"), ("s", "scan"), ("?", "help"), ("b", "back")],
        1 => vec![("↑↓", "pick"), ("/", "find"), ("enter", "switch"), ("s", "scan"), ("?", "help"), ("b", "back")],
        _ => vec![("c", "country"), ("/", "find"), ("enter", "connect"), ("s", "scan"), ("?", "help"), ("b", "back")],
    };
    let mut kline = hud::keycaps(&keys, t);
    kline.spans.insert(0, Span::raw(" "));
    f.render_widget(Paragraph::new(kline), rows[4]);

    if let Some(input) = &app.input {
        input::draw(f, area, t, input, tick);
    }
    if let Some(picker) = &app.picker {
        input::draw_picker(f, area, t, picker);
    }
    if app.help {
        help::draw(f, area, t, "WIFI", app.help_scroll);
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
    let st = app.selected_link();
    let name = app.selected_iface().map(|i| i.name.clone()).unwrap_or_default();
    let (color, text) = if st.connected {
        (t.ok, st.summary())
    } else if !st.ssid.is_empty() {
        (t.warn, st.summary())
    } else {
        (t.dim, st.summary())
    };
    let right = vec![
        Span::styled(format!("{name} "), t.bold(t.accent)),
        Span::styled(format!("{text}  "), t.fg(color)),
    ];
    let crumb = t.breadcrumb(&["WIFI", TABS[app.pane.min(2)], &name]);
    hud::header_bar(f, area, &crumb, "", right, t);
}

fn draw_bss(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "ACCESS POINTS", t);
    let row_w = inner.width.saturating_sub(1) as usize;
    let mut lines = vec![Line::from(Span::styled(
        format!("  {:<22} {:>6}  {}", "SSID", "SIGNAL", "SECURITY"),
        t.fg(t.dim),
    ))];
    if app.bss.is_empty() {
        lines.push(Line::raw(""));
        let hint = if app.ifaces.is_empty() {
            "no wireless interface found in /sys/class/net"
        } else if !sys::which("iw") && !app.demo {
            "`iw` is not installed (see docs/NETWORK.md)"
        } else {
            "no scan yet: press s to scan for networks"
        };
        lines.push(Line::from(Span::styled(
            format!("{} {hint}", t.g.warn),
            t.fg(t.warn),
        )));
    }
    for (i, b) in app.bss.iter().enumerate() {
        let sig = b.signal.map(|s| format!("{s}")).unwrap_or_else(|| "-".into());
        let ssid = if b.ssid.is_empty() { "<hidden>" } else { b.ssid.as_str() };
        let ssid = t.clip(ssid, 22);
        if i == app.bss_sel {
            let text = format!(
                "{} {:<22} {:>6}  {}",
                t.cursor().trim_end(),
                ssid,
                sig,
                b.security
            );
            lines.push(hud::line_with_hint(
                vec![Span::styled(text, t.highlight())],
                "enter connect",
                row_w,
                t,
            ));
        } else {
            let color = if b.security == "OPEN" { t.warn } else { t.ok };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(format!("{ssid:<22} "), t.bold(t.fg)),
                Span::styled(format!("{sig:>6}  "), t.fg(t.accent)),
                Span::styled(b.security.clone(), t.fg(color)),
            ]));
        }
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_ifaces(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "WIRELESS IFACE", t);
    let mut lines = Vec::new();
    if app.ifaces.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("{} none found", t.g.warn),
            t.fg(t.warn),
        )));
    }
    for (i, iface) in app.ifaces.iter().enumerate() {
        let sel = i == app.iface_sel;
        let st = app.links.get(i).cloned().unwrap_or_default();
        let status = if st.connected {
            let ssid = if st.ssid.is_empty() { "<hidden>" } else { st.ssid.as_str() };
            format!("connected: {ssid}")
        } else if !st.ssid.is_empty() {
            format!("saved: {}", st.ssid)
        } else {
            String::new()
        };
        let line = format!("{:<8} {:<9} {:<17} {}", iface.name, iface.driver, iface.mac, status);
        if sel {
            lines.push(hud::line_with_hint(
                vec![
                    Span::styled(t.cursor(), t.bold(t.accent2)),
                    Span::styled(line, t.highlight()),
                ],
                "enter switch",
                inner.width.saturating_sub(1) as usize,
                t,
            ));
        } else {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line, t.fg(t.fg)),
            ]));
        }
    }
    lines.push(Line::from(Span::styled(
        "i cycles interface (clears scan)",
        t.fg(t.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_details(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "DETAILS — EDIT: form", t);
    let mut lines = Vec::new();
    match app.bss.get(app.bss_sel) {
        Some(b) => {
            lines.push(hud::field("SSID", 10, vec![Span::styled(b.ssid.clone(), t.bold(t.fg))], t));
            lines.push(hud::field("BSSID", 10, vec![Span::styled(b.bssid.clone(), t.fg(t.dim))], t));
            lines.push(hud::field(
                "SIGNAL",
                10,
                vec![Span::styled(
                    b.signal.map(|s| format!("{s} dBm")).unwrap_or_else(|| "?".into()),
                    t.fg(t.accent),
                )],
                t,
            ));
            lines.push(hud::field("SECURITY", 10, vec![Span::styled(b.security.clone(), t.fg(t.ok))], t));
        }
        None => lines.push(Line::from(Span::styled("no network selected", t.fg(t.dim)))),
    }
    lines.push(Line::raw(""));
    lines.push(hud::field(
        "COUNTRY",
        10,
        vec![Span::styled(
            if app.country.is_empty() { "-".into() } else { app.country.clone() },
            t.fg(t.fg),
        )],
        t,
    ));
    lines.push(Line::from(Span::styled(
        "c picks country, enter enters the passphrase",
        t.fg(t.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_result(f: &mut Frame, area: Rect, app: &App) {
    let msg = app.message.as_ref().map(|(tone, m)| (*tone, m.as_str()));
    hud::status_row(f, area, msg, None, "Press s to scan.", &app.t);
}

/// Run the screen on the real terminal.
pub fn run() -> std::io::Result<()> {
    crate::screen::run(
        App::new(false),
        draw,
        |app| app.poll_tick(),
        |app, key| app.on_key(key),
        |app| app.exit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_renders_networks() {
        let app = App::new(true);
        let text = crate::screen::render_text(&app, 100, 32, draw).unwrap();
        assert!(text.contains("WIFI"));
        assert!(text.contains("CoffeeShop"));
        assert!(text.contains("OpenCafe"));
        assert!(text.contains("WPA2"));
        assert!(text.contains("wlan0"));
    }

    #[test]
    fn connect_requires_a_passphrase() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.input.is_some(), "passphrase modal opens");
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.input.as_ref().unwrap().error.is_some());
        assert!(app.input.is_some());
    }

    #[test]
    fn connecting_in_demo_succeeds_after_review() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        for c in "hunter2hunter2".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.input.is_none());
        assert!(app.review.is_some(), "connect opens REVIEW");
        assert!(app.picker.is_none());
        // cancel first: nothing connects
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.review.is_none());
        // now do it again and confirm
        app.on_key(KeyEvent::from(KeyCode::Enter));
        for c in "hunter2hunter2".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.message, Some((Tone::Ok, _))), "{:?}", app.message);
    }

    #[test]
    fn connect_is_recorded_for_the_deck() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        for c in "hunter2hunter2".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter)); // REVIEW
        assert!(app.take_recent().is_empty(), "nothing recorded before the confirm");
        app.on_key(KeyEvent::from(KeyCode::Enter)); // confirm
        let r = app.take_recent();
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].starts_with("Wifi: connected"), "{r:?}");
    }

    #[test]
    fn slash_jump_selects_an_ap_or_interface() {
        let mut app = App::new(true);
        // AP: jump to the strongest/second entry without connecting
        app.on_key(KeyEvent::from(KeyCode::Char('/')));
        assert!(app.jump.is_some());
        for c in "OpenCafe".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.jump.is_none());
        assert_eq!(app.bss[app.bss_sel].ssid, "OpenCafe", "jump selects the AP");
        assert_eq!(app.pane, 0);
        assert!(app.review.is_none(), "jump never connects");
        // interface: lands on the WIRELESS IFACE pane
        app.on_key(KeyEvent::from(KeyCode::Char('/')));
        for c in "wlan0".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.pane, 1);
        assert_eq!(app.ifaces[app.iface_sel].name, "wlan0");
    }

    #[test]
    fn arrows_switch_panes_and_esc_steps_back() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.pane, 1);
        app.on_key(KeyEvent::from(KeyCode::Right));
        assert_eq!(app.pane, 2);
        app.on_key(KeyEvent::from(KeyCode::Left));
        assert_eq!(app.pane, 1);
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.pane, 0);
        assert!(!app.exit);
        app.on_key(KeyEvent::from(KeyCode::Char('b')));
        assert!(app.exit);
    }

    #[test]
    fn country_picker_selects_and_clears() {
        let mut app = App::new(true);
        app.country = "GB".into();
        app.on_key(KeyEvent::from(KeyCode::Char('c')));
        let p = app.picker.as_ref().unwrap();
        let gb = wifi::REGIONS.iter().position(|(c, _)| *c == "GB").unwrap() + 1;
        assert_eq!(p.sel, gb, "the current country is preselected");
        app.on_key(KeyEvent::from(KeyCode::Home));
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.picker.is_none());
        assert!(app.country.is_empty(), "the first entry clears the country");
    }

    #[test]
    fn country_picker_renders_and_cancels() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('c')));
        let text = crate::screen::render_text(&app, 60, 20, draw).unwrap();
        assert!(text.contains("COUNTRY"), "{text}");
        assert!(text.contains("Andorra"), "{text}");
        app.on_key(KeyEvent::from(KeyCode::Esc));
        assert!(app.picker.is_none());
    }

    #[test]
    fn country_picker_windows_to_the_selection() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('c')));
        if let Some(p) = app.picker.as_mut() {
            p.sel = p.items.len() - 1;
        }
        let text = crate::screen::render_text(&app, 60, 20, draw).unwrap();
        assert!(text.contains("Zimbabwe"), "the list scrolls to the selection:\n{text}");
        assert!(!text.contains("Andorra"), "the top has scrolled away:\n{text}");
    }

    #[test]
    fn scan_without_iface_reports_clearly() {
        let mut app = App::new(true);
        app.ifaces.clear();
        app.bss.clear();
        app.on_key(KeyEvent::from(KeyCode::Char('s')));
        assert!(matches!(app.message, Some((Tone::Bad, _))));
    }
}
