//! wayang-installer — installs WayangOS onto a disk, HUD-style.
//!
//! Runs on the installer's console (booted with `wayang.install`), or over
//! SSH on the live installer once a key has been added.
//!
//!   wayang-installer              the installer
//!   wayang-installer --demo       fake disks and a simulated install
//!   wayang-installer --screens DIR [--size COLSxROWS]
//!                                 render every screen (demo data) to text

mod app;
mod disks;
mod hud;
mod install;
mod keys;
mod sha256;
mod sys;
mod views;

use std::io::{self, Write};
use std::process::{Command, ExitCode};
use std::time::Duration;

use ratatui::backend::{CrosstermBackend, TestBackend};
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::crossterm::{cursor, execute};
use ratatui::Terminal;

use app::{App, Exit};
use hud::Theme;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    if flag("--version") {
        println!("wayang-installer {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if flag("--help") || flag("-h") {
        println!("usage: wayang-installer [--demo] [--screens DIR [--size COLSxROWS]]");
        return ExitCode::SUCCESS;
    }
    if let Some(dir) = value("--screens") {
        let size = value("--size").unwrap_or_else(|| "100x32".into());
        return match dump_screens(&dir, &size) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("wayang-installer: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let demo = flag("--demo");
    match run(demo) {
        Ok(Exit::Shell) => ExitCode::SUCCESS,
        Ok(exit) if demo => {
            println!("(demo) would {:?}", exit);
            ExitCode::SUCCESS
        }
        Ok(Exit::Reboot) => power("reboot"),
        Ok(Exit::PowerOff) => power("poweroff"),
        Err(e) => {
            eprintln!("wayang-installer: {e}");
            ExitCode::FAILURE
        }
    }
}

fn power(cmd: &str) -> ExitCode {
    let _ = Command::new("sync").status();
    match Command::new(cmd).status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

/// Leave the terminal as we found it (also from the panic hook).
fn restore(console: bool) {
    let mut out = io::stdout();
    let _ = disable_raw_mode();
    let _ = execute!(out, LeaveAlternateScreen, cursor::Show);
    if console {
        // back to the default console colours, then a clean screen
        let _ = write!(out, "\x1b]R\x1b[0m\x1b[2J\x1b[H");
    }
    let _ = out.flush();
}

fn run(demo: bool) -> io::Result<Exit> {
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

    let mut app = App::new(demo, theme);
    let result = (|| -> io::Result<Exit> {
        loop {
            app.poll();
            terminal.draw(|f| views::draw(f, &app))?;
            if let Some(exit) = app.exit {
                return Ok(exit);
            }
            // spin faster while something is animating
            let wait = if app.busy() { 80 } else { 250 };
            if event::poll(Duration::from_millis(wait))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        app.on_key(key);
                    }
                }
            }
        }
    })();
    restore(console);
    result
}

fn dump_screens(dir: &str, size: &str) -> Result<(), String> {
    let (w, h) = size
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)))
        .ok_or_else(|| format!("bad --size '{size}', want COLSxROWS"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{dir}: {e}"))?;
    for (name, setup) in app::demo_states() {
        let mut app = App::new(true, Theme::detect());
        setup(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).map_err(|e| e.to_string())?;
        terminal
            .draw(|f| views::draw(f, &app))
            .map_err(|e| e.to_string())?;
        let buf = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..h {
            let row: String = (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect();
            text.push_str(row.trim_end());
            text.push('\n');
        }
        let path = format!("{dir}/{name}.txt");
        std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}
