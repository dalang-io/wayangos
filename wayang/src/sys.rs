//! Tiny checked command runner for the runtime network/wifi screens.
//!
//! Mirrors `installer/src/sys.rs`: stdout on success, `prog args failed: err`
//! on failure, so callers can surface a readable message in the HUD.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Run a command; stdout on success, `cmd args failed: stderr` on failure.
pub fn run(prog: &str, args: &[&str]) -> Result<String, String> {
    run_input(prog, args, None)
}

/// Like [`run`], feeding `input` to stdin (used for `wpa_passphrase`-free PSKs).
pub fn run_input(prog: &str, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        Err(format!(
            "{prog} {} failed{}",
            args.join(" "),
            if err.is_empty() { String::new() } else { format!(": {err}") }
        ))
    }
}

/// Spawn a background daemon without waiting (e.g. renewing `udhcpc`,
/// `wpa_supplicant -B` is handled by `-B` itself).
pub fn spawn(prog: &str, args: &[&str]) -> Result<(), String> {
    Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("{prog}: {e}"))
}

/// Whether `prog` resolves to an executable on `$PATH`.
pub fn which(prog: &str) -> bool {
    which_in(prog, std::env::var("PATH").unwrap_or_default().as_str())
}

/// Pure form of [`which`] for tests.
pub fn which_in(prog: &str, path: &str) -> bool {
    if prog.contains('/') {
        return is_executable(PathBuf::from(prog));
    }
    path.split(':').filter(|p| !p.is_empty()).any(|dir| is_executable(PathBuf::from(dir).join(prog)))
}

#[cfg(unix)]
fn is_executable(p: PathBuf) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(&p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: PathBuf) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpdir() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "wayang-sys-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn which_finds_executables_on_path() {
        let d = tmpdir();
        let bin = d.join("frobnicator");
        fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = d.to_string_lossy().to_string();
        assert!(which_in("frobnicator", &path));
        assert!(!which_in("definitely-not-here", &path));
        assert!(which_in(&bin.to_string_lossy(), ""));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn run_reports_failures() {
        let err = run("sh", &["-c", "echo boom >&2; exit 3"]).unwrap_err();
        assert!(err.contains("boom"), "{err}");
        assert_eq!(run("sh", &["-c", "echo hi"]).unwrap().trim(), "hi");
    }
}
