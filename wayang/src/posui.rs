//! `wayang` HUD screen for the WayangPOS kiosk service.
//!
//! Shows which binary is installed (`/data/bin` preferred over `/usr/bin`),
//! whether it is running, and the autostart / exit policy. `a` and `x` toggle
//! the policy by rewriting `/data/etc/pos.conf`; `s`/`t`/`e` start, stop and
//! restart. WayangPOS is userspace-only, so nothing here touches the kernel or
//! needs an image rebuild.
//!
//! Start/stop/restart run on a worker thread and are polled once per UI tick,
//! so the HUD never blocks (see `netui`).

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use crate::hud::{self, Theme};
use crate::pos::{self, Status};
use crate::tui::Tone;

pub struct App {
    pub t: Theme,
    pub demo: bool,
    pub status: Status,
    pub message: Option<(Tone, String)>,
    /// Running service action; polled each tick so the HUD stays responsive.
    job: Option<Receiver<Result<String, String>>>,
    pub exit: bool,
}

impl App {
    pub fn new(demo: bool) -> App {
        let (status, message) = if demo {
            (demo_status(), Some((Tone::Ok, "WayangPOS ready.".into())))
        } else {
            (pos::snapshot(), None)
        };
        App {
            t: Theme::detect(),
            demo,
            status,
            message,
            job: None,
            exit: false,
        }
    }

    fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn refresh(&mut self) {
        if self.demo {
            return;
        }
        self.status = pos::snapshot();
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

    /// Called once per UI tick: finish a background action when it reports back.
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

    fn run(&mut self, action: &'static str, label: &str) {
        let demo = self.demo;
        self.start_job(label, move || {
            if demo {
                Ok(format!("pos {action}: done (demo, not applied)"))
            } else {
                pos::action_output(action)
            }
        });
    }

    fn toggle_autostart(&mut self) {
        let on = !self.status.autostart;
        if self.demo {
            self.status.autostart = on;
            self.message = Some((Tone::Ok, format!("AUTOSTART {} (demo, not written)", onword(on))));
            return;
        }
        match pos::set_autostart(on) {
            Ok(()) => {
                self.status.autostart = on;
                self.message = Some((
                    Tone::Ok,
                    format!("AUTOSTART {} (written {})", onword(on), pos::conf_file().display()),
                ));
            }
            Err(e) => self.message = Some((Tone::Bad, e)),
        }
    }

    fn toggle_exit(&mut self) {
        let on = !self.status.allow_exit;
        if self.demo {
            self.status.allow_exit = on;
            self.message = Some((Tone::Warn, format!("ALLOW_EXIT {} (demo, not written)", onword(on))));
            return;
        }
        match pos::set_allow_exit(on) {
            Ok(()) => {
                self.status.allow_exit = on;
                self.message = Some(if on {
                    (
                        Tone::Ok,
                        format!("ALLOW_EXIT on: leave the kiosk via Settings -> F10 ({})", pos::conf_file().display()),
                    )
                } else {
                    (
                        Tone::Warn,
                        "ALLOW_EXIT off: recover over SSH with `wayang pos stop`".into(),
                    )
                });
            }
            Err(e) => self.message = Some((Tone::Bad, e)),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // While an action runs, only allow quitting so conflicting operations
        // can't be queued.
        if self.busy() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.exit = true;
            }
            return;
        }
        match key.code {
            KeyCode::Char('a') => self.toggle_autostart(),
            KeyCode::Char('x') => self.toggle_exit(),
            KeyCode::Char('s') | KeyCode::Enter => self.run("start", "Start"),
            KeyCode::Char('t') => self.run("stop", "Stop"),
            KeyCode::Char('e') => self.run("restart", "Restart"),
            KeyCode::Char('r') => {
                self.refresh();
                self.message = Some((Tone::Ok, "POS status refreshed.".into()));
            }
            KeyCode::Esc | KeyCode::Char('q') => self.exit = true,
            _ => {}
        }
    }
}

fn onword(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

fn demo_status() -> Status {
    Status {
        bin: Some(PathBuf::from("/data/bin/wayang-pos")),
        autostart: true,
        allow_exit: true,
        running: true,
        supervised: true,
        log: PathBuf::from("/data/log/wayang-pos.log"),
    }
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
            Constraint::Min(10),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, rows[0], app);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(54), Constraint::Percentage(46)])
        .split(rows[1]);
    draw_service(f, cols[0], app);
    draw_policy(f, cols[1], app);
    draw_result(f, rows[2], app, tick);

    let keys = hud::keycaps(
        &[
            ("enter/s", "start"),
            ("t", "stop"),
            ("e", "restart"),
            ("a", "autostart"),
            ("x", "exit policy"),
            ("r", "refresh"),
            ("q", "back"),
        ],
        t,
    );
    let mut keys = keys;
    keys.spans.insert(0, Span::raw(" "));
    f.render_widget(Paragraph::new(keys), rows[3]);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let st = &app.status;
    let (color, sym, text) = if !st.installed() {
        (t.warn, t.g.warn, "NOT INSTALLED")
    } else if st.running {
        (t.ok, t.g.ok, "RUNNING")
    } else {
        (t.dim, t.g.none, "STOPPED")
    };
    let right = vec![hud::badge(color, sym, &format!("WAYANG-POS {text}"), t), Span::raw(" ")];
    hud::header_bar(f, area, "POS KIOSK", "", right, t);
}

fn draw_service(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "SERVICE", t);
    let st = &app.status;
    let bin = match &st.bin {
        Some(p) => Span::styled(p.display().to_string(), t.bold(t.fg)),
        None => Span::styled("not installed", t.fg(t.warn)),
    };
    let state = Span::styled(
        st.state().to_string(),
        t.fg(if st.running { t.ok } else { t.dim }),
    );
    let autostart = Span::styled(
        onword(st.autostart).to_string(),
        t.fg(if st.autostart { t.ok } else { t.warn }),
    );
    let exit = Span::styled(
        st.exit_policy().to_string(),
        t.fg(if st.allow_exit { t.ok } else { t.warn }),
    );
    let lines = vec![
        hud::field("BINARY", 10, vec![bin], t),
        hud::field("STATE", 10, vec![state], t),
        hud::field("AUTOSTART", 10, vec![autostart], t),
        hud::field("EXIT", 10, vec![exit], t),
        hud::field(
            "CONFIG",
            10,
            vec![Span::styled(pos::conf_file().display().to_string(), t.fg(t.dim))],
            t,
        ),
        hud::field(
            "LOG",
            10,
            vec![Span::styled(st.log.display().to_string(), t.fg(t.dim))],
            t,
        ),
    ];
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }), inner);
}

fn draw_policy(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "POLICY", t);
    let st = &app.status;
    let mut lines = vec![
        Line::from(Span::styled("AUTOSTART", t.bold(t.accent))),
        Line::from(Span::styled(
            "start WayangPOS at boot when a binary is installed.",
            t.fg(t.dim),
        )),
        Line::raw(""),
        Line::from(Span::styled("ALLOW_EXIT", t.bold(t.accent))),
        Line::from(Span::styled(
            "let an admin leave the kiosk (Settings -> F10).",
            t.fg(t.dim),
        )),
    ];
    if !st.allow_exit {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("{} exit disabled — local terminal is unreachable.", t.g.warn),
            t.bold(t.warn),
        )));
        lines.push(Line::from(Span::styled(
            "Recover over SSH (root key): wayang pos stop",
            t.fg(t.warn),
        )));
    } else if !st.autostart {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            format!("{} autostart off — the terminal is free at boot.", t.g.ok),
            t.fg(t.ok),
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "a / x toggles are saved immediately; restart applies exit policy.",
        t.fg(t.dim),
    )));
    f.render_widget(Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true }), inner);
}

fn draw_result(f: &mut Frame, area: Rect, app: &App, tick: usize) {
    let msg = app.message.as_ref().map(|(tone, m)| (*tone, m.as_str()));
    hud::status_row(f, area, msg, app.busy().then(|| hud::spinner(tick)), "s start, t stop, a autostart, x exit policy.", &app.t);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_renders_service_and_policy() {
        let app = App::new(true);
        let text = crate::screen::render_text(&app, 100, 32, draw).unwrap();
        assert!(text.contains("POS KIOSK"));
        assert!(text.contains("/data/bin/wayang-pos"));
        assert!(text.contains("AUTOSTART"));
        assert!(text.contains("ALLOW_EXIT"));
        assert!(text.contains("running"));
    }

    #[test]
    fn toggles_write_policy_in_demo() {
        let mut app = App::new(true);
        assert!(app.status.autostart);
        app.on_key(KeyEvent::from(KeyCode::Char('a')));
        assert!(!app.status.autostart);
        assert!(app.status.allow_exit);
        app.on_key(KeyEvent::from(KeyCode::Char('x')));
        assert!(!app.status.allow_exit);
    }

    #[test]
    fn start_runs_in_background_and_reports() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('s')));
        assert!(app.busy(), "start schedules a job");
        for _ in 0..200 {
            app.poll_job();
            if !app.busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!app.busy());
        assert!(matches!(app.message, Some((Tone::Ok, _))), "{:?}", app.message);
    }

    #[test]
    fn exit_disabled_shows_ssh_recovery_hint() {
        let mut app = App::new(true);
        app.status.allow_exit = false;
        let text = crate::screen::render_text(&app, 100, 32, draw).unwrap();
        assert!(text.contains("SSH"), "{text}");
        assert!(text.contains("wayang pos stop"));
    }

    #[test]
    fn quit_key_sets_exit() {
        let mut app = App::new(true);
        app.on_key(KeyEvent::from(KeyCode::Char('q')));
        assert!(app.exit);
    }
}
