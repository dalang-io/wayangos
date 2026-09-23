//! Plain-text rendering of device lists and detailed reports.
//!
//! `device_report_lines` returns the report as lines so both the CLI and the
//! TUI can render it.

use crate::health;
use crate::model::Device;
use crate::smartctl;

/// Format a byte count in decimal units, e.g. `465 GB`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else if value >= 100.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

fn opt(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("-")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// First mount point among the device's partitions, or `-`.
pub fn mount_summary(dev: &Device) -> String {
    for p in &dev.partitions {
        if let Some(m) = &p.mountpoint {
            return m.clone();
        }
    }
    "-".to_string()
}

/// Print the storage device table.
pub fn print_list(devices: &[Device]) {
    if devices.is_empty() {
        eprintln!("No block devices found.");
        eprintln!("dcheck reads devices from /sys/block (Linux only).");
        return;
    }

    println!(
        "{:<14} {:<5} {:<7} {:<28} {:>10}  {:<9} {}",
        "DEVICE", "TYPE", "BUS", "MODEL", "CAPACITY", "HEALTH", "MOUNT"
    );
    for d in devices {
        println!(
            "{:<14} {:<5} {:<7} {:<28} {:>10}  {:<9} {}",
            d.path,
            d.kind.to_string(),
            d.bus.to_string(),
            truncate(&d.label(), 28),
            human_size(d.size_bytes),
            "?",
            truncate(&mount_summary(d), 24),
        );
    }
}

/// Print a full report for one device, including SMART health when available.
pub fn print_report(d: &Device) {
    for line in device_report_lines(d) {
        println!("{line}");
    }
}

/// Build the full report for one device as lines.
pub fn device_report_lines(d: &Device) -> Vec<String> {
    let smart = read_smart(d);
    let nid = crate::native::identity(d);
    let mut out: Vec<String> = Vec::new();

    out.push(format!("dcheck report — {}", d.path));
    out.push("=".repeat(60));

    // Prefer SMART, then native identity, then sysfs.
    let vendor = d.vendor.clone();
    let model = smart
        .as_ref()
        .and_then(|s| s.model.clone())
        .or_else(|| nid.model.clone())
        .or_else(|| d.model.clone());
    let serial = smart
        .as_ref()
        .and_then(|s| s.serial.clone())
        .or_else(|| nid.serial.clone())
        .or_else(|| d.serial.clone());
    let firmware = smart
        .as_ref()
        .and_then(|s| s.firmware.clone())
        .or_else(|| nid.firmware.clone())
        .or_else(|| d.firmware.clone());

    out.push(String::new());
    out.push("[ Identity ]".into());
    out.push(format!("  Device       : {}", d.path));
    out.push(format!("  Kernel name  : {}", d.name));
    out.push(format!("  Vendor       : {}", opt(&vendor)));
    out.push(format!("  Model        : {}", opt(&model)));
    out.push(format!("  Serial       : {}", opt(&serial)));
    out.push(format!("  Firmware     : {}", opt(&firmware)));
    out.push(format!("  Bus          : {}", d.bus));
    out.push(format!("  Type         : {}", d.kind));
    if let Some(ff) = smart.as_ref().and_then(|s| s.form_factor.clone()) {
        out.push(format!("  Form factor  : {ff}"));
    }
    if let Some(rr) = smart.as_ref().and_then(|s| s.rotation_rate) {
        if rr > 0 {
            out.push(format!("  Rotation     : {rr} rpm"));
        }
    }
    if d.removable {
        out.push("  Removable    : yes".into());
    }

    out.push(String::new());
    out.push("[ Capacity ]".into());
    out.push(format!("  Total        : {}", human_size(d.size_bytes)));
    out.push(format!("  Raw bytes    : {} bytes", d.size_bytes));
    out.push(format!("  Block size   : {} bytes", d.logical_block_size));
    if d.partitions.is_empty() {
        out.push("  Partitions   : none".into());
    } else {
        out.push(format!("  Partitions   : {}", d.partitions.len()));
        for p in &d.partitions {
            let mount = p.mountpoint.as_deref().unwrap_or("not mounted");
            let fs = p.filesystem.as_deref().unwrap_or("-");
            out.push(format!(
                "    {:<16} {:>10}  {:<8} {}",
                p.path,
                human_size(p.size_bytes),
                fs,
                mount
            ));
        }
    }

    out.push(String::new());
    out.push("[ Interface ]".into());
    out.push(format!("  Transport    : {}", d.bus));
    let speed = smart
        .as_ref()
        .and_then(|s| s.interface_speed.clone())
        .or_else(|| crate::native::link_speed(d));
    match speed {
        Some(sp) => {
            let ver = smart
                .as_ref()
                .and_then(|s| s.sata_version.clone())
                .unwrap_or_default();
            if ver.is_empty() {
                out.push(format!("  Link speed   : {sp}"));
            } else {
                out.push(format!("  Link speed   : {sp} ({ver})"));
            }
        }
        None => out.push("  Link speed   : unavailable".into()),
    }

    out.push(String::new());
    out.push("[ Health ]".into());
    match &smart {
        Some(s) => health_lines(d, s, &mut out),
        None => {
            out.push("  SMART        : unavailable (run as root, or install smartmontools)".into());
            if d.bus == crate::model::Bus::Scsi {
                out.push(
                    "  Note         : native SCSI/SAS health unavailable (controller may block LOG SENSE)"
                        .into(),
                );
            } else {
                out.push("  Note         : run as root for raw-device SMART access".into());
            }
        }
    }

    if let Some(s) = smart.as_ref() {
        if !s.attributes.is_empty() {
            out.push(String::new());
            out.push("[ SMART attributes ]".into());
            out.push(format!(
                "  {:<4} {:<28} {:>4} {:>4} {:>4} {:>14}  {}",
                "ID", "NAME", "VAL", "WST", "THR", "RAW", "ST"
            ));
            for a in &s.attributes {
                out.push(format!(
                    "  {:<4} {:<28} {:>4} {:>4} {:>4} {:>14}  {}",
                    a.id,
                    truncate(&a.name, 28),
                    a.value,
                    a.worst,
                    a.threshold,
                    a.raw,
                    a.status()
                ));
            }
        }
    }

    out
}

fn health_lines(d: &Device, s: &smartctl::SmartData, out: &mut Vec<String>) {
    let h = health::evaluate(d, s);

    out.push(format!("  Verdict      : {}", h.verdict.label()));
    if !s.source.is_empty() {
        out.push(format!("  Source       : {}", s.source));
    }
    if let Some(passed) = s.passed {
        out.push(format!(
            "  SMART status : {}",
            if passed { "passed" } else { "FAILED" }
        ));
    }
    if s.smart_available == Some(false) {
        out.push("  SMART        : device reports SMART as unavailable".into());
    }
    if let Some(t) = s.temperature_c {
        out.push(format!("  Temperature  : {t}°C"));
    }
    if let Some(poh) = s.power_on_hours {
        let cycles = s
            .power_cycles
            .map(|c| format!(", {c} power cycles"))
            .unwrap_or_default();
        out.push(format!("  Power-on     : {poh} h{cycles}"));
    }
    if let Some(w) = h.tbw_bytes {
        out.push(format!("  Written      : {} (host)", human_size(w)));
    }
    if let Some(r) = s.bytes_read() {
        out.push(format!("  Read         : {}", human_size(r)));
    }
    if let Some(used) = h.wear_used_percent {
        out.push(format!("  Wear         : {used}% used"));
    }
    if let Some(m) = s.media_errors {
        if m > 0 {
            out.push(format!("  Media errors : {m}"));
        }
    }
    if let (Some(spare), Some(thresh)) = (s.available_spare, s.available_spare_threshold) {
        out.push(format!("  Spare        : {spare}% (threshold {thresh}%)"));
    }
    if let Some(w) = s.warning_temp_time {
        if w > 0 {
            out.push(format!("  Temp warnings: {w} min above threshold"));
        }
    }
    if let Some(c) = s.critical_temp_time {
        if c > 0 {
            out.push(format!("  Critical temp: {c} min"));
        }
    }
    match h.rated_tbw_bytes {
        Some(r) => out.push(format!("  Rated TBW    : {}", human_size(r))),
        None => out.push("  Rated TBW    : unknown (model not in endurance table)".into()),
    }

    match (h.days_247, h.days_87, h.remaining_poh) {
        (Some(d247), Some(d87), Some(hours)) => out.push(format!(
            "  Life left    : ~{hours} h  (~{}y @24/7  |  ~{}y @8/7)",
            fmt_years(d247),
            fmt_years(d87)
        )),
        _ => out.push("  Life left    : unknown (insufficient endurance data)".into()),
    }
    out.push(format!("  Confidence   : {}", h.confidence.label()));

    if !h.issues.is_empty() {
        out.push("  Issues       :".into());
        for i in &h.issues {
            out.push(format!("    - {i}"));
        }
    }
    for n in &h.notes {
        out.push(format!("  Note         : {n}"));
    }
    if let Some(err) = &s.error {
        out.push(format!("  Error        : {err} (run as root)"));
    }
}

fn fmt_years(days: u64) -> String {
    if days < 365 {
        format!("{days}d")
    } else {
        format!("{:.1}", days as f64 / 365.0)
    }
}

/// Read and evaluate health for a device (`None` when SMART is unavailable).
pub fn health_summary(d: &Device) -> Option<health::Health> {
    device_metrics(d).map(|(_, h)| h)
}

/// Read SMART data and the evaluated health together.
pub fn device_metrics(d: &Device) -> Option<(smartctl::SmartData, health::Health)> {
    read_smart(d).map(|s| {
        let h = health::evaluate(d, &s);
        (s, h)
    })
}

/// Prometheus text exposition for all devices.
pub fn prometheus(devices: &[Device]) -> String {
    let mut out = String::new();
    out.push_str("# HELP dcheck_capacity_bytes Device capacity in bytes.\n");
    out.push_str("# TYPE dcheck_capacity_bytes gauge\n");
    for d in devices {
        out.push_str(&metric("dcheck_capacity_bytes", &d.path, None, d.size_bytes as f64));
    }
    out.push_str("# HELP dcheck_health_severity 0=ok 1=unknown 2=monitor 3=backup/replace.\n");
    out.push_str("# TYPE dcheck_health_severity gauge\n");
    for d in devices {
        if let Some((_, h)) = device_metrics(d) {
            let sev = match h.verdict {
                health::Verdict::Ok => 0.0,
                health::Verdict::Unknown => 1.0,
                health::Verdict::Monitor => 2.0,
                _ => 3.0,
            };
            out.push_str(&metric("dcheck_health_severity", &d.path, Some(h.verdict.label()), sev));
        }
    }
    let gauges: &[(&str, fn(&smartctl::SmartData) -> Option<f64>)] = &[
        ("dcheck_temperature_celsius", |s| s.temperature_c.map(|v| v as f64)),
        ("dcheck_power_on_hours", |s| s.power_on_hours.map(|v| v as f64)),
        ("dcheck_wear_used_percent", |s| s.life_percent.map(|p| (100u64.saturating_sub(p)) as f64)),
        ("dcheck_written_bytes", |s| s.bytes_written().map(|v| v as f64)),
        ("dcheck_media_errors", |s| s.media_errors.map(|v| v as f64)),
    ];
    for (name, get) in gauges {
        out.push_str(&format!("# TYPE {name} gauge\n"));
        for d in devices {
            if let Some((s, _)) = device_metrics(d) {
                if let Some(v) = get(&s) {
                    out.push_str(&metric(name, &d.path, None, v));
                }
            }
        }
    }
    out
}

fn metric(name: &str, device: &str, label: Option<&str>, value: f64) -> String {
    match label {
        Some(l) => format!("{name}{{device=\"{device}\",verdict=\"{l}\"}} {value}\n"),
        None => format!("{name}{{device=\"{device}\"}} {value}\n"),
    }
}

/// Prefer smartctl when it works; otherwise fall back to the native reader.
/// `DCHECK_NATIVE=1` forces the native path (used for testing).
fn read_smart(d: &Device) -> Option<smartctl::SmartData> {
    let force_native = std::env::var_os("DCHECK_NATIVE").is_some();
    let smartctl_result = if force_native {
        None
    } else {
        smartctl::read_smart_dev(d)
    };

    if let Some(s) = &smartctl_result {
        if s.error.is_none() {
            return smartctl_result;
        }
    }
    if let Some(native) = crate::native::read(d) {
        return Some(native);
    }
    if let Some(s) = smartctl_result {
        return Some(s);
    }
    // Platform-provided status (e.g. macOS `diskutil` SMART Status).
    d.smart_status.map(|passed| smartctl::SmartData {
        passed: Some(passed),
        source: "platform".to_string(),
        ..Default::default()
    })
}

/// Basic device object (no SMART read) for list output.
pub fn device_json_basic(d: &Device) -> crate::json::Json {
    let mut partitions = Vec::new();
    for p in &d.partitions {
        partitions.push(crate::json::object(vec![
            ("path", crate::json::string(p.path.clone())),
            ("size_bytes", crate::json::num(p.size_bytes as f64)),
            (
                "filesystem",
                opt_json(&p.filesystem),
            ),
            ("mountpoint", opt_json(&p.mountpoint)),
        ]));
    }
    crate::json::object(vec![
        ("device", crate::json::string(d.path.clone())),
        ("name", crate::json::string(d.name.clone())),
        ("vendor", opt_json(&d.vendor)),
        ("model", opt_json(&d.model)),
        ("bus", crate::json::string(d.bus.to_string())),
        ("type", crate::json::string(d.kind.to_string())),
        ("removable", crate::json::Json::Bool(d.removable)),
        ("capacity_bytes", crate::json::num(d.size_bytes as f64)),
        ("logical_block_size", crate::json::num(d.logical_block_size as f64)),
        ("partitions", crate::json::Json::Arr(partitions)),
    ])
}

/// Full device object including identity, interface and health.
pub fn device_json(d: &Device) -> crate::json::Json {
    let smart = read_smart(d);
    let nid = crate::native::identity(d);

    let model = smart
        .as_ref()
        .and_then(|s| s.model.clone())
        .or_else(|| nid.model.clone())
        .or_else(|| d.model.clone());
    let serial = smart
        .as_ref()
        .and_then(|s| s.serial.clone())
        .or_else(|| nid.serial.clone())
        .or_else(|| d.serial.clone());
    let firmware = smart
        .as_ref()
        .and_then(|s| s.firmware.clone())
        .or_else(|| nid.firmware.clone())
        .or_else(|| d.firmware.clone());

    let link_speed = smart
        .as_ref()
        .and_then(|s| s.interface_speed.clone())
        .or_else(|| crate::native::link_speed(d));

    let health = smart.as_ref().map(|s| {
        let h = health::evaluate(d, s);
        crate::json::object(vec![
            ("verdict", crate::json::string(h.verdict.label())),
            ("source", crate::json::string(s.source.clone())),
            ("passed", opt_bool(s.passed)),
            ("temperature_c", opt_num(s.temperature_c.map(|v| v as f64))),
            ("power_on_hours", opt_num(s.power_on_hours.map(|v| v as f64))),
            ("power_cycles", opt_num(s.power_cycles.map(|v| v as f64))),
            ("written_bytes", opt_num(h.tbw_bytes.map(|v| v as f64))),
            ("read_bytes", opt_num(s.bytes_read().map(|v| v as f64))),
            ("wear_used_percent", opt_num(h.wear_used_percent.map(|v| v as f64))),
            ("media_errors", opt_num(s.media_errors.map(|v| v as f64))),
            ("available_spare", opt_num(s.available_spare.map(|v| v as f64))),
            ("warning_temp_time", opt_num(s.warning_temp_time.map(|v| v as f64))),
            ("critical_temp_time", opt_num(s.critical_temp_time.map(|v| v as f64))),
            ("rated_tbw_bytes", opt_num(h.rated_tbw_bytes.map(|v| v as f64))),
            ("remaining_hours", opt_num(h.remaining_poh.map(|v| v as f64))),
            ("life_days_247", opt_num(h.days_247.map(|v| v as f64))),
            ("life_days_87", opt_num(h.days_87.map(|v| v as f64))),
            ("confidence", crate::json::string(h.confidence.label())),
            ("issues", {
                let v: Vec<crate::json::Json> =
                    h.issues.iter().map(|i| crate::json::string(i.clone())).collect();
                crate::json::Json::Arr(v)
            }),
            ("notes", {
                let v: Vec<crate::json::Json> =
                    h.notes.iter().map(|n| crate::json::string(n.clone())).collect();
                crate::json::Json::Arr(v)
            }),
        ])
    });

    // Merge basic fields with the extra full fields.
    let basic = device_json_basic(d);
    let mut map = match basic {
        crate::json::Json::Obj(m) => m,
        other => return other,
    };
    map.insert("serial".to_string(), opt_json(&serial));
    map.insert("firmware".to_string(), opt_json(&firmware));
    map.insert("model".to_string(), opt_json(&model));
    map.insert(
        "interface".to_string(),
        crate::json::object(vec![
            ("transport", crate::json::string(d.bus.to_string())),
            ("link_speed", opt_json(&link_speed)),
        ]),
    );
    map.insert("health".to_string(), health.unwrap_or(crate::json::Json::Null));

    let attrs: Vec<crate::json::Json> = smart
        .as_ref()
        .map(|s| {
            s.attributes
                .iter()
                .map(|a| {
                    crate::json::object(vec![
                        ("id", crate::json::num(a.id as f64)),
                        ("name", crate::json::string(a.name.clone())),
                        ("value", crate::json::num(a.value as f64)),
                        ("worst", crate::json::num(a.worst as f64)),
                        ("threshold", crate::json::num(a.threshold as f64)),
                        ("raw", crate::json::num(a.raw as f64)),
                        ("failing", crate::json::Json::Bool(a.failing())),
                        ("prefailure", crate::json::Json::Bool(a.prefailure)),
                        ("online", crate::json::Json::Bool(a.online)),
                        ("status", crate::json::string(a.status())),
                    ])
                })
                .collect()
        })
        .unwrap_or_default();
    map.insert("attributes".to_string(), crate::json::Json::Arr(attrs));

    crate::json::Json::Obj(map)
}

fn opt_json(v: &Option<String>) -> crate::json::Json {
    match v {
        Some(s) => crate::json::string(s.clone()),
        None => crate::json::Json::Null,
    }
}

fn opt_num(v: Option<f64>) -> crate::json::Json {
    match v {
        Some(n) => crate::json::num(n),
        None => crate::json::Json::Null,
    }
}

fn opt_bool(v: Option<bool>) -> crate::json::Json {
    match v {
        Some(b) => crate::json::Json::Bool(b),
        None => crate::json::Json::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1000), "1.0 KB");
        assert_eq!(human_size(465_000_000_000), "465 GB");
        assert_eq!(human_size(1_000_000_000_000), "1.0 TB");
        assert_eq!(human_size(300_000_000_000_000), "300 TB");
    }

    #[test]
    fn long_labels_are_truncated() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn device_json_has_core_fields() {
        let d = Device {
            name: "sda".into(),
            path: "/dev/sda".into(),
            vendor: Some("ATA".into()),
            model: Some("SSD 1TB".into()),
            firmware: None,
            serial: None,
            bus: crate::model::Bus::Sata,
            kind: crate::model::MediaKind::Ssd,
            size_bytes: 500_000_000_000,
            logical_block_size: 512,
            removable: false,
            smart_status: None,
            partitions: vec![],
        };
        let s = device_json_basic(&d).to_string();
        assert!(s.contains("\"device\":\"/dev/sda\""));
        assert!(s.contains("\"capacity_bytes\":500000000000"));
        assert!(s.contains("\"bus\":\"SATA\""));
    }
}
