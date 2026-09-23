//! Colours and glyphs for the TUI.
//!
//! Three colour modes:
//! - **neon**: 24-bit RGB (cyan / magenta / amber on blue-black), picked when
//!   the terminal advertises truecolor (`COLORTERM`) or `DCHECK_COLOR=truecolor`;
//! - **ansi**: the terminal's own 16-colour palette (Linux console, older
//!   terminals, `DCHECK_COLOR=ansi`);
//! - **mono**: `NO_COLOR` — no colour at all; selection uses reverse video.
//!
//! Glyphs are independent: `--plain` swaps every Unicode symbol for ASCII.

use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Neon,
    Ansi,
    Mono,
}

impl ColorMode {
    /// Resolve from the environment: `NO_COLOR` > `DCHECK_COLOR` > `COLORTERM`.
    pub fn detect() -> Self {
        Self::from_env(
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("DCHECK_COLOR").ok().as_deref(),
            std::env::var("COLORTERM").ok().as_deref(),
        )
    }

    pub fn from_env(no_color: bool, dcheck_color: Option<&str>, colorterm: Option<&str>) -> Self {
        if no_color {
            return ColorMode::Mono;
        }
        match dcheck_color.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("truecolor" | "24bit" | "rgb" | "neon") => return ColorMode::Neon,
            Some("ansi" | "16" | "basic") => return ColorMode::Ansi,
            Some("none" | "mono" | "off") => return ColorMode::Mono,
            _ => {}
        }
        match colorterm.map(|s| s.to_ascii_lowercase()).as_deref() {
            Some("truecolor" | "24bit") => ColorMode::Neon,
            _ => ColorMode::Ansi,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Palette {
    pub mode: ColorMode,
    /// Primary accent (titles, logo, focus).
    pub accent: Color,
    /// Secondary accent (logo glyphs, selection).
    pub accent2: Color,
    /// Labels and secondary text.
    pub dim: Color,
    /// Panel borders.
    pub border: Color,
    /// Empty part of a gauge.
    pub track: Color,
    pub ok: Color,
    pub warn: Color,
    pub bad: Color,
    pub fg: Color,
    /// Forced page background; `None` keeps the terminal's.
    pub bg: Option<Color>,
    /// Header bar / keycap background.
    pub bar: Option<Color>,
    /// Text drawn on top of a coloured badge.
    pub on_badge: Color,
    selection: Option<(Color, Color)>,
}

impl Palette {
    pub fn new(mode: ColorMode, light: bool, transparent: bool) -> Self {
        let mut p = match (mode, light) {
            (ColorMode::Mono, _) => Palette::mono(),
            (ColorMode::Neon, false) => Palette::neon_dark(),
            (ColorMode::Neon, true) => Palette::neon_light(),
            (ColorMode::Ansi, light) => Palette::ansi(light),
        };
        if transparent {
            p.bg = None;
        }
        p
    }

    fn neon_dark() -> Self {
        Palette {
            mode: ColorMode::Neon,
            accent: Color::Rgb(0, 229, 255),
            accent2: Color::Rgb(255, 46, 151),
            dim: Color::Rgb(112, 132, 152),
            border: Color::Rgb(28, 74, 92),
            track: Color::Rgb(34, 46, 60),
            ok: Color::Rgb(57, 255, 136),
            warn: Color::Rgb(255, 176, 0),
            bad: Color::Rgb(255, 59, 59),
            fg: Color::Rgb(196, 210, 224),
            bg: Some(Color::Rgb(10, 14, 20)),
            bar: Some(Color::Rgb(18, 26, 38)),
            on_badge: Color::Rgb(10, 14, 20),
            selection: Some((Color::Rgb(74, 14, 52), Color::Rgb(255, 255, 255))),
        }
    }

    fn neon_light() -> Self {
        Palette {
            mode: ColorMode::Neon,
            accent: Color::Rgb(0, 112, 160),
            accent2: Color::Rgb(196, 0, 112),
            dim: Color::Rgb(96, 110, 126),
            border: Color::Rgb(150, 182, 200),
            track: Color::Rgb(214, 222, 230),
            ok: Color::Rgb(0, 140, 72),
            warn: Color::Rgb(176, 104, 0),
            bad: Color::Rgb(204, 24, 24),
            fg: Color::Rgb(20, 30, 40),
            bg: Some(Color::Rgb(242, 246, 250)),
            bar: Some(Color::Rgb(222, 232, 240)),
            on_badge: Color::Rgb(255, 255, 255),
            selection: Some((Color::Rgb(255, 214, 236), Color::Rgb(20, 30, 40))),
        }
    }

    fn ansi(light: bool) -> Self {
        Palette {
            mode: ColorMode::Ansi,
            accent: if light { Color::Blue } else { Color::Cyan },
            accent2: Color::Magenta,
            dim: if light { Color::DarkGray } else { Color::Gray },
            border: if light { Color::Blue } else { Color::DarkGray },
            track: Color::DarkGray,
            ok: Color::Green,
            warn: Color::Yellow,
            bad: Color::Red,
            fg: if light { Color::Black } else { Color::Gray },
            bg: Some(if light { Color::White } else { Color::Black }),
            bar: None,
            on_badge: Color::Black,
            selection: None,
        }
    }

    fn mono() -> Self {
        Palette {
            mode: ColorMode::Mono,
            accent: Color::Reset,
            accent2: Color::Reset,
            dim: Color::Reset,
            border: Color::Reset,
            track: Color::Reset,
            ok: Color::Reset,
            warn: Color::Reset,
            bad: Color::Reset,
            fg: Color::Reset,
            bg: None,
            bar: None,
            on_badge: Color::Reset,
            selection: None,
        }
    }

    pub fn is_mono(&self) -> bool {
        self.mode == ColorMode::Mono
    }

    /// Default text style on the page background.
    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg.unwrap_or(Color::Reset))
    }

    pub fn fg(&self, color: Color) -> Style {
        Style::default().fg(color)
    }

    pub fn bold(&self, color: Color) -> Style {
        Style::default().fg(color).add_modifier(Modifier::BOLD)
    }

    /// Selected row / menu entry.
    pub fn highlight(&self) -> Style {
        match self.selection {
            Some((bg, fg)) => Style::default().bg(bg).fg(fg).add_modifier(Modifier::BOLD),
            // Reverse video stays readable on any terminal theme.
            None => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        }
    }

    /// 0 ok, 1 unknown, 2 monitor, 3 back up, 4 replace.
    pub fn severity(&self, sev: u8) -> Color {
        match sev {
            0 => self.ok,
            2 => self.warn,
            3 | 4 => self.bad,
            _ => self.dim,
        }
    }

    /// Colour for a reading where higher is worse.
    pub fn level(&self, value: f64, warn_at: f64, bad_at: f64) -> Color {
        if value >= bad_at {
            self.bad
        } else if value >= warn_at {
            self.warn
        } else {
            self.ok
        }
    }
}

/// Glyph set: Unicode HUD symbols, or ASCII with `--plain`.
#[derive(Debug, Clone, Copy)]
pub struct Ui {
    pub plain: bool,
}

const HUD_BORDER: border::Set = border::Set {
    top_left: "┌",
    top_right: "┐",
    bottom_left: "└",
    bottom_right: "┘",
    vertical_left: "│",
    vertical_right: "│",
    horizontal_top: "─",
    horizontal_bottom: "─",
};

const ASCII_BORDER: border::Set = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

const SPIN_FANCY: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPIN_PLAIN: [&str; 4] = ["|", "/", "-", "\\"];

impl Ui {
    pub fn border(self) -> border::Set {
        if self.plain {
            ASCII_BORDER
        } else {
            HUD_BORDER
        }
    }

    /// Heavy corner brackets drawn over the panel corners (HUD look).
    pub fn corners(self) -> Option<[&'static str; 4]> {
        (!self.plain).then_some(["┏", "┓", "┗", "┛"])
    }

    pub fn cursor(self) -> &'static str {
        if self.plain {
            "> "
        } else {
            "▶ "
        }
    }

    pub fn sym(self, sev: u8) -> &'static str {
        match (self.plain, sev) {
            (false, 0) => "✔",
            (false, 2) => "▲",
            (false, 3 | 4) => "✖",
            (false, _) => "?",
            (true, 0) => "+",
            (true, 2) => "!",
            (true, 3 | 4) => "X",
            (true, _) => "?",
        }
    }

    pub fn bullet(self) -> &'static str {
        if self.plain {
            "-"
        } else {
            "›"
        }
    }

    pub fn dot(self) -> &'static str {
        if self.plain {
            "|"
        } else {
            "·"
        }
    }

    /// Filled / empty gauge cells.
    pub fn gauge_cells(self, mono: bool) -> (&'static str, &'static str) {
        match (self.plain, mono) {
            (true, _) => ("#", "."),
            (false, false) => ("━", "─"),
            (false, true) => ("█", "░"),
        }
    }

    /// DIMM slot / CPU thread cells (used, free).
    pub fn slot_cells(self) -> (&'static str, &'static str) {
        if self.plain {
            ("#", ".")
        } else {
            ("■", "□")
        }
    }

    pub fn spinner(self, tick: u8) -> &'static str {
        if self.plain {
            SPIN_PLAIN[tick as usize % SPIN_PLAIN.len()]
        } else {
            SPIN_FANCY[tick as usize % SPIN_FANCY.len()]
        }
    }

    pub fn arrow(self) -> &'static str {
        if self.plain {
            ">"
        } else {
            "▸"
        }
    }

    pub fn degrees(self) -> &'static str {
        if self.plain {
            "C"
        } else {
            "°C"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_color_mode() {
        assert_eq!(ColorMode::from_env(true, Some("truecolor"), None), ColorMode::Mono);
        assert_eq!(ColorMode::from_env(false, None, Some("truecolor")), ColorMode::Neon);
        assert_eq!(ColorMode::from_env(false, None, Some("24bit")), ColorMode::Neon);
        assert_eq!(ColorMode::from_env(false, None, None), ColorMode::Ansi);
        assert_eq!(ColorMode::from_env(false, Some("ansi"), Some("truecolor")), ColorMode::Ansi);
        assert_eq!(ColorMode::from_env(false, Some("truecolor"), None), ColorMode::Neon);
    }

    #[test]
    fn level_thresholds() {
        let p = Palette::new(ColorMode::Neon, false, false);
        assert_eq!(p.level(10.0, 70.0, 90.0), p.ok);
        assert_eq!(p.level(75.0, 70.0, 90.0), p.warn);
        assert_eq!(p.level(95.0, 70.0, 90.0), p.bad);
    }

    #[test]
    fn transparent_drops_background() {
        assert!(Palette::new(ColorMode::Neon, false, true).bg.is_none());
        assert!(Palette::new(ColorMode::Neon, false, false).bg.is_some());
    }
}
