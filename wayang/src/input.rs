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
}
