//! ESP discovery order (frozen): `WAYANG_ESP` → `/dev/disk/by-label/WAYANGBOOT`
//! → `blkid` search for `LABEL="WAYANGBOOT"`.

use std::path::PathBuf;
use std::process::Command;

pub const BY_LABEL: &str = "/dev/disk/by-label/WAYANGBOOT";

/// Pure discovery given the environment value, whether the by-label symlink
/// exists, and the captured `blkid` output.
pub fn discover_from(env_esp: Option<String>, by_label_exists: bool, blkid: &str) -> Option<PathBuf> {
    if let Some(e) = env_esp {
        if !e.trim().is_empty() {
            return Some(PathBuf::from(e));
        }
    }
    if by_label_exists {
        return Some(PathBuf::from(BY_LABEL));
    }
    parse_blkid(blkid)
}

pub fn parse_blkid(output: &str) -> Option<PathBuf> {
    for line in output.lines() {
        let (dev, rest) = match line.split_once(':') {
            Some(v) => v,
            None => continue,
        };
        if rest.contains("LABEL=\"WAYANGBOOT\"") {
            let dev = dev.trim();
            if !dev.is_empty() {
                return Some(PathBuf::from(dev));
            }
        }
    }
    None
}

pub fn discover() -> Option<PathBuf> {
    let env_esp = std::env::var("WAYANG_ESP").ok();
    discover_from(env_esp, PathBuf::from(BY_LABEL).exists(), &run_blkid())
}

fn run_blkid() -> String {
    match Command::new("blkid").output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLKID: &str = "/dev/sda1: LABEL=\"WAYANGDATA\" UUID=\"x\"\n/dev/sda2: LABEL=\"WAYANGBOOT\" UUID=\"y\"\n";

    #[test]
    fn env_wins() {
        let got = discover_from(Some("/dev/sdz9".into()), true, BLKID);
        assert_eq!(got, Some(PathBuf::from("/dev/sdz9")));
    }

    #[test]
    fn by_label_next() {
        let got = discover_from(None, true, BLKID);
        assert_eq!(got, Some(PathBuf::from(BY_LABEL)));
    }

    #[test]
    fn blkid_fallback() {
        let got = discover_from(None, false, BLKID);
        assert_eq!(got, Some(PathBuf::from("/dev/sda2")));
    }

    #[test]
    fn none_when_absent() {
        assert_eq!(discover_from(None, false, ""), None);
        assert_eq!(discover_from(None, false, "/dev/sda1: LABEL=\"OTHER\"\n"), None);
    }

    #[test]
    fn empty_env_falls_through() {
        let got = discover_from(Some("  ".into()), false, BLKID);
        assert_eq!(got, Some(PathBuf::from("/dev/sda2")));
    }
}
