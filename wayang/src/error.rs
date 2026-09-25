//! Error type carrying the frozen exit codes from `docs/UPDATE-DESIGN.md`:
//! `0` ok · `1` error · `2` no update · `3` verify/signature · `4` incompatible.

use std::fmt;

#[derive(Debug)]
pub struct AppError {
    pub code: i32,
    pub msg: String,
}

impl AppError {
    pub fn err(msg: impl Into<String>) -> Self {
        AppError { code: 1, msg: msg.into() }
    }
    pub fn verify(msg: impl Into<String>) -> Self {
        AppError { code: 3, msg: msg.into() }
    }
    pub fn incompatible(msg: impl Into<String>) -> Self {
        AppError { code: 4, msg: msg.into() }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
