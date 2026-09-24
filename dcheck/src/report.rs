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

/// Format a byte count in binary units, e.g. `32 GiB` (memory sizes).
pub fn human_size_bin(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value.fract() < 0.05 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

fn opt(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("-")
}

/// A simple text meter, e.g. `████████░░░░░░░░░░░░`. Uses block glyphs unless
/// `DCHECK_PLAIN`/`--plain` is set (then `#`/`-`).
pub fn bar(percent: f64, width: usize) -> String {
    let plain = std::env::var_os("DCHECK_PLAIN").is_some();
    let (full, empty) = if plain { ('#', '-') } else { ('█', '░') };
    let p = percent.clamp(0.0, 100.0);
    let filled = ((p / 100.0) * width as f64).round() as usize;
    let mut s = String::with_capacity(width);
    for i in 0..width {
        s.push(if i < filled { full } else { empty });
    }
    s
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
        "{:<14} {:<5} {:<7} {:<28} {:>10}  {:<9} MOUNT",
        "DEVICE", "TYPE", "BUS", "MODEL", "CAPACITY", "HEALTH"
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

/// Whether to use ASCII-only glyphs.
pub fn ui_plain() -> bool {
    std::env::var_os("DCHECK_PLAIN").is_some()
}

/// Section header, e.g. `▐ IDENTITY` (plain: `[ IDENTITY ]`).
pub fn section(name: &str) -> String {
    if ui_plain() {
        format!("[ {name} ]")
    } else {
        format!("▐ {name}")
    }
}

/// Horizontal rule.
fn rule() -> String {
    if ui_plain() {
        "-".repeat(64)
    } else {
        "─".repeat(64)
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
    device_report_lines_with(d, read_smart(d).as_ref())
}

/// Read SMART once and return both the report lines and the evaluated health
/// (for the TUI dashboard).
pub fn device_report(d: &Device) -> (Vec<String>, Option<(smartctl::SmartData, health::Health)>) {
    let smart = read_smart(d);
    let lines = device_report_lines_with(d, smart.as_ref());
    let metrics = smart.map(|s| {
        let h = health::evaluate(d, &s);
        (s, h)
    });
    (lines, metrics)
}

/// Report lines for already-read SMART data.
pub fn device_report_lines_with(d: &Device, smart: Option<&smartctl::SmartData>) -> Vec<String> {
    if let Some(reason) = &d.failure {
        return failed_port_lines(d, reason);
    }
    // SMART usually carries the identity; the native INQUIRY/IDENTIFY is
    // slow on some controllers, so only ask when something is missing.
    let nid = if smart.is_some_and(|s| s.model.is_some() && s.serial.is_some() && s.firmware.is_some()) {
        crate::native::IdInfo::default()
    } else {
        crate::native::identity(d)
    };
    let mut out: Vec<String> = Vec::new();

    let banner = if ui_plain() {
        format!("dcheck report - {}", d.path)
    } else {
        format!("▚ dcheck report · {}", d.path)
    };
    out.push(banner);
    out.push(rule());

    // Prefer SMART, then native identity, then sysfs.
    let vendor = d.vendor.clone();
    let model = smart
        .and_then(|s| s.model.clone())
        .or_else(|| nid.model.clone())
        .or_else(|| d.model.clone());
    let serial = smart
        .and_then(|s| s.serial.clone())
        .or_else(|| nid.serial.clone())
        .or_else(|| d.serial.clone());
    let firmware = smart
        .and_then(|s| s.firmware.clone())
        .or_else(|| nid.firmware.clone())
        .or_else(|| d.firmware.clone());

    out.push(String::new());
    out.push(section("IDENTITY"));
    out.push(format!("  Device       : {}", d.path));
    out.push(format!("  Kernel name  : {}", d.name));
    out.push(format!("  Vendor       : {}", opt(&vendor)));
    out.push(format!("  Model        : {}", opt(&model)));
    out.push(format!("  Serial       : {}", opt(&serial)));
    out.push(format!("  Firmware     : {}", opt(&firmware)));
    out.push(format!("  Bus          : {}", d.bus));
    out.push(format!("  Type         : {}", d.kind));
    if let Some(ff) = smart.and_then(|s| s.form_factor.clone()) {
        out.push(format!("  Form factor  : {ff}"));
    }
    if let Some(rr) = smart.and_then(|s| s.rotation_rate) {
        if rr > 0 {
            out.push(format!("  Rotation     : {rr} rpm"));
        }
    }
    if d.removable {
        out.push("  Removable    : yes".into());
    }
    authenticity_lines(
        &crate::authenticity::for_device(d, smart, model.as_deref().unwrap_or(""), serial.as_deref()),
        &mut out,
    );

    out.push(String::new());
    out.push(section("CAPACITY"));
    out.push(format!("  Total        : {}", human_size(d.size_bytes)));
    out.push(format!("  Raw bytes    : {} bytes", d.size_bytes));
    out.push(format!("  Block size   : {} bytes", d.logical_block_size));
    if d.partitions.is_empty() {
        out.push("  Partitions   : none".into());
    } else {
        out.push(format!("  Partitions   : {}", d.partitions.len()));
        for p in &d.partitions {
            let fs = p.filesystem.as_deref().unwrap_or("-");
            match p.mountpoint.as_deref() {
                Some(mp) => match crate::mount::usage(mp) {
                    Some(u) => out.push(format!(
                        "    {:<16} {:>10}  {:<8} {:<12} {} {:>3.0}%  {}/{} used, {} free",
                        p.path,
                        human_size(p.size_bytes),
                        fs,
                        mp,
                        bar(u.percent, 16),
                        u.percent,
                        human_size(u.used),
                        human_size(u.total),
                        human_size(u.avail)
                    )),
                    None => out.push(format!(
                        "    {:<16} {:>10}  {:<8} {}",
                        p.path,
                        human_size(p.size_bytes),
                        fs,
                        mp
                    )),
                },
                None => out.push(format!(
                    "    {:<16} {:>10}  {:<8} {}",
                    p.path,
                    human_size(p.size_bytes),
                    fs,
                    "not mounted"
                )),
            }
        }
    }

    out.push(String::new());
    out.push(section("INTERFACE"));
    out.push(format!("  Transport    : {}", d.bus));
    let speed = smart
        .and_then(|s| s.interface_speed.clone())
        .or_else(|| crate::native::link_speed(d));
    match speed {
        Some(sp) => {
            let ver = smart
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
    out.push(section("HEALTH"));
    match smart {
        Some(s) => health_lines(d, s, &mut out),
        None => {
            let (why, hint) = smart_unavailable(d);
            out.push(format!("  SMART        : unavailable — {why}"));
            if let Some(h) = hint {
                out.push(format!("  Hint         : {h}"));
            }
        }
    }

    if let Some(s) = smart {
        if !s.attributes.is_empty() {
            out.push(String::new());
            out.push(section("SMART ATTRIBUTES"));
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

/// Report for a SATA port whose drive never came up.
/// "Is it what the label says?" (see `authenticity.rs`).
fn authenticity_lines(a: &crate::authenticity::Authenticity, out: &mut Vec<String>) {
    use crate::authenticity::{Level, Mark};
    if a.level == Level::Unknown {
        return;
    }
    out.push(String::new());
    out.push(section("AUTHENTICITY"));
    out.push(format!("  Identity     : {} — {}", a.level.label(), a.summary()));
    if let Some(w) = &a.wwn {
        out.push(format!("  WWN          : {w}"));
    }
    for (mark, text) in &a.signals {
        let sym = match (mark, ui_plain()) {
            (Mark::Good, false) => "✔",
            (Mark::Bad, false) => "✖",
            (Mark::Info, false) => "·",
            (Mark::Good, true) => "+",
            (Mark::Bad, true) => "!",
            (Mark::Info, true) => "-",
        };
        out.push(format!("    {sym} {text}"));
    }
    match a.level {
        Level::Unbranded => out.push(
            "  Note         : if the label or casing shows a brand, this drive is not what it claims; \
             `dcheck verify` proves the real capacity"
                .into(),
        ),
        Level::Suspicious | Level::LikelyFake => out.push(
            "  Note         : likely rebranded/counterfeit — prove the real capacity with `dcheck verify`".into(),
        ),
        _ => {}
    }
}

fn failed_port_lines(d: &Device, reason: &str) -> Vec<String> {
    let banner = if ui_plain() {
        format!("dcheck report - {} (SATA port)", d.name)
    } else {
        format!("▚ dcheck report · {} (SATA port)", d.name)
    };
    vec![
        banner,
        rule(),
        String::new(),
        section("HEALTH"),
        "  Verdict      : REPLACE".into(),
        "  Source       : kernel log (no block device was created)".into(),
        format!("  Port         : {} — {reason}", d.name),
        "  Identity     : unknown (the drive never answered IDENTIFY)".into(),
        "  Next steps   : reseat / swap the data and power cable, try another port or".into(),
        "                 machine; if it fails everywhere the drive is dead (common".into(),
        "                 with counterfeit or rebranded SSDs) — replace / claim it".into(),
    ]
}

fn health_lines(d: &Device, s: &smartctl::SmartData, out: &mut Vec<String>) {
    let h = health::evaluate(d, s);

    out.push(format!("  Verdict      : {}", h.verdict.label()));
    if !s.source.is_empty() {
        out.push(format!("  Source       : {}", s.source));
    }
    if let Some(age) = crate::cache::age(d).filter(|a| *a >= 5) {
        out.push(format!(
            "  Data age     : read {} (cached; r in the UI or --fresh re-reads)",
            crate::cache::fmt_age(age)
        ));
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
        let mut extra = Vec::new();
        if let (Some(lo), Some(hi)) = (s.temp_min_c, s.temp_max_c) {
            extra.push(format!("lifetime {lo}–{hi}°C"));
        }
        if let Some(max) = s.temp_rated_max_c {
            extra.push(format!("rated max {max}°C"));
        }
        if let Some(trip) = s.trip_temp_c {
            extra.push(format!("trips at {trip}°C"));
        }
        if extra.is_empty() {
            out.push(format!("  Temperature  : {t}°C"));
        } else {
            out.push(format!("  Temperature  : {t}°C ({})", extra.join(", ")));
        }
    }
    let age = s.manufactured.and_then(|(y, w)| age_years(y, w));
    match s.manufactured {
        Some((y, w)) => {
            let old = age.map(|a| format!(" — {a:.1} years old")).unwrap_or_default();
            out.push(format!("  Manufactured : {y} week {w}{old}"));
        }
        None => out.push(format!("  Manufactured : {}", manufacture_unknown(d, s))),
    }
    if let Some(line) = in_service(s, age) {
        out.push(format!("  In service   : {line}"));
    }
    if let Some(poh) = s.power_on_hours {
        let cycles = s
            .power_cycles
            .map(|c| format!(", {c} power cycles"))
            .unwrap_or_default();
        out.push(format!("  Power-on     : {poh} h{cycles}"));
    }
    if let (Some(a), Some(r)) = (s.power_cycles, s.rated_start_stop) {
        out.push(format!("  Start-stop   : {a} of {r} rated"));
    }
    if let Some(lu) = s.load_unload {
        match s.rated_load_unload {
            Some(r) => out.push(format!("  Load-unload  : {lu} of {r} rated")),
            None => out.push(format!("  Load-unload  : {lu}")),
        }
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
    if (d.bus == crate::model::Bus::Scsi || s.rated_start_stop.is_some()) && !s.is_ata() {
        if let Some(g) = s.reallocated {
            out.push(format!("  Grown defects: {g}"));
        }
        if let Some(u) = s.uncorrectable {
            out.push(format!("  Uncorrected  : {u} errors"));
        }
        if let Some(n) = s.non_medium_errors {
            out.push(format!("  Non-medium   : {n} errors (transport/controller)"));
        }
    }
    if let Some(t) = &s.last_self_test {
        out.push(format!("  Self-test    : {t}"));
    }
    if let Some(n) = s.error_log_count {
        out.push(format!("  Error log    : {n} entries"));
    }
    if let Some(r) = s.hardware_resets {
        out.push(format!("  HW resets    : {r}"));
    }
    let mut features = Vec::new();
    if let Some(t) = s.trim {
        features.push(if t { "TRIM" } else { "no TRIM" });
    }
    if let Some(w) = s.write_cache {
        features.push(if w { "write cache on" } else { "write cache off" });
    }
    if !features.is_empty() {
        out.push(format!("  Features     : {}", features.join(", ")));
    }
    if let Some([inv, disp, sync, reset]) = s.phy_errors {
        out.push(format!(
            "  SAS phy      : invalid dword {inv}, disparity {disp}, loss of sync {sync}, reset problems {reset}"
        ));
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
    if let (Some(used), Some(what), Some(hours)) = (h.design_life_used, h.design_limit, h.design_hours) {
        out.push(format!(
            "  Design life  : {hours} h ({:.1} y @24/7, assumed — drives do not report it)",
            hours as f64 / (365.0 * 24.0)
        ));
        out.push(format!("  Life used    : {used}% (limited by {what})"));
    } else {
        match h.rated_tbw_bytes {
            Some(r) => out.push(format!(
                "  Rated TBW    : {} ({})",
                human_size(r),
                h.rated_tbw_source.unwrap_or("built-in table")
            )),
            None => out.push("  Rated TBW    : unknown (model not in endurance table)".into()),
        }
    }
    match life_left(&h) {
        Some(text) => out.push(format!("  Life left    : {text}")),
        None => out.push("  Life left    : unknown (insufficient endurance data)".into()),
    }
    if let Some(basis) = h.life_basis {
        out.push(format!("  Estimate from: {basis}"));
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

/// "~4.2y @24/7 | ~12.6y @8h/day", or how far past its rated life it is.
/// Not capped: long projections from little wear are shown as computed
/// (the confidence line says how much to trust them).
pub fn life_left(h: &health::Health) -> Option<String> {
    let fmt = |d: u64| {
        let y = fmt_years(d);
        if y.ends_with('d') {
            format!("~{y}")
        } else {
            format!("~{y}y")
        }
    };
    if h.remaining_poh == Some(0) {
        return Some(match h.overdue_poh {
            Some(over) => format!(
                "0 — past its rated life by {} @24/7",
                fmt(over / 24)
            ),
            None => "0 — at or beyond its rated life".into(),
        });
    }
    let (d247, d87) = (h.days_247?, h.days_87?);
    Some(format!("{} @24/7  |  {} @8h/day", fmt(d247), fmt(d87)))
}

/// Why no manufacture date is shown. Only SCSI/SAS drives carry one (log
/// page 0x0E); ATA and NVMe have no such field.
pub fn manufacture_unknown(d: &Device, s: &smartctl::SmartData) -> &'static str {
    if d.bus == crate::model::Bus::Scsi && !s.is_ata() {
        "not reported by the drive"
    } else {
        "not reported (SATA/NVMe drives do not store a manufacture date)"
    }
}

/// "2785 h powered on ≈ 0.3 y @24/7, 32 power-on resets[, on 20% of the
/// time since manufacture]".
pub fn in_service(s: &smartctl::SmartData, age_years: Option<f64>) -> Option<String> {
    let poh = s.power_on_hours?;
    let years = poh as f64 / (365.0 * 24.0);
    let mut out = format!("{poh} h powered on ≈ {years:.1} y @24/7");
    if let Some(r) = s.power_on_resets {
        out.push_str(&format!(", {r} power-on resets"));
    }
    if let Some(age) = age_years.filter(|a| *a > 0.0) {
        let duty = (years / age * 100.0).min(100.0);
        out.push_str(&format!(", on {duty:.0}% of the time since manufacture"));
    }
    Some(out)
}

/// Years since a (year, ISO week) manufacture date.
pub fn age_years(year: u16, week: u8) -> Option<f64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs_f64();
    let now_years = 1970.0 + now / (365.2425 * 86400.0);
    let made = year as f64 + (week.clamp(1, 53) as f64 - 1.0) / 52.18;
    (now_years >= made).then_some(now_years - made)
}

pub fn fmt_years(days: u64) -> String {
    if days < 365 {
        format!("{days}d")
    } else {
        format!("{:.1}", days as f64 / 365.0)
    }
}

/// RAM report lines.
pub fn ram_report_lines(r: &crate::ram::RamInfo) -> Vec<String> {
    let mut out = Vec::new();
    out.push(section("MEMORY"));
    out.push(format!("  Total        : {}", human_size_bin(r.total_bytes)));
    out.push(format!(
        "  Used         : {} ({:.0}%)  {}",
        human_size_bin(r.used_bytes()),
        r.used_percent(),
        bar(r.used_percent(), 20)
    ));
    out.push(format!("  Available    : {}", human_size_bin(r.available_bytes)));
    if r.swap_total_bytes > 0 {
        out.push(format!(
            "  Swap         : {} used of {}",
            human_size_bin(r.swap_total_bytes.saturating_sub(r.swap_free_bytes)),
            human_size_bin(r.swap_total_bytes)
        ));
    } else {
        out.push("  Swap         : none".to_string());
    }
    if let Some(t) = r.memory_type() {
        out.push(format!("  Type         : {t}"));
    }
    let on_package = r.on_package();
    if on_package {
        out.push("  Layout       : on-package (unified memory, not replaceable)".to_string());
    } else if r.slots_total > 0 || !r.modules.is_empty() {
        let populated = r.populated();
        let slots = if r.slots_total > 0 { format!(" / {}", r.slots_total) } else { String::new() };
        if populated != r.modules.len() {
            out.push(format!(
                "  DIMM slots   : {populated} used{slots} (firmware lists {})",
                r.modules.len()
            ));
        } else {
            out.push(format!("  DIMM slots   : {populated} used{slots}"));
        }
    }
    if let Some(t) = r.ram_temp_c {
        out.push(format!("  Temperature  : {t}°C"));
    }
    out.push(format!("  Source       : {}", r.source));
    out.push(String::new());
    out.push(section("HEALTH"));
    let (label, sev) = r.verdict();
    out.push(format!("  Verdict      : {label}"));
    if r.ecc_correctable > 0 || r.ecc_uncorrectable > 0 {
        out.push(format!(
            "  ECC          : {} correctable, {} uncorrectable",
            r.ecc_correctable, r.ecc_uncorrectable
        ));
    } else {
        if r.edac_dimms.is_empty() {
            out.push("  ECC          : no errors reported (or EDAC unavailable)".to_string());
        } else {
            out.push(format!(
                "  ECC          : 0 errors on {} DIMM(s) monitored by the memory controller",
                r.edac_dimms.len()
            ));
        }
    }
    let _ = sev;
    for n in r.notes() {
        out.push(format!("  Note         : {n}"));
    }
    if !r.edac_dimms.is_empty() {
        out.push(String::new());
        out.push(section("MEMORY CONTROLLER (EDAC)"));
        for d in &r.edac_dimms {
            out.push(format!(
                "  {:<34} {:>8}  CE {:<4} UE {}",
                truncate(&d.label, 34),
                human_size_bin(d.size_bytes),
                d.ce,
                d.ue
            ));
        }
    }
    if !r.modules.is_empty() {
        out.push(String::new());
        out.push(section(if on_package { "MODULES (SYSTEM PROFILER)" } else { "MODULES (FIRMWARE / SMBIOS)" }));
        let hidden = r.populated().saturating_sub(r.modules.len());
        for m in &r.modules {
            let speed = m
                .speed_mts
                .map(|s| format!("{s} MT/s"))
                .unwrap_or_default();
            let vendor = m.manufacturer.clone().unwrap_or_default();
            let part = m.part_number.clone().unwrap_or_default();
            out.push(format!(
                "  {:<6} {:>6}  {:<6} {:<10} {:<12} {}",
                m.locator,
                human_size_bin(m.size_bytes),
                m.kind,
                speed,
                vendor,
                part
            ));
        }
        if hidden > 0 {
            out.push(format!(
                "  (+{hidden} more module(s) installed but not described by the firmware — a BIOS table bug, not a memory fault; a BIOS update may fix it)"
            ));
        }
    }
    out
}

/// CPU report lines.
pub fn cpu_report_lines(c: &crate::cpu::CpuInfo) -> Vec<String> {
    let mut out = Vec::new();
    out.push(section("CPU"));
    let model = if c.model.is_empty() { "-" } else { &c.model };
    out.push(format!("  Model        : {model}"));
    if let Some(v) = &c.vendor {
        out.push(format!("  Vendor       : {v}"));
    }
    out.push(format!("  Sockets      : {}", c.sockets));
    out.push(format!("  Cores        : {}", c.cores));
    out.push(format!("  Threads      : {}", c.threads));
    match (c.mhz, c.max_mhz) {
        (Some(cur), Some(max)) => out.push(format!("  Clock        : {cur:.0} MHz (max {max:.0})")),
        (Some(cur), None) => out.push(format!("  Clock        : {cur:.0} MHz")),
        (None, Some(max)) => out.push(format!("  Clock        : max {max:.0} MHz")),
        (None, None) => {}
    }
    if let Some(kb) = c.cache_kb {
        out.push(format!("  Cache        : {kb} KB"));
    }
    if c.sensors.len() > 1 || c.sensors.iter().any(|s| s.high_c.is_some() || s.crit_c.is_some()) {
        for s in &c.sensors {
            let mut lim = Vec::new();
            if let Some(h) = s.high_c {
                lim.push(format!("high {h}°C"));
            }
            if let Some(cr) = s.crit_c {
                lim.push(format!("crit {cr}°C"));
            }
            let lim = if lim.is_empty() { String::new() } else { format!(" ({})", lim.join(", ")) };
            out.push(format!("  {:<13}: {}°C{lim}", truncate(&s.label, 13), s.temp_c));
        }
    } else if let Some(t) = c.temp_c {
        out.push(format!("  Temperature  : {t}°C"));
    }
    if let Some(l) = c.load1 {
        let pct = if c.cores > 0 {
            (l / c.cores as f64 * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        out.push(format!("  Load (1m)    : {l:.2}  {}", bar(pct, 20)));
    }
    out.push(format!("  Source       : {}", c.source));
    out.push(String::new());
    out.push(section("HEALTH"));
    let h = c.health();
    out.push(format!("  Verdict      : {}", h.label));
    if !h.issues.is_empty() {
        out.push("  Issues       :".into());
        for i in &h.issues {
            out.push(format!("    - {i}"));
        }
    }
    for n in &h.notes {
        out.push(format!("  Note         : {n}"));
    }
    if h.issues.is_empty() && h.notes.is_empty() && c.temp_c.is_some() {
        out.push("  Reason       : all temperatures below their limits".into());
    }
    out
}

/// RAM JSON object.
pub fn ram_json(r: &crate::ram::RamInfo) -> crate::json::Json {
    let (verdict, _) = r.verdict();
    crate::json::object(vec![
        ("total_bytes", crate::json::num(r.total_bytes as f64)),
        ("used_bytes", crate::json::num(r.used_bytes() as f64)),
        ("available_bytes", crate::json::num(r.available_bytes as f64)),
        ("used_percent", crate::json::num(r.used_percent())),
        ("swap_total_bytes", crate::json::num(r.swap_total_bytes as f64)),
        ("swap_free_bytes", crate::json::num(r.swap_free_bytes as f64)),
        ("ecc_correctable", crate::json::num(r.ecc_correctable as f64)),
        ("ecc_uncorrectable", crate::json::num(r.ecc_uncorrectable as f64)),
        ("memory_type", opt_json(&r.memory_type())),
        ("slots_total", crate::json::num(r.slots_total as f64)),
        ("slots_populated", crate::json::num(r.populated() as f64)),
        ("estimated_modules", opt_num(r.estimated_modules().map(|v| v as f64))),
        (
            "edac_dimms",
            crate::json::Json::Arr(
                r.edac_dimms
                    .iter()
                    .map(|d| {
                        crate::json::object(vec![
                            ("label", crate::json::string(d.label.clone())),
                            ("size_bytes", crate::json::num(d.size_bytes as f64)),
                            ("ce", crate::json::num(d.ce as f64)),
                            ("ue", crate::json::num(d.ue as f64)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "notes",
            crate::json::Json::Arr(r.notes().into_iter().map(crate::json::string).collect()),
        ),
        ("ram_temp_c", opt_num(r.ram_temp_c.map(|v| v as f64))),
        ("modules", {
            let v: Vec<crate::json::Json> = r
                .modules
                .iter()
                .map(|m| {
                    crate::json::object(vec![
                        ("locator", crate::json::string(m.locator.clone())),
                        ("size_bytes", crate::json::num(m.size_bytes as f64)),
                        ("kind", crate::json::string(m.kind.clone())),
                        ("speed_mts", opt_num(m.speed_mts.map(|v| v as f64))),
                        ("manufacturer", opt_json(&m.manufacturer)),
                        ("part_number", opt_json(&m.part_number)),
                        ("rank", opt_num(m.rank.map(|v| v as f64))),
                    ])
                })
                .collect();
            crate::json::Json::Arr(v)
        }),
        ("verdict", crate::json::string(verdict)),
        ("source", crate::json::string(r.source.clone())),
    ])
}

/// CPU JSON object.
pub fn cpu_json(c: &crate::cpu::CpuInfo) -> crate::json::Json {
    let h = c.health();
    let strings = |v: &[String]| {
        crate::json::Json::Arr(v.iter().map(|s| crate::json::string(s.clone())).collect())
    };
    let sensors = crate::json::Json::Arr(
        c.sensors
            .iter()
            .map(|s| {
                crate::json::object(vec![
                    ("label", crate::json::string(s.label.clone())),
                    ("temp_c", crate::json::num(s.temp_c as f64)),
                    ("high_c", opt_num(s.high_c.map(|v| v as f64))),
                    ("crit_c", opt_num(s.crit_c.map(|v| v as f64))),
                ])
            })
            .collect(),
    );
    crate::json::object(vec![
        ("model", crate::json::string(c.model.clone())),
        ("vendor", opt_json(&c.vendor)),
        ("sockets", crate::json::num(c.sockets as f64)),
        ("cores", crate::json::num(c.cores as f64)),
        ("threads", crate::json::num(c.threads as f64)),
        ("mhz", opt_num(c.mhz)),
        ("max_mhz", opt_num(c.max_mhz)),
        ("cache_kb", opt_num(c.cache_kb.map(|v| v as f64))),
        ("temp_c", opt_num(c.temp_c.map(|v| v as f64))),
        ("sensors", sensors),
        ("load1", opt_num(c.load1)),
        ("verdict", crate::json::string(h.label)),
        ("issues", strings(&h.issues)),
        ("notes", strings(&h.notes)),
        ("source", crate::json::string(c.source.clone())),
    ])
}

/// Why SMART data is missing for `d`, plus an optional hint.
pub fn smart_unavailable(d: &Device) -> (String, Option<String>) {
    use crate::model::Bus;
    if !crate::native::is_root() && !crate::enumerate::is_demo() {
        return (
            "reading SMART needs root".into(),
            Some("run `sudo dcheck`".into()),
        );
    }
    let why = match d.bus {
        Bus::Scsi => "the drive/controller rejected the SCSI log pages",
        Bus::Usb => "the USB bridge does not pass SMART through",
        Bus::Mmc => "SD/eMMC cards do not report SMART",
        _ => "the drive returned no SMART data",
    };
    let hint = match d.bus {
        Bus::Mmc => None,
        Bus::Scsi | Bus::Usb if !smartctl_installed() => Some(
            "smartmontools, if installed, is tried as a fallback (it knows vendor passthroughs such as -d sat / megaraid,N)"
                .to_string(),
        ),
        _ => None,
    };
    (why.into(), hint)
}

fn smartctl_installed() -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|p| p.join("smartctl").is_file())
    })
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
    let all = metrics_all(devices);
    let mut out = String::new();
    out.push_str("# HELP dcheck_capacity_bytes Device capacity in bytes.\n");
    out.push_str("# TYPE dcheck_capacity_bytes gauge\n");
    for d in devices {
        out.push_str(&metric("dcheck_capacity_bytes", &d.path, None, d.size_bytes as f64));
    }
    out.push_str("# HELP dcheck_health_severity 0=ok 1=unknown 2=monitor 3=backup/replace.\n");
    out.push_str("# TYPE dcheck_health_severity gauge\n");
    for (d, m) in devices.iter().zip(&all) {
        if let Some((_, h)) = m {
            let sev = match h.verdict {
                health::Verdict::Ok => 0.0,
                health::Verdict::Unknown => 1.0,
                health::Verdict::Monitor => 2.0,
                _ => 3.0,
            };
            out.push_str(&metric("dcheck_health_severity", &d.path, Some(h.verdict.label()), sev));
        }
    }
    type Gauge = (&'static str, fn(&smartctl::SmartData) -> Option<f64>);
    let gauges: &[Gauge] = &[
        ("dcheck_temperature_celsius", |s| s.temperature_c.map(|v| v as f64)),
        ("dcheck_power_on_hours", |s| s.power_on_hours.map(|v| v as f64)),
        ("dcheck_wear_used_percent", |s| s.life_percent.map(|p| (100u64.saturating_sub(p)) as f64)),
        ("dcheck_written_bytes", |s| s.bytes_written().map(|v| v as f64)),
        ("dcheck_media_errors", |s| s.media_errors.map(|v| v as f64)),
    ];
    for (name, get) in gauges {
        out.push_str(&format!("# TYPE {name} gauge\n"));
        for (d, m) in devices.iter().zip(&all) {
            if let Some((s, _)) = m {
                if let Some(v) = get(s) {
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
    crate::cache::smart(d, || read_smart_uncached(d))
}

/// SMART + health for every device, read in parallel (slow controllers answer
/// one command at a time per disk, not per host).
pub fn metrics_all(devices: &[Device]) -> Vec<Option<(smartctl::SmartData, health::Health)>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = devices.iter().map(|d| scope.spawn(move || device_metrics(d))).collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    })
}

fn read_smart_uncached(d: &Device) -> Option<smartctl::SmartData> {
    if crate::enumerate::is_demo() {
        return crate::enumerate::demo_smart(d);
    }
    // A port the kernel gave up on: nothing to read, the log is the evidence.
    if d.failure.is_some() {
        return Some(smartctl::SmartData {
            source: "kernel log".into(),
            passed: Some(false),
            ..Default::default()
        });
    }
    // Both sources are read and merged: smartctl (when installed) knows vendor
    // quirks, the native ioctl path fills whatever smartctl leaves out.
    let force_native = std::env::var_os("DCHECK_NATIVE").is_some();
    let smartctl = if force_native {
        None
    } else {
        smartctl::read_smart_dev(d)
    };
    let native = crate::native::read(d);

    match (smartctl, native) {
        (Some(mut s), Some(n)) if s.error.is_none() => {
            s.fill_from(&n);
            Some(s)
        }
        (Some(s), None) if s.error.is_none() => Some(s),
        (_, Some(n)) => Some(n),
        (Some(s), None) => Some(s),
        // Platform-provided status (e.g. macOS `diskutil` SMART Status).
        (None, None) => d.smart_status.map(|passed| smartctl::SmartData {
            passed: Some(passed),
            source: "platform".to_string(),
            ..Default::default()
        }),
    }
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
    let nid = if smart.as_ref().is_some_and(|s| s.model.is_some() && s.serial.is_some() && s.firmware.is_some()) {
        crate::native::IdInfo::default()
    } else {
        crate::native::identity(d)
    };

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
            ("rated_tbw_source", h.rated_tbw_source.map_or(crate::json::Json::Null, crate::json::string)),
            ("reallocated", opt_num(s.reallocated.map(|v| v as f64))),
            ("pending", opt_num(s.pending.map(|v| v as f64))),
            ("uncorrectable", opt_num(s.uncorrectable.map(|v| v as f64))),
            ("non_medium_errors", opt_num(s.non_medium_errors.map(|v| v as f64))),
            (
                "manufactured",
                match s.manufactured {
                    Some((y, w)) => crate::json::string(format!("{y}-W{w:02}")),
                    None => crate::json::Json::Null,
                },
            ),
            ("start_stop_cycles_rated", opt_num(s.rated_start_stop.map(|v| v as f64))),
            ("load_unload_cycles", opt_num(s.load_unload.map(|v| v as f64))),
            ("load_unload_cycles_rated", opt_num(s.rated_load_unload.map(|v| v as f64))),
            ("trip_temperature_c", opt_num(s.trip_temp_c.map(|v| v as f64))),
            ("temperature_min_c", opt_num(s.temp_min_c.map(|v| v as f64))),
            ("temperature_max_c", opt_num(s.temp_max_c.map(|v| v as f64))),
            ("temperature_rated_max_c", opt_num(s.temp_rated_max_c.map(|v| v as f64))),
            ("power_on_resets", opt_num(s.power_on_resets.map(|v| v as f64))),
            ("hardware_resets", opt_num(s.hardware_resets.map(|v| v as f64))),
            ("error_log_entries", opt_num(s.error_log_count.map(|v| v as f64))),
            ("trim", opt_bool(s.trim)),
            ("write_cache", opt_bool(s.write_cache)),
            ("last_self_test", opt_json(&s.last_self_test)),
            (
                "sas_phy_errors",
                match s.phy_errors {
                    Some([inv, disp, sync, reset]) => crate::json::object(vec![
                        ("invalid_dword", crate::json::num(inv as f64)),
                        ("running_disparity", crate::json::num(disp as f64)),
                        ("loss_of_dword_sync", crate::json::num(sync as f64)),
                        ("phy_reset_problem", crate::json::num(reset as f64)),
                    ]),
                    None => crate::json::Json::Null,
                },
            ),
            ("design_life_used_percent", opt_num(h.design_life_used.map(|v| v as f64))),
            ("design_life_hours", opt_num(h.design_hours.map(|v| v as f64))),
            (
                "design_life_assumed",
                if h.design_hours.is_some() { crate::json::Json::Bool(true) } else { crate::json::Json::Null },
            ),
            ("design_life_limit", opt_json(&h.design_limit.map(str::to_string))),
            ("life_basis", opt_json(&h.life_basis.map(str::to_string))),
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
    map.insert(
        "authenticity".to_string(),
        crate::authenticity::to_json(&crate::authenticity::for_device(
            d,
            smart.as_ref(),
            model.as_deref().unwrap_or(""),
            serial.as_deref(),
        )),
    );

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
    fn memory_sizes_are_binary() {
        assert_eq!(human_size_bin(32 << 30), "32 GiB");
        assert_eq!(human_size_bin(131_727_204 * 1024), "125.6 GiB");
        assert_eq!(human_size_bin(512 << 20), "512 MiB");
    }

    #[test]
    fn in_service_and_manufacture_lines() {
        let s = crate::smartctl::SmartData {
            power_on_hours: Some(25_660),
            power_on_resets: Some(40),
            ..Default::default()
        };
        let line = in_service(&s, Some(14.5)).unwrap();
        assert!(line.starts_with("25660 h powered on ≈ 2.9 y @24/7, 40 power-on resets"), "{line}");
        assert!(line.ends_with("on 20% of the time since manufacture"), "{line}");
        let d = crate::enumerate::demo_devices().remove(1); // SATA
        assert!(manufacture_unknown(&d, &s).contains("SATA/NVMe"));
        // A SATA SSD behind a SAS/RAID controller shows up on the SCSI bus.
        let mut behind_raid = d.clone();
        behind_raid.bus = crate::model::Bus::Scsi;
        let ata = crate::smartctl::SmartData {
            sata_version: Some("SATA 3.2".into()),
            ..Default::default()
        };
        assert!(manufacture_unknown(&behind_raid, &ata).contains("SATA/NVMe"));
        let sas = crate::smartctl::SmartData::default();
        assert_eq!(manufacture_unknown(&behind_raid, &sas), "not reported by the drive");
    }

    #[test]
    fn life_left_is_capped_and_handles_zero() {
        let d = crate::enumerate::demo_devices().remove(0);
        let s = crate::smartctl::SmartData {
            passed: Some(true),
            power_on_hours: Some(2785),
            life_percent: Some(99), // 1% wear -> ~31 y extrapolated
            ..Default::default()
        };
        let h = health::evaluate(&d, &s);
        // Not capped: 1% in 2785 h projects ~31 years at 24/7.
        assert!(life_left(&h).unwrap().starts_with("~31."), "{:?}", life_left(&h));

        let s = crate::smartctl::SmartData {
            passed: Some(true),
            power_on_hours: Some(91_992),
            ..Default::default()
        };
        let mut hdd = d.clone();
        hdd.kind = crate::model::MediaKind::Hdd;
        let h = health::evaluate(&hdd, &s);
        // 91,992 h vs a 43,800 h design life: ~5.5 y over.
        let text = life_left(&h).unwrap();
        assert!(text.starts_with("0 — past its rated life by ~5.5y"), "{text}");
    }

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
            failure: None,
        };
        let s = device_json_basic(&d).to_string();
        assert!(s.contains("\"device\":\"/dev/sda\""));
        assert!(s.contains("\"capacity_bytes\":500000000000"));
        assert!(s.contains("\"bus\":\"SATA\""));
    }
}
