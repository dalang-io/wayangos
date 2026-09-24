//! `dcheck snapshot DIR`: render every TUI screen to SVG files, from live
//! data on this host (or demo data), for docs and the landing page.
//!
//! Screens are drawn with the same `views::draw` as the interactive UI into
//! ratatui's in-memory `TestBackend`, then each cell grid is written as SVG
//! (background runs as `<rect>`, text runs as `<text>` pinned to the cell grid
//! with `textLength`). Serial numbers can be masked for publishing.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use super::{views, App, ColorMode, DevHealth, Palette, Screen, Ui, VerifyState};
use crate::model::Device;
use crate::report;

pub struct Options {
    pub demo: bool,
    pub light: bool,
    pub host: Option<String>,
    pub mask_serials: bool,
    pub width: u16,
    pub height: u16,
    /// Device reports to capture (paths); empty = every device with SMART.
    pub reports: Vec<String>,
    /// Device for the recovery and capacity-test screens (default: the
    /// first captured report). The capacity test is only ever shown at its
    /// plan step here; nothing is written.
    pub tools: Option<String>,
}

/// "S5GXNX0R123456" -> "S5GX••••••••••".
fn mask(serial: &str) -> String {
    let keep = serial.chars().count().min(4);
    let head: String = serial.chars().take(keep).collect();
    format!("{head}{}", "•".repeat(serial.chars().count().saturating_sub(keep).max(4)))
}

pub fn run(dir: &Path, mut devices: Vec<Device>, opts: &Options) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;

    // Every serial we might print, so report text can be scrubbed too.
    let mut secrets: Vec<String> = Vec::new();
    let mut metrics = Vec::new();
    for d in &devices {
        let m = report::device_metrics(d);
        if opts.mask_serials {
            secrets.extend(d.serial.clone());
            secrets.extend(crate::native::identity(d).serial);
            secrets.extend(m.as_ref().and_then(|(s, _)| s.serial.clone()));
        }
        metrics.push(m);
    }
    secrets.retain(|s| s.trim().len() >= 4);
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    secrets.dedup();
    let scrub = |line: &str| -> String {
        let mut out = line.to_string();
        for s in &secrets {
            out = out.replace(s.trim(), &mask(s.trim()));
        }
        out
    };
    if opts.mask_serials {
        for d in &mut devices {
            d.serial = d.serial.as_deref().map(mask);
        }
        for (s, _) in metrics.iter_mut().flatten() {
            s.serial = s.serial.as_deref().map(mask);
        }
    }

    let pal = Palette::new(ColorMode::Neon, opts.light, false);
    let mut app = App::new(devices, pal.clone(), Ui { plain: false }, opts.demo, false, false);
    if let Some(h) = &opts.host {
        app.host = h.clone();
    }
    app.health = metrics.iter().map(|m| DevHealth::from_metrics(m.as_ref())).collect();
    let ram = crate::ram::read();
    app.ram_lines = report::ram_report_lines(&ram);
    app.ram = Some(ram);
    let cpu = crate::cpu::read();
    app.cpu_lines = report::cpu_report_lines(&cpu);
    app.cpu = Some(cpu);

    let mut written = Vec::new();
    let mut shot = |app: &mut App, name: &str| -> io::Result<()> {
        let mut term = Terminal::new(TestBackend::new(opts.width, opts.height))?;
        term.draw(|f| views::draw(f, app))?;
        let svg = to_svg(term.backend().buffer(), &pal);
        let path = dir.join(format!("{name}.svg"));
        std::fs::write(&path, svg)?;
        written.push(path);
        Ok(())
    };

    app.screen = Screen::Splash;
    app.started = Instant::now() - Duration::from_millis(450);
    shot(&mut app, "01-splash")?;

    app.screen = Screen::Menu;
    for (i, name) in ["02-deck-storage", "03-deck-memory", "04-deck-processor"].iter().enumerate() {
        app.menu.select(Some(i));
        shot(&mut app, name)?;
    }
    app.menu.select(Some(0));

    app.screen = Screen::Storage;
    shot(&mut app, "05-storage")?;

    let wanted: Vec<usize> = (0..app.devices.len())
        .filter(|&i| {
            if opts.reports.is_empty() {
                metrics[i].is_some()
            } else {
                let d = &app.devices[i];
                opts.reports.iter().any(|r| *r == d.path || *r == d.name)
            }
        })
        .collect();
    let tool_dev = match &opts.tools {
        Some(t) => app.devices.iter().position(|d| d.path == *t || d.name == *t),
        None => wanted.first().copied(),
    };
    for i in wanted {
        let d = app.devices[i].clone();
        let smart = metrics[i].as_ref().map(|(s, _)| s);
        app.table.select(Some(i));
        app.report_dev = Some(i);
        app.report_lines = report::device_report_lines_with(&d, smart)
            .iter()
            .map(|l| scrub(l))
            .collect();
        app.report_metrics = metrics[i].clone();
        app.scroll = 0;
        app.screen = Screen::Report;
        shot(&mut app, &format!("06-report-{}", d.name))?;
    }

    app.scroll = 0;
    app.screen = Screen::Ram;
    shot(&mut app, "07-memory")?;
    app.screen = Screen::Cpu;
    shot(&mut app, "08-processor")?;

    app.screen = Screen::Storage;
    app.table.select(Some(0));
    app.help = true;
    shot(&mut app, "09-help")?;
    app.help = false;

    if let Some(i) = tool_dev {
        let d = app.devices[i].clone();
        app.tool_dev = Some(i);
        app.tool_back = Screen::Storage;
        // Recovery: read-only (the map samples the disk as root).
        let (g, m) = if opts.demo {
            let (g, m) = crate::recover::demo(&d);
            (Ok(g), Some(Ok(m)))
        } else {
            let g = crate::recover::gather(&d.path);
            let m = match &g {
                Ok(g) if crate::native::is_root() => Some(crate::recover::sample_map(g, 512)),
                _ => None,
            };
            (g, m)
        };
        if let Ok(g) = g {
            app.recover_lines = crate::recover::report_lines(&g).iter().map(|l| scrub(l)).collect();
            if let Some(Ok(m)) = &m {
                app.recover_lines.extend(crate::recover::share_lines(m));
            }
            app.recover = Some(g);
            app.recover_map = m;
            app.scroll = 0;
            app.screen = Screen::Recover;
            shot(&mut app, &format!("10-recover-{}", d.name))?;
        }
        // Capacity test: the plan only.
        let plan = if opts.demo { Ok(crate::verify::demo_plan(&d, false)) } else { crate::verify::plan(&d, None) };
        app.verify = VerifyState::Plan { plan, full: false };
        app.screen = Screen::Verify;
        shot(&mut app, &format!("11-verify-plan-{}", d.name))?;
        // Demo only: a simulated counterfeit drive, run to the verdict.
        if opts.demo {
            let plan = crate::verify::demo_plan(&d, true);
            let (o, _) = crate::verify::run_plan(&plan, plan.room, &mut |_, _, _| {}).map_err(io::Error::other)?;
            let (lines, code) = crate::verify::outcome_lines(&format!("{} (simulated counterfeit)", d.path), &o, true);
            app.verify_lines = lines;
            app.verify = VerifyState::Done { code };
            app.scroll = 0;
            shot(&mut app, &format!("12-verify-fake-{}", d.name))?;
        }
        app.verify = VerifyState::Idle;
        // Deleted files with the block map (demo data only: a real scan
        // would show the file names of the machine).
        if opts.demo {
            app.undel = Some(crate::undelete::demo_scan(d.size_bytes));
            app.undel_src = d.path.clone();
            app.undel_table.select(Some(2));
            app.undel_marked = [0usize, 1].into_iter().collect();
            app.screen = Screen::Undelete;
            shot(&mut app, &format!("13-undelete-{}", d.name))?;
        }
    }

    Ok(written)
}

// ─── SVG ────────────────────────────────────────────────────────────────────

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
        vec![(hx, cy - t / 2.0, w / 2.0 + t / 2.0, t), (cx - t / 2.0, vy, t, h / 2.0 + t / 2.0)]
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
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Cell grid -> standalone SVG.
pub fn to_svg(buf: &Buffer, pal: &Palette) -> String {
    let (w, h) = (buf.area.width, buf.area.height);
    let page_bg = hex(pal.bg.unwrap_or(Color::Rgb(10, 14, 20)), "#0a0e14");
    let page_fg = hex(pal.fg, "#c4d2e0");
    let (px_w, px_h) = (w as f64 * CELL_W, h as f64 * CELL_H);

    let mut out = String::new();
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {px_w} {px_h}" width="{px_w}" height="{px_h}" font-family="'JetBrains Mono','DejaVu Sans Mono',Menlo,Consolas,monospace" font-size="{FONT_SIZE}">"#
    );
    let _ = write!(out, r#"<rect width="100%" height="100%" fill="{page_bg}"/>"#);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_serials() {
        assert_eq!(mask("S5GXNX0R123456"), "S5GX••••••••••");
        assert_eq!(mask("AB"), "AB••••");
    }

    #[test]
    fn renders_demo_screens_to_svg() {
        let dir = std::env::temp_dir().join(format!("dcheck-snap-{}", std::process::id()));
        let opts = Options {
            demo: true,
            light: false,
            host: Some("dell-r630".into()),
            mask_serials: true,
            width: 120,
            height: 34,
            reports: vec![],
            tools: None,
        };
        let files = run(&dir, crate::enumerate::demo_devices(), &opts).unwrap();
        assert!(files.len() >= 9);
        let storage = std::fs::read_to_string(dir.join("05-storage.svg")).unwrap();
        assert!(storage.starts_with("<svg"));
        assert!(storage.contains("STORAGE ARRAY"));
        let report = std::fs::read_to_string(dir.join("06-report-nvme0n1.svg")).unwrap();
        assert!(!report.contains("S5GXNX0R123456"), "serial must be masked");
        assert!(report.contains("dell-r630"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
