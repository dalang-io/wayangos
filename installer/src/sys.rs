//! Machine facts for the header and welcome screen, plus a checked command
//! runner for the tools the install uses.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Default)]
pub struct SysInfo {
    pub cpu: String,
    pub mem_bytes: u64,
    pub uefi: bool,
    /// (interface, IPv4/prefix) for every globally scoped address.
    pub net: Vec<(String, String)>,
}

pub fn sysinfo(demo: bool) -> SysInfo {
    if demo {
        return SysInfo {
            cpu: "Intel(R) Core(TM) i7-7700T CPU @ 2.90GHz".into(),
            mem_bytes: 16 * 1024 * 1024 * 1024,
            uefi: true,
            net: vec![("eth1".into(), "192.168.1.42/24".into())],
        };
    }
    let cpu = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .unwrap_or_else(|| "unknown CPU".into());
    let mem_bytes = fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        .unwrap_or(0);
    SysInfo {
        cpu,
        mem_bytes,
        uefi: std::path::Path::new("/sys/firmware/efi").exists(),
        net: addresses(),
    }
}

/// `ip -o -4 addr show scope global`: "2: eth1    inet 10.0.2.15/24 brd ..."
fn addresses() -> Vec<(String, String)> {
    let out = run("ip", &["-o", "-4", "addr", "show", "scope", "global"]).unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let i = w.iter().position(|x| *x == "inet")?;
            Some((w.get(1)?.to_string(), w.get(i + 1)?.to_string()))
        })
        .collect()
}

/// Run a command; stdout on success, "cmd args: stderr" on failure.
pub fn run(prog: &str, args: &[&str]) -> Result<String, String> {
    run_input(prog, args, None)
}

pub fn run_input(prog: &str, args: &[&str], input: Option<&str>) -> Result<String, String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        let _ = stdin.write_all(text.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        Err(format!(
            "{prog} {} failed{}",
            args.join(" "),
            if err.is_empty() {
                String::new()
            } else {
                format!(": {err}")
            }
        ))
    }
}

pub fn chmod(path: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

/// Decimal size like disk vendors print it: 512 GB, 2.0 TB, 480 MB.
pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e12 {
        format!("{:.1} TB", b / 1e12)
    } else if b >= 1e9 {
        format!("{:.0} GB", b / 1e9)
    } else {
        format!("{:.0} MB", b / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::human;

    #[test]
    fn sizes() {
        assert_eq!(human(512_110_190_592), "512 GB");
        assert_eq!(human(2_000_398_934_016), "2.0 TB");
        assert_eq!(human(536_870_912), "537 MB");
    }
}
