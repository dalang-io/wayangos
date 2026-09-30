//! A small centered text-input modal, mirroring the installer's `Modal::Input`
//! but self-contained for the runtime HUD (different crate, same look).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::hud::{self, Theme};
use wayang_tui::overlay::Overlay;

/// What a key did to the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Still editing.
    None,
    /// Enter pressed; the trimmed value.
    Submit(String),
    /// Esc pressed.
    Cancel,
}

pub struct Input {
    pub title: String,
    pub prompt: String,
    pub hint: String,
    pub buf: String,
    pub error: Option<String>,
    /// Hide the value (passphrases).
    pub mask: bool,
}

impl Input {
    pub fn new(title: impl Into<String>, prompt: impl Into<String>, hint: impl Into<String>) -> Input {
        Input {
            title: title.into(),
            prompt: prompt.into(),
            hint: hint.into(),
            buf: String::new(),
            error: None,
            mask: false,
        }
    }

    pub fn value(mut self, buf: impl Into<String>) -> Input {
        self.buf = buf.into();
        self
    }

    pub fn masked(mut self) -> Input {
        self.mask = true;
        self
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Esc => Outcome::Cancel,
            KeyCode::Backspace => {
                self.buf.pop();
                self.error = None;
                Outcome::None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.buf.chars().count() < 1024 {
                    self.buf.push(c);
                }
                self.error = None;
                Outcome::None
            }
            KeyCode::Enter => Outcome::Submit(self.buf.trim().to_string()),
            _ => Outcome::None,
        }
    }
}

/// A centered list-picker modal: choose one value from a finite set instead of
/// typing it. Same look as [`Input`].
pub struct Picker {
    pub title: String,
    pub prompt: String,
    pub items: Vec<String>,
    pub sel: usize,
}

/// What a key did to the picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Still choosing.
    None,
    /// Enter pressed; the selected index.
    Choose(usize),
    /// Esc (or q) pressed.
    Cancel,
}

impl Picker {
    pub fn new(title: impl Into<String>, prompt: impl Into<String>, items: Vec<String>) -> Picker {
        Picker { title: title.into(), prompt: prompt.into(), items, sel: 0 }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Pick {
        let n = self.items.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Pick::Cancel,
            KeyCode::Enter if n > 0 => Pick::Choose(self.sel),
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.sel = self.sel.saturating_sub(1);
                Pick::None
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab if n > 0 => {
                self.sel = (self.sel + 1).min(n - 1);
                Pick::None
            }
            KeyCode::Home if n > 0 => {
                self.sel = 0;
                Pick::None
            }
            KeyCode::End if n > 0 => {
                self.sel = n - 1;
                Pick::None
            }
            _ => Pick::None,
        }
    }
}

/// Indices of `items` that fuzzy-match `q`: case-insensitive substring, or a
/// word that starts with it. An empty query matches nothing.
pub fn fuzzy_matches(q: &str, items: &[String]) -> Vec<usize> {
    let q = q.trim().to_ascii_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    items
        .iter()
        .enumerate()
        .filter(|(_, hay)| {
            let hay = hay.to_ascii_lowercase();
            hay.contains(&q) || hay.split_whitespace().any(|w| w.starts_with(&q))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Draw the matched candidates just under a jump input box (read-only jump):
/// `enter` selects the first.
pub fn draw_hits(f: &mut Frame, area: Rect, t: &Theme, title: &str, hits: &[String]) {
    if hits.is_empty() {
        return;
    }
    let ir = hud::centered(area, 76, 9);
    let h = (hits.len() as u16 + 2).min(8).min(area.bottom().saturating_sub(ir.y + 9));
    if h < 3 {
        return;
    }
    let rect = Rect { x: ir.x, y: ir.y + 9, width: ir.width, height: h };
    f.render_widget(Clear, rect);
    let inner = hud::panel_focused(f, rect, title, None, t);
    let mut lines = Vec::new();
    for s in hits.iter().take(inner.height as usize) {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(s.clone(), t.palette.bold(t.palette.fg)),
            Span::styled("   enter selects it", t.palette.fg(t.palette.dim)),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Draw the modal centered in `area`; `tick` drives the cursor blink. The
/// chrome is the shared modal frame [`Overlay`].
pub fn draw(f: &mut Frame, area: Rect, t: &Theme, input: &Input, tick: usize) {
    let ov = Overlay::new(&input.title).size(76, 9);
    f.render_widget(Clear, hud::centered(area, 76, 9));
    let inner = ov.render(f, area, t).content;

    let room = inner.width.saturating_sub(6) as usize;
    let shown: String = if input.mask {
        "*".repeat(input.buf.chars().count().min(room))
    } else {
        let n = input.buf.chars().count();
        input.buf.chars().skip(n.saturating_sub(room)).collect()
    };
    let cursor = if tick.is_multiple_of(2) { "_" } else { " " };
    let field = Line::from(vec![
        Span::styled(format!("{} ", t.selection_mark().trim_end()), t.palette.bold(t.palette.accent2)),
        Span::styled(shown, t.palette.bold(t.palette.fg).add_modifier(Modifier::UNDERLINED)),
        Span::styled(cursor.to_string(), t.palette.fg(t.palette.accent)),
    ]);
    let msg = match &input.error {
        Some(e) => Line::from(Span::styled(format!("{} {e}", t.ui.sym(3)), t.palette.fg(t.palette.bad))),
        None => Line::from(Span::styled(input.hint.clone(), t.palette.fg(t.palette.dim))),
    };
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(input.prompt.clone(), t.palette.fg(t.palette.fg))),
        Line::raw(""),
        field,
        Line::raw(""),
        msg,
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// Draw a picker centered in `area`, windowed so long lists still fit.
pub fn draw_picker(f: &mut Frame, area: Rect, t: &Theme, p: &Picker) {
    // prompt + blank + items + blank + hint, plus the panel's two borders
    let rows = (p.items.len() as u16).saturating_add(6).max(6);
    let ov = Overlay::new(&p.title).size(64, rows);
    f.render_widget(Clear, hud::centered(area, 64, rows));
    let inner = ov.render(f, area, t).content;
    let text_w = inner.width.saturating_sub(4) as usize;
    // prompt, blank, one blank above the hint, hint
    let room = inner.height.saturating_sub(4).max(1) as usize;
    let start = if p.sel >= room { p.sel + 1 - room } else { 0 };

    let mut lines = vec![
        Line::from(Span::styled(t.clip(&p.prompt, inner.width as usize), t.palette.fg(t.palette.fg))),
        Line::raw(""),
    ];
    for (i, item) in p.items.iter().enumerate().skip(start).take(room) {
        let text = t.clip(item, text_w);
        if i == p.sel {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", t.selection_mark().trim_end()), t.palette.bold(t.palette.accent2)),
                Span::styled(text, t.palette.highlight()),
            ]));
        } else {
            lines.push(Line::from(vec![Span::raw("  "), Span::styled(text, t.palette.fg(t.palette.fg))]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{} / {}   ↑↓ move   enter choose   esc cancel", p.sel + 1, p.items.len()),
        t.palette.fg(t.palette.dim),
    )));
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn types_backspaces_and_submits() {
        let mut i = Input::new("T", "P", "H");
        assert_eq!(i.on_key(key(KeyCode::Char('a'))), Outcome::None);
        assert_eq!(i.on_key(key(KeyCode::Char('b'))), Outcome::None);
        assert_eq!(i.on_key(key(KeyCode::Backspace)), Outcome::None);
        assert_eq!(i.buf, "a");
        assert_eq!(i.on_key(key(KeyCode::Enter)), Outcome::Submit("a".into()));
    }

    #[test]
    fn escape_cancels_and_submit_trims() {
        let mut i = Input::new("T", "P", "H").value("  x  ");
        assert_eq!(i.on_key(key(KeyCode::Enter)), Outcome::Submit("x".into()));
        assert_eq!(i.on_key(key(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn fuzzy_matches_substring_and_word_prefix() {
        let items = vec!["eth0 e1000e 192.168.1.42/24".into(), "wlan0 rtl8xxxu".into(), "enp0s20u1 r8152".into()];
        assert_eq!(fuzzy_matches("wlan", &items), vec![1]);
        assert_eq!(fuzzy_matches("u1", &items), vec![2]);
        assert_eq!(fuzzy_matches("", &items), Vec::<usize>::new());
        assert_eq!(fuzzy_matches("nope", &items), Vec::<usize>::new());
    }

    fn picker() -> Picker {
        Picker::new("T", "P", vec!["a".into(), "b".into(), "c".into()])
    }

    #[test]
    fn picker_moves_clamps_and_chooses() {
        let mut p = picker();
        assert_eq!(p.on_key(key(KeyCode::Up)), Pick::None);
        assert_eq!(p.sel, 0, "clamps at the top");
        assert_eq!(p.on_key(key(KeyCode::Char('j'))), Pick::None);
        assert_eq!(p.on_key(key(KeyCode::Char('j'))), Pick::None);
        assert_eq!(p.sel, 2);
        assert_eq!(p.on_key(key(KeyCode::Down)), Pick::None);
        assert_eq!(p.sel, 2, "clamps at the bottom");
        assert_eq!(p.on_key(key(KeyCode::Home)), Pick::None);
        assert_eq!(p.sel, 0);
        assert_eq!(p.on_key(key(KeyCode::End)), Pick::None);
        assert_eq!(p.on_key(key(KeyCode::Enter)), Pick::Choose(2));
    }

    #[test]
    fn picker_cancels_and_empty_is_inert() {
        let mut p = picker();
        assert_eq!(p.on_key(key(KeyCode::Esc)), Pick::Cancel);
        assert_eq!(p.on_key(key(KeyCode::Char('q'))), Pick::Cancel);
        let mut e = Picker::new("T", "P", Vec::new());
        assert_eq!(e.on_key(key(KeyCode::Enter)), Pick::None);
        assert_eq!(e.on_key(key(KeyCode::Down)), Pick::None);
    }
}
