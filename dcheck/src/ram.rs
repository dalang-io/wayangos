//! RAM information: usage, ECC counters, and (via SMBIOS) module details.

#[derive(Debug, Default, Clone)]
pub struct RamModule {
    pub locator: String,
    pub size_bytes: u64,
    pub kind: String, // DDR4, DDR5, ...
    pub speed_mts: Option<u32>,
    pub configured_mts: Option<u32>,
    pub manufacturer: Option<String>,
    pub part_number: Option<String>,
    pub serial: Option<String>,
    pub rank: Option<u32>,
}

#[derive(Debug, Default, Clone)]
pub struct RamInfo {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
    /// ECC correctable / uncorrectable counts (Linux EDAC; 0 when unknown).
    pub ecc_correctable: u64,
    pub ecc_uncorrectable: u64,
    /// DIMM modules from SMBIOS (Linux/macOS; empty when unavailable).
    pub modules: Vec<RamModule>,
    /// Total DIMM slots (populated + empty).
    pub slots_total: u32,
    /// On-DIMM temperature °C (DDR5 SPD5118 / JC42 sensors), when available.
    pub ram_temp_c: Option<i64>,
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

    /// Dominant memory type ("DDR4", "DDR5", ...), when known.
    pub fn memory_type(&self) -> Option<String> {
        self.modules
            .iter()
            .find(|m| !m.kind.is_empty() && m.kind != "Unknown")
            .map(|m| m.kind.clone())
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

/// Parse `dmidecode -t 17` (Memory Device) into modules + total slot count.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_dmidecode17(text: &str) -> (Vec<RamModule>, u32) {
    let mut modules = Vec::new();
    let mut total = 0u32;
    for block in text.split("\n\n") {
        if !block.contains("DMI type 17") {
            continue;
        }
        total += 1;
        let mut m = RamModule::default();
        let mut populated = false;
        for line in block.lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let k = k.trim();
            let v = v.trim();
            match k {
                "Size" => {
                    if v.contains("No Module") {
                        populated = false;
                    } else if let Some(bytes) = parse_dmi_size(v) {
                        m.size_bytes = bytes;
                        populated = true;
                    }
                }
                "Locator" => m.locator = v.to_string(),
                "Type" if m.kind.is_empty() => m.kind = v.to_string(),
                "Speed" => m.speed_mts = parse_mts(v),
                "Configured Memory Speed" => m.configured_mts = parse_mts(v),
                "Manufacturer" => m.manufacturer = nonempty(v),
                "Part Number" => m.part_number = nonempty(v),
                "Serial Number" => m.serial = nonempty(v),
                "Rank" => m.rank = v.parse().ok(),
                _ => {}
            }
        }
        if populated {
            if let Some(mf) = m.manufacturer.clone() {
                m.manufacturer = Some(infer_vendor(&mf, m.part_number.as_deref()));
            }
            modules.push(m);
        }
    }
    (modules, total)
}

fn parse_dmi_size(v: &str) -> Option<u64> {
    let mut it = v.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    let unit = it.next().unwrap_or("").to_ascii_uppercase();
    let mult: u64 = match unit.as_str() {
        // dmidecode prints both "GB" and "GiB".
        "KB" | "KIB" => 1024,
        "MB" | "MIB" => 1024 * 1024,
        "GB" | "GIB" => 1024 * 1024 * 1024,
        "TB" | "TIB" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };
    Some(n * mult)
}

fn parse_mts(v: &str) -> Option<u32> {
    v.split_whitespace().next()?.parse().ok()
}

fn nonempty(v: &str) -> Option<String> {
    let t = v.trim();
    if t.is_empty() || t == "Unknown" || t == "Not Specified" || t == "[Empty]" {
        None
    } else {
        Some(t.to_string())
    }
}

/// Best-effort vendor from a JEDEC-hex manufacturer code and/or part number.
fn infer_vendor(manufacturer: &str, part: Option<&str>) -> String {
    let hexish = manufacturer.len() >= 8 && manufacturer.chars().all(|c| c.is_ascii_hexdigit());
    if let Some(p) = part {
        let u = p.to_ascii_uppercase();
        for (prefix, name) in [
            ("HMA", "SK Hynix"),
            ("HMT", "SK Hynix"),
            ("MTA", "Micron"),
            ("CT", "Crucial"),
            ("KHX", "Kingston"),
            ("KVR", "Kingston"),
            ("KSM", "Kingston"),
            ("M378", "Samsung"),
            ("M471", "Samsung"),
        ] {
            if u.starts_with(prefix) {
                return name.to_string();
            }
        }
    }
    if hexish {
        let m = manufacturer.to_ascii_uppercase();
        for (code, name) in [
            ("2C", "Micron"),
            ("AD", "SK Hynix"),
            ("CE", "Samsung"),
            ("98", "Kingston"),
        ] {
            if m.contains(code) {
                return name.to_string();
            }
        }
    }
    manufacturer.to_string()
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::process::Command;

    use super::{parse_dmidecode17, parse_meminfo, RamInfo};

    pub fn read() -> RamInfo {
        let mut info = std::fs::read_to_string("/proc/meminfo")
            .map(|t| parse_meminfo(&t))
            .unwrap_or_default();
        let (ce, ue) = edac_counts(Path::new("/sys/devices/system/edac/mc"));
        info.ecc_correctable = ce;
        info.ecc_uncorrectable = ue;
        info.ram_temp_c = ram_temp(Path::new("/sys/class/hwmon"));

        if let Ok(out) = Command::new("dmidecode").arg("-t").arg("17").output() {
            if out.status.success() {
                let (modules, slots) =
                    parse_dmidecode17(&String::from_utf8_lossy(&out.stdout));
                info.modules = modules;
                info.slots_total = slots;
            }
        }
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

    /// On-DIMM temperature from a DDR5 SPD5118 / JC42 hwmon sensor.
    fn ram_temp(hwmon: &Path) -> Option<i64> {
        const SENSORS: [&str; 2] = ["spd5118", "jc42"];
        for entry in std::fs::read_dir(hwmon).ok()?.flatten() {
            let dir = entry.path();
            let name = std::fs::read_to_string(dir.join("name"))
                .unwrap_or_default()
                .trim()
                .to_string();
            if SENSORS.contains(&name.as_str()) {
                if let Some(milli) = std::fs::read_to_string(dir.join("temp1_input"))
                    .ok()
                    .and_then(|s| s.trim().parse::<i64>().ok())
                {
                    return Some(milli / 1000);
                }
            }
        }
        None
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::process::Command;

    use super::{infer_vendor, RamInfo, RamModule};

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
        let modules = system_profiler_memory();
        RamInfo {
            total_bytes: total,
            available_bytes: free,
            modules,
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
            let n: u64 = v.trim().trim_end_matches('.').parse().unwrap_or(0);
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

    /// DIMM details from `system_profiler SPMemoryDataType` (Intel Macs only).
    fn system_profiler_memory() -> Vec<RamModule> {
        let Ok(out) = Command::new("system_profiler")
            .args(["SPMemoryDataType", "-json"])
            .output()
        else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        let Some(crate::json::Json::Obj(root)) =
            crate::json::Json::parse(&String::from_utf8_lossy(&out.stdout))
        else {
            return Vec::new();
        };
        let Some(crate::json::Json::Arr(items)) = root.get("SPMemoryDataType") else {
            return Vec::new();
        };
        let mut modules = Vec::new();
        for item in items {
            let Some(crate::json::Json::Arr(dimms)) = item.get("_items") else {
                continue;
            };
            for d in dimms {
                let get = |k: &str| d.get(k).and_then(|v| v.as_str()).map(str::to_string);
                let size = get("dimm_size")
                    .or_else(|| get("size"))
                    .and_then(|s| parse_size(&s))
                    .unwrap_or(0);
                let mf = get("dimm_manufacturer").or_else(|| get("manufacturer"));
                let part = get("dimm_part_number").or_else(|| get("part_number"));
                modules.push(RamModule {
                    locator: get("_name").unwrap_or_default(),
                    size_bytes: size,
                    kind: get("dimm_type").or_else(|| get("type")).unwrap_or_default(),
                    speed_mts: get("dimm_speed")
                        .or_else(|| get("speed"))
                        .and_then(|s| s.split_whitespace().next().unwrap_or("").parse().ok()),
                    manufacturer: mf.map(|m| infer_vendor(&m, part.as_deref())),
                    part_number: part,
                    serial: get("dimm_serial_number").or_else(|| get("serial_number")),
                    configured_mts: None,
                    rank: None,
                });
            }
        }
        modules
    }

    fn parse_size(s: &str) -> Option<u64> {
        let mut it = s.split_whitespace();
        let n: u64 = it.next()?.parse().ok()?;
        Some(match it.next().unwrap_or("").to_ascii_uppercase().as_str() {
            "GB" => n * 1024 * 1024 * 1024,
            "MB" => n * 1024 * 1024,
            _ => return None,
        })
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
        assert!((r.used_percent() - 50.0).abs() < 0.01);
    }

    #[test]
    fn parses_dmidecode_memory() {
        let text = "Handle 0x0001, DMI type 17, 84 bytes\nMemory Device\n\tTotal Width: 72 bits\n\tSize: 32 GiB\n\tLocator: A1\n\tType: DDR4\n\tSpeed: 2133 MT/s\n\tManufacturer: Hynix Semiconductor\n\tPart Number: HMA84GL7MMR4N-TF\n\tRank: 4\n\tConfigured Memory Speed: 2133 MT/s\n\nHandle 0x0002, DMI type 17, 84 bytes\nMemory Device\n\tSize: No Module Installed\n\tLocator: A2\n\tType: Unknown\n";
        let (modules, total) = parse_dmidecode17(text);
        assert_eq!(total, 2);
        assert_eq!(modules.len(), 1);
        let m = &modules[0];
        assert_eq!(m.locator, "A1");
        assert_eq!(m.size_bytes, 32 * 1024 * 1024 * 1024);
        assert_eq!(m.kind, "DDR4");
        assert_eq!(m.speed_mts, Some(2133));
        assert_eq!(m.manufacturer.as_deref(), Some("SK Hynix"));
        // ASCII manufacturer without a known part prefix is kept as-is.
        let (m2, _) = parse_dmidecode17(
            "Handle 0x1, DMI type 17, 84 bytes\nMemory Device\n\tSize: 8 GB\n\tType: DDR4\n\tManufacturer: Kingston\n",
        );
        assert_eq!(m2[0].manufacturer.as_deref(), Some("Kingston"));
    }

    #[test]
    fn parses_dmi_sizes() {
        assert_eq!(parse_dmi_size("32 GiB"), Some(32 * 1024 * 1024 * 1024));
        assert_eq!(parse_dmi_size("32 GB"), Some(32 * 1024 * 1024 * 1024));
        assert_eq!(parse_dmi_size("512 MB"), Some(512 * 1024 * 1024));
        assert_eq!(parse_dmi_size("No Module Installed"), None);
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