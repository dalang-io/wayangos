//! Look & feel shared with the installer's HUD: neon palette, bracket panels,
//! keycaps, badges and the pixel logo (see `installer/src/hud.rs`).
//!
//! Only the pieces the updater HUD uses are kept here.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 24-bit colour (COLORTERM=truecolor), dcheck's neon_dark values.
    Neon,
    /// Linux console with the palette re-programmed to neon.
    Console,
    /// The terminal's own 16 colours.
    Ansi,
    /// NO_COLOR.
    Mono,
}

pub struct Glyphs {
    pub border: border::Set,
    pub corners: Option<[&'static str; 4]>,
    pub title: (&'static str, &'static str),
    pub cursor: &'static str,
    pub ok: &'static str,
    pub warn: &'static str,
    pub bad: &'static str,
    pub brand: &'static str,
    pub ellipsis: char,
}

const THIN: border::Set = border::Set {
    top_left: "┌",
    top_right: "┐",
    bottom_left: "└",
    bottom_right: "┘",
    vertical_left: "│",
    vertical_right: "│",
    horizontal_top: "─",
    horizontal_bottom: "─",
};

/// Same symbols as dcheck's Unicode HUD.
pub const FANCY: Glyphs = Glyphs {
    border: THIN,
    corners: Some(["┏", "┓", "┗", "┛"]),
    title: ("◢ ", " ◣"),
    cursor: "▶ ",
    ok: "✔",
    warn: "▲",
    bad: "✖",
    brand: "▰",
    ellipsis: '…',
};

/// cp437-only symbols for the kernel console font.
pub const CONSOLE: Glyphs = Glyphs {
    border: THIN,
    corners: None,
    title: ("▐ ", " ▌"),
    cursor: "► ",
    ok: "√",
    warn: "▲",
    bad: "x",
    brand: "■",
    ellipsis: '~',
};

pub struct Theme {
    pub mode: Mode,
    pub g: &'static Glyphs,
    pub accent: Color,
    pub accent2: Color,
    pub dim: Color,
    pub border: Color,
    pub ok: Color,
    pub warn: Color,
    pub bad: Color,
    pub fg: Color,
    pub bg: Color,
    /// Header bar / keycap background (`None`: bracket keycaps instead).
    pub bar: Option<Color>,
    /// Selected row (`None`: reverse video).
    pub sel: Option<(Color, Color)>,
    pub on_badge: Color,
}

/// Neon values: (console slot, RGB), slots 0-7 double as backgrounds.
const CONSOLE_PALETTE: [(u8, (u8, u8, u8)); 16] = [
    (0, (10, 14, 20)),
    (1, (255, 59, 59)),
    (2, (57, 255, 136)),
    (3, (255, 176, 0)),
    (4, (28, 74, 92)),
    (5, (74, 14, 52)),
    (6, (0, 229, 255)),
    (7, (196, 210, 224)),
    (8, (112, 132, 152)),
    (9, (255, 59, 59)),
    (10, (57, 255, 136)),
    (11, (255, 176, 0)),
    (12, (0, 160, 200)),
    (13, (255, 46, 151)),
    (14, (0, 229, 255)),
    (15, (255, 255, 255)),
];

impl Theme {
    pub fn detect() -> Theme {
        let env = |k: &str| std::env::var(k).ok().unwrap_or_default().to_ascii_lowercase();
        let console = matches!(env("TERM").as_str(), "" | "linux");
        let mode = if std::env::var_os("NO_COLOR").is_some() {
            Mode::Mono
        } else if matches!(env("COLORTERM").as_str(), "truecolor" | "24bit") {
            Mode::Neon
        } else if console {
            Mode::Console
        } else {
            Mode::Ansi
        };
        Theme::new(mode, if console { &CONSOLE } else { &FANCY })
    }

    pub fn new(mode: Mode, g: &'static Glyphs) -> Theme {
        match mode {
            Mode::Neon => Theme {
                mode,
                g,
                accent: Color::Rgb(0, 229, 255),
                accent2: Color::Rgb(255, 46, 151),
                dim: Color::Rgb(112, 132, 152),
                border: Color::Rgb(28, 74, 92),
                ok: Color::Rgb(57, 255, 136),
                warn: Color::Rgb(255, 176, 0),
                bad: Color::Rgb(255, 59, 59),
                fg: Color::Rgb(196, 210, 224),
                bg: Color::Rgb(10, 14, 20),
                bar: Some(Color::Rgb(18, 26, 38)),
                sel: Some((Color::Rgb(74, 14, 52), Color::Rgb(255, 255, 255))),
                on_badge: Color::Rgb(10, 14, 20),
            },
            Mode::Console => Theme {
                mode,
                g,
                accent: Color::Cyan,
                accent2: Color::LightMagenta,
                dim: Color::DarkGray,
                border: Color::Blue,
                ok: Color::Green,
                warn: Color::Yellow,
                bad: Color::Red,
                fg: Color::Gray,
                bg: Color::Black,
                bar: Some(Color::Blue),
                sel: Some((Color::Magenta, Color::White)),
                on_badge: Color::Black,
            },
            Mode::Ansi => Theme {
                mode,
                g,
                accent: Color::Cyan,
                accent2: Color::Magenta,
                dim: Color::DarkGray,
                border: Color::DarkGray,
                ok: Color::Green,
                warn: Color::Yellow,
                bad: Color::Red,
                fg: Color::Reset,
                bg: Color::Reset,
                bar: None,
                sel: None,
                on_badge: Color::Black,
            },
            Mode::Mono => Theme {
                mode,
                g,
                accent: Color::Reset,
                accent2: Color::Reset,
                dim: Color::Reset,
                border: Color::Reset,
                ok: Color::Reset,
                warn: Color::Reset,
                bad: Color::Reset,
                fg: Color::Reset,
                bg: Color::Reset,
                bar: None,
                sel: None,
                on_badge: Color::Reset,
            },
        }
    }

    /// Escape sequence re-programming the console palette (Console mode only).
    pub fn console_palette(&self) -> Option<String> {
        (self.mode == Mode::Console).then(|| {
            CONSOLE_PALETTE
                .iter()
                .map(|(i, (r, g, b))| format!("\x1b]P{i:x}{r:02x}{g:02x}{b:02x}"))
                .collect()
        })
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }

    pub fn fg(&self, c: Color) -> Style {
        Style::default().fg(c)
    }

    pub fn bold(&self, c: Color) -> Style {
        Style::default().fg(c).add_modifier(Modifier::BOLD)
    }

    pub fn highlight(&self) -> Style {
        match self.sel {
            Some((bg, fg)) => Style::default().bg(bg).fg(fg).add_modifier(Modifier::BOLD),
            None => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        }
    }

    pub fn clip(&self, s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            s.to_string()
        } else if max == 0 {
            String::new()
        } else {
            let mut out: String = s.chars().take(max - 1).collect();
            out.push(self.g.ellipsis);
            out
        }
    }
}

/// HUD panel: thin frame, accent corners, `◢ TITLE ◣`. Returns the inner area.
pub fn panel(f: &mut Frame, area: Rect, title: &str, t: &Theme) -> Rect {
    let (l, r) = t.g.title;
    let title_line = Line::from(vec![
        Span::styled(l, t.fg(t.accent2)),
        Span::styled(title.to_string(), t.bold(t.accent)),
        Span::styled(r, t.fg(t.accent2)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(t.g.border)
        .border_style(t.fg(t.border))
        .style(t.base())
        .title(title_line);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if area.width >= 2 && area.height >= 2 {
        let corners = t.g.corners.unwrap_or(["┌", "┐", "└", "┘"]);
        let (x1, y1) = (area.right() - 1, area.bottom() - 1);
        let buf = f.buffer_mut();
        for (x, y, sym) in [
            (area.x, area.y, corners[0]),
            (x1, area.y, corners[1]),
            (area.x, y1, corners[2]),
            (x1, y1, corners[3]),
        ] {
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_symbol(sym).set_style(t.fg(t.accent));
            }
        }
    }
    inner
}

/// Footer key hints: keycaps followed by dim labels.
pub fn keycaps(items: &[(&str, &str)], t: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for (key, label) in items {
        match t.bar {
            Some(bg) => spans.push(Span::styled(
                format!(" {key} "),
                Style::default().bg(bg).fg(t.accent).add_modifier(Modifier::BOLD),
            )),
            None => spans.push(Span::styled(format!("[{key}]"), t.bold(t.accent))),
        }
        spans.push(Span::styled(format!(" {label}  "), t.fg(t.dim)));
    }
    Line::from(spans)
}

/// Coloured status chip, e.g. ` ✔ OK ` on green.
pub fn badge(color: Color, sym: &str, label: &str, t: &Theme) -> Span<'static> {
    if t.mode == Mode::Mono {
        return Span::styled(
            format!("[{sym} {label}]"),
            Style::default().add_modifier(Modifier::BOLD),
        );
    }
    Span::styled(
        format!(" {sym} {label} "),
        Style::default().bg(color).fg(t.on_badge).add_modifier(Modifier::BOLD),
    )
}

/// `LABEL     value` row.
pub fn field(label: &str, w: usize, value: Vec<Span<'static>>, t: &Theme) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{label:<w$}"), t.fg(t.dim))];
    spans.extend(value);
    Line::from(spans)
}

// ---- pixel logo --------------------------------------------------------

/// 4-pixel-high letters; drawn two pixel rows per text row with half blocks.
const FONT: [(char, [&str; 4]); 8] = [
    ('W', ["X...X", "X.X.X", "X.X.X", ".X.X."]),
    ('A', [".X.", "X.X", "XXX", "X.X"]),
    ('Y', ["X.X", ".X.", ".X.", ".X."]),
    ('N', ["X..X", "XX.X", "X.XX", "X..X"]),
    ('G', ["XXX", "X..", "X.X", "XXX"]),
    ('O', ["XXX", "X.X", "X.X", "XXX"]),
    ('S', ["XXX", "X..", "..X", "XXX"]),
    (' ', [".", ".", ".", "."]),
];

/// "WAYANG OS" in half-block pixels, `scale`× (1 → 2 rows, 2 → 4 rows).
pub fn logo(scale: usize, t: &Theme) -> Vec<Line<'static>> {
    let mut px: Vec<Vec<bool>> = vec![Vec::new(); 4];
    for (i, ch) in "WAYANG OS".chars().enumerate() {
        let glyph = FONT.iter().find(|(c, _)| *c == ch).map(|(_, g)| g).unwrap();
        for (row, bits) in glyph.iter().enumerate() {
            if i > 0 {
                px[row].push(false);
            }
            px[row].extend(bits.chars().map(|c| c == 'X'));
        }
    }
    let rows: Vec<Vec<bool>> = px
        .iter()
        .flat_map(|r| {
            let wide: Vec<bool> = r.iter().flat_map(|&b| vec![b; scale]).collect();
            vec![wide; scale]
        })
        .collect();
    let n = rows.len() / 2;
    (0..n)
        .map(|i| {
            let (top, bot) = (&rows[2 * i], &rows[2 * i + 1]);
            let s: String = top
                .iter()
                .zip(bot)
                .map(|(&a, &b)| match (a, b) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect();
            let color = if i < n / 2 { t.accent } else { t.accent2 };
            Line::from(Span::styled(s, t.bold(color)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_rows() {
        let t = Theme::new(Mode::Mono, &CONSOLE);
        let small: Vec<String> = logo(1, &t).iter().map(|l| l.to_string()).collect();
        assert_eq!(small.len(), 2);
        assert!(small[0].contains('█') || small[0].contains('▀'));
    }

    #[test]
    fn console_palette_escapes() {
        let p = Theme::new(Mode::Console, &CONSOLE).console_palette().unwrap();
        assert!(p.starts_with("\x1b]P00a0e14"));
        assert!(Theme::new(Mode::Neon, &FANCY).console_palette().is_none());
    }
}
