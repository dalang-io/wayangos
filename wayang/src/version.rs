//! Reading the installed version/channel from `/etc/wayang`.

use crate::error::{AppError, Result};
use crate::paths;

pub fn read() -> Result<String> {
    let path = paths::version_file();
    let s = std::fs::read_to_string(&path).map_err(|e| AppError::err(format!("{}: {e}", path.display())))?;
    let s = s.trim().to_string();
    if s.is_empty() {
        return Err(AppError::err(format!("{}: empty version file", path.display())));
    }
    Ok(s)
}

pub fn read_channel() -> Option<String> {
    let s = std::fs::read_to_string(paths::channel_file()).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

pub fn channel_or(default: &str) -> String {
    read_channel().unwrap_or_else(|| default.to_string())
}
