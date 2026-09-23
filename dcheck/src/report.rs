//! Plain-text rendering of device lists and detailed reports.

use crate::health;
use crate::model::{Device, MediaKind};
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

fn mount_summary(dev: &Device) -> String {
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
            "?", // per-device SMART is shown in the report, not the list
            truncate(&mount_summary(d), 24),
        );
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

/// Print a full report for one device, including SMART health when available.
pub fn print_report(d: &Device) {
    let smart = read_smart(d);

    println!("dcheck report — {}", d.path);
    println!("{}", "=".repeat(60));

    // Prefer SMART identity fields when they are more complete.
    let vendor = d.vendor.clone();
    let model = smart
        .as_ref()
        .and_then(|s| s.model.clone())
        .or_else(|| d.model.clone());
    let serial = smart
        .as_ref()
        .and_then(|s| s.serial.clone())
        .or_else(|| d.serial.clone());
    let firmware = smart
        .as_ref()
        .and_then(|s| s.firmware.clone())
        .or_else(|| d.firmware.clone());

    println!("\n[ Identity ]");
    println!("  Device       : {}", d.path);
    println!("  Kernel name  : {}", d.name);
    println!("  Vendor       : {}", opt(&vendor));
    println!("  Model        : {}", opt(&model));
    println!("  Serial       : {}", opt(&serial));
    println!("  Firmware     : {}", opt(&firmware));
    println!("  Bus          : {}", d.bus);
    println!("  Type         : {}", d.kind);
    if let Some(ff) = smart.as_ref().and_then(|s| s.form_factor.clone()) {
        println!("  Form factor  : {ff}");
    }
    if let Some(rr) = smart.as_ref().and_then(|s| s.rotation_rate) {
        if rr > 0 {
            println!("  Rotation     : {rr} rpm");
        }
    }
    if d.removable {
        println!("  Removable    : yes");
    }

    println!("\n[ Capacity ]");
    println!("  Total        : {}", human_size(d.size_bytes));
    println!("  Raw bytes    : {} bytes", d.size_bytes);
    println!("  Block size   : {} bytes", d.logical_block_size);
    if d.partitions.is_empty() {
        println!("  Partitions   : none");
    } else {
        println!("  Partitions   : {}", d.partitions.len());
        for p in &d.partitions {
            let mount = p.mountpoint.as_deref().unwrap_or("not mounted");
            let fs = p.filesystem.as_deref().unwrap_or("-");
            println!(
                "    {:<16} {:>10}  {:<8} {}",
                p.path,
                human_size(p.size_bytes),
                fs,
                mount
            );
        }
    }

    println!("\n[ Interface ]");
    println!("  Transport    : {}", d.bus);
    match smart.as_ref() {
        Some(s) if s.interface_speed.is_some() || s.sata_version.is_some() => {
            let link = s.interface_speed.as_deref().unwrap_or("unknown");
            let ver = s.sata_version.as_deref().unwrap_or("");
            if ver.is_empty() {
                println!("  Link speed   : {link}");
            } else {
                println!("  Link speed   : {link} ({ver})");
            }
        }
        Some(_) if d.kind == MediaKind::Nvme => {
            println!("  PCIe link    : (link speed available in M4)");
        }
        Some(_) => println!("  SATA link    : (link speed available in M4)"),
        None => println!("  Link speed   : unavailable"),
    }

    println!("\n[ Health ]");
    match smart {
        Some(s) => print_health(d, &s),
        None => {
            println!("  SMART        : unavailable (run as root, or install smartmontools)");
            if d.bus == crate::model::Bus::Scsi {
                println!("  Note         : native SCSI/SAS health is not implemented yet");
            } else {
                println!("  Note         : run as root for raw-device SMART access");
            }
        }
    }

    println!();
}

fn print_health(d: &Device, s: &smartctl::SmartData) {
    let h = health::evaluate(d, s);

    println!("  Verdict      : {}", h.verdict.label());
    if !s.source.is_empty() {
        println!("  Source       : {}", s.source);
    }
    if let Some(passed) = s.passed {
        println!("  SMART status : {}", if passed { "passed" } else { "FAILED" });
    }
    if s.smart_available == Some(false) {
        println!("  SMART        : device reports SMART as unavailable");
    }
    if let Some(t) = s.temperature_c {
        println!("  Temperature  : {t}°C");
    }
    if let Some(poh) = s.power_on_hours {
        let cycles = s
            .power_cycles
            .map(|c| format!(", {c} power cycles"))
            .unwrap_or_default();
        println!("  Power-on     : {poh} h{cycles}");
    }
    if let Some(w) = h.tbw_bytes {
        println!("  Written      : {} (host)", human_size(w));
    }
    if let Some(r) = s.bytes_read() {
        println!("  Read         : {}", human_size(r));
    }
    if let Some(used) = h.wear_used_percent {
        println!("  Wear         : {used}% used");
    }
    match h.rated_tbw_bytes {
        Some(r) => println!("  Rated TBW    : {}", human_size(r)),
        None => println!("  Rated TBW    : unknown (model not in endurance table)"),
    }

    match (h.days_247, h.days_87, h.remaining_poh) {
        (Some(d247), Some(d87), Some(hours)) => {
            println!(
                "  Life left    : ~{hours} h  (~{}y @24/7  |  ~{}y @8/7)",
                fmt_years(d247),
                fmt_years(d87)
            );
        }
        _ => println!("  Life left    : unknown (insufficient endurance data)"),
    }
    println!("  Confidence   : {}", h.confidence.label());

    if !h.issues.is_empty() {
        println!("  Issues       :");
        for i in &h.issues {
            println!("    - {i}");
        }
    }
    for n in &h.notes {
        println!("  Note         : {n}");
    }

    if let Some(err) = &s.error {
        println!("  Error        : {err} (run as root)");
    }
}

fn fmt_years(days: u64) -> String {
    if days < 365 {
        format!("{days}d")
    } else {
        format!("{:.1}", days as f64 / 365.0)
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
}
