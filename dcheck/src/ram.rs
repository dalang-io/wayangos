//! RAM information and (on Linux) ECC error counters.

#[derive(Debug, Default, Clone)]
pub struct RamInfo {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
    /// ECC correctable / uncorrectable counts (Linux EDAC; 0 when unknown).
    pub ecc_correctable: u64,
    pub ecc_uncorrectable: u64,
    pub source: String,
}

impl RamInfo {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.available_bytes)
    }

    pub fn used_percent(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            self.used_bytes() as f64 * 100.0 / self.total_bytes as f64
        }
    }

    /// (verdict label, severity) — 0 ok, 2 monitor, 3 replace.
    pub fn verdict(&self) -> (&'static str, u8) {
        if self.ecc_uncorrectable > 0 {
            ("REPLACE", 3)
        } else if self.ecc_correctable > 0 {
            ("MONITOR", 2)
        } else if self.total_bytes == 0 {
            ("UNKNOWN", 1)
        } else {
            ("OK", 0)
        }
    }
}

pub fn read() -> RamInfo {
    #[cfg(target_os = "linux")]
    {
        linux::read()
    }
    #[cfg(target_os = "macos")]
    {
        macos::read()
    }
    #[cfg(target_os = "freebsd")]
    {
        freebsd::read()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "freebsd")))]
    {
        RamInfo::default()
    }
}

/// Parse the key fields of `/proc/meminfo` (values are in kB).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_meminfo(text: &str) -> RamInfo {
    use std::collections::HashMap;
    let mut map: HashMap<&str, u64> = HashMap::new();
    for line in text.lines() {
        if let Some((k, rest)) = line.split_once(':') {
            if let Some(v) = rest.trim().split_whitespace().next() {
                if let Ok(kb) = v.parse::<u64>() {
                    map.insert(k.trim(), kb);
                }
            }
        }
    }
    let avail = map
        .get("MemAvailable")
        .copied()
        .unwrap_or_else(|| {
            map.get("MemFree").copied().unwrap_or(0)
                + map.get("Buffers").copied().unwrap_or(0)
                + map.get("Cached").copied().unwrap_or(0)
        });
    RamInfo {
        total_bytes: map.get("MemTotal").copied().unwrap_or(0) * 1024,
        available_bytes: avail * 1024,
        swap_total_bytes: map.get("SwapTotal").copied().unwrap_or(0) * 1024,
        swap_free_bytes: map.get("SwapFree").copied().unwrap_or(0) * 1024,
        source: "meminfo".to_string(),
        ..RamInfo::default()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    use super::{parse_meminfo, RamInfo};

    pub fn read() -> RamInfo {
        let mut info = std::fs::read_to_string("/proc/meminfo")
            .map(|t| parse_meminfo(&t))
            .unwrap_or_default();
        let (ce, ue) = edac_counts(Path::new("/sys/devices/system/edac/mc"));
        info.ecc_correctable = ce;
        info.ecc_uncorrectable = ue;
        info
    }

    /// Sum `ce_count`/`ue_count` under the EDAC memory-controller tree.
    fn edac_counts(dir: &Path) -> (u64, u64) {
        let mut ce = 0u64;
        let mut ue = 0u64;
        let Ok(entries) = std::fs::read_dir(dir) else {
            return (ce, ue);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let (c, u) = edac_counts(&path);
                ce += c;
                ue += u;
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                let v = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                match name {
                    "ce_count" => ce += v,
                    "ue_count" => ue += v,
                    _ => {}
                }
            }
        }
        (ce, ue)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::process::Command;

    use super::RamInfo;

    fn sysctl(key: &str) -> Option<u64> {
        let out = Command::new("sysctl").arg("-n").arg(key).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }

    pub fn read() -> RamInfo {
        let total = sysctl("hw.memsize").unwrap_or(0);
        let page = sysctl("hw.pagesize").unwrap_or(4096);
        let free = parse_vm_stat(&String::from_utf8_lossy(
            &Command::new("vm_stat")
                .output()
                .map(|o| o.stdout)
                .unwrap_or_default(),
        ))
        .map(|pages| pages * page)
        .unwrap_or(0);
        RamInfo {
            total_bytes: total,
            available_bytes: free,
            source: "sysctl".to_string(),
            ..RamInfo::default()
        }
    }

    /// Free + inactive pages from `vm_stat`.
    fn parse_vm_stat(text: &str) -> Option<u64> {
        let mut free = 0u64;
        let mut inactive = 0u64;
        let mut any = false;
        for line in text.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let n: u64 = v
                .trim()
                .trim_end_matches('.')
                .parse()
                .unwrap_or(0);
            match k.trim() {
                "Pages free" => {
                    free = n;
                    any = true;
                }
                "Pages inactive" => {
                    inactive = n;
                    any = true;
                }
                _ => {}
            }
        }
        if any {
            Some(free + inactive)
        } else {
            None
        }
    }
}

#[cfg(target_os = "freebsd")]
mod freebsd {
    use std::process::Command;

    use super::RamInfo;

    fn sysctl(key: &str) -> Option<u64> {
        let out = Command::new("sysctl").arg("-n").arg(key).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }

    pub fn read() -> RamInfo {
        let total = sysctl("hw.physmem").unwrap_or(0);
        let page = sysctl("hw.pagesize").unwrap_or(4096);
        let free = sysctl("vm.stats.vm.v_free_count").unwrap_or(0) * page;
        RamInfo {
            total_bytes: total,
            available_bytes: free,
            source: "sysctl".to_string(),
            ..RamInfo::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo() {
        let text = "MemTotal:       16000000 kB\nMemFree:         2000000 kB\nMemAvailable:    8000000 kB\nBuffers:          100000 kB\nSwapTotal:       4000000 kB\nSwapFree:        3000000 kB\n";
        let r = parse_meminfo(text);
        assert_eq!(r.total_bytes, 16_000_000 * 1024);
        assert_eq!(r.available_bytes, 8_000_000 * 1024);
        assert_eq!(r.swap_total_bytes, 4_000_000 * 1024);
        assert_eq!(r.swap_free_bytes, 3_000_000 * 1024);
        assert!((r.used_percent() - 50.0).abs() < 0.01);
    }

    #[test]
    fn ecc_errors_change_verdict() {
        let mut r = RamInfo {
            total_bytes: 1,
            available_bytes: 1,
            ..RamInfo::default()
        };
        assert_eq!(r.verdict().0, "OK");
        r.ecc_correctable = 3;
        assert_eq!(r.verdict().0, "MONITOR");
        r.ecc_uncorrectable = 1;
        assert_eq!(r.verdict().0, "REPLACE");
    }
}