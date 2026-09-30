//! The REVIEW modal: show the exact effect/plan lines before a mutating action
//! runs, with `enter`/`y` = confirm and `esc`/`n`/`q` = cancel. One renderer,
//! used by every apply path (updates, reset, network, wifi, ssh).

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::hud::{self, Theme};
use wayang_tui::overlay::Overlay;

/// A pending mutation: what it will change, awaiting a deliberate confirm.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Review {
    pub title: String,
    /// The exact plan lines (what will change / what will run).
    pub effect: Vec<String>,
    /// An extra warning shown at the bottom (e.g. a destructive side effect).
    pub note: Option<String>,
    pub scroll: usize,
}

/// What a key did to the review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Still reviewing.
    None,
    /// `enter` / `y`: run the action.
    Confirm,
    /// `esc` / `n` / `q`: discard, nothing changes.
    Cancel,
}

impl Review {
    pub fn new(title: impl Into<String>, effect: Vec<String>) -> Review {
        Review { title: title.into(), effect, note: None, scroll: 0 }
    }

    pub fn note(mut self, note: impl Into<String>) -> Review {
        self.note = Some(note.into());
        self
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Decision {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') => Decision::Confirm,
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => Decision::Cancel,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                Decision::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                Decision::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(5);
                Decision::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(5);
                Decision::None
            }
            KeyCode::Home => {
                self.scroll = 0;
                Decision::None
            }
            _ => Decision::None,
        }
    }
}

/// Draw the review centered in `area`. Starts with a `REVIEW` marker so users
/// learn to look for it, then the plan lines, then the confirm footer. The
/// chrome (focused panel + keycap footer) is the shared modal frame
/// [`Overlay`].
pub fn draw(f: &mut Frame, area: Rect, t: &Theme, r: &Review) {
    let content = 1 + r.effect.len() + if r.note.is_some() { 2 } else { 0 };
    let height = (content as u16 + 3).clamp(8, 22);
    let title = format!("REVIEW — {}", r.title);
    let ov = Overlay::new(&title)
        .keys(vec![
            ("enter", "confirm"),
            ("esc", "cancel"),
            ("↑↓", "scroll"),
        ])
        .size(78, height);
    // The modal replaces whatever the deck drew underneath it.
    f.render_widget(Clear, hud::centered(area, 78, height));
    let body = ov.render(f, area, t).content;

    let room = body.height as usize;
    let start = r.scroll.min(r.effect.len().saturating_sub(1));
    let end = (start + room.saturating_sub(2)).min(r.effect.len());
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "This is what will run — nothing has changed yet.",
        t.palette.fg(t.palette.dim),
    ))];
    for e in &r.effect[start..end] {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", t.ui.arrow()), t.palette.fg(t.palette.accent2)),
            Span::styled(e.clone(), t.palette.fg(t.palette.fg)),
        ]));
    }
    if let Some(note) = &r.note {
        if lines.len() < room {
            lines.push(Line::from(""));
        }
        if lines.len() < room {
            lines.push(Line::from(Span::styled(note.clone(), t.palette.bold(t.palette.bad))));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::from(code)
    }

    #[test]
    fn enter_confirms_escape_cancels() {
        let mut r = Review::new("update", vec!["download 1.5.0".into()]);
        assert_eq!(r.on_key(key(KeyCode::Char('j'))), Decision::None);
        assert_eq!(r.scroll, 1);
        assert_eq!(r.on_key(key(KeyCode::Char('y'))), Decision::Confirm);
        assert_eq!(r.on_key(key(KeyCode::Esc)), Decision::Cancel);
        assert_eq!(r.on_key(key(KeyCode::Char('n'))), Decision::Cancel);
        assert_eq!(r.on_key(key(KeyCode::Enter)), Decision::Confirm);
    }

    #[test]
    fn renders_the_plan_lines() {
        let t = Theme::from_env(hud::WAYANG_OS, hud::Flags::default(), true, None, None);
        let r = Review::new("reset", vec!["remove /data/etc/router".into(), "clear pending update".into()])
            .note("SSH keys are kept.");
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, f.area(), &t, &r)).unwrap();
        let buf = term.backend().buffer();
        let text: String = (0..30)
            .flat_map(|y| (0..100).map(move |x| buf[(x, y)].symbol().to_string()))
            .collect();
        assert!(text.contains("REVIEW"));
        assert!(text.contains("remove /data/etc/router"));
        assert!(text.contains("SSH keys are kept."));
        assert!(text.contains("confirm"));
    }
}
