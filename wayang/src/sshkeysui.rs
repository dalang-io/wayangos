//! `wayang` SSH screen (deck module 05): list root's `authorized_keys`, add one
//! by pasting a public key or `github:USER` / `gitlab:USER`, and remove one.
//!
//! New keys are written to `/data/etc/ssh/authorized_keys` (survives updates)
//! and appended to the live `/root/.ssh/authorized_keys`; the boot script
//! re-applies the persistent copy every boot. The network fetch runs in the
//! background so the HUD stays responsive.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::hud::{self, Theme};
use crate::input::{self, Input, Outcome, Pick, Picker};
use crate::sshkeys::{self, SshKey};
use crate::tui::Tone;

/// What the open text input is for: paste a key line, or type the username
/// behind a GitHub/GitLab fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddMode {
    Paste,
    Remote(&'static str),
}

pub struct App {
    pub t: Theme,
    pub demo: bool,
    pub keys: Vec<SshKey>,
    pub sel: usize,
    input: Option<(AddMode, Input)>,
    picker: Option<Picker>,
    pub message: Option<(Tone, String)>,
    /// Background GitHub/GitLab fetch; polled each tick.
    job: Option<Receiver<Result<Vec<SshKey>, String>>>,
    pub exit: bool,
}

impl App {
    pub fn new(demo: bool) -> App {
        let keys = if demo { sshkeys::demo() } else { sshkeys::load() };
        let mut app = App {
            t: Theme::detect(),
            demo,
            keys,
            sel: 0,
            input: None,
            picker: None,
            message: None,
            job: None,
            exit: false,
        };
        app.message = Some(if app.keys.is_empty() {
            (Tone::Warn, "No authorized keys: add one, or the console is the only way in.".into())
        } else {
            (Tone::Ok, format!("{} authorized key(s).", app.keys.len()))
        });
        app
    }

    fn refresh(&mut self) {
        let keep = self.keys.get(self.sel).map(|k| k.blob.clone());
        self.keys = if self.demo { sshkeys::demo() } else { sshkeys::load() };
        self.sel = keep.and_then(|b| self.keys.iter().position(|k| k.blob == b)).unwrap_or(0);
    }

    fn open_add_menu(&mut self) {
        self.picker = Some(Picker::new(
            "ADD SSH KEY",
            "Where does the key come from?",
            vec![
                "Paste a public key".to_string(),
                "GitHub (github.com/USER.keys)".to_string(),
                "GitLab (gitlab.com/USER.keys)".to_string(),
            ],
        ));
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        let outcome = match self.picker.as_mut() {
            Some(p) => p.on_key(key),
            None => return,
        };
        match outcome {
            Pick::None => {}
            Pick::Cancel => self.picker = None,
            Pick::Choose(0) => {
                self.picker = None;
                self.open_add();
            }
            Pick::Choose(1) => {
                self.picker = None;
                self.open_remote("github", "GitHub");
            }
            Pick::Choose(2) => {
                self.picker = None;
                self.open_remote("gitlab", "GitLab");
            }
            Pick::Choose(_) => self.picker = None,
        }
    }

    fn open_add(&mut self) {
        self.input = Some((
            AddMode::Paste,
            Input::new(
                "ADD SSH KEY",
                "Paste an ssh-ed25519/ssh-rsa/ecdsa-... line:",
                "a public key (.pub), never a private key",
            ),
        ));
    }

    fn open_remote(&mut self, host: &'static str, name: &str) {
        self.input = Some((
            AddMode::Remote(host),
            Input::new(
                format!("ADD FROM {name}"),
                format!("{name} username:"),
                format!("fetch https://{host}.com/USER.keys"),
            ),
        ));
    }

    /// Add already-validated keys (from a paste or a fetch).
    fn finish_add(&mut self, new: Vec<SshKey>) {
        if new.is_empty() {
            self.message = Some((Tone::Warn, "No keys found.".into()));
            return;
        }
        if self.demo {
            let n = sshkeys::merge(&mut self.keys, new);
            self.message = Some((Tone::Ok, format!("demo: added {n} key(s) (nothing written)")));
            return;
        }
        match sshkeys::save_add(&new) {
            Ok(0) => {
                sshkeys::merge(&mut self.keys, new);
                self.message = Some((Tone::Warn, "Already authorized.".into()));
            }
            Ok(n) => {
                sshkeys::merge(&mut self.keys, new);
                self.message = Some((Tone::Ok, format!("Authorized {n} new key(s), saved in /data.")));
            }
            Err(e) => self.message = Some((Tone::Bad, e)),
        }
    }

    fn start_fetch(&mut self, spec: String) {
        if self.job.is_some() {
            return;
        }
        let demo = self.demo;
        let label = spec.clone();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(sshkeys::fetch_remote(&spec, demo));
        });
        self.job = Some(rx);
        self.message = Some((Tone::Warn, format!("Fetching {label}...")));
    }

    /// Called once per UI tick: finish a background fetch when it reports back.
    pub fn poll_job(&mut self) {
        let Some(rx) = &self.job else { return };
        match rx.try_recv() {
            Ok(Ok(keys)) => {
                self.job = None;
                self.finish_add(keys);
            }
            Ok(Err(e)) => {
                self.job = None;
                self.message = Some((Tone::Bad, e));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.job = None,
        }
    }

    fn remove(&mut self) {
        let Some(k) = self.keys.get(self.sel) else {
            self.message = Some((Tone::Bad, "No key selected.".into()));
            return;
        };
        let blob = k.blob.clone();
        let desc = format!("{} {}", k.kind(), k.fingerprint());
        if self.demo {
            self.keys.retain(|k| k.blob != blob);
            self.sel = self.sel.min(self.keys.len().saturating_sub(1));
            self.message = Some((Tone::Ok, format!("demo: removed {desc} (nothing written)")));
            return;
        }
        match sshkeys::remove(&blob) {
            Ok(true) => {
                self.keys.retain(|k| k.blob != blob);
                self.sel = self.sel.min(self.keys.len().saturating_sub(1));
                self.message = Some((Tone::Ok, format!("Removed {desc}.")));
            }
            Ok(false) => self.message = Some((Tone::Warn, "Key not found in authorized_keys.".into())),
            Err(e) => self.message = Some((Tone::Bad, e)),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.picker.is_some() {
            self.on_picker_key(key);
            return;
        }
        if self.input.is_some() {
            self.on_input_key(key);
            return;
        }
        // While a fetch runs only quitting is allowed, so operations can't queue.
        if self.job.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.exit = true;
            }
            return;
        }
        let rows = self.keys.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if rows > 0 => self.sel = (self.sel + rows - 1) % rows,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab if rows > 0 => {
                self.sel = (self.sel + 1) % rows;
            }
            KeyCode::Char('a') => self.open_add_menu(),
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => self.remove(),
            KeyCode::Char('r') => {
                self.refresh();
                self.message = Some((Tone::Ok, "Keys reloaded.".into()));
            }
            KeyCode::Esc | KeyCode::Char('q') => self.exit = true,
            _ => {}
        }
    }

    fn on_input_key(&mut self, key: KeyEvent) {
        let Some((mode, input)) = self.input.as_mut() else { return };
        let mode = *mode;
        match input.on_key(key) {
            Outcome::None => {}
            Outcome::Cancel => self.input = None,
            Outcome::Submit(value) => match mode {
                AddMode::Remote(host) => {
                    if value.is_empty() {
                        if let Some((_, i)) = self.input.as_mut() {
                            i.error = Some("enter a username".into());
                        }
                        return;
                    }
                    let spec = format!("{host}:{value}");
                    if sshkeys::remote_spec(&spec).is_ok() {
                        self.input = None;
                        self.start_fetch(spec);
                    } else if let Some((_, i)) = self.input.as_mut() {
                        i.error = Some("letters, digits, - _ and . only".into());
                    }
                }
                AddMode::Paste => {
                    if let Some(k) = SshKey::parse(&value, "typed") {
                        self.input = None;
                        self.finish_add(vec![k]);
                    } else if sshkeys::remote_spec(&value).is_ok() {
                        self.input = None;
                        self.start_fetch(value);
                    } else if let Some((_, i)) = self.input.as_mut() {
                        i.error = Some("not an SSH public key (ssh-ed25519 AAAA... comment)".into());
                    }
                }
            },
        }
    }
}

/// Demo app with the add-key source picker open, for snapshots.
pub fn demo_add() -> App {
    let mut app = App::new(true);
    app.open_add_menu();
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
            Constraint::Min(8),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, rows[0], app);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rows[1]);
    draw_keys(f, cols[0], app);
    draw_details(f, cols[1], app);
    let busy = app.job.as_ref().map(|_| hud::spinner(tick));
    let msg = app.message.as_ref().map(|(tone, m)| (*tone, m.as_str()));
    hud::status_row(f, rows[2], msg, busy, "a adds a key, d removes one.", t);

    let keys = hud::keycaps(
        &[
            ("↑↓", "pick"),
            ("a", "add"),
            ("d", "remove"),
            ("r", "reload"),
            ("q", "back"),
        ],
        t,
    );
    let mut keys = keys;
    keys.spans.insert(0, Span::raw(" "));
    f.render_widget(Paragraph::new(keys), rows[3]);

    if let Some((_, input)) = &app.input {
        input::draw(f, area, t, input, tick);
    }
    if let Some(picker) = &app.picker {
        input::draw_picker(f, area, t, picker);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let right = vec![Span::styled(
        format!("{} key(s)  persists /data/etc/ssh  ", app.keys.len()),
        t.fg(t.dim),
    )];
    hud::header_bar(f, area, "SSH", "", right, t);
}

fn draw_keys(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "AUTHORIZED KEYS", t);
    let mut lines = vec![Line::from(Span::styled(
        format!("  {:<9} {:<44} {}", "TYPE", "FINGERPRINT", "COMMENT"),
        t.fg(t.dim),
    ))];
    if app.keys.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(format!("{} no authorized keys", t.g.warn), t.fg(t.warn))));
        lines.push(Line::from(Span::styled("press a to add one", t.fg(t.dim))));
    }
    for (i, k) in app.keys.iter().enumerate() {
        let fp = t.clip(&k.fingerprint(), 44);
        let comment = if k.comment.is_empty() { k.source.clone() } else { k.comment.clone() };
        let line = format!("{:<9} {:<44} {}", k.kind(), fp, comment);
        if i == app.sel {
            lines.push(Line::from(Span::styled(
                format!("{} {}", t.g.cursor.trim_end(), line),
                t.highlight(),
            )));
        } else {
            lines.push(Line::from(vec![Span::raw("  "), Span::styled(line, t.fg(t.fg))]));
        }
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_details(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "DETAILS", t);
    let mut lines = Vec::new();
    match app.keys.get(app.sel) {
        Some(k) => {
            lines.push(hud::field("TYPE", 8, vec![Span::styled(k.kind().to_string(), t.bold(t.fg))], t));
            lines.push(Line::from(Span::styled(k.fingerprint(), t.fg(t.accent))));
            lines.push(hud::field(
                "COMMENT",
                8,
                vec![Span::styled(
                    if k.comment.is_empty() { "-".into() } else { k.comment.clone() },
                    t.fg(t.fg),
                )],
                t,
            ));
            lines.push(hud::field("SOURCE", 8, vec![Span::styled(k.source.clone(), t.fg(t.dim))], t));
        }
        None => lines.push(Line::from(Span::styled("no key selected", t.fg(t.dim)))),
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("New keys are written to", t.fg(t.dim))));
    lines.push(Line::from(Span::styled("/data/etc/ssh/authorized_keys", t.fg(t.accent))));
    lines.push(Line::from(Span::styled("and appended to /root/.ssh/authorized_keys.", t.fg(t.dim))));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("a adds a key (paste, GitHub or GitLab)", t.fg(t.dim))));
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(app: &mut App, c: KeyCode) {
        app.on_key(KeyEvent::from(c));
    }

    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)));
        }
    }

    const ED: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPX2zdvjV01sS6kSNEuFCraBHx+RtIL5q/nnODxc+Dys test@box";

    #[test]
    fn demo_renders_keys() {
        let app = App::new(true);
        let text = crate::screen::render_text(&app, 100, 32, draw).unwrap();
        assert!(text.contains("SSH"));
        assert!(text.contains("AUTHORIZED KEYS"));
        assert!(text.contains("ED25519"));
    }

    #[test]
    fn add_menu_pastes_a_key_then_remove() {
        let mut app = App::new(true);
        let before = app.keys.len();
        key(&mut app, KeyCode::Char('a'));
        assert!(app.picker.is_some(), "a opens the source picker");
        key(&mut app, KeyCode::Enter); // "Paste a public key"
        assert!(app.picker.is_none());
        assert!(app.input.is_some());
        type_str(&mut app, ED);
        key(&mut app, KeyCode::Enter);
        assert!(app.input.is_none());
        assert_eq!(app.keys.len(), before + 1);
        assert!(matches!(app.message, Some((Tone::Ok, _))), "{:?}", app.message);
        // the new key is appended, so select it and delete
        app.sel = before;
        key(&mut app, KeyCode::Char('d'));
        assert_eq!(app.keys.len(), before);
    }

    #[test]
    fn add_menu_can_be_cancelled() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Esc);
        assert!(app.picker.is_none() && app.input.is_none());
    }

    #[test]
    fn rejects_junk_input() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Enter);
        type_str(&mut app, "not a key");
        key(&mut app, KeyCode::Enter);
        assert!(app.input.is_some(), "invalid input keeps the modal open");
        assert!(app.input.as_ref().unwrap().1.error.is_some());
    }

    #[test]
    fn remote_picker_builds_the_spec_and_needs_a_username() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Down); // "GitHub"
        key(&mut app, KeyCode::Enter);
        assert!(app.picker.is_none() && app.input.is_some());
        key(&mut app, KeyCode::Enter); // empty user
        assert!(app.input.as_ref().unwrap().1.error.is_some());
        assert!(app.job.is_none(), "nothing is fetched without a username");
        type_str(&mut app, "alice");
        key(&mut app, KeyCode::Enter);
        assert!(app.job.is_some(), "the fetch is a background job");
        assert!(app.input.is_none());
    }

    #[test]
    fn remote_fetch_runs_in_the_background() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('a'));
        key(&mut app, KeyCode::Down); // "GitHub"
        key(&mut app, KeyCode::Enter);
        type_str(&mut app, "alice");
        key(&mut app, KeyCode::Enter);
        assert!(app.job.is_some(), "the fetch is a background job");
        assert!(app.input.is_none(), "the modal closes while the fetch runs");
        for _ in 0..300 {
            app.poll_job();
            if app.job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.job.is_none());
        assert!(matches!(app.message, Some((Tone::Ok, _))), "{:?}", app.message);
    }
}
