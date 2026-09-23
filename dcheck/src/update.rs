//! `dcheck update`: replace the running binary with the latest release.
//!
//! Release layout (same one `install.sh` uses):
//!
//! ```text
//! <base>/LATEST                                  e.g. "0.2.0"
//! <base>/v<ver>/dcheck-<ver>-<target>.tar.gz     binary + README
//! <base>/v<ver>/SHA256SUMS
//! ```
//!
//! Downloads go through `curl` or `wget` and the checksum through `sha256sum`
//! or `shasum` (no TLS or hashing in-tree, same as the webhook). The new binary
//! is verified (`--version`) before it atomically replaces the old one.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_BASE: &str = "https://wayang.dalang.io/dcheck";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Rust target of the release asset matching this build, if one is published.
fn release_target() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-musl")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-musl")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else {
        None
    }
}

/// `"1.2.3"` → `(1, 2, 3)`; missing parts are 0, junk is rejected.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split('.');
    let mut next = || -> Option<u64> {
        match parts.next() {
            None => Some(0),
            Some(p) => p.parse().ok(),
        }
    };
    let out = (next()?, next()?, next()?);
    if parts.next().is_some() || v.is_empty() {
        return None;
    }
    Some(out)
}

/// Checksum for `file` in a `sha256sum`-style listing.
pub fn find_checksum<'a>(sums: &'a str, file: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let name = it.next()?.trim_start_matches('*');
        (name == file && hash.len() == 64).then_some(hash)
    })
}

fn fetch_to(url: &str, dest: &Path) -> Result<(), String> {
    let dest_s = dest.to_string_lossy();
    let tries: [(&str, Vec<&str>); 2] = [
        ("curl", vec!["-fsSL", "--retry", "2", "-o", &dest_s, url]),
        ("wget", vec!["-q", "-O", &dest_s, url]),
    ];
    let mut last = String::from("neither curl nor wget is installed");
    for (cmd, args) in tries {
        match Command::new(cmd).args(&args).status() {
            Ok(s) if s.success() => return Ok(()),
            Ok(s) => last = format!("{cmd} failed ({s}) for {url}"),
            Err(_) => continue,
        }
    }
    Err(last)
}

fn fetch_text(url: &str, tmp: &Path) -> Result<String, String> {
    let path = tmp.join("fetch.txt");
    fetch_to(url, &path)?;
    fs::read_to_string(&path).map_err(|e| format!("reading {url}: {e}"))
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let tries: [(&str, &[&str]); 2] = [("sha256sum", &[]), ("shasum", &["-a", "256"])];
    for (cmd, pre) in tries {
        if let Ok(out) = Command::new(cmd).args(pre).arg(path).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(h) = s.split_whitespace().next() {
                    return Ok(h.to_ascii_lowercase());
                }
            }
        }
    }
    Err("no sha256sum/shasum available to verify the download".into())
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> Result<TempDir, String> {
    let dir = std::env::temp_dir().join(format!("dcheck-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    Ok(TempDir(dir))
}

pub fn cmd(args: &[String]) -> i32 {
    let mut check_only = false;
    let mut force = false;
    let mut base = std::env::var("DCHECK_UPDATE_URL").unwrap_or_else(|_| DEFAULT_BASE.to_string());
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--check" => check_only = true,
            "--force" => force = true,
            "--url" => {
                i += 1;
                match args.get(i) {
                    Some(u) => base = u.clone(),
                    None => {
                        eprintln!("dcheck: --url needs a value");
                        return 2;
                    }
                }
            }
            "-h" | "--help" => {
                println!(
                    "Usage: dcheck update [--check] [--force] [--url BASE]\n\n  \
                     --check   only report whether a newer release exists\n  \
                     --force   reinstall even when already up to date\n  \
                     --url     release base URL (default {DEFAULT_BASE}, env DCHECK_UPDATE_URL)"
                );
                return 0;
            }
            other => {
                eprintln!("dcheck: unknown update option '{other}' (see dcheck update --help)");
                return 2;
            }
        }
        i += 1;
    }
    match run(base.trim_end_matches('/'), check_only, force) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("dcheck: update failed: {e}");
            1
        }
    }
}

fn run(base: &str, check_only: bool, force: bool) -> Result<i32, String> {
    let tmp = temp_dir()?;
    let latest_raw = fetch_text(&format!("{base}/LATEST"), &tmp.0)?;
    let latest = latest_raw.trim().to_string();
    let (Some(want), Some(have)) = (parse_version(&latest), parse_version(VERSION)) else {
        return Err(format!("unexpected version '{latest}' at {base}/LATEST"));
    };

    let newer = want > have;
    println!("installed: {VERSION}   latest: {latest}");
    if check_only {
        println!(
            "{}",
            if newer { "an update is available — run `dcheck update`" } else { "dcheck is up to date" }
        );
        return Ok(0);
    }
    if !newer && !force {
        println!("dcheck is up to date");
        return Ok(0);
    }

    let Some(target) = release_target() else {
        return Err(
            "no prebuilt release for this platform (Linux / macOS, x86_64 / aarch64); build from source"
                .into(),
        );
    };
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("locating the running binary: {e}"))?;

    let pkg = format!("dcheck-{latest}-{target}.tar.gz");
    let url = format!("{base}/v{latest}/{pkg}");
    println!("downloading {url}");
    let tarball = tmp.0.join(&pkg);
    fetch_to(&url, &tarball)?;
    let sums = fetch_text(&format!("{base}/v{latest}/SHA256SUMS"), &tmp.0)?;
    let expected = find_checksum(&sums, &pkg).ok_or_else(|| format!("{pkg} missing from SHA256SUMS"))?;
    let actual = sha256_of(&tarball)?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!("checksum mismatch for {pkg} (expected {expected}, got {actual})"));
    }
    println!("checksum ok");

    let status = Command::new("tar")
        .arg("xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&tmp.0)
        .arg("dcheck")
        .status()
        .map_err(|e| format!("running tar: {e}"))?;
    if !status.success() {
        return Err("could not extract the release archive".into());
    }
    let new_bin = tmp.0.join("dcheck");
    let out = Command::new(&new_bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("new binary does not run: {e}"))?;
    let reported = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !reported.contains(&latest) {
        return Err(format!("new binary reports '{}', expected {latest}", reported.trim()));
    }

    replace(&new_bin, &exe)?;
    println!("updated {} → {latest}", exe.display());
    Ok(0)
}

/// Copy next to the target and rename over it (atomic on the same filesystem).
fn replace(new_bin: &Path, exe: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let staged = exe.with_file_name(".dcheck.update");
    let hint = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("no permission to write {} — run `sudo dcheck update`", exe.display())
        } else {
            format!("installing to {}: {e}", exe.display())
        }
    };
    fs::copy(new_bin, &staged).map_err(hint)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).map_err(hint)?;
    fs::rename(&staged, exe).map_err(|e| {
        let _ = fs::remove_file(&staged);
        hint(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("v1.10\n"), Some((1, 10, 0)));
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version("<html>"), None);
        assert_eq!(parse_version(""), None);
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
    }

    #[test]
    fn finds_checksum_lines() {
        let h = "a".repeat(64);
        let sums = format!("{h}  dcheck-0.2.0-x86_64-unknown-linux-musl.tar.gz\n{}  other.tar.gz\n", "b".repeat(64));
        assert_eq!(
            find_checksum(&sums, "dcheck-0.2.0-x86_64-unknown-linux-musl.tar.gz"),
            Some(h.as_str())
        );
        assert_eq!(find_checksum(&sums, "missing.tar.gz"), None);
        assert_eq!(find_checksum(&format!("{h} *x.tar.gz"), "x.tar.gz"), Some(h.as_str()));
    }
}
