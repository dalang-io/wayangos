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
use crate::{help, review};

/// Visible tab row; `←→`/`tab` moves between the panes.
const TABS: [&str; 2] = ["AUTHORIZED KEYS", "DETAILS"];

/// A mutation awaiting REVIEW.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Add(Vec<SshKey>),
    Remove { blob: String, desc: String },
}

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
    /// Active tab/pane: 0 = KEYS, 1 = DETAILS.
    pub pane: usize,
    input: Option<(AddMode, Input)>,
    picker: Option<Picker>,
    /// `/` quick-jump query over the authorized keys, if open.
    jump: Option<Input>,
    /// Pending REVIEW before an add/remove runs.
    review: Option<(Pending, review::Review)>,
    /// Actions run here since the deck last drained them (RECENT strip).
    recent: Vec<String>,
    pub help: bool,
    pub help_scroll: usize,
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
            pane: 0,
            input: None,
            picker: None,
            jump: None,
            review: None,
            recent: Vec::new(),
            help: false,
            help_scroll: 0,
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

    /// Review then add already-validated keys (from a paste or a fetch).
    fn request_add(&mut self, new: Vec<SshKey>) {
        if new.is_empty() {
            self.message = Some((Tone::Warn, "No keys found.".into()));
            return;
        }
        let mut effect = vec!["write /data/etc/ssh/authorized_keys (survives updates)".to_string()];
        for k in &new {
            effect.push(format!("{} {} ({})", k.kind(), k.fingerprint(), k.comment));
        }
        effect.push("append to /root/.ssh/authorized_keys (this boot)".to_string());
        let note = Some("A new key can log in as root; you can remove it here later.".to_string());
        let mut r = review::Review::new(format!("AUTHORIZE {} key(s)", new.len()), effect);
        if let Some(n) = note {
            r = r.note(n);
        }
        self.review = Some((Pending::Add(new), r));
    }

    /// Record a mutating action so the deck's RECENT strip shows it.
    fn record(&mut self, what: impl Into<String>) {
        self.recent.push(what.into());
    }

    /// Drain the actions recorded since the last call (oldest first).
    pub fn take_recent(&mut self) -> Vec<String> {
        std::mem::take(&mut self.recent)
    }

    fn do_add(&mut self, new: Vec<SshKey>) {
        self.record(format!("SSH: authorized {} key(s)", new.len()));
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
                self.request_add(keys);
            }
            Ok(Err(e)) => {
                self.job = None;
                self.message = Some((Tone::Bad, e));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.job = None,
        }
    }

    fn request_remove(&mut self) {
        let Some(k) = self.keys.get(self.sel) else {
            self.message = Some((Tone::Bad, "No key selected.".into()));
            return;
        };
        let blob = k.blob.clone();
        let desc = format!("{} {}", k.kind(), k.fingerprint());
        let r = review::Review::new(
            "REMOVE SSH KEY",
            vec![
                format!("remove {desc} from /data/etc/ssh/authorized_keys"),
                "remove it from /root/.ssh/authorized_keys (this boot)".into(),
            ],
        )
        .note("If this is your only key, you could lock yourself out.");
        self.review = Some((Pending::Remove { blob, desc }, r));
    }

    fn do_remove(&mut self, blob: &str, desc: &str) {
        self.record(format!("SSH: removed {desc}"));
        if self.demo {
            self.keys.retain(|k| k.blob != blob);
            self.sel = self.sel.min(self.keys.len().saturating_sub(1));
            self.message = Some((Tone::Ok, format!("demo: removed {desc} (nothing written)")));
            return;
        }
        match sshkeys::remove(blob) {
            Ok(true) => {
                self.keys.retain(|k| k.blob != blob);
                self.sel = self.sel.min(self.keys.len().saturating_sub(1));
                self.message = Some((Tone::Ok, format!("Removed {desc}.")));
            }
            Ok(false) => self.message = Some((Tone::Warn, "Key not found in authorized_keys.".into())),
            Err(e) => self.message = Some((Tone::Bad, e)),
        }
    }

    /// `/` quick-jump candidates: the authorized keys (read-only: selecting a
    /// key never removes it).
    fn jump_targets(&self) -> Vec<String> {
        self.keys
            .iter()
            .map(|k| format!("{} {} {}", k.kind(), k.fingerprint(), k.comment))
            .collect()
    }

    fn open_jump(&mut self) {
        self.jump = Some(Input::new(
            "QUICK JUMP",
            "Type a key comment or fingerprint (fuzzy):",
            "e.g. laptop, SHA256",
        ));
    }

    fn submit_jump(&mut self, q: &str) {
        let targets = self.jump_targets();
        match input::fuzzy_matches(q, &targets).first().copied() {
            Some(i) => {
                self.sel = i;
                self.pane = 0;
                let desc = format!("{} {}", self.keys[i].kind(), self.keys[i].comment);
                self.message = Some((Tone::Ok, format!("Jumped to {desc}.")));
            }
            None if !q.trim().is_empty() => {
                self.message = Some((Tone::Warn, format!("No key matches '{q}'.")));
            }
            None => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if self.review.is_some() {
            let decision = self.review.as_mut().map(|(_, r)| r.on_key(key));
            match decision {
                Some(review::Decision::Confirm) => {
                    if let Some((pending, _)) = self.review.take() {
                        match pending {
                            Pending::Add(keys) => self.do_add(keys),
                            Pending::Remove { blob, desc } => self.do_remove(&blob, &desc),
                        }
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
                    self.message = Some(match help::print_keys("SSH") {
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
        // While a fetch runs, don't let a stray key queue another operation or
        // drop the pending result.
        if self.job.is_some() {
            self.message = Some((Tone::Warn, "An action is running; wait for it to finish.".into()));
            return;
        }
        match key.code {
            KeyCode::Left | KeyCode::BackTab => self.pane = self.pane.saturating_sub(1),
            KeyCode::Right | KeyCode::Tab => self.pane = (self.pane + 1).min(TABS.len() - 1),
            KeyCode::Up | KeyCode::Char('k') => {
                let n = self.keys.len();
                if n > 0 {
                    self.sel = (self.sel + n - 1) % n;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let n = self.keys.len();
                if n > 0 {
                    self.sel = (self.sel + 1) % n;
                }
            }
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = self.keys.len().saturating_sub(1),
            KeyCode::Enter => self.pane = 1 - self.pane.min(1),
            KeyCode::Char('a') => self.open_add_menu(),
            KeyCode::Char('d') | KeyCode::Delete | KeyCode::Backspace => self.request_remove(),
            KeyCode::Char('r') => {
                self.refresh();
                self.message = Some((Tone::Ok, "Keys reloaded.".into()));
            }
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
                        self.request_add(vec![k]);
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
            Constraint::Length(1),
            Constraint::Min(6),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, rows[0], app);
    hud::tab_row(f, rows[1], &TABS, app.pane, t);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(rows[2]);
    draw_keys(f, cols[0], app);
    draw_details(f, cols[1], app);
    let busy = app.job.as_ref().map(|_| hud::spinner(tick));
    let msg = app.message.as_ref().map(|(tone, m)| (*tone, m.as_str()));
    hud::status_row(f, rows[3], msg, busy, "a adds a key, d removes one.", t);

    let mut keys = hud::keycaps(
        &[
            ("↑↓", "pick"),
            ("/", "find"),
            ("a", "add"),
            ("d", "remove"),
            ("?", "help"),
            ("b", "back"),
        ],
        t,
    );
    keys.spans.insert(0, Span::raw(" "));
    f.render_widget(Paragraph::new(keys), rows[4]);

    if let Some((_, input)) = &app.input {
        input::draw(f, area, t, input, tick);
    }
    if let Some(picker) = &app.picker {
        input::draw_picker(f, area, t, picker);
    }
    if app.help {
        help::draw(f, area, t, "SSH", app.help_scroll);
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
    let right = vec![Span::styled(
        format!("{} key(s)  persists /data/etc/ssh  ", app.keys.len()),
        t.fg(t.dim),
    )];
    let crumb = t.breadcrumb(&["SSH", TABS[app.pane.min(1)], &format!("{} key(s)", app.keys.len())]);
    hud::header_bar(f, area, &crumb, "", right, t);
}

fn draw_keys(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "AUTHORIZED KEYS", t);
    let row_w = inner.width.saturating_sub(1) as usize;
    let mut lines = vec![Line::from(Span::styled(
        format!("  {:<9} {:<44} {}", "TYPE", "FINGERPRINT", "COMMENT"),
        t.fg(t.dim),
    ))];
    if app.keys.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(format!("{} no authorized keys", t.g.warn), t.fg(t.warn))));
        lines.push(Line::from(Span::styled("press a to add one (paste, GitHub or GitLab)", t.fg(t.dim))));
    }
    for (i, k) in app.keys.iter().enumerate() {
        let fp = t.clip(&k.fingerprint(), 44);
        let comment = if k.comment.is_empty() { k.source.clone() } else { k.comment.clone() };
        let line = format!("{:<9} {:<44} {}", k.kind(), fp, comment);
        if i == app.sel {
            lines.push(hud::line_with_hint(
                vec![Span::styled(format!("{} {}", t.g.cursor.trim_end(), line), t.highlight())],
                "d remove",
                row_w,
                t,
            ));
        } else {
            lines.push(Line::from(vec![Span::raw("  "), Span::styled(line, t.fg(t.fg))]));
        }
    }
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_details(f: &mut Frame, area: Rect, app: &App) {
    let t = &app.t;
    let inner = hud::panel(f, area, "DETAILS — EDIT: form", t);
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
    fn add_menu_pastes_a_key_then_remove_each_after_review() {
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
        assert!(app.review.is_some(), "adding opens REVIEW");
        assert_eq!(app.keys.len(), before, "nothing is written before the confirm");
        key(&mut app, KeyCode::Enter); // confirm
        assert_eq!(app.keys.len(), before + 1);
        assert!(matches!(app.message, Some((Tone::Ok, _))), "{:?}", app.message);
        // the new key is appended, so select it and delete (with a REVIEW)
        app.sel = before;
        key(&mut app, KeyCode::Char('d'));
        assert!(app.review.is_some(), "removing opens REVIEW");
        assert_eq!(app.keys.len(), before + 1);
        key(&mut app, KeyCode::Enter);
        assert_eq!(app.keys.len(), before);
    }

    #[test]
    fn remove_can_be_cancelled() {
        let mut app = App::new(true);
        let before = app.keys.len();
        key(&mut app, KeyCode::Char('d'));
        assert!(app.review.is_some());
        key(&mut app, KeyCode::Esc);
        assert!(app.review.is_none());
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
        assert!(app.review.is_some(), "the fetched keys wait for a REVIEW");
        key(&mut app, KeyCode::Enter);
        assert!(matches!(app.message, Some((Tone::Ok, _))), "{:?}", app.message);
    }

    #[test]
    fn mutations_are_recorded_for_the_deck() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Char('d')); // remove REVIEW
        assert!(app.take_recent().is_empty(), "nothing recorded before the confirm");
        key(&mut app, KeyCode::Enter);
        let r = app.take_recent();
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].starts_with("SSH: removed"), "{r:?}");
        assert!(app.take_recent().is_empty(), "drained once");
    }

    #[test]
    fn slash_jump_selects_a_key_without_removing_it() {
        let mut app = App::new(true);
        let before = app.keys.len();
        app.sel = 0;
        key(&mut app, KeyCode::Char('/'));
        assert!(app.jump.is_some());
        type_str(&mut app, "desktop");
        key(&mut app, KeyCode::Enter);
        assert!(app.jump.is_none());
        assert_eq!(app.sel, 1, "jump selects the matching key");
        assert_eq!(app.pane, 0);
        assert!(app.review.is_none(), "jump never removes");
        assert_eq!(app.keys.len(), before);
    }

    #[test]
    fn arrows_switch_panes_and_esc_steps_back() {
        let mut app = App::new(true);
        key(&mut app, KeyCode::Right);
        assert_eq!(app.pane, 1);
        key(&mut app, KeyCode::Esc);
        assert_eq!(app.pane, 0);
        assert!(!app.exit);
        key(&mut app, KeyCode::Char('b'));
        assert!(app.exit);
    }
}
