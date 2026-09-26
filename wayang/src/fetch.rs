//! Online channel fetch via `curl` (present in the WayangOS rootfs).
//!
//! `GET <base>/<channel>/<arch>/manifest.json`
//! `GET <base>/<channel>/<arch>/wayang-<version>-<arch>.wup`

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

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

/// Stream `url` to `dest`, calling `on_progress(downloaded, total)` after every
/// chunk. `total` is 0 when the server does not advertise a length, so the
/// caller can keep the UI indeterminate.
pub fn download_with_progress<F: FnMut(u64, u64)>(url: &str, dest: &Path, mut on_progress: F) -> Result<()> {
    let total = content_length(url);
    let mut child = Command::new("curl")
        .args(["-fsSL", "-o", "-", url])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::err("curl not found; install curl or use `wayang update --from FILE.wup`")
            } else {
                AppError::err(format!("failed to run curl: {e}"))
            }
        })?;
    let mut stdout = child.stdout.take().ok_or_else(|| AppError::err("curl: no stdout"))?;
    let mut file = File::create(dest).map_err(|e| AppError::err(format!("{}: {e}", dest.display())))?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut got: u64 = 0;
    on_progress(0, total);
    loop {
        let n = stdout.read(&mut buf).map_err(|e| AppError::err(format!("curl: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| AppError::err(format!("{}: {e}", dest.display())))?;
        got += n as u64;
        on_progress(got, total);
    }
    let out = child.wait_with_output().map_err(|e| AppError::err(format!("curl: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::err(format!("curl {}: {}", url, err.trim())));
    }
    file.sync_all().map_err(|e| AppError::err(format!("{}: {e}", dest.display())))?;
    Ok(())
}

/// `Content-Length` via a HEAD request; 0 when unknown or unavailable.
fn content_length(url: &str) -> u64 {
    let out = match Command::new("curl").args(["-fsSLI", url]).output() {
        Ok(o) if o.status.success() => o,
        _ => return 0,
    };
    parse_content_length(&String::from_utf8_lossy(&out.stdout))
}

/// Last `Content-Length` header in an HTTP response (follows redirects, so the
/// final one wins); 0 when absent or unparseable.
fn parse_content_length(headers: &str) -> u64 {
    headers
        .lines()
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .filter_map(|(_, v)| v.trim().parse::<u64>().ok())
        .last()
        .unwrap_or(0)
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

    #[test]
    fn parses_content_length_header() {
        let headers = "HTTP/1.1 302 Found\r\nContent-Length: 12\r\nLocation: /x\r\n\r\n\
                       HTTP/1.1 200 OK\r\nContent-Length: 4242\r\n\r\n";
        assert_eq!(parse_content_length(headers), 4242);
        assert_eq!(parse_content_length("HTTP/1.1 200 OK\r\n\r\n"), 0);
        assert_eq!(parse_content_length("Content-Length: nope\r\n"), 0);
    }
}
