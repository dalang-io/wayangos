//! Optional interactive HUD (`wayang` with no subcommand on a TTY).
//!
//! Mirrors the installer's HUD theme (dark background, bracket panels, badges,
//! pixel logo) and offers Status / Check / Update / Upgrade / Rollback / Quit.
//! Actions reuse the exact same code paths as the CLI subcommands; their
//! console output is silenced via [`crate::ui`] while the HUD is on screen.
//!
//! `wayang --demo` and `wayang --screens DIR [--size WxH]` render the screens
//! to text for snapshotting, without touching the system.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::cli::UpdateArgs;
use crate::hud::{self, Theme};
use crate::manifest::SlotMeta;
use crate::netui;
use crate::slot::{self, Slot};
use crate::status::{self, Status};
use crate::update;
use crate::ui;
use crate::wifiui;

pub const ITEMS: [&str; 8] = [
    "STATUS",
    "CHECK FOR UPDATES",
    "UPDATE",
    "UPGRADE",
    "ROLLBACK",
    "NETWORK",
    "WIFI",
    "QUIT",
];

/// A modal sub-screen opened from the menu.
pub enum Sub {
    Net(netui::App),
    Wifi(wifiui::App),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Ok,
    Warn,
    Bad,
}

pub struct App {
    pub t: Theme,
    pub sel: usize,
    pub status: Status,
    pub error: Option<String>,
    pub message: Option<(Tone, String)>,
    pub exit: bool,
    /// Open sub-screen (network/wifi), if any.
    pub sub: Option<Sub>,
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
        App { t: Theme::detect(), sel: 0, status, error, message: None, exit: false, sub: None }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.sub.is_some() {
            match self.sub.as_mut() {
                Some(Sub::Net(a)) => a.on_key(key),
                Some(Sub::Wifi(a)) => a.on_key(key),
                None => {}
            }
            if self.sub_exited() {
                self.sub = None;
            }
            return;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.sel = (self.sel + ITEMS.len() - 1) % ITEMS.len();
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.sel = (self.sel + 1) % ITEMS.len();
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Enter => self.act(self.sel),
            KeyCode::Esc | KeyCode::Char('q') => self.exit = true,
            KeyCode::Char(c @ '1'..='8') => {
                let idx = c as usize - '1' as usize;
                if idx < ITEMS.len() {
                    self.sel = idx;
                    self.act(idx);
                }
            }
            _ => {}
        }
    }

    pub fn sub_exited(&self) -> bool {
        match &self.sub {
            Some(Sub::Net(a)) => a.exit,
            Some(Sub::Wifi(a)) => a.exit,
            None => false,
        }
    }

    fn refresh(&mut self) {
        match status::snapshot(None) {
            Ok(s) => {
                self.status = s;
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn act(&mut self, idx: usize) {
        match idx {
            0 => {
                self.refresh();
                self.message = Some((Tone::Ok, "Status refreshed.".into()));
            }
            1 => self.run_update(true, false),
            2 => self.run_update(false, false),
            3 => self.run_update(false, true),
            4 => self.run_rollback(),
            5 => self.sub = Some(Sub::Net(netui::App::new(false))),
            6 => self.sub = Some(Sub::Wifi(wifiui::App::new(false))),
            7 => self.exit = true,
            _ => {}
        }
    }

    fn run_update(&mut self, check: bool, upgrade: bool) {
        let args = UpdateArgs { check, ..Default::default() };
        ui::set_quiet(true);
        let result = update::run(upgrade, &args);
        ui::set_quiet(false);
        match result {
            Ok(0) if check => self.message = Some((Tone::Ok, "Update available.".into())),
            Ok(0) => {
                self.message = Some((Tone::Ok, "Update staged; reboot to apply.".into()));
                self.refresh();
            }
            Ok(2) => self.message = Some((Tone::Warn, "No update available.".into())),
            Ok(code) => self.message = Some((Tone::Bad, format!("Finished with exit code {code}."))),
            Err(e) => self.message = Some((Tone::Bad, e.to_string())),
        }
    }

    fn run_rollback(&mut self) {
        let args = UpdateArgs { rollback: true, ..Default::default() };
        ui::set_quiet(true);
        let result = update::run(false, &args);
        ui::set_quiet(false);
        match result {
            Ok(0) => {
                self.message = Some((Tone::Warn, "Rollback staged; reboot to apply.".into()));
                self.refresh();
            }
            Ok(code) => self.message = Some((Tone::Bad, format!("Finished with exit code {code}."))),
            Err(e) => self.message = Some((Tone::Bad, e.to_string())),
        }
    }
}

fn demo_status() -> Status {
    let mut s = status::unavailable();
    s.version = Some("1.4.1".into());
    s.channel = "stable".into();
    s.active = Slot::B;
    s.boot_next = Slot::A;
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
        }
        return;
    }
    let t = &app.t;
    let area = f.area();
    f.render_widget(ratatui::widgets::Block::default().style(t.base()), area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, rows[0], app);

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(rows[1]);
    draw_system(f, cols[0], app);
    draw_actions(f, cols[1], app);
    draw_result(f, rows[2], app);

    let keys = hud::keycaps(&[("↑↓", "move"), ("enter", "run"), ("r", "refresh"), ("q", "quit")], t);
    f.render_widget(Paragraph::new(keys), rows[3]);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "UPDATER", t);
    let mut lines = vec![Line::from(vec![
        Span::styled(t.g.brand, t.bold(t.accent2)),
        Span::styled(" wayang", t.bold(t.accent)),
        Span::styled("  A/B updates without GRUB", t.fg(t.dim)),
    ])];
    lines.extend(hud::logo(1, t));
    lines.push(Line::from(Span::styled(
        format!("installed {}", app.status.version.clone().unwrap_or_else(|| "unknown".into())),
        t.fg(t.dim),
    )));
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_system(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "SYSTEM", t);
    let st = &app.status;
    let good = st.good.map(Slot::as_str).unwrap_or("-");
    let mut lines = vec![
        hud::field("version", 10, vec![Span::styled(st.version.clone().unwrap_or_else(|| "unknown".into()), t.bold(t.accent))], t),
        hud::field("channel", 10, vec![Span::styled(st.channel.clone(), t.fg(t.fg))], t),
        hud::field("backend", 10, vec![Span::styled(st.backend.clone(), t.fg(t.dim))], t),
        hud::field("active", 10, vec![Span::styled(st.active.as_str().to_string(), t.bold(t.ok))], t),
        hud::field(
            "boot next",
            10,
            vec![Span::styled(
                format!("{} (good {}, attempts {})", st.boot_next.as_str(), good, st.attempts),
                t.fg(if st.attempts >= slot::ATTEMPT_LIMIT { t.warn } else { t.fg }),
            )],
            t,
        ),
        Line::from(""),
    ];
    for si in &st.slots {
        let v = si.meta.as_ref().map(|m| m.version.as_str()).unwrap_or("-");
        lines.push(hud::field(
            &format!("slot {}", si.slot.as_str()),
            10,
            vec![Span::styled(v.to_string(), t.fg(t.fg))],
            t,
        ));
    }
    lines.push(hud::field(
        "data",
        10,
        vec![Span::styled(
            if st.data { "present" } else { "missing" }.to_string(),
            t.fg(if st.data { t.ok } else { t.warn }),
        )],
        t,
    ));
    if let Some(e) = &app.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            t.clip(e, inner.width.saturating_sub(2) as usize),
            t.fg(t.bad),
        )));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn draw_actions(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "ACTIONS", t);
    let lines: Vec<Line> = ITEMS
        .iter()
        .enumerate()
        .map(|(i, item)| {
            if i == app.sel {
                Line::from(vec![
                    Span::styled(t.g.cursor, t.bold(t.accent2)),
                    Span::styled((*item).to_string(), t.highlight()),
                ])
            } else {
                Line::from(vec![
                    Span::styled("  ", t.fg(t.dim)),
                    Span::styled((*item).to_string(), t.fg(t.fg)),
                ])
            }
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
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
                Span::styled(format!(" {}", text), t.fg(t.fg)),
            ])
        }
        None => Line::from(Span::styled("Ready.", t.fg(t.dim))),
    };
    f.render_widget(Paragraph::new(line).alignment(Alignment::Left), inner);
}

// ---- interactive runner ------------------------------------------------

/// Run the HUD on the real terminal.
pub fn run() -> std::io::Result<()> {
    crate::screen::run(
        App::new(false),
        draw,
        |app, key| app.on_key(key),
        |app| app.exit,
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
            "result",
            Box::new(|app: &mut App| {
                app.message = Some((Tone::Ok, "Update available: 1.4.0 -> 1.4.1.".into()));
            }),
        ),
        (
            "error",
            Box::new(|app: &mut App| {
                app.error = Some("no ESP found (set WAYANG_ESP or pass --esp DEV)".into());
                app.message = Some((Tone::Bad, "trusted keys: not found".into()));
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

/// `wayang --screens DIR`: write one text file per screen.
pub fn dump_screens(dir: &str, size: &str) -> Result<(), String> {
    let (w, h) = parse_size(size)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
    for (name, setup) in demo_states() {
        let mut app = App::new(true);
        setup(&mut app);
        let path = format!("{dir}/{name}.txt");
        let text = crate::screen::render_text(&app, w, h, draw)?;
        std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(app: &App, w: u16, h: u16) -> Result<String, String> {
        crate::screen::render_text(app, w, h, draw)
    }

    #[test]
    fn demo_dashboard_renders() {
        if std::env::var_os("WAYANG_ROOT").is_none() {
            std::env::set_var("WAYANG_ROOT", "/tmp/wayang-tui-test");
        }
        let app = App::new(true);
        let text = render(&app, 90, 24).unwrap();
        assert!(text.contains("wayang"));
        assert!(text.contains("ACTIONS"));
        assert!(text.contains("UPDATER"));
        assert!(text.contains("1.4.1"));
        std::env::remove_var("WAYANG_ROOT");
    }

    #[test]
    fn menu_opens_network_and_wifi() {
        let mut app = App::new(true);
        assert_eq!(ITEMS[5], "NETWORK");
        assert_eq!(ITEMS[6], "WIFI");
        app.sel = 5;
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.sub, Some(Sub::Net(_))));
        // a sub-screen key is delegated; q closes it back to the menu
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(app.sub.is_none());
        app.sel = 6;
        app.on_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(app.sub, Some(Sub::Wifi(_))));
    }

    #[test]
    fn navigation_wraps_and_quits() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(app.sel, ITEMS.len() - 1);
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(app.exit);
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_size("nope").is_err());
        assert_eq!(parse_size("80x24").unwrap(), (80, 24));
    }
}
