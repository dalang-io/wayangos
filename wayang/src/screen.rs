//! Shared terminal harness for the HUD screens: raw mode, alternate screen,
//! console-palette reprogramming and a panic hook that restores the terminal.

use std::io::{self, Write};
use std::time::Duration;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::{cursor, execute};
use ratatui::{Frame, Terminal};

use crate::hud::Theme;

fn restore(console: bool) {
    let mut out = io::stdout();
    let _ = disable_raw_mode();
    let _ = execute!(out, LeaveAlternateScreen, cursor::Show);
    if console {
        let _ = write!(out, "\x1b]R\x1b[0m\x1b[2J\x1b[H");
    }
    let _ = out.flush();
}

/// Run `app` on the real terminal until `exited` returns true.
///
/// `draw` receives the frame and a tick counter (for blinking cursors);
/// `poll` runs once per loop so the app can pick up background work; `on_key`
/// handles press events only.
pub fn run<A>(
    mut app: A,
    draw: impl Fn(&mut Frame, &A, usize),
    poll: impl Fn(&mut A),
    on_key: impl Fn(&mut A, KeyEvent),
    exited: impl Fn(&A) -> bool,
) -> io::Result<()> {
    let theme = Theme::detect();
    let palette = theme.console_palette();
    let console = palette.is_some();

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore(console);
        hook(info);
    }));

    let mut out = io::stdout();
    enable_raw_mode()?;
    execute!(out, EnterAlternateScreen, cursor::Hide)?;
    if let Some(p) = &palette {
        write!(out, "{p}\x1b[2J")?;
        out.flush()?;
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;
    terminal.clear()?;

    let mut tick: usize = 0;
    let result = loop {
        poll(&mut app);
        terminal.draw(|f| draw(f, &app, tick))?;
        if exited(&app) {
            break Ok(());
        }
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    on_key(&mut app, key);
                }
            }
        }
        tick = tick.wrapping_add(1);
    };
    restore(console);
    result
}

/// Render an app to text with a `TestBackend` (snapshot/demo mode).
pub fn render_text<A>(app: &A, w: u16, h: u16, draw: impl Fn(&mut Frame, &A, usize)) -> Result<String, String> {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).map_err(|e| e.to_string())?;
    terminal.draw(|f| draw(f, app, 0)).map_err(|e| e.to_string())?;
    let buf = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..h {
        let row: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
        text.push_str(row.trim_end());
        text.push('\n');
    }
    Ok(text)
}
