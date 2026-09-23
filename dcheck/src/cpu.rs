//! CPU information (model, topology, clock, temperature, load).

#[derive(Debug, Default, Clone)]
pub struct CpuInfo {
    pub model: String,
    pub vendor: Option<String>,
    pub sockets: u32,
    pub cores: u32,
    pub threads: u32,
    pub mhz: Option<f64>,
    pub max_mhz: Option<f64>,
    pub cache_kb: Option<u64>,
    /// Hottest CPU sensor (°C).
    pub temp_c: Option<i64>,
    /// Per-socket (package) temperature sensors with their own limits.
    pub sensors: Vec<CpuSensor>,
    pub load1: Option<f64>,
    pub source: String,
}

/// One CPU temperature sensor, e.g. coretemp "Package id 1".
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CpuSensor {
    pub label: String,
    pub temp_c: i64,
    /// "High" (max) and critical limits reported by the sensor, if any.
    pub high_c: Option<i64>,
    pub crit_c: Option<i64>,
}

/// Default CPU warning temperature when the sensor reports no limit.
pub const DEFAULT_CPU_WARN_C: i64 = 85;

/// Verdict for the CPU with the reasons behind it.
#[derive(Debug, Clone)]
pub struct CpuHealth {
    pub label: &'static str,
    pub severity: u8,
    pub issues: Vec<String>,
    pub notes: Vec<String>,
}

impl CpuInfo {
    /// (verdict label, severity) — 0 ok, 2 monitor, 3 critical.
    pub fn verdict(&self) -> (&'static str, u8) {
        let h = self.health();
        (h.label, h.severity)
    }

    /// Temperatures are judged against each sensor's own limits (Intel
    /// coretemp / AMD k10temp report them), not the disk threshold:
    /// ≥ critical → CRITICAL, ≥ high (or `cpu_temp_warn_c`, or 85°C) →
    /// MONITOR; close to the limit and uneven sockets are notes.
    pub fn health(&self) -> CpuHealth {
        self.health_with(crate::config::load().cpu_temp_warn_c)
    }

    pub fn health_with(&self, warn_override: Option<i64>) -> CpuHealth {
        let mut issues = Vec::new();
        let mut notes = Vec::new();
        let mut severity = 0u8;

        let sensors: Vec<CpuSensor> = if self.sensors.is_empty() {
            self.temp_c
                .map(|t| CpuSensor {
                    label: "CPU".into(),
                    temp_c: t,
                    ..Default::default()
                })
                .into_iter()
                .collect()
        } else {
            self.sensors.clone()
        };

        for s in &sensors {
            let (limit, source) = match (warn_override, s.high_c, s.crit_c) {
                (Some(w), _, _) => (w, "config cpu_temp_warn_c"),
                (None, Some(h), _) if h > 0 => (h, "sensor high limit"),
                (None, None, Some(c)) if c > 10 => (c - 10, "10°C below the sensor's critical limit"),
                _ => (DEFAULT_CPU_WARN_C, "default"),
            };
            if let Some(crit) = s.crit_c.filter(|c| *c > 0 && s.temp_c >= *c) {
                issues.push(format!(
                    "{} at {}°C — at/over its critical limit {crit}°C (the CPU throttles or shuts down); check cooling now",
                    s.label, s.temp_c
                ));
                severity = severity.max(3);
            } else if s.temp_c >= limit {
                issues.push(format!(
                    "{} at {}°C ≥ {limit}°C ({source}) — check cooling: fans, heatsink, airflow, dust",
                    s.label, s.temp_c
                ));
                severity = severity.max(2);
            } else if limit - s.temp_c <= 5 {
                notes.push(format!(
                    "{} at {}°C, only {}°C below its {limit}°C limit",
                    s.label,
                    s.temp_c,
                    limit - s.temp_c
                ));
            }
        }

        if sensors.len() >= 2 {
            let hot = sensors.iter().max_by_key(|s| s.temp_c).unwrap();
            let cold = sensors.iter().min_by_key(|s| s.temp_c).unwrap();
            let spread = hot.temp_c - cold.temp_c;
            if spread >= 10 {
                notes.push(format!(
                    "{} runs {spread}°C hotter than {} — check that socket's heatsink and fans",
                    hot.label, cold.label
                ));
            }
        }

        let label = match severity {
            0 if sensors.is_empty() && self.model.is_empty() => {
                return CpuHealth { label: "UNKNOWN", severity: 1, issues, notes };
            }
            0 => "OK",
            2 => "MONITOR",
            _ => "CRITICAL",
        };
        CpuHealth { label, severity, issues, notes }
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
    let mut vendor = None;
    let mut threads = 0u32;
    let mut cores_per_socket = 0u32;
    let mut mhz = None;
    let mut cache_kb = None;
    let mut physical_ids: BTreeSet<String> = BTreeSet::new();

    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim();
        match k {
            "model name" if model.is_empty() => model = v.to_string(),
            "vendor_id" if vendor.is_none() => vendor = Some(v.to_string()),
            "processor" => threads += 1,
            "cpu cores" if cores_per_socket == 0 => {
                cores_per_socket = v.parse().unwrap_or(0);
            }
            "physical id" => {
                physical_ids.insert(v.to_string());
            }
            "cpu MHz" if mhz.is_none() => mhz = v.parse().ok(),
            "cache size" if cache_kb.is_none() => {
                cache_kb = v.split_whitespace().next().and_then(|n| n.parse().ok());
            }
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
        vendor,
        sockets,
        cores,
        threads,
        mhz,
        cache_kb,
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
    use std::path::{Path, PathBuf};

    use super::{parse_cpuinfo, parse_load, CpuInfo, CpuSensor};

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
        if let Ok(freq) = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq") {
            if let Ok(khz) = freq.trim().parse::<f64>() {
                info.max_mhz = Some(khz / 1000.0);
            }
        }
        info.load1 = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|t| parse_load(&t));
        info.sensors = cpu_sensors(Path::new("/sys/class/hwmon"));
        info.temp_c = info.sensors.iter().map(|s| s.temp_c).max();
        info
    }

    /// CPU temperature sensors from hwmon: one per package/socket where the
    /// driver exposes it (coretemp "Package id N", k10temp "Tctl"/"Tdie"),
    /// each with its own high / critical limits. Falls back to the first
    /// temperature of any sensor.
    pub fn cpu_sensors(hwmon: &Path) -> Vec<CpuSensor> {
        const CPU_SENSORS: [&str; 4] = ["coretemp", "k10temp", "zenpower", "cpu_thermal"];
        let milli = |p: &Path| -> Option<i64> {
            std::fs::read_to_string(p).ok()?.trim().parse::<i64>().ok().map(|m| m / 1000)
        };
        let mut dirs: Vec<_> = match std::fs::read_dir(hwmon) {
            Ok(d) => d.flatten().map(|e| e.path()).collect(),
            Err(_) => return Vec::new(),
        };
        dirs.sort();
        let mut out = Vec::new();
        let mut fallback = None;
        for dir in dirs {
            let name = std::fs::read_to_string(dir.join("name")).unwrap_or_default().trim().to_string();
            let mut inputs: Vec<(u32, PathBuf)> = std::fs::read_dir(&dir)
                .map(|d| {
                    d.flatten()
                        .filter_map(|f| {
                            let n = f.file_name().to_string_lossy().to_string();
                            let idx = n.strip_prefix("temp")?.strip_suffix("_input")?.parse().ok()?;
                            Some((idx, f.path()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            inputs.sort();
            let sensor = |idx: u32, path: &Path, label: String| -> Option<CpuSensor> {
                Some(CpuSensor {
                    label,
                    temp_c: milli(path)?,
                    high_c: milli(&dir.join(format!("temp{idx}_max"))).filter(|v| *v > 0),
                    crit_c: milli(&dir.join(format!("temp{idx}_crit"))).filter(|v| *v > 0),
                })
            };
            if !CPU_SENSORS.contains(&name.as_str()) {
                if fallback.is_none() {
                    if let Some((idx, path)) = inputs.first() {
                        fallback = sensor(*idx, path, name.clone());
                    }
                }
                continue;
            }
            let labelled: Vec<(u32, &PathBuf, String)> = inputs
                .iter()
                .map(|(idx, path)| {
                    let label = std::fs::read_to_string(dir.join(format!("temp{idx}_label")))
                        .map(|l| l.trim().to_string())
                        .unwrap_or_default();
                    (*idx, path, label)
                })
                .collect();
            // Package-level readings; per-core readings only as a fallback.
            let wanted: Vec<_> = labelled
                .iter()
                .filter(|(_, _, l)| l.starts_with("Package") || l == "Tdie" || l == "Tctl")
                .collect();
            // k10temp exposes both Tctl (offset) and Tdie; prefer Tdie.
            let has_tdie = wanted.iter().any(|(_, _, l)| l == "Tdie");
            let before = out.len();
            for (idx, path, label) in wanted {
                if has_tdie && label == "Tctl" {
                    continue;
                }
                out.extend(sensor(*idx, path, label.clone()));
            }
            if out.len() == before {
                if let Some((idx, path, label)) = labelled.first() {
                    let label = if label.is_empty() { name.clone() } else { label.clone() };
                    out.extend(sensor(*idx, path, label));
                }
            }
        }
        if out.is_empty() {
            out.extend(fallback);
        }
        out
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::process::Command;

    use super::{parse_load, CpuInfo};

    fn sysctl_str(key: &str) -> Option<String> {
        let out = Command::new("sysctl").arg("-n").arg(key).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    fn sysctl_u32(key: &str) -> Option<u32> {
        sysctl_str(key)?.parse().ok()
    }

    pub fn read() -> CpuInfo {
        let model = sysctl_str("machdep.cpu.brand_string").unwrap_or_default();
        let vendor = sysctl_str("machdep.cpu.vendor").or_else(|| {
            if model.contains("Apple") {
                Some("Apple".to_string())
            } else {
                None
            }
        });
        let mhz = sysctl_str("hw.cpufrequency")
            .and_then(|s| s.parse::<f64>().ok())
            .map(|hz| hz / 1.0e6);
        let max_mhz = sysctl_str("hw.cpufrequency_max")
            .and_then(|s| s.parse::<f64>().ok())
            .map(|hz| hz / 1.0e6);
        let load1 = sysctl_str("vm.loadavg").and_then(|s| parse_load(&s));
        CpuInfo {
            model,
            vendor,
            sockets: 1,
            cores: sysctl_u32("hw.physicalcpu").unwrap_or(0),
            threads: sysctl_u32("hw.logicalcpu").unwrap_or(0),
            mhz,
            max_mhz,
            cache_kb: None,
            temp_c: None, // not exposed on macOS
            sensors: Vec::new(),
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
            vendor: None,
            sockets: 1,
            cores: ncpu,
            threads: ncpu,
            mhz: None,
            max_mhz: None,
            cache_kb: None,
            temp_c,
            sensors: Vec::new(),
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

    fn pkg(label: &str, t: i64, high: Option<i64>, crit: Option<i64>) -> CpuSensor {
        CpuSensor { label: label.into(), temp_c: t, high_c: high, crit_c: crit }
    }

    #[test]
    fn server_cpu_at_64c_is_ok_not_monitor() {
        // 10.0.0.177: E5-2682 v4, sockets at 64 / 73°C, high 77, crit 87.
        let c = CpuInfo {
            model: "Xeon".into(),
            sensors: vec![
                pkg("Package id 0", 64, Some(77), Some(87)),
                pkg("Package id 1", 73, Some(77), Some(87)),
            ],
            ..Default::default()
        };
        let h = c.health_with(None);
        assert_eq!((h.label, h.severity), ("OK", 0));
        assert!(h.issues.is_empty());
        assert!(h.notes.iter().any(|n| n.contains("Package id 1 at 73°C, only 4°C below")), "{:?}", h.notes);
        // 9°C apart: below the 10°C spread note.
        assert!(!h.notes.iter().any(|n| n.contains("hotter")), "{:?}", h.notes);
    }

    #[test]
    fn cpu_limits_come_from_the_sensor() {
        let hot = |t| CpuInfo {
            model: "x".into(),
            sensors: vec![pkg("Package id 0", t, Some(77), Some(87)), pkg("Package id 1", 50, Some(77), Some(87))],
            ..Default::default()
        };
        let h = hot(80).health_with(None);
        assert_eq!(h.label, "MONITOR");
        assert!(h.issues[0].contains("≥ 77°C (sensor high limit)"), "{:?}", h.issues);
        assert!(h.notes.iter().any(|n| n.contains("30°C hotter")));
        assert_eq!(hot(90).health_with(None).label, "CRITICAL");
        // Override from config.
        assert_eq!(hot(70).health_with(Some(65)).label, "MONITOR");
        // No limits reported: default 85°C.
        let bare = CpuInfo { model: "x".into(), temp_c: Some(80), ..Default::default() };
        assert_eq!(bare.health_with(None).label, "OK");
        let bare = CpuInfo { model: "x".into(), temp_c: Some(86), ..Default::default() };
        assert_eq!(bare.health_with(None).label, "MONITOR");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_package_sensors_with_limits() {
        let root = std::env::temp_dir().join(format!("dcheck-hwmon-{}", std::process::id()));
        let d = root.join("hwmon1");
        std::fs::create_dir_all(&d).unwrap();
        for (f, v) in [
            ("name", "coretemp"),
            ("temp1_label", "Package id 0"), ("temp1_input", "64000"), ("temp1_max", "77000"), ("temp1_crit", "87000"),
            ("temp2_label", "Core 0"), ("temp2_input", "60000"),
            ("temp3_label", "Package id 1"), ("temp3_input", "73000"), ("temp3_max", "77000"), ("temp3_crit", "87000"),
        ] {
            std::fs::write(d.join(f), v).unwrap();
        }
        let s = linux::cpu_sensors(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(s, vec![pkg("Package id 0", 64, Some(77), Some(87)), pkg("Package id 1", 73, Some(77), Some(87))]);
    }
}