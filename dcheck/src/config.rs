//! Optional user configuration: `$DCHECK_CONFIG` or
//! `~/.config/dcheck/config.json`.
//!
//! ```json
//! { "temp_warn_c": 60, "watch_interval": 60 }
//! ```

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    /// Temperature (°C) at/above which health flags a warning.
    pub temp_warn_c: i64,
    /// Default `watch` interval in seconds.
    pub watch_interval: u64,
    /// UI theme: "dark" (default) or "light".
    pub theme: Option<String>,
    /// Capture the mouse for wheel scrolling (disables native text selection).
    pub mouse: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            temp_warn_c: 60,
            watch_interval: 60,
            theme: None,
            mouse: false,
        }
    }
}

/// Resolve whether the TUI should use a light palette.
/// Order: explicit CLI flag, `DCHECK_THEME`, config, `COLORFGBG`, dark default.
pub fn resolve_light(cli: Option<bool>) -> bool {
    if let Some(light) = cli {
        return light;
    }
    if let Ok(t) = std::env::var("DCHECK_THEME") {
        match t.trim().to_ascii_lowercase().as_str() {
            "light" => return true,
            "dark" => return false,
            _ => {}
        }
    }
    if let Some(t) = load().theme.as_deref() {
        match t.trim().to_ascii_lowercase().as_str() {
            "light" => return true,
            "dark" => return false,
            _ => {}
        }
    }
    if let Ok(v) = std::env::var("COLORFGBG") {
        if let Some(light) = bg_is_light(&v) {
            return light;
        }
    }
    false
}

fn bg_is_light(colorfgbg: &str) -> Option<bool> {
    let bg = colorfgbg.rsplit(';').next()?.trim();
    bg.parse::<u8>().ok().map(|n| n == 7 || n == 15)
}

pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => parse(&text),
        Err(_) => Config::default(),
    }
}

fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DCHECK_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/dcheck/config.json"))
}

pub fn parse(text: &str) -> Config {
    let mut cfg = Config::default();
    if let Some(crate::json::Json::Obj(map)) = crate::json::Json::parse(text) {
        if let Some(v) = map.get("temp_warn_c").and_then(|v| v.as_i64()) {
            cfg.temp_warn_c = v;
        }
        if let Some(v) = map.get("watch_interval").and_then(|v| v.as_u64()) {
            if v > 0 {
                cfg.watch_interval = v;
            }
        }
        if let Some(v) = map.get("theme").and_then(|v| v.as_str()) {
            cfg.theme = Some(v.to_string());
        }
        if let Some(v) = map.get("mouse").and_then(|v| v.as_bool()) {
            cfg.mouse = v;
        }
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_config() {
        let c = parse(r#"{"temp_warn_c": 55}"#);
        assert_eq!(c.temp_warn_c, 55);
        assert_eq!(c.watch_interval, 60); // default kept
    }

    #[test]
    fn bad_input_uses_defaults() {
        let c = parse("not json");
        assert_eq!(c.temp_warn_c, 60);
        assert!(c.theme.is_none());
    }

    #[test]
    fn detects_light_background() {
        assert_eq!(bg_is_light("0;15"), Some(true)); // black on bright white
        assert_eq!(bg_is_light("15;0"), Some(false)); // white on black
        assert_eq!(bg_is_light("7"), Some(true));
        assert_eq!(bg_is_light("garbage"), None);
    }

    #[test]
    fn parses_theme() {
        assert_eq!(parse(r#"{"theme":"light"}"#).theme.as_deref(), Some("light"));
    }

    #[test]
    fn parses_mouse_flag() {
        assert!(!parse("{}").mouse);
        assert!(parse(r#"{"mouse":true}"#).mouse);
    }
}