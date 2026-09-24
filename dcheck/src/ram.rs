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
    /// DIMMs as seen by the memory controller (Linux EDAC), with per-DIMM
    /// ECC counts. Independent of the firmware's SMBIOS table.
    pub edac_dimms: Vec<EdacDimm>,
    pub source: String,
}

/// One DIMM reported by the EDAC memory-controller driver.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct EdacDimm {
    pub label: String,
    pub size_bytes: u64,
    pub ce: u64,
    pub ue: u64,
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

    /// Total size of the modules the firmware (SMBIOS) lists.
    pub fn smbios_bytes(&self) -> u64 {
        self.modules.iter().map(|m| m.size_bytes).sum()
    }

    /// When the OS sees clearly more memory than SMBIOS lists and all listed
    /// modules are the same size: the likely number of installed modules.
    pub fn estimated_modules(&self) -> Option<usize> {
        let size = self.modules.first()?.size_bytes;
        if size == 0 || self.modules.iter().any(|m| m.size_bytes != size) {
            return None;
        }
        // MemTotal is a little below the installed amount (firmware/kernel
        // reservations), so only trust a clear surplus.
        if (self.total_bytes as f64) < self.smbios_bytes() as f64 * 1.05 {
            return None;
        }
        let n = self.total_bytes.div_ceil(size) as usize;
        Some(if self.slots_total > 0 { n.min(self.slots_total as usize) } else { n })
    }

    /// Best count of populated slots across SMBIOS, EDAC and the estimate.
    /// Apple Silicon: memory is part of the SoC package, there are no slots.
    pub fn on_package(&self) -> bool {
        self.modules.iter().any(|m| m.locator == "on-package")
    }

    pub fn populated(&self) -> usize {
        self.modules
            .len()
            .max(self.edac_dimms.len())
            .max(self.estimated_modules().unwrap_or(0))
    }

    /// Cross-checks between SMBIOS, the OS total and EDAC.
    pub fn notes(&self) -> Vec<String> {
        let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
        let mut notes = Vec::new();
        let listed = self.smbios_bytes();
        if !self.modules.is_empty() && self.total_bytes as f64 > listed as f64 * 1.05 {
            let base = format!(
                "the firmware (SMBIOS) lists {} module(s) = {:.0} GiB, but the OS sees {:.1} GiB — its DIMM table is incomplete",
                self.modules.len(),
                gib(listed),
                gib(self.total_bytes)
            );
            match self.estimated_modules() {
                Some(n) => notes.push(format!(
                    "{base}; about {n} × {:.0} GiB are installed",
                    gib(self.modules[0].size_bytes)
                )),
                None => notes.push(base),
            }
        }
        if !self.edac_dimms.is_empty() && self.edac_dimms.len() != self.modules.len() && !self.modules.is_empty() {
            notes.push(format!(
                "the memory controller (EDAC) reports {} DIMM(s), the firmware lists {} — an incomplete firmware table, or mirrored/spare DIMMs",
                self.edac_dimms.len(),
                self.modules.len()
            ));
        }
        if listed > 0 && (self.total_bytes as f64) < listed as f64 * 0.85 {
            notes.push(format!(
                "the OS sees {:.1} GiB of {:.0} GiB installed — memory mirroring/sparing, a BIOS limit, or a disabled DIMM",
                gib(self.total_bytes),
                gib(listed)
            ));
        }
        notes
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
            if let Some(v) = rest.split_whitespace().next() {
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

/// Optional fallback: parse `lshw -class memory -json`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_lshw_memory(text: &str) -> (Vec<RamModule>, u32) {
    let mut modules = Vec::new();
    let mut slots = 0u32;
    let Some(crate::json::Json::Obj(root)) = crate::json::Json::parse(text) else {
        return (modules, slots);
    };
    // lshw nests memory devices under "children"; walk recursively.
    fn walk(node: &crate::json::Json, modules: &mut Vec<RamModule>, slots: &mut u32) {
        if node.get("id").and_then(|v| v.as_str()) == Some("memory")
            || node.get("class").and_then(|v| v.as_str()) == Some("memory")
        {
            *slots += 1;
            let get = |k: &str| node.get(k).and_then(|v| v.as_str()).map(str::to_string);
            let size = node
                .get("size")
                .and_then(|v| v.as_f64())
                .map(|b| b as u64)
                .unwrap_or(0);
            if size > 0 {
                let part = get("product");
                let mf = get("vendor");
                modules.push(RamModule {
                    locator: get("slot").or_else(|| get("physid")).unwrap_or_default(),
                    size_bytes: size,
                    kind: get("description").unwrap_or_default(),
                    speed_mts: None,
                    configured_mts: None,
                    manufacturer: mf.map(|m| infer_vendor(&m, part.as_deref())),
                    part_number: part,
                    serial: get("serial"),
                    rank: None,
                });
            }
        }
        if let Some(crate::json::Json::Arr(children)) = node.get("children") {
            for c in children {
                walk(c, modules, slots);
            }
        }
    }
    walk(&crate::json::Json::Obj(root), &mut modules, &mut slots);
    (modules, slots)
}

/// Parse one SMBIOS type-17 (Memory Device) structure from its raw bytes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_smbios17(bytes: &[u8]) -> Option<RamModule> {
    if bytes.len() < 0x15 || bytes[0] != 17 {
        return None;
    }
    let len = bytes[1] as usize;
    if len < 0x15 || len > bytes.len() {
        return None;
    }
    let fmt = &bytes[..len];
    let strings = &bytes[len..];
    let u8_at = |o: usize| -> u8 {
        if o < len {
            fmt[o]
        } else {
            0
        }
    };
    let u16_at = |o: usize| -> u16 {
        if o + 2 <= len {
            u16::from_le_bytes([fmt[o], fmt[o + 1]])
        } else {
            0
        }
    };
    let str_at = |idx: u8| -> Option<String> {
        if idx == 0 {
            return None;
        }
        let mut n = 1u8;
        let mut i = 0usize;
        while i < strings.len() {
            let end = strings[i..].iter().position(|&b| b == 0)?;
            if n == idx {
                return nonempty(&String::from_utf8_lossy(&strings[i..i + end]));
            }
            n += 1;
            i += end + 1;
        }
        None
    };

    let raw_size = u16_at(0x0C);
    let size_bytes: u64 = if raw_size == 0 {
        return None; // no module installed
    } else if raw_size == 0x7FFF {
        // Extended size in MB (offset 0x1C).
        if len >= 0x20 {
            let ext = u32::from_le_bytes([fmt[0x1C], fmt[0x1D], fmt[0x1E], fmt[0x1F]]);
            ext as u64 * 1024 * 1024
        } else {
            return None;
        }
    } else if raw_size & 0x8000 != 0 {
        (raw_size & 0x7FFF) as u64 * 1024 // KB
    } else {
        raw_size as u64 * 1024 * 1024 // MB
    };

    let kind = memory_type_name(u8_at(0x12));
    let speed_mts = {
        let s = u16_at(0x15);
        if s == 0 || s == 0xFFFF {
            None
        } else {
            Some(s as u32)
        }
    };
    let configured_mts = if len >= 0x22 {
        let c = u16_at(0x20);
        if c == 0 || c == 0xFFFF {
            None
        } else {
            Some(c as u32)
        }
    } else {
        None
    };
    let rank = if len >= 0x1C {
        let r = (u8_at(0x1B) & 0x0F) as u32;
        if r > 0 {
            Some(r)
        } else {
            None
        }
    } else {
        None
    };

    let part = str_at(u8_at(0x1A));
    let manufacturer = str_at(u8_at(0x17)).map(|m| infer_vendor(&m, part.as_deref()));

    Some(RamModule {
        locator: str_at(u8_at(0x10)).unwrap_or_default(),
        size_bytes,
        kind,
        speed_mts,
        configured_mts,
        manufacturer,
        part_number: part,
        serial: str_at(u8_at(0x18)),
        rank,
    })
}

fn memory_type_name(code: u8) -> String {
    let name = match code {
        0x0F => "SMD",
        0x12 => "SDRAM",
        0x13 => "SGRAM",
        0x15 => "DDR",
        0x16 => "DDR2",
        0x17 => "DDR2 FB-DIMM",
        0x18 => "DDR3",
        0x19 => "FBD2",
        0x1A => "DDR4",
        0x1B => "LPDDR",
        0x1C => "LPDDR2",
        0x1D => "LPDDR3",
        0x1E => "LPDDR4",
        0x1F => "Logical non-volatile",
        0x20 => "HBM",
        0x21 => "HBM2",
        0x22 => "DDR5",
        0x23 => "LPDDR5",
        0x24 => "HBM3",
        _ => return "Unknown".to_string(),
    };
    name.to_string()
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

/// Memory from `system_profiler SPMemoryDataType -json`. Intel Macs list
/// DIMMs under `_items`; Apple Silicon has one flat entry for the on-package
/// memory (`"SPMemoryDataType": "16 GB"`, `dimm_manufacturer`, `dimm_type`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_sp_memory(text: &str) -> Vec<RamModule> {
    use crate::json::Json;
    let Some(Json::Obj(root)) = Json::parse(text) else { return Vec::new() };
    let Some(Json::Arr(items)) = root.get("SPMemoryDataType") else { return Vec::new() };
    let module = |d: &Json, on_package: bool| {
        let get = |k: &str| d.get(k).and_then(|v| v.as_str()).and_then(nonempty);
        let size = ["dimm_size", "size", "SPMemoryDataType"]
            .iter()
            .find_map(|k| get(k).and_then(|s| parse_sp_size(&s)))
            .unwrap_or(0);
        let part = get("dimm_part_number").or_else(|| get("part_number"));
        let mf = get("dimm_manufacturer").or_else(|| get("manufacturer"));
        RamModule {
            locator: get("_name").unwrap_or_else(|| if on_package { "on-package".into() } else { String::new() }),
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
        }
    };
    let mut modules = Vec::new();
    for item in items {
        match item.get("_items") {
            Some(Json::Arr(dimms)) => modules.extend(dimms.iter().map(|d| module(d, false))),
            _ => modules.push(module(item, true)),
        }
    }
    // Intel Macs report empty slots as "Empty" with no size.
    modules.retain(|m| m.size_bytes > 0);
    modules
}

fn parse_sp_size(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let mul: u64 = match it.next().unwrap_or("").to_ascii_uppercase().as_str() {
        "TB" => 1 << 40,
        "GB" => 1 << 30,
        "MB" => 1 << 20,
        _ => return None,
    };
    Some((n * mul as f64) as u64)
}

/// `sysctl -n vm.swapusage`: "total = 8192.00M  used = 7327.94M  free = 864.06M
/// (encrypted)" -> (total, free) in bytes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parse_swapusage(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        let rest = text.split(&format!("{name} = ")).nth(1)?;
        let tok = rest.split_whitespace().next()?;
        let (num, unit) = tok.split_at(tok.find(|c: char| c.is_ascii_alphabetic())?);
        let mul: f64 = match unit {
            "K" => 1024.0,
            "M" => 1024.0 * 1024.0,
            "G" => 1024.0 * 1024.0 * 1024.0,
            _ => return None,
        };
        Some((num.parse::<f64>().ok()? * mul) as u64)
    };
    Some((field("total")?, field("free")?))
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
        info.edac_dimms = edac_dimms(Path::new("/sys/devices/system/edac/mc"));

        // 1) dmidecode (richest).
        if std::env::var_os("DCHECK_NO_DMIDECODE").is_none() {
            if let Ok(out) = Command::new("dmidecode").arg("-t").arg("17").output() {
                if out.status.success() {
                    let (modules, slots) = parse_dmidecode17(&String::from_utf8_lossy(&out.stdout));
                    info.modules = modules;
                    info.slots_total = slots;
                }
            }
        }

        // 2) Raw SMBIOS from sysfs (no external tools).
        if info.modules.is_empty() {
            let (modules, slots) = sysfs_dmi17();
            if !modules.is_empty() {
                info.modules = modules;
                info.slots_total = slots;
            }
        }

        // 3) lshw, when installed.
        if info.modules.is_empty() {
            if let Ok(out) = Command::new("lshw").args(["-class", "memory", "-json"]).output() {
                if out.status.success() {
                    let (modules, slots) =
                        super::parse_lshw_memory(&String::from_utf8_lossy(&out.stdout));
                    if !modules.is_empty() {
                        info.modules = modules;
                        info.slots_total = slots;
                    }
                }
            }
        }
        info
    }

    /// DIMMs from EDAC: `mc*/dimm*/{dimm_label,size (MB),dimm_ce_count,dimm_ue_count}`.
    pub fn edac_dimms(root: &Path) -> Vec<super::EdacDimm> {
        let read = |p: &Path| std::fs::read_to_string(p).map(|s| s.trim().to_string()).ok();
        let num = |p: &Path| read(p).and_then(|s| s.parse::<u64>().ok());
        let mut out = Vec::new();
        let Ok(mcs) = std::fs::read_dir(root) else { return out };
        let mut mcs: Vec<_> = mcs.flatten().map(|e| e.path()).collect();
        mcs.sort();
        for mc in mcs {
            let Ok(dimms) = std::fs::read_dir(&mc) else { continue };
            let mut dimms: Vec<_> = dimms
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("dimm")))
                .collect();
            dimms.sort();
            for d in dimms {
                let size_mb = num(&d.join("size")).unwrap_or(0);
                if size_mb == 0 {
                    continue;
                }
                out.push(super::EdacDimm {
                    label: read(&d.join("dimm_label")).unwrap_or_else(|| {
                        format!("{}/{}", mc.file_name().unwrap().to_string_lossy(), d.file_name().unwrap().to_string_lossy())
                    }),
                    size_bytes: size_mb << 20,
                    ce: num(&d.join("dimm_ce_count")).unwrap_or(0),
                    ue: num(&d.join("dimm_ue_count")).unwrap_or(0),
                });
            }
        }
        out
    }

    /// Read SMBIOS type-17 structures straight from `/sys/firmware/dmi/entries`.
    fn sysfs_dmi17() -> (Vec<super::RamModule>, u32) {
        let base = Path::new("/sys/firmware/dmi/entries");
        let mut modules = Vec::new();
        let mut slots = 0u32;
        let Ok(entries) = std::fs::read_dir(base) else {
            return (modules, slots);
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| n.starts_with("17-"))
            .collect();
        names.sort();
        for name in names {
            slots += 1;
            if let Ok(bytes) = std::fs::read(base.join(&name).join("raw")) {
                if let Some(m) = super::parse_smbios17(&bytes) {
                    modules.push(m);
                }
            }
        }
        (modules, slots)
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

    use super::{parse_sp_memory, parse_swapusage, RamInfo, RamModule};

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
        let (swap_total, swap_free) = Command::new("sysctl")
            .args(["-n", "vm.swapusage"])
            .output()
            .ok()
            .and_then(|o| parse_swapusage(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or((0, 0));
        RamInfo {
            total_bytes: total,
            available_bytes: free,
            swap_total_bytes: swap_total,
            swap_free_bytes: swap_free,
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

    /// DIMM details from `system_profiler SPMemoryDataType -json`.
    fn system_profiler_memory() -> Vec<RamModule> {
        match Command::new("system_profiler").args(["SPMemoryDataType", "-json"]).output() {
            Ok(out) if out.status.success() => parse_sp_memory(&String::from_utf8_lossy(&out.stdout)),
            _ => Vec::new(),
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

    fn module(locator: &str, gib: u64) -> RamModule {
        RamModule {
            locator: locator.into(),
            size_bytes: gib << 30,
            kind: "DDR3".into(),
            ..Default::default()
        }
    }

    #[test]
    fn incomplete_smbios_table_is_detected() {
        // lab-243: firmware lists 2 × 32 GiB, the OS sees 125.6 GiB.
        let r = RamInfo {
            total_bytes: 131_727_204 * 1024,
            modules: vec![module("DIMM_B1", 32), module("DIMM_D1", 32)],
            slots_total: 4,
            ..Default::default()
        };
        assert_eq!(r.estimated_modules(), Some(4));
        assert_eq!(r.populated(), 4);
        let notes = r.notes();
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("lists 2 module(s) = 64 GiB"), "{notes:?}");
        assert!(notes[0].contains("about 4 × 32 GiB are installed"), "{notes:?}");
    }

    #[test]
    fn complete_table_has_no_notes() {
        let r = RamInfo {
            total_bytes: 32_609_508 * 1024, // 31.1 GiB visible of 32 GiB
            modules: vec![module("A1", 32)],
            slots_total: 24,
            ..Default::default()
        };
        assert_eq!(r.estimated_modules(), None);
        assert!(r.notes().is_empty());
    }

    #[test]
    fn edac_mismatch_is_noted() {
        // 10.0.0.251: SMBIOS 1 module, EDAC 2 DIMMs, OS ~31 GiB.
        let d = |l: &str| EdacDimm { label: l.into(), size_bytes: 32 << 30, ce: 0, ue: 0 };
        let r = RamInfo {
            total_bytes: 32_609_508 * 1024,
            modules: vec![module("A1", 32)],
            edac_dimms: vec![d("CPU_SrcID#0_Ha#0_Chan#0_DIMM#0"), d("CPU_SrcID#1_Ha#0_Chan#0_DIMM#0")],
            slots_total: 24,
            ..Default::default()
        };
        assert_eq!(r.populated(), 2);
        let notes = r.notes();
        assert!(notes.iter().any(|n| n.contains("EDAC) reports 2 DIMM(s), the firmware lists 1")), "{notes:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_edac_dimms() {
        let root = std::env::temp_dir().join(format!("dcheck-edac-{}", std::process::id()));
        let d = root.join("mc0/dimm0");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::create_dir_all(root.join("mc0/dimm1")).unwrap();
        for (f, v) in [("dimm_label", "A1"), ("size", "32768"), ("dimm_ce_count", "3"), ("dimm_ue_count", "0")] {
            std::fs::write(d.join(f), v).unwrap();
        }
        std::fs::write(root.join("mc0/dimm1/size"), "0").unwrap(); // empty slot
        let dimms = linux::edac_dimms(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(dimms, vec![EdacDimm { label: "A1".into(), size_bytes: 32 << 30, ce: 3, ue: 0 }]);
    }

    #[test]
    fn parses_system_profiler_apple_silicon() {
        let m = parse_sp_memory(
            r#"{"SPMemoryDataType":[{"dimm_manufacturer":"Hynix","dimm_type":"LPDDR5","SPMemoryDataType":"16 GB"}]}"#,
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].locator, "on-package");
        assert_eq!(m[0].size_bytes, 16 << 30);
        assert_eq!(m[0].kind, "LPDDR5");
        assert_eq!(m[0].manufacturer.as_deref(), Some("Hynix"));
    }

    #[test]
    fn parses_system_profiler_intel() {
        let m = parse_sp_memory(
            r#"{"SPMemoryDataType":[{"_name":"Memory Slots","_items":[
              {"_name":"BANK 0/ChannelA-DIMM0","dimm_size":"8 GB","dimm_type":"DDR4","dimm_speed":"2667 MHz",
               "dimm_manufacturer":"0x802C","dimm_part_number":"8ATF1G64HZ-2G6E1","dimm_serial_number":"1234"},
              {"_name":"BANK 1/ChannelB-DIMM0","dimm_size":"Empty"}]}]}"#,
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].locator, "BANK 0/ChannelA-DIMM0");
        assert_eq!(m[0].size_bytes, 8 << 30);
        assert_eq!(m[0].speed_mts, Some(2667));
    }

    #[test]
    fn parses_macos_swapusage() {
        let (total, free) =
            parse_swapusage("total = 8192.00M  used = 7327.94M  free = 864.06M  (encrypted)").unwrap();
        assert_eq!(total, 8192 << 20);
        assert_eq!(free / (1 << 20), 864);
        assert_eq!(parse_swapusage("total = 0.00M  used = 0.00M  free = 0.00M"), Some((0, 0)));
    }

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
    fn parses_raw_smbios17() {
        let mut b = vec![0u8; 0x22];
        b[0] = 17;
        b[1] = 0x22;
        b[0x0C] = 0xFF;
        b[0x0D] = 0x7F; // size = 0x7FFF -> use extended size
        b[0x1C] = 0x00;
        b[0x1D] = 0x80;
        b[0x1E] = 0x00;
        b[0x1F] = 0x00; // extended size 32768 MB = 32 GiB
        b[0x12] = 0x1A; // DDR4
        b[0x15] = 0x80;
        b[0x16] = 0x0C; // 3200 MT/s
        b[0x1B] = 0x02; // rank 2
        b[0x20] = 0x80;
        b[0x21] = 0x0C; // configured 3200
        b[0x10] = 1; // locator
        b[0x17] = 2; // manufacturer
        b[0x18] = 3; // serial
        b[0x1A] = 4; // part number
        b.extend_from_slice(b"A1\0Samsung\0SN1\0M378A1K43\0\0");
        let m = parse_smbios17(&b).unwrap();
        assert_eq!(m.size_bytes, 32 * 1024 * 1024 * 1024);
        assert_eq!(m.kind, "DDR4");
        assert_eq!(m.speed_mts, Some(3200));
        assert_eq!(m.locator, "A1");
        assert_eq!(m.manufacturer.as_deref(), Some("Samsung"));
        assert_eq!(m.part_number.as_deref(), Some("M378A1K43"));
        assert_eq!(m.rank, Some(2));
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