//! Online channel fetch via `curl` (present in the WayangOS rootfs).
//!
//! `GET <base>/<channel>/<arch>/manifest.json`
//! `GET <base>/<channel>/<arch>/wayang-<version>-<arch>.wup`

use std::path::Path;
use std::process::Command;

use crate::error::{AppError, Result};

pub const DEFAULT_BASE: &str = "https://wayang.dalang.io/channel";

pub fn base_url() -> String {
    std::env::var("WAYANG_REPO_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
}

pub fn join_base(base: &str, rest: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), rest)
}

pub fn manifest_url(base: &str, channel: &str, arch: &str) -> String {
    join_base(base, &format!("{channel}/{arch}/manifest.json"))
}

pub fn bundle_url(base: &str, channel: &str, arch: &str, version: &str) -> String {
    join_base(base, &format!("{channel}/{arch}/wayang-{version}-{arch}.wup"))
}

fn curl(args: &[&str]) -> Result<std::process::Output> {
    let out = Command::new("curl").args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::err("curl not found; install curl or use `wayang update --from FILE.wup`")
        } else {
            AppError::err(format!("failed to run curl: {e}"))
        }
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::err(format!("curl {}: {}", args.join(" "), err.trim())));
    }
    Ok(out)
}

pub fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    let out = curl(&["-fsSL", url])?;
    Ok(out.stdout)
}

pub fn download(url: &str, dest: &Path) -> Result<()> {
    let dest = dest.to_string_lossy().into_owned();
    curl(&["-fsSL", "-o", &dest, url])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let b = "https://example.com/dl/";
        assert_eq!(manifest_url(b, "stable", "x86_64"), "https://example.com/dl/stable/x86_64/manifest.json");
        assert_eq!(
            bundle_url(b, "stable", "x86_64", "1.4.1"),
            "https://example.com/dl/stable/x86_64/wayang-1.4.1-x86_64.wup"
        );
    }

    #[test]
    fn default_base_used_without_env() {
        if std::env::var("WAYANG_REPO_URL").is_err() {
            assert_eq!(base_url(), DEFAULT_BASE);
        }
    }
}
