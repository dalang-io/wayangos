//! A small centered text-input modal, mirroring the installer's `Modal::Input`
//! but self-contained for the runtime HUD (different crate, same look).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use crate::hud::{self, Theme};

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

/// Draw the modal centered in `area`; `tick` drives the cursor blink.
pub fn draw(f: &mut Frame, area: Rect, t: &Theme, input: &Input, tick: usize) {
    let r = hud::centered(area, 76, 9);
    f.render_widget(Clear, r);
    let inner = hud::panel(f, r, &input.title, t);

    let room = inner.width.saturating_sub(6) as usize;
    let shown: String = if input.mask {
        "*".repeat(input.buf.chars().count().min(room))
    } else {
        let n = input.buf.chars().count();
        input.buf.chars().skip(n.saturating_sub(room)).collect()
    };
    let cursor = if tick % 2 == 0 { "_" } else { " " };
    let field = Line::from(vec![
        Span::styled(format!("{} ", t.g.cursor.trim_end()), t.bold(t.accent2)),
        Span::styled(shown, t.bold(t.fg).add_modifier(Modifier::UNDERLINED)),
        Span::styled(cursor.to_string(), t.fg(t.accent)),
    ]);
    let msg = match &input.error {
        Some(e) => Line::from(Span::styled(format!("{} {e}", t.g.bad), t.fg(t.bad))),
        None => Line::from(Span::styled(input.hint.clone(), t.fg(t.dim))),
    };
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(input.prompt.clone(), t.fg(t.fg))),
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
    let r = hud::centered(area, 64, rows);
    f.render_widget(Clear, r);
    let inner = hud::panel(f, r, &p.title, t);
    let text_w = inner.width.saturating_sub(4) as usize;
    // prompt, blank, one blank above the hint, hint
    let room = inner.height.saturating_sub(4).max(1) as usize;
    let start = if p.sel >= room { p.sel + 1 - room } else { 0 };

    let mut lines = vec![
        Line::from(Span::styled(t.clip(&p.prompt, inner.width as usize), t.fg(t.fg))),
        Line::raw(""),
    ];
    for (i, item) in p.items.iter().enumerate().skip(start).take(room) {
        let text = t.clip(item, text_w);
        if i == p.sel {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", t.g.cursor.trim_end()), t.bold(t.accent2)),
                Span::styled(text, t.highlight()),
            ]));
        } else {
            lines.push(Line::from(vec![Span::raw("  "), Span::styled(text, t.fg(t.fg))]));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!("{} / {}   ↑↓ move   enter choose   esc cancel", p.sel + 1, p.items.len()),
        t.fg(t.dim),
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
