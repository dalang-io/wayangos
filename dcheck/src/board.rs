//! Motherboard: identity (DMI), BIOS / firmware, PCIe and USB devices, board
//! sensors (hwmon, IPMI) and health.
//!
//! Health comes from what the platform reports, never from guesses:
//! - sensors: hwmon alarms / limits, IPMI threshold and discrete states
//!   (fans, voltages, temperatures, power supplies, intrusion);
//! - IPMI system event log: recent critical events (PSU AC lost, memory,
//!   processor, critical interrupts);
//! - PCIe: AER error counters, links running narrower than both ends allow
//!   (bad seating / riser), devices without a driver;
//! - notes: BIOS age, generic (white-label) boards, UEFI / Secure Boot.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ipmi::{Event, Status};

#[derive(Debug, Clone, Default)]
pub struct Ident {
    pub vendor: Option<String>,
    pub product: Option<String>,
    pub version: Option<String>,
    pub serial: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Bios {
    pub vendor: Option<String>,
    pub version: Option<String>,
    /// (year, month, day)
    pub date: Option<(u16, u8, u8)>,
    pub uefi: Option<bool>,
    pub secure_boot: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub cur_gts: f32,
    pub cur_w: u8,
    pub max_gts: f32,
    pub max_w: u8,
    /// What the slot (upstream port) supports.
    pub port_w: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct PciDev {
    pub addr: String,
    pub class: u32,
    pub vendor_id: u16,
    pub device_id: u16,
    pub name: String,
    pub driver: Option<String>,
    pub link: Option<Link>,
    /// AER totals: correctable, non-fatal, fatal.
    pub aer: Option<[u64; 3]>,
}

impl PciDev {
    /// Chipset internals (bridges, system peripherals, uncore): hidden in
    /// the device list.
    pub fn internal(&self) -> bool {
        matches!(self.class >> 16, 0x05 | 0x06 | 0x08 | 0x11 | 0xFF)
    }

    pub fn class_name(&self) -> &'static str {
        class_name(self.class)
    }
}

#[derive(Debug, Clone)]
pub struct UsbDev {
    pub id: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub name: String,
    pub speed_mbps: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Temp,
    Fan,
    Volt,
    Current,
    Power,
    Other,
}

#[derive(Debug, Clone)]
pub struct BoardSensor {
    /// "IPMI" or the hwmon chip name.
    pub source: String,
    pub name: String,
    pub kind: Kind,
    pub value: Option<f64>,
    pub unit: &'static str,
    pub state: Option<String>,
    pub status: Status,
}

#[derive(Debug, Clone, Default)]
pub struct BoardInfo {
    pub system: Ident,
    pub board: Ident,
    pub chassis: Option<String>,
    pub bios: Bios,
    pub bmc_firmware: Option<String>,
    pub pci: Vec<PciDev>,
    pub usb: Vec<UsbDev>,
    pub sensors: Vec<BoardSensor>,
    pub events: Vec<Event>,
    pub sel_entries: u16,
    /// IPMI device exists but could not be read (usually: not root).
    pub ipmi_note: Option<String>,
    pub source: String,
}

pub struct BoardHealth {
    pub label: &'static str,
    pub severity: u8,
    pub issues: Vec<String>,
    pub notes: Vec<String>,
}

/// Strings firmware vendors leave in unfilled DMI fields.
pub fn placeholder(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    t.is_empty()
        || [
            "default string",
            "to be filled by o.e.m.",
            "to be filled by oem",
            "system product name",
            "system manufacturer",
            "system version",
            "not specified",
            "not applicable",
            "none",
            "0123456789",
            "oem",
            "o.e.m.",
            "x.x",
        ]
        .contains(&t.as_str())
}

pub fn chassis_name(code: u32) -> &'static str {
    match code {
        3 => "desktop",
        4 => "low-profile desktop",
        5 => "pizza box",
        6 => "mini tower",
        7 => "tower",
        8 => "portable",
        9 => "laptop",
        10 => "notebook",
        13 => "all-in-one",
        14 => "sub-notebook",
        17 => "main server chassis",
        23 => "rack mount chassis",
        24 => "sealed-case PC",
        25 => "multi-system chassis",
        28 => "blade",
        30 => "tablet",
        31 => "convertible",
        32 => "detachable",
        35 => "mini PC",
        36 => "stick PC",
        _ => "other",
    }
}

pub fn class_name(class: u32) -> &'static str {
    match class >> 8 {
        0x0100 => "SCSI controller",
        0x0101 => "IDE controller",
        0x0104 => "RAID controller",
        0x0106 => "SATA controller",
        0x0107 => "SAS controller",
        0x0108 => "NVMe controller",
        0x0200 => "Ethernet controller",
        0x0207 => "InfiniBand controller",
        0x0280 => "Network controller",
        0x0300 => "VGA controller",
        0x0302 => "3D controller",
        0x0380 => "Display controller",
        0x0400 => "Video device",
        0x0401 => "Audio device",
        0x0403 => "Audio device",
        0x0600 => "Host bridge",
        0x0601 => "ISA bridge",
        0x0604 => "PCI bridge",
        0x0780 => "Communication controller",
        0x0805 => "SD host controller",
        0x0c03 => "USB controller",
        0x0c05 => "SMBus",
        0x0c80 => "Serial bus controller",
        0x0d11 => "Bluetooth",
        0x1080 => "Encryption controller",
        0x1180 => "Signal processing",
        _ => match class >> 16 {
            0x01 => "Storage controller",
            0x02 => "Network controller",
            0x03 => "Display controller",
            0x04 => "Multimedia controller",
            0x0c => "Serial bus controller",
            _ => "Device",
        },
    }
}

fn vendor_fallback(v: u16) -> &'static str {
    match v {
        0x8086 => "Intel",
        0x1022 => "AMD",
        0x1002 => "AMD/ATI",
        0x10de => "NVIDIA",
        0x14e4 => "Broadcom",
        0x1000 => "Broadcom / LSI",
        0x10ec => "Realtek",
        0x1b4b => "Marvell",
        0x15b3 => "Mellanox",
        0x1b21 => "ASMedia",
        0x102b => "Matrox",
        0x144d => "Samsung",
        0x1912 => "Renesas",
        0x19a2 => "Emulex",
        0x1077 => "QLogic",
        0x1924 => "Solarflare",
        0x1af4 => "Red Hat (virtio)",
        0x15ad => "VMware",
        0x106b => "Apple",
        0x1d6b => "Linux Foundation",
        _ => "",
    }
}

/// Names from a `pci.ids` / `usb.ids` file for the wanted (vendor, device)
/// pairs: vendor name and "vendor device" names.
pub fn parse_ids(text: &str, wanted: &[(u16, u16)]) -> HashMap<(u16, Option<u16>), String> {
    let vendors: std::collections::HashSet<u16> = wanted.iter().map(|w| w.0).collect();
    let mut out = HashMap::new();
    let mut cur: Option<u16> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('C') && !line.starts_with('\t') {
            break; // class section
        }
        if !line.starts_with('\t') {
            cur = u16::from_str_radix(line.get(..4).unwrap_or(""), 16).ok().filter(|v| vendors.contains(v));
            if let Some(v) = cur {
                out.insert((v, None), line[4..].trim().to_string());
            }
        } else if let (Some(v), false) = (cur, line.starts_with("\t\t")) {
            if let Ok(d) = u16::from_str_radix(line.trim_start().get(..4).unwrap_or(""), 16) {
                if wanted.contains(&(v, d)) {
                    out.insert((v, Some(d)), line.trim_start()[4..].trim().to_string());
                }
            }
        }
    }
    out
}

fn read_str(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn hex_u(p: &Path) -> Option<u64> {
    let s = read_str(p)?;
    u64::from_str_radix(s.trim_start_matches("0x"), 16).ok()
}

/// "8.0 GT/s PCIe" → 8.0
fn gts(s: &str) -> Option<f32> {
    s.split_whitespace().next()?.parse().ok()
}

fn aer_total(p: &Path) -> Option<u64> {
    let t = std::fs::read_to_string(p).ok()?;
    t.lines()
        .find(|l| l.starts_with("TOTAL_ERR"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .or_else(|| Some(t.lines().filter_map(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).sum()))
}

/// Parse a DMI date "MM/DD/YYYY".
pub fn parse_bios_date(s: &str) -> Option<(u16, u8, u8)> {
    let mut it = s.trim().split('/');
    let (m, d, y) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse::<u16>().ok()?);
    let y = if y < 100 { 1900 + y + if y < 70 { 100 } else { 0 } } else { y };
    (1..=12).contains(&m).then_some((y, m, d))
}

/// "YYYY-MM-DD HH:MM" (UTC) for a Unix time.
pub fn fmt_time(t: u32) -> String {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", secs / 3600, (secs % 3600) / 60)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Hwmon chips covered elsewhere (CPU, memory, disks).
const OTHER_MODULES: &[&str] = &["coretemp", "k10temp", "zenpower", "nvme", "drivetemp", "jc42", "spd5118"];

/// Board sensors from `/sys/class/hwmon`.
pub fn hwmon_sensors(root: &Path) -> Vec<BoardSensor> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root.join("sys/class/hwmon")) else { return out };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for d in dirs {
        let chip = read_str(&d.join("name")).unwrap_or_else(|| "hwmon".into());
        if OTHER_MODULES.contains(&chip.as_str()) {
            continue;
        }
        let num = |f: &str| read_str(&d.join(f)).and_then(|v| v.parse::<f64>().ok());
        let mut files: Vec<String> = std::fs::read_dir(&d)
            .map(|r| r.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
            .unwrap_or_default();
        files.sort();
        for f in files {
            let Some(base) = f.strip_suffix("_input") else { continue };
            let (kind, scale, unit) = match base.trim_end_matches(|c: char| c.is_ascii_digit()) {
                "temp" => (Kind::Temp, 1000.0, "°C"),
                "fan" => (Kind::Fan, 1.0, "RPM"),
                "in" => (Kind::Volt, 1000.0, "V"),
                "curr" => (Kind::Current, 1000.0, "A"),
                "power" => (Kind::Power, 1_000_000.0, "W"),
                _ => continue,
            };
            let Some(raw) = num(&f) else { continue };
            let v = raw / scale;
            let lim = |s: &str| num(&format!("{base}_{s}")).map(|x| x / scale);
            let alarm = num(&format!("{base}_alarm")).is_some_and(|a| a > 0.0);
            let mut status = if alarm { Status::Warn } else { Status::Ok };
            match kind {
                Kind::Temp => {
                    if lim("crit").is_some_and(|c| c > 0.0 && v >= c) {
                        status = Status::Crit;
                    } else if lim("max").is_some_and(|m| m > 0.0 && v >= m) {
                        status = status.max(Status::Warn);
                    }
                }
                Kind::Fan => {
                    if lim("min").is_some_and(|m| m > 0.0 && v < m) {
                        status = status.max(Status::Warn);
                    }
                }
                Kind::Volt => {
                    if let (Some(lo), Some(hi)) = (lim("min"), lim("max")) {
                        if hi > lo && (v < lo || v > hi) {
                            status = status.max(Status::Warn);
                        }
                    }
                }
                _ => {}
            }
            let label = read_str(&d.join(format!("{base}_label"))).unwrap_or_else(|| base.to_string());
            out.push(BoardSensor { source: chip.clone(), name: label, kind, value: Some(v), unit, state: None, status });
        }
        if num("intrusion0_alarm").is_some_and(|a| a > 0.0) {
            out.push(BoardSensor {
                source: chip.clone(),
                name: "Chassis intrusion".into(),
                kind: Kind::Other,
                value: None,
                unit: "",
                state: Some("chassis was opened".into()),
                status: Status::Warn,
            });
        }
    }
    out
}

/// PCI devices from sysfs; names from pci.ids when available.
pub fn pci_devices(root: &Path) -> Vec<PciDev> {
    let base = root.join("sys/bus/pci/devices");
    let Ok(rd) = std::fs::read_dir(&base) else { return Vec::new() };
    let mut addrs: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
    addrs.sort();
    let mut devs = Vec::new();
    for a in addrs {
        let d = base.join(&a);
        let (Some(class), Some(v), Some(dv)) = (hex_u(&d.join("class")), hex_u(&d.join("vendor")), hex_u(&d.join("device"))) else {
            continue;
        };
        let driver = std::fs::read_link(d.join("driver")).ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let w = |f: &str| read_str(&d.join(f)).and_then(|v| v.parse::<u8>().ok());
        let s = |f: &str| read_str(&d.join(f)).and_then(|v| gts(&v));
        let link = match (s("current_link_speed"), w("current_link_width"), s("max_link_speed"), w("max_link_width")) {
            (Some(cs), Some(cw), Some(ms), Some(mw)) if cw > 0 && mw > 0 && mw < 64 => {
                // The upstream port (parent directory) limits the width too.
                let port_w = std::fs::canonicalize(&d)
                    .ok()
                    .and_then(|p| p.parent().map(Path::to_path_buf))
                    .and_then(|p| read_str(&p.join("max_link_width")))
                    .and_then(|v| v.parse::<u8>().ok())
                    .filter(|w| *w > 0 && *w < 64);
                Some(Link { cur_gts: cs, cur_w: cw, max_gts: ms, max_w: mw, port_w })
            }
            _ => None,
        };
        let aer = if d.join("aer_dev_correctable").exists() {
            Some([
                aer_total(&d.join("aer_dev_correctable")).unwrap_or(0),
                aer_total(&d.join("aer_dev_nonfatal")).unwrap_or(0),
                aer_total(&d.join("aer_dev_fatal")).unwrap_or(0),
            ])
        } else {
            None
        };
        devs.push(PciDev { addr: a, class: class as u32, vendor_id: v as u16, device_id: dv as u16, name: String::new(), driver, link, aer });
    }
    let wanted: Vec<(u16, u16)> = devs.iter().map(|d| (d.vendor_id, d.device_id)).collect();
    let ids = ["usr/share/hwdata/pci.ids", "usr/share/misc/pci.ids", "usr/share/pci.ids"]
        .iter()
        .find_map(|p| std::fs::read_to_string(root.join(p)).ok())
        .map(|t| parse_ids(&t, &wanted))
        .unwrap_or_default();
    for d in &mut devs {
        let vendor = ids.get(&(d.vendor_id, None)).cloned().unwrap_or_else(|| vendor_fallback(d.vendor_id).to_string());
        d.name = match ids.get(&(d.vendor_id, Some(d.device_id))) {
            Some(n) => format!("{vendor} {n}").trim().to_string(),
            None => format!("{vendor} {:04x}:{:04x}", d.vendor_id, d.device_id).trim().to_string(),
        };
    }
    devs
}

pub fn usb_devices(root: &Path) -> Vec<UsbDev> {
    let base = root.join("sys/bus/usb/devices");
    let Ok(rd) = std::fs::read_dir(&base) else { return Vec::new() };
    let mut ids: Vec<String> = rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
    ids.sort();
    let mut out = Vec::new();
    for id in ids {
        let d = base.join(&id);
        let (Some(v), Some(p)) = (hex_u(&d.join("idVendor")), hex_u(&d.join("idProduct"))) else { continue };
        if v == 0x1d6b || read_str(&d.join("bDeviceClass")).as_deref() == Some("09") {
            continue; // root and internal hubs
        }
        let mf = read_str(&d.join("manufacturer")).unwrap_or_default();
        let prod = read_str(&d.join("product")).unwrap_or_default();
        let name = format!("{mf} {prod}").trim().to_string();
        let name = if name.is_empty() { format!("{v:04x}:{p:04x}") } else { name };
        let speed = read_str(&d.join("speed")).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        out.push(UsbDev { id, vendor_id: v as u16, product_id: p as u16, name, speed_mbps: speed });
    }
    out
}

/// Read everything from sysfs under `root` (tests use a fake tree).
pub fn read_from(root: &Path) -> BoardInfo {
    let dmi = |f: &str| read_str(&root.join("sys/class/dmi/id").join(f));
    let mut b = BoardInfo {
        system: Ident {
            vendor: dmi("sys_vendor"),
            product: dmi("product_name"),
            version: dmi("product_version"),
            serial: dmi("product_serial"),
        },
        board: Ident {
            vendor: dmi("board_vendor"),
            product: dmi("board_name"),
            version: dmi("board_version"),
            serial: dmi("board_serial"),
        },
        chassis: dmi("chassis_type").and_then(|c| c.parse().ok()).map(|c: u32| chassis_name(c).to_string()),
        bios: Bios {
            vendor: dmi("bios_vendor"),
            version: dmi("bios_version"),
            date: dmi("bios_date").and_then(|d| parse_bios_date(&d)),
            uefi: Some(root.join("sys/firmware/efi").exists()),
            secure_boot: None,
        },
        source: "sysfs".into(),
        ..Default::default()
    };
    if b.bios.uefi == Some(true) {
        let vars = root.join("sys/firmware/efi/efivars");
        b.bios.secure_boot = std::fs::read_dir(&vars).ok().and_then(|rd| {
            rd.flatten()
                .find(|e| e.file_name().to_string_lossy().starts_with("SecureBoot-"))
                .and_then(|e| std::fs::read(e.path()).ok())
                .and_then(|v| v.get(4).map(|x| *x == 1))
        });
    }
    b.pci = pci_devices(root);
    b.usb = usb_devices(root);
    b.sensors = hwmon_sensors(root);
    b
}

/// Add the BMC's view (sensors, event log).
pub fn add_ipmi(b: &mut BoardInfo, i: crate::ipmi::Ipmi) {
    b.bmc_firmware = i.bmc_firmware;
    for s in i.sensors {
        let (kind, unit) = match s.kind {
            0x01 => (Kind::Temp, "°C"),
            0x02 => (Kind::Volt, "V"),
            0x03 => (Kind::Current, "A"),
            0x04 => (Kind::Fan, "RPM"),
            _ => (Kind::Other, ""),
        };
        let (value, unit) = match s.value {
            Some((v, u)) if !u.is_empty() => (Some(v), u),
            Some((v, _)) => (Some(v), unit),
            None => (None, ""),
        };
        let kind = if s.kind == 0x08 || s.kind == 0x09 || unit == "W" { Kind::Power } else { kind };
        b.sensors.push(BoardSensor { source: "IPMI".into(), name: s.name, kind, value, unit, state: s.state, status: s.status });
    }
    b.events = i.events;
    b.sel_entries = i.sel_entries;
    if let Some(e) = i.error {
        b.ipmi_note = Some(e);
    }
}

/// Read the board (sysfs + IPMI on Linux, system_profiler on macOS).
pub fn read() -> BoardInfo {
    #[cfg(target_os = "macos")]
    {
        macos()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let root = match std::env::var_os("DCHECK_SYS_ROOT") {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => PathBuf::from("/"),
        };
        let mut b = read_from(&root);
        if std::env::var_os("DCHECK_SYS_ROOT").is_none() {
            if let Some(i) = crate::ipmi::read() {
                add_ipmi(&mut b, i);
            } else if root.join("dev/ipmi0").exists() && !crate::native::is_root() {
                b.ipmi_note = Some("a BMC (IPMI) is present: run as root for fans, power supplies and its event log".into());
            }
        }
        b
    }
}

#[cfg(target_os = "macos")]
fn macos() -> BoardInfo {
    let out = std::process::Command::new("system_profiler").args(["SPHardwareDataType", "-json"]).output();
    let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut b = BoardInfo { source: "system_profiler".into(), ..Default::default() };
    if let Some(crate::json::Json::Obj(root)) = crate::json::Json::parse(&text) {
        if let Some(crate::json::Json::Arr(items)) = root.get("SPHardwareDataType") {
            if let Some(h) = items.first() {
                let g = |k: &str| h.get(k).and_then(|v| v.as_str()).map(str::to_string);
                b.system = Ident {
                    vendor: Some("Apple".into()),
                    product: g("machine_name").map(|n| match g("machine_model") {
                        Some(m) => format!("{n} ({m})"),
                        None => n,
                    }),
                    version: g("chip_type").or_else(|| g("cpu_type")),
                    serial: g("serial_number"),
                };
                b.board = Ident { vendor: Some("Apple".into()), product: g("model_number"), ..Default::default() };
                b.bios = Bios {
                    vendor: Some("Apple".into()),
                    version: g("boot_rom_version"),
                    uefi: Some(true),
                    ..Default::default()
                };
            }
        }
    }
    b
}

/// Demo data: a rack server with a BMC, one power supply without AC.
pub fn demo() -> BoardInfo {
    let ipmi = |name: &str, kind: Kind, value: Option<f64>, unit: &'static str, state: Option<&str>, status: Status| BoardSensor {
        source: "IPMI".into(),
        name: name.into(),
        kind,
        value,
        unit,
        state: state.map(str::to_string),
        status,
    };
    let now = now() as u32;
    let pci = |addr: &str, class: u32, v: u16, d: u16, name: &str, drv: Option<&str>, w: (u8, u8)| PciDev {
        addr: addr.into(),
        class,
        vendor_id: v,
        device_id: d,
        name: name.into(),
        driver: drv.map(str::to_string),
        link: (w.0 > 0).then_some(Link { cur_gts: 8.0, cur_w: w.0, max_gts: 8.0, max_w: w.1, port_w: Some(w.1) }),
        aer: Some([0, 0, 0]),
    };
    let mut sensors: Vec<BoardSensor> = (1..=6)
        .map(|i| ipmi(&format!("Fan{i}"), Kind::Fan, Some(3000.0 + i as f64 * 120.0), "RPM", None, Status::Ok))
        .collect();
    sensors.extend([
        ipmi("Inlet Temp", Kind::Temp, Some(24.0), "°C", None, Status::Ok),
        ipmi("Exhaust Temp", Kind::Temp, Some(38.0), "°C", None, Status::Ok),
        ipmi("Temp (CPU 1)", Kind::Temp, Some(61.0), "°C", None, Status::Ok),
        ipmi("Temp (CPU 2)", Kind::Temp, Some(57.0), "°C", None, Status::Ok),
        ipmi("Status (PSU 1)", Kind::Power, None, "", Some("present, AC lost"), Status::Crit),
        ipmi("Status (PSU 2)", Kind::Power, None, "", Some("present"), Status::Ok),
        ipmi("Pwr Consumption", Kind::Power, Some(196.0), "W", None, Status::Ok),
        ipmi("Voltage 2", Kind::Volt, Some(220.0), "V", None, Status::Ok),
        ipmi("Intrusion", Kind::Other, None, "", Some("chassis closed"), Status::Ok),
    ]);
    BoardInfo {
        system: Ident { vendor: Some("Dell Inc.".into()), product: Some("PowerEdge R630".into()), version: None, serial: Some("DEMO123".into()) },
        board: Ident { vendor: Some("Dell Inc.".into()), product: Some("02C2CP".into()), version: Some("A07".into()), serial: None },
        chassis: Some("rack mount chassis".into()),
        bios: Bios { vendor: Some("Dell Inc.".into()), version: Some("2.19.0".into()), date: Some((2023, 12, 12)), uefi: Some(true), secure_boot: Some(false) },
        bmc_firmware: Some("2.86".into()),
        pci: vec![
            pci("0000:01:00.0", 0x020000, 0x14e4, 0x165f, "Broadcom NetXtreme BCM5720 Gigabit Ethernet", Some("tg3"), (1, 2)),
            pci("0000:02:00.0", 0x010400, 0x1000, 0x005f, "Broadcom / LSI MegaRAID SAS-3 3008 [Fury] (PERC H330)", Some("megaraid_sas"), (8, 8)),
            pci("0000:03:00.0", 0x030000, 0x102b, 0x0534, "Matrox G200eR2", Some("mgag200"), (0, 0)),
            pci("0000:00:1d.0", 0x0c0320, 0x8086, 0x8d26, "Intel C610/X99 USB Enhanced Host Controller", Some("ehci-pci"), (0, 0)),
        ],
        usb: vec![UsbDev { id: "1-1.5".into(), vendor_id: 0x04b8, product_id: 0x1188, name: "EPSON L3210 Series".into(), speed_mbps: 12.0 }],
        sensors,
        events: vec![
            Event { time: now.saturating_sub(40 * 86_400), sensor: "Intrusion".into(), text: "chassis opened".into(), status: Status::Warn },
            Event { time: now.saturating_sub(40 * 86_400), sensor: "Status (PSU 1)".into(), text: "AC lost".into(), status: Status::Crit },
            Event { time: now.saturating_sub(2 * 86_400), sensor: "Status (PSU 1)".into(), text: "AC lost".into(), status: Status::Crit },
        ],
        sel_entries: 28,
        ipmi_note: None,
        source: "demo".into(),
    }
}

impl BoardInfo {
    pub fn bios_age_years(&self) -> Option<f64> {
        let (y, m, d) = self.bios.date?;
        let then = (y as f64 - 1970.0) * 365.25 + (m as f64 - 1.0) * 30.44 + d as f64;
        Some((now() as f64 / 86_400.0 - then) / 365.25)
    }

    pub fn health(&self) -> BoardHealth {
        let mut sev = 0u8;
        let (mut issues, mut notes) = (Vec::new(), Vec::new());
        // A supply without AC while another one runs: the server is fine,
        // its power redundancy is not.
        let psu_ok = self
            .sensors
            .iter()
            .filter(|s| s.kind == Kind::Power && s.state.as_deref().is_some_and(|t| t.contains("present")) && s.status == Status::Ok)
            .count();
        for s in &self.sensors {
            if s.kind == Kind::Power && s.status == Status::Crit && psu_ok > 0 && s.state.as_deref().is_some_and(|t| t.contains("AC lost")) {
                sev = sev.max(2);
                issues.push(format!(
                    "{}: no AC input — the server runs on the other power supply (redundancy lost): check the cable / PDU",
                    s.name
                ));
                continue;
            }
            let what = match (&s.value, &s.state) {
                (Some(v), _) => format!("{} {v:.1}{}", s.name, if s.unit.is_empty() { String::new() } else { format!(" {}", s.unit) }),
                (None, Some(st)) => format!("{}: {st}", s.name),
                _ => s.name.clone(),
            };
            match s.status {
                Status::Crit => {
                    sev = sev.max(3);
                    issues.push(format!("{what} — critical ({})", s.source));
                }
                Status::Warn => {
                    sev = sev.max(2);
                    issues.push(format!("{what} — out of range ({})", s.source));
                }
                Status::Ok => {}
            }
        }
        // Event log: critical events of the last 30 days are an issue,
        // older ones a note.
        let recent = now().saturating_sub(30 * 86_400) as u32;
        let bad: Vec<&Event> = self.events.iter().filter(|e| e.status != Status::Ok).collect();
        let new: Vec<&&Event> = bad.iter().filter(|e| e.time >= recent).collect();
        if let Some(e) = new.last() {
            sev = sev.max(2);
            issues.push(format!(
                "{} warning/critical event(s) in the BMC log in the last 30 days, latest {} {}: {}",
                new.len(),
                fmt_time(e.time),
                e.sensor,
                e.text
            ));
        }
        if bad.len() > new.len() {
            notes.push(format!("{} older warning/critical event(s) in the BMC log (see EVENT LOG)", bad.len() - new.len()));
        }
        // Narrow links, grouped per (device name, width, allowed width).
        let mut narrow: Vec<((String, u8, u8), Vec<String>)> = Vec::new();
        for d in &self.pci {
            if let Some([cor, nonfatal, fatal]) = d.aer {
                if fatal > 0 || nonfatal > 0 {
                    sev = sev.max(2);
                    issues.push(format!("{} {}: PCIe errors ({fatal} fatal, {nonfatal} non-fatal) — reseat / check the card", d.addr, d.name));
                } else if cor >= 100 {
                    notes.push(format!("{} {}: {cor} corrected PCIe errors (signal quality: slot, riser, card)", d.addr, d.name));
                }
            }
            if d.internal() {
                continue;
            }
            if let Some(l) = &d.link {
                let expect = l.max_w.min(l.port_w.unwrap_or(l.max_w));
                if l.cur_w < expect {
                    // Often by design (onboard devices wired narrower), so a
                    // note.
                    let key = (d.name.clone(), l.cur_w, expect);
                    let addr = d.addr.trim_start_matches("0000:").to_string();
                    match narrow.iter_mut().find(|(k, _)| *k == key) {
                        Some((_, v)) => v.push(addr),
                        None => narrow.push((key, vec![addr])),
                    }
                }
            }
            if d.driver.is_none() && matches!(d.class >> 16, 0x01 | 0x02 | 0x03 | 0x04 | 0x0c) {
                notes.push(format!(
                    "{} {} ({}) has no driver: the OS cannot use it (driver / firmware missing, or disabled)",
                    d.addr,
                    d.name,
                    d.class_name()
                ));
            }
        }
        for ((name, cur, expect), addrs) in narrow {
            notes.push(format!(
                "{name} ({}): PCIe link x{cur} while card and slot allow x{expect} — normal for onboard wiring; \
                 if it used to run wider, reseat the card / check the riser",
                addrs.join(", ")
            ));
        }
        if let Some(age) = self.bios_age_years() {
            if age >= 5.0 {
                notes.push(format!(
                    "BIOS is {age:.0} years old ({}): check the vendor for updates (security fixes, CPU microcode)",
                    self.bios.date.map(|(y, m, d)| format!("{y}-{m:02}-{d:02}")).unwrap_or_default()
                ));
            }
        }
        let generic = |i: &Ident| i.vendor.as_deref().is_some_and(placeholder) || i.product.as_deref().is_some_and(placeholder) || i.version.as_deref().is_some_and(placeholder);
        if generic(&self.board) || generic(&self.system) {
            notes.push("the maker left the board identity unfilled (\"Default string\"): a generic / white-label board".into());
        }
        if self.bios.uefi == Some(true) && self.bios.secure_boot == Some(false) {
            notes.push("UEFI with Secure Boot off".into());
        }
        if let Some(n) = &self.ipmi_note {
            notes.push(n.clone());
        }
        if self.sensors.is_empty() && self.bmc_firmware.is_none() && self.source == "sysfs" {
            notes.push("no board sensors exposed (no BMC, and no hwmon driver for the board's sensor chip)".into());
        }
        let known = self.system.vendor.is_some() || self.board.product.is_some() || !self.pci.is_empty();
        let label = match sev {
            0 if !known => return BoardHealth { label: "UNKNOWN", severity: 1, issues, notes },
            0 => "OK",
            2 => "MONITOR",
            _ => "CRITICAL",
        };
        BoardHealth { label, severity: sev, issues, notes }
    }

    pub fn verdict(&self) -> (&'static str, u8) {
        let h = self.health();
        (h.label, h.severity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("dcheck-board-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&root);
        let w = |p: &str, v: &str| {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, v).unwrap();
        };
        // DMI of lab-243 (a generic X99 board).
        for (f, v) in [
            ("sys_vendor", "INTEL"),
            ("product_name", "X99"),
            ("product_version", "Default string"),
            ("board_vendor", "INTEL"),
            ("board_name", "X99"),
            ("board_version", "Default string"),
            ("chassis_type", "3"),
            ("bios_vendor", "American Megatrends Inc."),
            ("bios_version", "5.11"),
            ("bios_date", "03/05/2016"),
        ] {
            w(&format!("sys/class/dmi/id/{f}"), &format!("{v}\n"));
        }
        // GPU without a driver, NIC with a degraded link, a bridge.
        let dev = |a: &str, class: &str, v: &str, d: &str| {
            w(&format!("sys/bus/pci/devices/{a}/class"), class);
            w(&format!("sys/bus/pci/devices/{a}/vendor"), v);
            w(&format!("sys/bus/pci/devices/{a}/device"), d);
        };
        dev("0000:02:00.0", "0x030000", "0x1002", "0x6611");
        dev("0000:03:00.0", "0x020000", "0x8086", "0x1521");
        for (f, v) in [
            ("current_link_speed", "5.0 GT/s PCIe"),
            ("current_link_width", "1"),
            ("max_link_speed", "5.0 GT/s PCIe"),
            ("max_link_width", "4"),
            ("aer_dev_correctable", "RxErr 3\nTOTAL_ERR_COR 3\n"),
            ("aer_dev_nonfatal", "TOTAL_ERR_NONFATAL 0\n"),
            ("aer_dev_fatal", "TOTAL_ERR_FATAL 1\n"),
        ] {
            w(&format!("sys/bus/pci/devices/0000:03:00.0/{f}"), v);
        }
        std::fs::create_dir_all(root.join("sys/bus/pci/devices/0000:03:00.0")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/sys/bus/pci/drivers/igb", root.join("sys/bus/pci/devices/0000:03:00.0/driver")).unwrap();
        dev("0000:00:01.0", "0x060400", "0x8086", "0x2f02");
        w("usr/share/hwdata/pci.ids", "1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t6611  Oland [Radeon HD 8570]\n8086  Intel Corporation\n\t1521  I350 Gigabit Network Connection\n\t\t1028 1f60  subsystem\nC 00  Unclassified\n");
        // hwmon: a board chip with a failed fan and a CPU chip (ignored).
        w("sys/class/hwmon/hwmon0/name", "coretemp");
        w("sys/class/hwmon/hwmon0/temp1_input", "55000");
        w("sys/class/hwmon/hwmon1/name", "nct6775");
        w("sys/class/hwmon/hwmon1/fan1_input", "0");
        w("sys/class/hwmon/hwmon1/fan1_min", "300");
        w("sys/class/hwmon/hwmon1/fan1_label", "CPU_FAN");
        w("sys/class/hwmon/hwmon1/in0_input", "3312");
        w("sys/class/hwmon/hwmon1/in0_label", "3VCC");
        w("sys/class/hwmon/hwmon1/temp1_input", "41000");
        w("sys/class/hwmon/hwmon1/temp1_max", "80000");
        w("sys/bus/usb/devices/2-1/idVendor", "0c45");
        w("sys/bus/usb/devices/2-1/idProduct", "636b");
        w("sys/bus/usb/devices/2-1/product", "USB Camera");
        w("sys/bus/usb/devices/2-1/speed", "480");
        w("sys/bus/usb/devices/usb1/idVendor", "1d6b");
        w("sys/bus/usb/devices/usb1/idProduct", "0002");
        root
    }

    #[test]
    fn reads_a_board_tree() {
        let root = tree();
        let b = read_from(&root);
        assert_eq!((b.system.vendor.as_deref(), b.board.product.as_deref()), (Some("INTEL"), Some("X99")));
        assert_eq!(b.chassis.as_deref(), Some("desktop"));
        assert_eq!(b.bios.date, Some((2016, 3, 5)));
        assert_eq!(b.bios.uefi, Some(false));
        let gpu = b.pci.iter().find(|d| d.addr == "0000:02:00.0").unwrap();
        assert_eq!(gpu.name, "Advanced Micro Devices, Inc. [AMD/ATI] Oland [Radeon HD 8570]");
        assert!(gpu.driver.is_none());
        let nic = b.pci.iter().find(|d| d.addr == "0000:03:00.0").unwrap();
        assert_eq!(nic.driver.as_deref(), Some("igb"));
        assert_eq!(nic.aer, Some([3, 0, 1]));
        assert_eq!(nic.link.as_ref().map(|l| (l.cur_w, l.max_w)), Some((1, 4)));
        assert!(b.pci.iter().find(|d| d.addr == "0000:00:01.0").unwrap().internal());
        // coretemp is the CPU module's; the board chip is read.
        assert!(b.sensors.iter().all(|s| s.source == "nct6775"));
        let fan = b.sensors.iter().find(|s| s.name == "CPU_FAN").unwrap();
        assert_eq!(fan.status, Status::Warn);
        let v = b.sensors.iter().find(|s| s.name == "3VCC").unwrap();
        assert!((v.value.unwrap() - 3.312).abs() < 1e-9);
        assert_eq!(b.usb.len(), 1);
        assert_eq!(b.usb[0].name, "USB Camera");

        let h = b.health();
        assert_eq!(h.label, "MONITOR");
        let all = format!("{:?} {:?}", h.issues, h.notes);
        for e in ["CPU_FAN", "fatal", "x1 while card and slot allow x4", "no driver", "BIOS is", "white-label"] {
            assert!(all.contains(e), "missing {e:?} in {all}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ipmi_events_and_sensors_drive_the_verdict() {
        let mut b = BoardInfo { system: Ident { vendor: Some("Dell Inc.".into()), product: Some("PowerEdge R630".into()), ..Default::default() }, ..Default::default() };
        assert_eq!(b.health().label, "OK");
        let now = now() as u32;
        add_ipmi(
            &mut b,
            crate::ipmi::Ipmi {
                bmc_firmware: Some("2.83".into()),
                sensors: vec![
                    crate::ipmi::Sensor { name: "Fan1 RPM".into(), kind: 0x04, entity: (0x1D, 1), value: Some((3840.0, "RPM")), state: None, status: Status::Ok },
                    crate::ipmi::Sensor { name: "PS2 Status".into(), kind: 0x08, entity: (0x0A, 2), value: None, state: Some("present, AC lost".into()), status: Status::Crit },
                ],
                events: vec![
                    Event { time: 1_600_000_000, sensor: "Intrusion".into(), text: "chassis opened".into(), status: Status::Warn },
                    Event { time: now - 86_400, sensor: "PS2 Status".into(), text: "AC lost".into(), status: Status::Crit },
                ],
                sel_entries: 2,
                error: None,
            },
        );
        let h = b.health();
        // Only one supply: losing AC is critical.
        assert_eq!(h.label, "CRITICAL");
        assert!(h.issues.iter().any(|i| i.contains("PS2 Status: present, AC lost")));
        // With a working second supply it is lost redundancy.
        b.sensors.push(BoardSensor {
            source: "IPMI".into(),
            name: "PS1 Status".into(),
            kind: Kind::Power,
            value: None,
            unit: "",
            state: Some("present".into()),
            status: Status::Ok,
        });
        let h = b.health();
        assert_eq!(h.label, "MONITOR");
        assert!(h.issues.iter().any(|i| i.contains("redundancy lost")), "{:?}", h.issues);
        assert!(h.issues.iter().any(|i| i.contains("1 warning/critical event(s)")));
        assert!(h.notes.iter().any(|n| n.contains("1 older")));
        assert_eq!(b.sensors.iter().find(|s| s.name == "PS2 Status").unwrap().kind, Kind::Power);
    }

    #[test]
    fn dates_and_ids() {
        assert_eq!(parse_bios_date("12/12/2023"), Some((2023, 12, 12)));
        assert_eq!(parse_bios_date("13/01/2020"), None);
        assert_eq!(fmt_time(0), "1970-01-01 00:00");
        assert_eq!(fmt_time(1_780_992_000), "2026-06-09 08:00");
        assert!(placeholder("Default string") && placeholder("To be filled by O.E.M.") && !placeholder("PowerEdge R630"));
        let ids = parse_ids("8086  Intel Corporation\n\t1521  I350\n10ec  Realtek\n\t8168  RTL8111\n", &[(0x10ec, 0x8168)]);
        assert_eq!(ids.get(&(0x10ec, Some(0x8168))).map(String::as_str), Some("RTL8111"));
        assert!(!ids.contains_key(&(0x8086, None)));
    }
}
