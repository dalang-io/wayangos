//! WayangPOS kiosk service — shared by `wayang pos <action>` (CLI) and the POS
//! screen in the HUD (`tui::Sub::Pos`).
//!
//! WayangPOS is **pure userspace**: all policy lives in `/data/etc/pos.conf`
//! and the rootfs supervisor `/etc/init.d/pos`. Toggling autostart or the exit
//! policy only rewrites that file — no kernel change and no image rebuild.
//!
//! ```text
//! /data/etc/pos.conf
//!   AUTOSTART=1    start at boot when a POS binary is installed (0 = never)
//!   ALLOW_EXIT=1   admins may exit to the terminal from Settings (0 = never)
//! ```
//!
//! The supervisor prefers `/data/bin/wayang-pos` (deployed with `scp`, survives
//! OS updates) and falls back to `/usr/bin/wayang-pos` (baked into a POS image).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::paths;
use crate::sys;

/// Actions accepted by `wayang pos` and `/etc/init.d/pos`.
pub const ACTIONS: [&str; 7] =
    ["status", "start", "stop", "restart", "enable", "disable", "log"];

/// Deployed binary (survives OS updates) and image-baked fallback.
pub const BIN_DATA: &str = "/data/bin/wayang-pos";
pub const BIN_USR: &str = "/usr/bin/wayang-pos";

/// The service script: `$WAYANG_POS_SERVICE` override, else `/etc/init.d/pos`.
pub fn service_script() -> PathBuf {
    std::env::var("WAYANG_POS_SERVICE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/init.d/pos"))
}

/// `/data/etc/pos.conf` (relocated under `WAYANG_ROOT` for tests).
pub fn conf_file() -> PathBuf {
    paths::pos_conf_file()
}

/// The POS log, matching `/etc/init.d/pos`: `/data/log` when `/data` is
/// mounted, else `/var/log`.
pub fn log_file() -> PathBuf {
    if paths::root().is_some() {
        return paths::data_dir().join("log/wayang-pos.log");
    }
    let on_data = fs::read_to_string("/proc/mounts")
        .map(|m| m.lines().any(|l| l.split_whitespace().nth(1) == Some("/data")))
        .unwrap_or(false);
    if on_data {
        PathBuf::from("/data/log/wayang-pos.log")
    } else {
        PathBuf::from("/var/log/wayang-pos.log")
    }
}

/// A snapshot of the service for display and decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The binary the supervisor would run, if any.
    pub bin: Option<PathBuf>,
    pub autostart: bool,
    pub allow_exit: bool,
    pub running: bool,
    /// The supervisor process is alive (vs. a stray POS).
    pub supervised: bool,
    pub log: PathBuf,
}

impl Status {
    pub fn installed(&self) -> bool {
        self.bin.is_some()
    }

    /// Human-readable state: `running (supervised)`, `running`, or `stopped`.
    pub fn state(&self) -> &'static str {
        if !self.running {
            "stopped"
        } else if self.supervised {
            "running (supervised)"
        } else {
            "running (not supervised)"
        }
    }

    /// Human-readable exit policy.
    pub fn exit_policy(&self) -> &'static str {
        if self.allow_exit {
            "allowed (admin: Settings -> F10)"
        } else {
            "disabled"
        }
    }
}

/// Read `(autostart, allow_exit)` from `path`; missing keys default to on.
pub fn read_conf_from(path: &Path) -> (bool, bool) {
    let text = fs::read_to_string(path).unwrap_or_default();
    let (mut autostart, mut allow_exit) = (true, true);
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("AUTOSTART=") {
            autostart = v.trim() == "1";
        } else if let Some(v) = line.strip_prefix("ALLOW_EXIT=") {
            allow_exit = v.trim() == "1";
        }
    }
    (autostart, allow_exit)
}

/// Set `KEY=value` in `path`, preserving unrelated lines (mirrors the shell
/// `set_conf` in `/etc/init.d/pos`).
pub fn write_conf_key(path: &Path, key: &str, value: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let old = fs::read_to_string(path).unwrap_or_default();
    let mut out = String::new();
    for line in old.lines() {
        if !line.starts_with(&format!("{key}=")) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&format!("{key}={value}\n"));
    fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_conf() -> (bool, bool) {
    read_conf_from(&conf_file())
}

/// Persist `AUTOSTART=1|0`.
pub fn set_autostart(on: bool) -> Result<(), String> {
    write_conf_key(&conf_file(), "AUTOSTART", if on { "1" } else { "0" })
}

/// Persist `ALLOW_EXIT=1|0`.
pub fn set_allow_exit(on: bool) -> Result<(), String> {
    write_conf_key(&conf_file(), "ALLOW_EXIT", if on { "1" } else { "0" })
}

/// First executable of `data_bin`, then `usr_bin`.
pub fn find_bin_at(data_bin: &Path, usr_bin: &Path) -> Option<PathBuf> {
    for p in [data_bin, usr_bin] {
        if is_exec(p) {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// The installed binary, matching the supervisor's preference order.
pub fn find_bin() -> Option<PathBuf> {
    match paths::root() {
        Some(root) => find_bin_at(
            &root.join("data/bin/wayang-pos"),
            &root.join("usr/bin/wayang-pos"),
        ),
        None => find_bin_at(Path::new(BIN_DATA), Path::new(BIN_USR)),
    }
}

/// Probe the service without touching it (read-only).
pub fn snapshot() -> Status {
    let (autostart, allow_exit) = read_conf();
    Status {
        bin: find_bin(),
        autostart,
        allow_exit,
        running: pidof("wayang-pos"),
        supervised: supervisor_pid().is_some(),
        log: log_file(),
    }
}

/// Run a service action, streaming output to the caller's terminal (the CLI
/// path). Returns the script's exit code.
pub fn run_action(action: &str) -> Result<i32, String> {
    let script = service_script();
    let status = Command::new(&script)
        .arg(action)
        .status()
        .map_err(|e| format!("{}: {e}", script.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Run a service action capturing its output, for the HUD (which owns the
/// terminal): the last non-empty line on success, the full output on failure.
pub fn action_output(action: &str) -> Result<String, String> {
    let script = service_script();
    let out = Command::new(&script)
        .arg(action)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", script.display()))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let text = text.trim().to_string();
    if out.status.success() {
        Ok(last_line(&text, &format!("pos {action}: ok")))
    } else if text.is_empty() {
        Err(format!("pos {action} failed (exit {})", out.status.code().unwrap_or(1)))
    } else {
        Err(text)
    }
}

fn last_line(text: &str, fallback: &str) -> String {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

fn pidof(prog: &str) -> bool {
    sys::run("pidof", &[prog])
        .map(|o| !o.trim().is_empty())
        .unwrap_or(false)
}

fn supervisor_pid() -> Option<u32> {
    let path = paths::run_dir().join("wayang-pos.supervisor.pid");
    let pid: u32 = fs::read_to_string(&path).ok()?.trim().parse().ok()?;
    sys::run("kill", &["-0", &pid.to_string()]).ok()?;
    Some(pid)
}

#[cfg(unix)]
fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_exec(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmpfile() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "wayang-pos-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d.join("pos.conf")
    }

    fn executable(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn missing_conf_defaults_to_on() {
        let p = tmpfile();
        assert_eq!(read_conf_from(&p), (true, true));
    }

    #[test]
    fn conf_round_trips_and_preserves_other_keys() {
        let p = tmpfile();
        write_conf_key(&p, "AUTOSTART", "0").unwrap();
        write_conf_key(&p, "ALLOW_EXIT", "0").unwrap();
        assert_eq!(read_conf_from(&p), (false, false));
        // an unrelated key survives a later write
        fs::write(&p, "AUTOSTART=1\nSOMETHING=keep\n").unwrap();
        write_conf_key(&p, "ALLOW_EXIT", "0").unwrap();
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.contains("SOMETHING=keep"), "{text}");
        assert_eq!(read_conf_from(&p), (true, false));
    }

    #[test]
    fn last_assignment_wins_and_whitespace_is_tolerated() {
        let p = tmpfile();
        fs::write(&p, " AUTOSTART = 0\nALLOW_EXIT=0\nAUTOSTART=1\n").unwrap();
        // first line is " AUTOSTART = 0" which does not match the strict prefix
        assert_eq!(read_conf_from(&p), (true, false));
    }

    #[test]
    fn find_bin_prefers_data_then_usr() {
        let d = tmpfile().parent().unwrap().to_path_buf();
        let data = d.join("data-bin");
        let usr = d.join("usr-bin");
        assert_eq!(find_bin_at(&data, &usr), None);
        executable(&usr);
        assert_eq!(find_bin_at(&data, &usr), Some(usr.clone()));
        executable(&data);
        assert_eq!(find_bin_at(&data, &usr), Some(data));
    }

    #[test]
    fn status_helpers_render_state() {
        let s = Status {
            bin: Some(PathBuf::from("/data/bin/wayang-pos")),
            autostart: true,
            allow_exit: true,
            running: true,
            supervised: true,
            log: PathBuf::from("/data/log/wayang-pos.log"),
        };
        assert!(s.installed());
        assert_eq!(s.state(), "running (supervised)");
        assert!(s.exit_policy().contains("allowed"));
    }
}
