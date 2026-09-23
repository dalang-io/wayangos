//! CPU information (model, topology, clock, temperature, load).

#[derive(Debug, Default, Clone)]
pub struct CpuInfo {
    pub model: String,
    pub sockets: u32,
    pub cores: u32,
    pub threads: u32,
    pub mhz: Option<f64>,
    pub temp_c: Option<i64>,
    pub load1: Option<f64>,
    pub source: String,
}

impl CpuInfo {
    /// (verdict label, severity) — 0 ok, 2 monitor.
    pub fn verdict(&self, temp_warn_c: i64) -> (&'static str, u8) {
        match self.temp_c {
            Some(t) if t >= temp_warn_c => ("MONITOR", 2),
            Some(_) => ("OK", 0),
            None if self.model.is_empty() => ("UNKNOWN", 1),
            None => ("OK", 0),
        }
    }
}

pub fn read() -> CpuInfo {
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
        CpuInfo::default()
    }
}

/// Parse `/proc/cpuinfo`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_cpuinfo(text: &str) -> CpuInfo {
    use std::collections::BTreeSet;
    let mut model = String::new();
    let mut threads = 0u32;
    let mut cores_per_socket = 0u32;
    let mut mhz = None;
    let mut physical_ids: BTreeSet<String> = BTreeSet::new();

    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim();
        match k {
            "model name" if model.is_empty() => model = v.to_string(),
            "processor" => threads += 1,
            "cpu cores" if cores_per_socket == 0 => {
                cores_per_socket = v.parse().unwrap_or(0);
            }
            "physical id" => {
                physical_ids.insert(v.to_string());
            }
            "cpu MHz" if mhz.is_none() => mhz = v.parse().ok(),
            _ => {}
        }
    }
    let sockets = physical_ids.len().max(1) as u32;
    let cores = if cores_per_socket > 0 {
        cores_per_socket * sockets
    } else {
        threads
    };
    CpuInfo {
        model,
        sockets,
        cores,
        threads,
        mhz,
        source: "cpuinfo".to_string(),
        ..CpuInfo::default()
    }
}

/// Parse the first load value from `/proc/loadavg` or `vm.loadavg`.
pub fn parse_load(text: &str) -> Option<f64> {
    text.trim()
        .trim_start_matches('{')
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    use super::{parse_cpuinfo, parse_load, CpuInfo};

    pub fn read() -> CpuInfo {
        let mut info = std::fs::read_to_string("/proc/cpuinfo")
            .map(|t| parse_cpuinfo(&t))
            .unwrap_or_default();
        // Prefer the live scaling frequency when available.
        if let Ok(freq) = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq") {
            if let Ok(khz) = freq.trim().parse::<f64>() {
                info.mhz = Some(khz / 1000.0);
            }
        }
        info.load1 = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|t| parse_load(&t));
        info.temp_c = cpu_temp(Path::new("/sys/class/hwmon"));
        info
    }

    /// Temperature from the first CPU-related hwmon sensor.
    fn cpu_temp(hwmon: &Path) -> Option<i64> {
        const CPU_SENSORS: [&str; 4] = ["coretemp", "k10temp", "zenpower", "cpu_thermal"];
        let mut fallback = None;
        for entry in std::fs::read_dir(hwmon).ok()?.flatten() {
            let dir = entry.path();
            let name = std::fs::read_to_string(dir.join("name"))
                .unwrap_or_default()
                .trim()
                .to_string();
            // temp1_input or the first tempN_input present.
            let mut temp = std::fs::read_to_string(dir.join("temp1_input"))
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .map(|milli| milli / 1000);
            if temp.is_none() {
                if let Ok(files) = std::fs::read_dir(&dir) {
                    for f in files.flatten() {
                        let n = f.file_name();
                        let n = n.to_string_lossy();
                        if n.starts_with("temp") && n.ends_with("_input") {
                            temp = std::fs::read_to_string(f.path())
                                .ok()
                                .and_then(|s| s.trim().parse::<i64>().ok())
                                .map(|milli| milli / 1000);
                            if temp.is_some() {
                                break;
                            }
                        }
                    }
                }
            }
            if let Some(t) = temp {
                if CPU_SENSORS.contains(&name.as_str()) {
                    return Some(t);
                }
                fallback.get_or_insert(t);
            }
        }
        fallback
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::process::Command;

    use super::{parse_load, CpuInfo};

    fn sysctl_str(key: &str) -> Option<String> {
        let out = Command::new("sysctl").arg("-n").arg(key).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn sysctl_u32(key: &str) -> Option<u32> {
        sysctl_str(key)?.parse().ok()
    }

    pub fn read() -> CpuInfo {
        let model = sysctl_str("machdep.cpu.brand_string").unwrap_or_default();
        let mhz = sysctl_str("hw.cpufrequency")
            .and_then(|s| s.parse::<f64>().ok())
            .map(|hz| hz / 1.0e6);
        let load1 = sysctl_str("vm.loadavg").and_then(|s| parse_load(&s));
        CpuInfo {
            model,
            sockets: 1,
            cores: sysctl_u32("hw.physicalcpu").unwrap_or(0),
            threads: sysctl_u32("hw.logicalcpu").unwrap_or(0),
            mhz,
            temp_c: None, // not exposed on macOS
            load1,
            source: "sysctl".to_string(),
        }
    }
}

#[cfg(target_os = "freebsd")]
mod freebsd {
    use std::process::Command;

    use super::{parse_load, CpuInfo};

    fn sysctl_str(key: &str) -> Option<String> {
        let out = Command::new("sysctl").arg("-n").arg(key).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn read() -> CpuInfo {
        let model = sysctl_str("hw.model").unwrap_or_default();
        let ncpu = sysctl_str("hw.ncpu").and_then(|s| s.parse().ok()).unwrap_or(0);
        let temp_c = sysctl_str("dev.cpu.0.temperature")
            .and_then(|s| s.trim_end_matches('C').parse::<f64>().ok())
            .map(|t| t as i64);
        let load1 = sysctl_str("vm.loadavg").and_then(|s| parse_load(&s));
        CpuInfo {
            model,
            sockets: 1,
            cores: ncpu,
            threads: ncpu,
            mhz: None,
            temp_c,
            load1,
            source: "sysctl".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cpuinfo() {
        let text = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i5\nphysical id\t: 0\ncpu cores\t: 4\ncpu MHz\t\t: 2600.000\n\nprocessor\t: 1\nmodel name\t: Intel(R) Core(TM) i5\nphysical id\t: 0\ncpu cores\t: 4\ncpu MHz\t\t: 2600.000\n";
        let c = parse_cpuinfo(text);
        assert!(c.model.contains("Intel"));
        assert_eq!(c.cores, 4);
        assert_eq!(c.threads, 2);
        assert_eq!(c.sockets, 1);
        assert_eq!(c.mhz, Some(2600.0));
    }

    #[test]
    fn parses_load_of_both_formats() {
        assert_eq!(parse_load("0.52 0.40 0.35 1/200 9999"), Some(0.52));
        assert_eq!(parse_load("{ 1.23 4.56 7.89 }"), Some(1.23));
    }

    #[test]
    fn temp_drives_verdict() {
        let mut c = CpuInfo {
            model: "x".into(),
            ..CpuInfo::default()
        };
        assert_eq!(c.verdict(60).0, "OK");
        c.temp_c = Some(75);
        assert_eq!(c.verdict(60).0, "MONITOR");
    }
}