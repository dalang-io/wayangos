//! Shared terminal harness for the HUD screens: raw mode, the crate's
//! OSC-11-themed alternate screen (`wayang_tui::term::TermGuard`), the
//! self-healing `Repaint` full-redraw flag, and a panic hook that restores the
//! terminal even though release builds `panic = "abort"` (so `Drop` never runs).
//!
//! Spec: `docs/TUI-UX-REVAMP.md` §5b (no-flash startup & handoff) and §5c
//! (robustness against external tty output). The clear is theme-coloured, never
//! a white flash; a repaint is forced at startup, on `SIGWINCH`, on `Ctrl-L`,
//! after returning from a child tool and on a slow tick.

use std::io;
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{Frame, Terminal};

use std::fmt::Write as _;

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use wayang_tui::term::{self, Repaint, TermGuard};

use crate::hud::Theme;

/// How often external tty output (kernel chatter, a straggler child) is
/// overwritten even with no input: §5c's slow self-healing tick.
const SLOW_TICK: Duration = Duration::from_secs(3);

/// Restore the terminal after a panic. Release builds are `panic = "abort"`, so
/// `TermGuard`'s `Drop` does not run — this hook calls [`term::leave`] itself.
fn restore(palette: &wayang_tui::theme::Palette) {
    let _ = disable_raw_mode();
    let _ = term::leave(&mut io::stdout(), palette);
}

/// Run `app` on the real terminal until `exited` returns true.
///
/// `theme` supplies the OSC-11 page background; `draw` receives the frame and a
/// tick counter (for blinking cursors); `poll` runs once per loop before the
/// frame so the app can pick up background work (including the first sample,
/// which draws the splash frame first); `on_key` handles press events only.
pub fn run<A>(
    app: A,
    theme: &Theme,
    draw: impl Fn(&mut Frame, &A, usize),
    poll: impl Fn(&mut A),
    on_key: impl Fn(&mut A, KeyEvent),
    exited: impl Fn(&A) -> bool,
) -> io::Result<()> {
    run_with_launch(app, theme, draw, poll, on_key, exited, |_| None, |_, _| {})
}

/// [`run`], plus handing the terminal to another full-screen program:
/// `take_launch` may return a command after a key press; the HUD keeps the
/// alternate screen, draws the `▸ launching …` transition, runs it in the
/// foreground, then draws `loading …` and reports the outcome through
/// `launched` (which requests a fresh first frame).
// Eight arguments are the price of keeping the runner generic over the app
// without a boxed-callback struct; the call sites stay readable.
#[allow(clippy::too_many_arguments)]
pub fn run_with_launch<A>(
    mut app: A,
    theme: &Theme,
    draw: impl Fn(&mut Frame, &A, usize),
    poll: impl Fn(&mut A),
    on_key: impl Fn(&mut A, KeyEvent),
    exited: impl Fn(&A) -> bool,
    take_launch: impl Fn(&mut A) -> Option<std::process::Command>,
    launched: impl Fn(&mut A, io::Result<std::process::ExitStatus>),
) -> io::Result<()> {
    let palette = theme.palette.clone();
    let hook = std::panic::take_hook();
    let hook_palette = palette.clone();
    std::panic::set_hook(Box::new(move |info| {
        restore(&hook_palette);
        hook(info);
    }));

    enable_raw_mode()?;
    // TermGuard themes the background (OSC 11), enters the alternate screen and
    // hides the cursor; its Drop restores them on normal exit.
    let mut guard = TermGuard::enter(io::stdout(), &palette)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut repaint = Repaint::new();
    repaint.request(); // startup: first frame is a full redraw
    let mut tick: usize = 0;
    let mut last_slow_tick = Instant::now();
    let result = loop {
        // Draw before sampling so the very first frame is the app's splash,
        // never a blank alternate screen (§5b).
        repaint.draw(&mut terminal, |f| draw(f, &app, tick))?;
        if exited(&app) {
            break Ok(());
        }
        poll(&mut app);
        if event::poll(Duration::from_millis(200))? {
            loop {
                match event::read()? {
                    Event::Resize(_, _) => repaint.request(), // SIGWINCH
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if key.code == KeyCode::Char('l')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            // Manual repaint: Ctrl-L, never passed to the app.
                            repaint.request();
                        } else {
                            on_key(&mut app, key);
                        }
                        if let Some(mut cmd) = take_launch(&mut app) {
                            let target = cmd.get_program().to_string_lossy().into_owned();
                            let _ = terminal.draw(|f| {
                                wayang_tui::transition::render(
                                    f,
                                    f.area(),
                                    "launching",
                                    &target,
                                    theme,
                                )
                            });
                            // Keep the alternate screen while the child runs so
                            // there is no flashblank between the two HUDs.
                            let res = cmd.status();
                            // A child TUI disables raw mode / leaves the alt
                            // screen on its own way out; reclaim both.
                            enable_raw_mode()?;
                            let _ = term::enter(&mut io::stdout(), &palette);
                            let _ = terminal.draw(|f| {
                                wayang_tui::transition::render(
                                    f,
                                    f.area(),
                                    "loading",
                                    &target,
                                    theme,
                                )
                            });
                            launched(&mut app, res);
                            // Return frame is a fresh first frame (§5b).
                            repaint.request();
                        }
                    }
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        if last_slow_tick.elapsed() >= SLOW_TICK {
            repaint.request();
            last_slow_tick = Instant::now();
        }
        tick = tick.wrapping_add(1);
    };
    let _ = guard.leave();
    result
}

/// Render an app to text with a `TestBackend` (snapshot/demo mode).
pub fn render_text<A>(
    app: &A,
    w: u16,
    h: u16,
    draw: impl Fn(&mut Frame, &A, usize),
) -> Result<String, String> {
    let mut terminal =
        Terminal::new(ratatui::backend::TestBackend::new(w, h)).map_err(|e| e.to_string())?;
    terminal
        .draw(|f| draw(f, app, 0))
        .map_err(|e| e.to_string())?;
    let buf = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..h {
        let row: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
        text.push_str(row.trim_end());
        text.push('\n');
    }
    Ok(text)
}

// ---- SVG snapshots (docs) ------------------------------------------------

/// Render an app to a standalone SVG (`wayang --screens DIR --svg`).
pub fn render_svg<A>(
    app: &A,
    w: u16,
    h: u16,
    draw: impl Fn(&mut Frame, &A, usize),
    t: &Theme,
) -> Result<String, String> {
    let mut terminal =
        Terminal::new(ratatui::backend::TestBackend::new(w, h)).map_err(|e| e.to_string())?;
    terminal
        .draw(|f| draw(f, app, 0))
        .map_err(|e| e.to_string())?;
    Ok(to_svg(terminal.backend().buffer(), t))
}

const CELL_W: f64 = 9.0;
const CELL_H: f64 = 18.0;
const FONT_SIZE: f64 = 15.0;

fn hex(c: Color, fallback: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Black => "#0a0e14".into(),
        Color::Red => "#ff3b3b".into(),
        Color::Green => "#39ff88".into(),
        Color::Yellow => "#ffb000".into(),
        Color::Blue => "#3a8dff".into(),
        Color::Magenta => "#ff2e97".into(),
        Color::Cyan => "#00e5ff".into(),
        Color::Gray => "#c4d2e0".into(),
        Color::DarkGray => "#4a5a6a".into(),
        Color::White => "#ffffff".into(),
        Color::Indexed(i) => format!("#{0:02x}{0:02x}{0:02x}", i),
        _ => fallback.into(),
    }
}

/// Block and box-drawing glyphs drawn as rectangles `(x, y, w, h)` in
/// pixels within the cell, so logos and borders are crisp and join up in any
/// font (fonts disagree on the height of these glyphs).
fn glyph_rects(sym: &str) -> Option<Vec<(f64, f64, f64, f64)>> {
    let (w, h) = (CELL_W, CELL_H);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let line = |t: f64| -> [(f64, f64, f64, f64); 2] {
        // [horizontal through the centre, vertical through the centre]
        [(0.0, cy - t / 2.0, w, t), (cx - t / 2.0, 0.0, t, h)]
    };
    // Corner = half horizontal + half vertical meeting at the centre.
    let corner = |t: f64, right: bool, down: bool| -> Vec<(f64, f64, f64, f64)> {
        let hx = if right { cx - t / 2.0 } else { 0.0 };
        let vy = if down { cy - t / 2.0 } else { 0.0 };
        vec![
            (hx, cy - t / 2.0, w / 2.0 + t / 2.0, t),
            (cx - t / 2.0, vy, t, h / 2.0 + t / 2.0),
        ]
    };
    let (thin, heavy) = (1.3, 2.6);
    Some(match sym {
        "█" => vec![(0.0, 0.0, w, h)],
        "▀" => vec![(0.0, 0.0, w, h / 2.0)],
        "▄" => vec![(0.0, h / 2.0, w, h / 2.0)],
        "▌" => vec![(0.0, 0.0, w / 2.0, h)],
        "▐" => vec![(w / 2.0, 0.0, w / 2.0, h)],
        "─" => vec![line(thin)[0]],
        "━" => vec![line(heavy)[0]],
        "│" => vec![line(thin)[1]],
        "┌" => corner(thin, true, true),
        "┐" => corner(thin, false, true),
        "└" => corner(thin, true, false),
        "┘" => corner(thin, false, false),
        "┏" => corner(heavy, true, true),
        "┓" => corner(heavy, false, true),
        "┗" => corner(heavy, true, false),
        "┛" => corner(heavy, false, false),
        _ => return None,
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Cell grid -> standalone SVG (copied from dcheck via wayang-fw).
fn to_svg(buf: &Buffer, t: &Theme) -> String {
    let (w, h) = (buf.area.width, buf.area.height);
    let page_bg = hex(t.palette.bg.unwrap_or(Color::Reset), "#0a0e14");
    let page_fg = hex(t.palette.fg, "#c4d2e0");
    let (px_w, px_h) = (w as f64 * CELL_W, h as f64 * CELL_H);

    let mut out = String::new();
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {px_w} {px_h}" width="{px_w}" height="{px_h}" font-family="'JetBrains Mono','DejaVu Sans Mono',Menlo,Consolas,monospace" font-size="{FONT_SIZE}">"#
    );
    let _ = write!(
        out,
        r#"<rect width="100%" height="100%" fill="{page_bg}"/>"#
    );

    // Glyph rectangles are drawn after all backgrounds of their row.
    let mut glyphs: Vec<(f64, f64, f64, f64, String)> = Vec::new();
    for y in 0..h {
        let mut x = 0u16;
        while x < w {
            let cell = &buf[(x, y)];
            let reversed = cell.modifier.contains(Modifier::REVERSED);
            let (mut fg, mut bg) = (hex(cell.fg, &page_fg), hex(cell.bg, &page_bg));
            if reversed {
                std::mem::swap(&mut fg, &mut bg);
            }
            let bold = cell.modifier.contains(Modifier::BOLD);
            // Extend the run while the style stays the same.
            let start = x;
            let mut text = String::new();
            while x < w {
                let c = &buf[(x, y)];
                let (mut f2, mut b2) = (hex(c.fg, &page_fg), hex(c.bg, &page_bg));
                if c.modifier.contains(Modifier::REVERSED) {
                    std::mem::swap(&mut f2, &mut b2);
                }
                if f2 != fg || b2 != bg || c.modifier.contains(Modifier::BOLD) != bold {
                    break;
                }
                match glyph_rects(c.symbol()) {
                    Some(rects) => {
                        for (gx, gy, gw, gh) in rects {
                            glyphs.push((
                                x as f64 * CELL_W + gx,
                                y as f64 * CELL_H + gy,
                                gw,
                                gh,
                                f2.clone(),
                            ));
                        }
                        text.push(' ');
                    }
                    None => text.push_str(c.symbol()),
                }
                x += 1;
            }
            let len = x - start;
            let (rx, ry) = (start as f64 * CELL_W, y as f64 * CELL_H);
            if bg != page_bg {
                let _ = write!(
                    out,
                    r#"<rect x="{rx}" y="{ry}" width="{}" height="{CELL_H}" fill="{bg}"/>"#,
                    len as f64 * CELL_W
                );
            }
            if !text.trim().is_empty() {
                let weight = if bold { r#" font-weight="bold""# } else { "" };
                let _ = write!(
                    out,
                    r#"<text x="{rx}" y="{}" fill="{fg}"{weight} textLength="{}" lengthAdjust="spacingAndGlyphs" xml:space="preserve">{}</text>"#,
                    ry + CELL_H * 0.76,
                    len as f64 * CELL_W,
                    escape(&text)
                );
            }
        }
    }
    for (gx, gy, gw, gh, color) in glyphs {
        // Overlap by a hair so adjacent cells join without seams.
        let _ = write!(
            out,
            r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{color}"/>"#,
            gx - 0.15,
            gy - 0.15,
            gw + 0.3,
            gh + 0.3
        );
    }
    out.push_str("</svg>\n");
    out
}
