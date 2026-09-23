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
    match smart {
        Some(s) => health_lines(d, &s, &mut out),
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

/// Prefer smartctl when it works; otherwise fall back to the native reader.
/// `DCHECK_NATIVE=1` forces the native path (used for testing).
fn read_smart(d: &Device) -> Option<smartctl::SmartData> {
    let force_native = std::env::var_os("DCHECK_NATIVE").is_some();
    let smartctl_result = if force_native {
        None
    } else {
        smartctl::read_smart(&d.path)
    };

    if let Some(s) = &smartctl_result {
        if s.error.is_none() {
            return smartctl_result;
        }
    }
    if let Some(native) = crate::native::read(d) {
        return Some(native);
    }
    smartctl_result
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
}
