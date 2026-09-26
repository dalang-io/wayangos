//! `wayang wifi` HUD screen: list wireless interfaces, scan with `iw`, pick an
//! SSID, enter the passphrase (and optional country) and Connect. Persists
//! `/data/etc/wpa_supplicant.conf` and pins the interface as primary.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::hud::{self, Theme};
use crate::input::{self, Input, Outcome};
use crate::sys;
use crate::tui::Tone;
use crate::wifi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Passphrase,
    Country,
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
    input: Option<(Field, Input)>,
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
            input: None,
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
        self.input = Some((
            Field::Passphrase,
            Input::new(
                format!("PASSPHRASE — {}", bss.ssid),
                "WPA-PSK passphrase:",
                "8-63 characters (or 64 hex); hidden as you type",
            )
            .masked(),
        ));
    }

    fn open_country(&mut self) {
        self.input = Some((
            Field::Country,
            Input::new(
                "COUNTRY",
                "Regulatory country code (optional, e.g. GB):",
                "applied with `iw reg set` before connecting",
            )
            .value(self.country.clone()),
        ));
    }

    fn connect(&mut self, psk: &str) {
        let (Some(iface), Some(bss)) = (self.selected_iface(), self.bss.get(self.bss_sel)) else {
            self.message = Some((Tone::Bad, "Scan and pick a network first.".into()));
            return;
        };
        let iface = iface.name.clone();
        let ssid = bss.ssid.clone();
        let country = if self.country.is_empty() { None } else { Some(self.country.as_str()) };
        match wifi::connect(&iface, &ssid, psk, country, self.demo) {
            Ok(msg) => {
                self.message = Some((Tone::Ok, msg));
                self.refresh_links();
            }
            Err(e) => self.message = Some((Tone::Bad, e)),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.input.is_some() {
            self.on_input_key(key);
            return;
        }
        let rows = if self.bss.is_empty() { self.ifaces.len() } else { self.bss.len() };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if rows > 0 => {
                if self.bss.is_empty() {
                    self.iface_sel = (self.iface_sel + rows - 1) % rows;
                } else {
                    self.bss_sel = (self.bss_sel + rows - 1) % rows;
                }
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab if rows > 0 => {
                if self.bss.is_empty() {
                    self.iface_sel = (self.iface_sel + 1) % rows;
                } else {
                    self.bss_sel = (self.bss_sel + 1) % rows;
                }
            }
            KeyCode::Char('i') if !self.ifaces.is_empty() => {
                self.iface_sel = (self.iface_sel + 1) % self.ifaces.len();
                self.bss.clear();
                self.bss_sel = 0;
                self.message = Some((Tone::Ok, format!("Interface: {}", self.ifaces[self.iface_sel].name)));
            }
            KeyCode::Char('s') | KeyCode::Char('r') => self.scan(),
            KeyCode::Char('c') => self.open_country(),
            KeyCode::Enter | KeyCode::Char('w') => self.open_passphrase(),
            KeyCode::Esc | KeyCode::Char('q') => self.exit = true,
            _ => {}
        }
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        let Some((field, input)) = self.input.as_mut() else { return };
        let field = *field;
        match input.on_key(key) {
            Outcome::None => {}
            Outcome::Cancel => self.input = None,
            Outcome::Submit(value) => match field {
                Field::Country => {
                    if value.is_empty() {
                        self.country.clear();
                        self.input = None;
                    } else if value.len() == 2 && value.chars().all(|c| c.is_ascii_alphabetic()) {
                        self.country = value.to_uppercase();
                        self.input = None;
                    } else if let Some((_, i)) = self.input.as_mut() {
                        i.error = Some("two letters, e.g. GB".into());
                    }
                }
                Field::Passphrase => match wifi::valid_credentials("placeholder", &value) {
                    Err(e) => {
                        if let Some((_, i)) = self.input.as_mut() {
                            i.error = Some(e);
                        }
                    }
                    Ok(()) => {
                        self.input = None;
                        self.connect(&value);
                    }
                },
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

// ---- drawing -----------------------------------------------------------

pub fn draw(f: &mut Frame, app: &App, tick: usize) {
    let t = &app.t;
    let area = f.area();
    f.render_widget(ratatui::widgets::Block::default().style(t.base()), area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, rows[0], app);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
        .split(rows[1]);
    draw_bss(f, cols[0], app);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(6)])
        .split(cols[1]);
    draw_ifaces(f, right[0], app);
    draw_details(f, right[1], app);
    draw_result(f, rows[2], app);

    let keys = hud::keycaps(
        &[
            ("↑↓", "pick"),
            ("s", "scan"),
            ("i", "iface"),
            ("c", "country"),
            ("enter", "connect"),
            ("q", "back"),
        ],
        t,
    );
    f.render_widget(Paragraph::new(keys), rows[3]);

    if let Some((_, input)) = &app.input {
        input::draw(f, area, t, input, tick);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "WIFI", t);
    let st = app.selected_link();
    let name = app.selected_iface().map(|i| i.name.clone()).unwrap_or_default();
    let (color, text) = if st.connected {
        (t.ok, st.summary())
    } else if !st.ssid.is_empty() {
        (t.warn, st.summary())
    } else {
        (t.dim, st.summary())
    };
    let line = Line::from(vec![
        Span::styled(t.g.brand, t.bold(t.accent2)),
        Span::styled(format!(" {name} "), t.bold(t.accent)),
        Span::styled(text, t.fg(color)),
        Span::styled("   persists /data/etc/wpa_supplicant.conf", t.fg(t.dim)),
    ]);
    f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), inner);
}

fn draw_bss(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "ACCESS POINTS", t);
    let mut lines = vec![Line::from(Span::styled(
        format!("  {:<22} {:>6}  {}", "SSID", "SIGNAL", "SECURITY"),
        t.fg(t.dim),
    ))];
    if app.bss.is_empty() {
        lines.push(Line::raw(""));
        let hint = if app.ifaces.is_empty() {
            "no wireless interface"
        } else if !sys::which("iw") && !app.demo {
            "`iw` is not installed"
        } else {
            "press s to scan"
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
                t.g.cursor.trim_end(),
                ssid,
                sig,
                b.security
            );
            lines.push(Line::from(Span::styled(text, t.highlight())));
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
            lines.push(Line::from(vec![
                Span::styled(t.g.cursor, t.bold(t.accent2)),
                Span::styled(line, t.highlight()),
            ]));
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
    let inner = hud::panel(f, area, "DETAILS", t);
    let mut lines = Vec::new();
    match app.bss.get(app.bss_sel) {
        Some(b) => {
            lines.push(hud::field("ssid", 10, vec![Span::styled(b.ssid.clone(), t.bold(t.fg))], t));
            lines.push(hud::field("bssid", 10, vec![Span::styled(b.bssid.clone(), t.fg(t.dim))], t));
            lines.push(hud::field(
                "signal",
                10,
                vec![Span::styled(
                    b.signal.map(|s| format!("{s} dBm")).unwrap_or_else(|| "?".into()),
                    t.fg(t.accent),
                )],
                t,
            ));
            lines.push(hud::field("security", 10, vec![Span::styled(b.security.clone(), t.fg(t.ok))], t));
        }
        None => lines.push(Line::from(Span::styled("no network selected", t.fg(t.dim)))),
    }
    lines.push(Line::raw(""));
    lines.push(hud::field(
        "country",
        8,
        vec![Span::styled(
            if app.country.is_empty() { "-".into() } else { app.country.clone() },
            t.fg(t.fg),
        )],
        t,
    ));
    lines.push(Line::from(Span::styled(
        "c edits country, enter enters the passphrase",
        t.fg(t.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_result(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "RESULT", t);
    let line = match &app.message {
        Some((tone, text)) => {
            let (color, sym) = match tone {
                Tone::Ok => (t.ok, t.g.ok),
                Tone::Warn => (t.warn, t.g.warn),
                Tone::Bad => (t.bad, t.g.bad),
            };
            Line::from(vec![
                hud::badge(color, sym, &text.to_uppercase(), t),
                Span::styled(format!(" {text}"), t.fg(t.fg)),
            ])
        }
        None => Line::from(Span::styled("Press s to scan.", t.fg(t.dim))),
    };
    f.render_widget(Paragraph::new(line), inner);
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
        assert!(app.input.as_ref().unwrap().1.error.is_some());
        assert!(app.input.is_some());
    }

    #[test]
    fn connecting_in_demo_succeeds() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Enter));
        for c in "hunter2hunter2".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(app.input.is_none());
        assert!(matches!(app.message, Some((Tone::Ok, _))), "{:?}", app.message);
    }

    #[test]
    fn country_validates_two_letters() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('c')));
        for c in "GB".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.country, "GB");
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
