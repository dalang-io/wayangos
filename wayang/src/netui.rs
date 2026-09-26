//! `wayang net` HUD screen: pick an interface, choose DHCP/Static, edit the
//! static fields and Apply. Writes `/data/etc/network/{primary,config}` and
//! brings the choice up immediately (see `crate::net`).

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crate::hud::{self, Theme};
use crate::input::{self, Input, Outcome};
use crate::net::{self, Family, Mode, NetChoice};
use crate::tui::Tone;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    V4Addr,
    V4Gw,
    V4Dns,
    V6Addr,
    V6Gw,
    V6Dns,
}

pub struct App {
    pub t: Theme,
    pub demo: bool,
    pub ifaces: Vec<net::Iface>,
    pub sel: usize,
    pub choice: NetChoice,
    input: Option<(Field, Input)>,
    pub message: Option<(Tone, String)>,
    /// Background operation (apply / link up-down / dhcp); polled each tick so
    /// the HUD never blocks on the network.
    job: Option<Receiver<Result<String, String>>>,
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
        let mut choice = NetChoice::default();
        if let Some(p) = &primary {
            choice.iface = Some(p.clone());
        }
        let sel = primary
            .as_ref()
            .and_then(|p| ifaces.iter().position(|i| &i.name == p))
            .unwrap_or(0);
        App {
            t: Theme::detect(),
            demo,
            ifaces,
            sel,
            choice,
            input: None,
            message: None,
            job: None,
            exit: false,
        }
    }

    fn refresh(&mut self) {
        let primary = net::primary_iface();
        let keep = self.ifaces.get(self.sel).map(|i| i.name.clone());
        self.ifaces = net::list(primary.as_deref());
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

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.input.is_some() {
            self.on_input_key(key);
            return;
        }
        // While a background job runs, only allow quitting; everything else is
        // ignored so the user can't queue conflicting operations.
        if self.job.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.exit = true;
            }
            return;
        }
        let rows = self.ifaces.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if rows > 0 => {
                self.sel = (self.sel + rows - 1) % rows;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab if rows > 0 => {
                self.sel = (self.sel + 1) % rows;
            }
            KeyCode::Char('m') => {
                self.choice.mode = match self.choice.mode {
                    Mode::Dhcp => Mode::Static,
                    Mode::Static => Mode::Dhcp,
                };
            }
            KeyCode::Char('f') if self.choice.mode == Mode::Static => {
                self.choice.family = self.choice.family.next();
            }
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
            KeyCode::Char('u') => self.link(true),
            KeyCode::Char('d') => self.link(false),
            KeyCode::Char('h') => self.dhcp(),
            KeyCode::Enter | KeyCode::Char('a') => self.apply(),
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
        self.start_job(&format!("{name}: link {state}"), move || net::set_link(&name, up));
    }

    /// Lease this interface without making it the primary uplink.
    fn dhcp(&mut self) {
        let Some(name) = self.selected() else {
            self.message = Some((Tone::Bad, "No interface selected.".into()));
            return;
        };
        let demo = self.demo;
        self.start_job(&format!("{name}: dhcp"), move || net::dhcp_now(&name, demo));
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
        let choice = self.choice.clone();
        let demo = self.demo;
        let label = choice.summary();
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
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(rows[1]);
    draw_ifaces(f, cols[0], app);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(9), Constraint::Min(6)])
        .split(cols[1]);
    draw_mode(f, right[0], app);
    draw_fields(f, right[1], app);
    draw_result(f, rows[2], app, tick);

    let keys = hud::keycaps(
        &[
            ("↑↓", "pick"),
            ("m", "dhcp/static"),
            ("f", "family"),
            ("1-6", "edit"),
            ("u/d", "link up/down"),
            ("h", "dhcp here"),
            ("enter", "apply"),
            ("r", "refresh"),
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
    let inner = hud::panel(f, area, "NETWORK", t);
    let line = Line::from(vec![
        Span::styled(t.g.brand, t.bold(t.accent2)),
        Span::styled(" uplink", t.bold(t.accent)),
        Span::styled("  persists /data/etc/network/{primary,config}", t.fg(t.dim)),
    ]);
    f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), inner);
}

fn iface_line(iface: &net::Iface, selected: bool, t: &Theme) -> Line<'static> {
    let link = if iface.link { "up" } else { "down" };
    let ip = iface.ipv4.first().map(String::as_str).unwrap_or("-");
    let tag = if iface.wireless { "~" } else { " " };
    let primary = if iface.primary { "*" } else { " " };
    if selected {
        let text = format!(
            "{} {:<2}{:<10} {:<4} {:<8} {:<17} {:<15} {}",
            t.g.cursor.trim_end(),
            tag,
            iface.name,
            link,
            iface.driver,
            iface.mac,
            ip,
            primary
        );
        Line::from(Span::styled(text, t.highlight()))
    } else {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{tag} "), t.fg(t.accent2)),
            Span::styled(format!("{:<10}", iface.name), t.bold(t.fg)),
            Span::styled(format!("{:<4} ", link), t.fg(if iface.link { t.ok } else { t.warn })),
            Span::styled(format!("{:<8} ", iface.driver), t.fg(t.accent)),
            Span::styled(format!("{:<17} ", iface.mac), t.fg(t.dim)),
            Span::styled(format!("{ip:<15} "), t.fg(t.fg)),
            Span::styled(primary.to_string(), t.bold(t.accent2)),
        ])
    }
}

fn draw_ifaces(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "INTERFACES", t);
    let mut lines = vec![Line::from(Span::styled(
        format!("  W {:<10} {:<4} {:<8} {:<17} {:<15} P", "IFACE", "LINK", "DRIVER", "MAC", "IPV4"),
        t.fg(t.dim),
    ))];
    if app.ifaces.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("{} no interface found in /sys/class/net", t.g.warn),
            t.fg(t.warn),
        )));
    }
    for (i, iface) in app.ifaces.iter().enumerate() {
        lines.push(iface_line(iface, i == app.sel, t));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{} wireless   * current primary", t.g.warn),
        t.fg(t.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_mode(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "MODE", t);
    let mut lines = Vec::new();
    for m in [Mode::Dhcp, Mode::Static] {
        let sel = app.choice.mode == m;
        let note = match m {
            Mode::Dhcp => "automatic lease (udhcpc)",
            Mode::Static => "fixed address, gateway and DNS",
        };
        lines.push(Line::from(vec![
            Span::styled(if sel { t.g.cursor } else { "  " }.to_string(), t.fg(t.accent)),
            Span::styled(
                format!("{:<8}", m.label()),
                if sel { t.bold(t.ok) } else { t.fg(t.fg) },
            ),
            Span::styled(note, t.fg(t.dim)),
        ]));
    }
    if app.choice.mode == Mode::Static {
        lines.push(Line::raw(""));
        let mut fam = vec![Span::styled("FAMILY  ", t.fg(t.dim))];
        for famly in [Family::Ipv4, Family::Ipv6, Family::Both] {
            let sel = app.choice.family == famly;
            fam.push(Span::styled(
                format!("{} ", famly.label()),
                if sel { t.bold(t.accent2) } else { t.fg(t.dim) },
            ));
        }
        lines.push(Line::from(fam));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "m toggles mode, f cycles family",
        t.fg(t.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_fields(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "ADDRESS", t);
    let mut lines = Vec::new();
    if app.choice.mode == Mode::Dhcp {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(format!("{} DHCP needs no fields", t.g.ok), t.bold(t.ok))));
        lines.push(Line::from(Span::styled("the interface gets address, route and DNS", t.fg(t.dim))));
        lines.push(Line::from(Span::styled("from the network automatically.", t.fg(t.dim))));
    } else {
        if app.choice.family.has_v4() {
            lines.push(net_field(t, "1", "ADDRESS", &app.choice.ipv4_address, "192.168.1.50/24"));
            lines.push(net_field(t, "2", "GATEWAY", &app.choice.ipv4_gateway, "192.168.1.1"));
            lines.push(net_field(t, "3", "DNS", &app.choice.ipv4_dns, "1.1.1.1 8.8.8.8"));
        }
        if app.choice.family.has_v6() {
            if app.choice.family.has_v4() {
                lines.push(Line::raw(""));
            }
            lines.push(net_field(t, "4", "ADDRESS", &app.choice.ipv6_address, "2001:db8::50/64"));
            lines.push(net_field(t, "5", "GATEWAY", &app.choice.ipv6_gateway, "2001:db8::1"));
            lines.push(net_field(t, "6", "DNS", &app.choice.ipv6_dns, "2001:4860:4860::8888"));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("gateway and DNS are optional", t.fg(t.dim))));
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn net_field(t: &Theme, n: &str, label: &str, value: &str, hint: &str) -> Line<'static> {
    let shown = if value.is_empty() {
        Span::styled(format!("{hint}  (press {n})"), t.fg(t.dim))
    } else {
        Span::styled(value.to_string(), t.bold(t.fg))
    };
    hud::field(&format!("{n} {label}"), 12, vec![shown], t)
}

fn draw_result(f: &mut Frame, area: Rect, app: &App, tick: usize) {
    let t = &app.t;
    let inner = hud::panel(f, area, "RESULT", t);
    let spinner = ["|", "/", "-", "\\"][tick % 4];
    let line = match &app.message {
        Some((tone, text)) => {
            let (color, sym) = match tone {
                Tone::Ok => (t.ok, t.g.ok),
                Tone::Warn => (t.warn, t.g.warn),
                Tone::Bad => (t.bad, t.g.bad),
            };
            let lead = if app.busy() { format!("{spinner} ") } else { String::new() };
            Line::from(vec![
                Span::styled(lead, t.bold(t.accent2)),
                hud::badge(color, sym, &text.to_uppercase(), t),
                Span::styled(format!(" {text}"), t.fg(t.fg)),
            ])
        }
        None => Line::from(Span::styled(
            "Pick an interface, then enter to apply.",
            t.fg(t.dim),
        )),
    };
    f.render_widget(Paragraph::new(line), inner);
}

/// Run the screen on the real terminal.
pub fn run() -> std::io::Result<()> {
    crate::screen::run(
        App::new(false),
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
    fn dhcp_here_is_demo_safe() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('h')));
        assert!(app.busy(), "dhcp starts a background job");
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
}
