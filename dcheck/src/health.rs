//! Health verdict and remaining-life estimation from SMART data.

use std::sync::OnceLock;

use crate::model::Device;
use crate::smartctl::SmartData;

const DAYS_247_PER_DAY: u64 = 24;
const DAYS_87_PER_DAY: u64 = 8; // office: 8h/day, 7 days/week

/// Hours in a year of 24/7 operation.
const HOURS_PER_YEAR: f64 = 365.0 * 24.0;

/// Assumed HDD design life in hours: `hdd_design_years` from the config,
/// default 5 years at 24/7 (typical enterprise service life / warranty;
/// consumer drives are rarely rated higher). Drives do not report this.
pub fn hdd_design_hours() -> u64 {
    (crate::config::load().hdd_design_years * HOURS_PER_YEAR).round() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Monitor,
    BackupNow,
    Replace,
    Unknown,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Ok => "OK",
            Verdict::Monitor => "MONITOR",
            Verdict::BackupNow => "BACK UP NOW",
            Verdict::Replace => "REPLACE",
            Verdict::Unknown => "UNKNOWN",
        }
    }

    /// 0=ok, 1=unknown, 2=monitor, 3=back up now, 4=replace.
    /// (`dcheck check` caps its exit code at 3.)
    pub fn severity(self) -> u8 {
        match self {
            Verdict::Ok => 0,
            Verdict::Unknown => 1,
            Verdict::Monitor => 2,
            Verdict::BackupNow => 3,
            Verdict::Replace => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Health {
    pub verdict: Verdict,
    pub issues: Vec<String>,
    pub tbw_bytes: Option<u64>,
    pub rated_tbw_bytes: Option<u64>,
    pub wear_used_percent: Option<u64>,
    pub remaining_poh: Option<u64>,
    /// HDD: share of the design life / rated cycles already used (may exceed
    /// 100), and which limit dominates.
    pub design_life_used: Option<u64>,
    pub design_limit: Option<&'static str>,
    /// What the life estimate is based on.
    pub life_basis: Option<&'static str>,
    /// Hours the drive has run past its rated life (0 remaining).
    pub overdue_poh: Option<u64>,
    /// Assumed HDD design life (hours at 24/7) the estimate used.
    pub design_hours: Option<u64>,
    pub days_247: Option<u64>,
    pub days_87: Option<u64>,
    pub confidence: Confidence,
    pub notes: Vec<String>,
}

pub fn evaluate(device: &Device, smart: &SmartData) -> Health {
    let mut issues = Vec::new();
    let mut notes = Vec::new();

    if let Some(err) = &smart.error {
        notes.push(err.clone());
    }
    if let Some(reason) = &device.failure {
        issues.push(format!("{}: {reason} — drive dead or incompatible", device.name));
    }

    let tbw_bytes = smart.bytes_written();
    let wear_used = smart.life_percent.map(|p| 100u64.saturating_sub(p));

    // SMART counters that indicate trouble.
    if smart.reallocated.unwrap_or(0) > 0 {
        issues.push(format!("{} reallocated sectors", smart.reallocated.unwrap()));
    }
    if smart.pending.unwrap_or(0) > 0 {
        issues.push(format!("{} pending sectors", smart.pending.unwrap()));
    }
    if smart.uncorrectable.unwrap_or(0) > 0 {
        issues.push(format!("{} offline-uncorrectable sectors", smart.uncorrectable.unwrap()));
    }
    if smart.crc_errors.unwrap_or(0) > 0 {
        issues.push(format!("{} UDMA CRC errors (check cable/port)", smart.crc_errors.unwrap()));
    }
    if smart.media_errors.unwrap_or(0) > 0 {
        issues.push(format!("{} NVMe media errors", smart.media_errors.unwrap()));
    }
    if let (Some(spare), Some(thresh)) = (smart.available_spare, smart.available_spare_threshold) {
        if spare < thresh {
            issues.push(format!("available spare {spare}% below threshold {thresh}%"));
        }
    }
    if let Some(t) = smart.critical_temp_time {
        if t > 0 {
            notes.push(format!("{t} min spent at critical temperature"));
        }
    }
    if let Some(t) = smart.temperature_c {
        if t >= crate::config::load().temp_warn_c {
            issues.push(format!("temperature high ({t}°C)"));
        }
    }

    // Attributes that have actually reached their pre-failure threshold.
    let failing: Vec<&str> = smart
        .attributes
        .iter()
        .filter(|a| a.failing())
        .map(|a| a.name.as_str())
        .collect();
    for name in &failing {
        issues.push(format!("SMART attribute {name} at/below threshold"));
    }

    // Endurance / remaining life. Prefer the SMART model when it differs from
    // the sysfs one (SMART is usually the more complete string).
    let model = smart
        .model
        .clone()
        .or_else(|| device.model.clone())
        .unwrap_or_default();
    let rated = rated_tbw_bytes(&model, device.size_bytes);

    let rate_plausible = match (tbw_bytes, smart.power_on_hours) {
        // Only judge after a month of runtime: idle archive disks legitimately
        // write very little.
        (Some(tbw), Some(poh)) if poh >= 720 => tbw >= device.size_bytes / 100,
        _ => true,
    };
    if !rate_plausible {
        notes.push(
            "reported host writes look implausibly low for the runtime — this drive may use non-standard SMART units"
                .to_string(),
        );
    }

    let mut remaining_poh = None;

    // Prefer a vendor wear indicator when present.
    if let (Some(poh), Some(used)) = (smart.power_on_hours, wear_used) {
        if poh > 0 && used > 0 {
            let rate = used as f64 / poh as f64;
            remaining_poh = Some(((100.0 - used as f64) / rate) as u64);
        }
    }

    // Otherwise extrapolate from TBW vs rated endurance.
    if remaining_poh.is_none() && rate_plausible {
        if let (Some(poh), Some(tbw), Some(rated)) = (smart.power_on_hours, tbw_bytes, rated) {
            if poh > 0 && tbw > 0 && rated > tbw {
                let rate = tbw as f64 / poh as f64;
                remaining_poh = Some(((rated - tbw) as f64 / rate) as u64);
            }
        }
    }

    let mut life_basis = if remaining_poh.is_none() {
        None
    } else if wear_used.is_some() {
        Some("vendor wear indicator")
    } else {
        Some("host writes vs rated endurance (TBW)")
    };

    // HDDs have no endurance rating: compare runtime and mechanical cycles
    // with the design life / rated counts and extrapolate the dominant one.
    let is_hdd = device.kind == crate::model::MediaKind::Hdd
        || smart.rotation_rate.is_some_and(|r| r > 0);
    let mut design_life_used = None;
    let mut design_limit = None;
    let mut overdue_poh = None;
    let mut design_hours = None;
    if is_hdd && wear_used.is_none() {
        let hours = hdd_design_hours();
        if let Some((what, ratio)) = hdd_design_ratio(smart, hours) {
            design_life_used = Some((ratio * 100.0).round() as u64);
            design_limit = Some(what);
            design_hours = Some(hours);
            life_basis = Some("assumed HDD design life and the drive's rated cycles");
            let poh = smart.power_on_hours.unwrap_or(0);
            if ratio >= 1.0 && poh > 0 {
                // Runtime beyond the point where the dominant limit hit 100%.
                overdue_poh = Some((poh as f64 * (1.0 - 1.0 / ratio)) as u64);
            }
            remaining_poh = if ratio >= 1.0 {
                Some(0)
            } else if ratio > 0.0 && poh > 0 {
                Some((poh as f64 * (1.0 / ratio - 1.0)) as u64)
            } else {
                None
            };
            if ratio >= 1.0 {
                issues.push(format!(
                    "past its design life ({what} at {}% of rating) — plan a replacement",
                    (ratio * 100.0).round()
                ));
            }
        }
    }

    // Wear is reported in whole percent, so 1-2% extrapolates very noisily.
    let confidence = if wear_used.is_some_and(|w| w > 2) {
        Confidence::High
    } else if wear_used.is_some() || (rated.is_some() && rate_plausible) {
        Confidence::Medium
    } else {
        Confidence::Low
    };

    let verdict = if smart.error.is_some() || smart.passed.is_none() {
        Verdict::Unknown
    } else if smart.passed == Some(false) {
        Verdict::Replace
    } else if !failing.is_empty()
        || wear_used.map(|w| w >= 90).unwrap_or(false)
        || smart.pending.unwrap_or(0) > 0
        || smart.uncorrectable.unwrap_or(0) > 0
        || smart.media_errors.unwrap_or(0) > 0
    {
        Verdict::BackupNow
    } else if !issues.is_empty() {
        Verdict::Monitor
    } else {
        Verdict::Ok
    };

    Health {
        verdict,
        issues,
        tbw_bytes,
        rated_tbw_bytes: rated,
        wear_used_percent: wear_used,
        remaining_poh,
        design_life_used,
        design_limit,
        life_basis,
        overdue_poh,
        design_hours,
        days_247: remaining_poh.map(|h| h / DAYS_247_PER_DAY),
        days_87: remaining_poh.map(|h| h / DAYS_87_PER_DAY),
        confidence,
        notes,
    }
}

/// Dominant HDD ageing ratio: power-on hours vs design life, start-stop and
/// load-unload cycles vs their rated counts.
fn hdd_design_ratio(s: &SmartData, design_hours: u64) -> Option<(&'static str, f64)> {
    let mut best: Option<(&'static str, f64)> = None;
    let mut consider = |what, used: Option<u64>, rated: Option<u64>| {
        if let (Some(u), Some(r)) = (used, rated) {
            if r > 0 {
                let ratio = u as f64 / r as f64;
                if best.is_none_or(|(_, b)| ratio > b) {
                    best = Some((what, ratio));
                }
            }
        }
    };
    consider("power-on hours", s.power_on_hours, Some(design_hours));
    consider("start-stop cycles", s.power_cycles, s.rated_start_stop);
    consider("load-unload cycles", s.load_unload, s.rated_load_unload);
    best
}

/// User-provided TBW overrides, loaded once.
fn overrides() -> &'static [(String, f64)] {
    static OVERRIDES: OnceLock<Vec<(String, f64)>> = OnceLock::new();
    OVERRIDES.get_or_init(load_overrides)
}

fn load_overrides() -> Vec<(String, f64)> {
    let path = std::env::var_os("DCHECK_TBW_JSON")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config/dcheck/tbw.json"))
        });
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Some(crate::json::Json::Obj(map)) = crate::json::Json::parse(&text) else {
        return Vec::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| v.as_f64().map(|t| (k, t)))
        .collect()
}

/// Rated endurance: user overrides first, then the built-in table.
/// Overrides come from `$DCHECK_TBW_JSON` or `~/.config/dcheck/tbw.json`
/// (`{ "model substring": tbw_in_tb, ... }`).
fn rated_tbw_bytes(model: &str, capacity_bytes: u64) -> Option<u64> {
    let lower = model.to_ascii_lowercase();
    if let Some(tbw_tb) = match_override(&lower, overrides()) {
        return Some((tbw_tb * 1e12) as u64);
    }
    rated_tbw_table(&lower, capacity_bytes)
}

fn match_override(lower: &str, list: &[(String, f64)]) -> Option<f64> {
    list.iter()
        .find(|(needle, _)| !needle.is_empty() && lower.contains(&needle.to_ascii_lowercase()))
        .map(|(_, tbw)| *tbw)
}

fn rated_tbw_table(m: &str, capacity_bytes: u64) -> Option<u64> {
    let tbw_tb: f64 = if m.contains("870 evo") || m.contains("860 evo") {
        match capacity_bytes {
            c if c <= 280_000_000_000 => 150.0,
            c if c <= 560_000_000_000 => 300.0,
            c if c <= 1_100_000_000_000 => 600.0,
            c if c <= 2_100_000_000_000 => 1200.0,
            _ => 2400.0,
        }
    } else if m.contains("samsung ssd 980") || m.contains("980 pro") {
        match capacity_bytes {
            c if c <= 280_000_000_000 => 150.0,
            c if c <= 560_000_000_000 => 300.0,
            c if c <= 1_100_000_000_000 => 600.0,
            _ => 1200.0,
        }
    } else if m.contains("sa400") || m.contains("a400") {
        match capacity_bytes {
            c if c <= 140_000_000_000 => 40.0,
            c if c <= 280_000_000_000 => 80.0,
            c if c <= 560_000_000_000 => 160.0,
            _ => 300.0,
        }
    } else if m.contains("mx500") || m.contains("crucial") {
        match capacity_bytes {
            c if c <= 280_000_000_000 => 100.0,
            c if c <= 560_000_000_000 => 180.0,
            c if c <= 1_100_000_000_000 => 360.0,
            _ => 700.0,
        }
    } else if m.contains("wd blue") || m.contains("wd green") {
        match capacity_bytes {
            c if c <= 280_000_000_000 => 100.0,
            c if c <= 560_000_000_000 => 200.0,
            c if c <= 1_100_000_000_000 => 400.0,
            _ => 600.0,
        }
    } else if m.contains("wd black") || m.contains("sn850") || m.contains("sn770") {
        match capacity_bytes {
            c if c <= 560_000_000_000 => 300.0,
            c if c <= 1_100_000_000_000 => 600.0,
            _ => 1200.0,
        }
    } else if m.contains("kingston") || m.contains("kc3000") || m.contains("a2000") {
        match capacity_bytes {
            c if c <= 280_000_000_000 => 80.0,
            c if c <= 560_000_000_000 => 160.0,
            c if c <= 1_100_000_000_000 => 320.0,
            _ => 640.0,
        }
    } else if m.contains("sk hynix") || m.contains("shgp31") || m.contains("gold p31") {
        match capacity_bytes {
            c if c <= 560_000_000_000 => 250.0,
            c if c <= 1_100_000_000_000 => 500.0,
            _ => 750.0,
        }
    } else if m.contains("intel") || m.contains("ssdpe") || m.contains("d3-s") {
        match capacity_bytes {
            c if c <= 280_000_000_000 => 72.0,
            c if c <= 560_000_000_000 => 144.0,
            c if c <= 1_100_000_000_000 => 288.0,
            _ => 576.0,
        }
    } else {
        return None;
    };
    Some((tbw_tb * 1e12) as u64)
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn hdd(size: u64) -> Device {
        Device {
            kind: MediaKind::Hdd,
            bus: Bus::Scsi,
            ..ssd("TOSHIBA MBF2300RC", size)
        }
    }

    #[test]
    fn hdd_life_from_design_hours() {
        let mut s = SmartData::default();
        s.passed = Some(true);
        s.power_on_hours = Some(21_900); // half of 5 y
        s.power_cycles = Some(40);
        s.rated_start_stop = Some(50_000);
        let h = evaluate(&hdd(300_000_000_000), &s);
        assert_eq!(h.design_life_used, Some(50));
        assert_eq!(h.design_limit, Some("power-on hours"));
        assert_eq!(h.remaining_poh, Some(21_900));
        assert_eq!(h.verdict, Verdict::Ok);
        assert_eq!(h.confidence, Confidence::Low);
    }

    #[test]
    fn hdd_cycles_can_dominate() {
        let mut s = SmartData::default();
        s.passed = Some(true);
        s.power_on_hours = Some(1_000);
        s.load_unload = Some(150_000);
        s.rated_load_unload = Some(200_000);
        let h = evaluate(&hdd(300_000_000_000), &s);
        assert_eq!(h.design_limit, Some("load-unload cycles"));
        assert_eq!(h.design_life_used, Some(75));
    }

    #[test]
    fn hdd_past_design_life_is_monitor() {
        let mut s = SmartData::default();
        s.passed = Some(true);
        s.power_on_hours = Some(91_992);
        let h = evaluate(&hdd(900_000_000_000), &s);
        assert_eq!(h.remaining_poh, Some(0));
        assert_eq!(h.verdict, Verdict::Monitor);
        assert!(h.issues.iter().any(|i| i.contains("design life")));
    }
    use crate::model::{Bus, MediaKind};

    fn ssd(model: &str, size: u64) -> Device {
        Device {
            name: "sda".into(),
            path: "/dev/sda".into(),
            vendor: None,
            model: Some(model.into()),
            firmware: None,
            serial: None,
            bus: Bus::Sata,
            kind: MediaKind::Ssd,
            size_bytes: size,
            logical_block_size: 512,
            removable: false,
            smart_status: None,
            partitions: vec![],
            failure: None,
        }
    }

    #[test]
    fn healthy_ssd_is_ok() {
        let mut s = SmartData::default();
        s.passed = Some(true);
        s.power_on_hours = Some(1000);
        s.lba_written = Some(1_000_000_000); // 512 GB
        let h = evaluate(&ssd("KINGSTON SA400S37", 240_000_000_000), &s);
        assert_eq!(h.verdict, Verdict::Ok);
        assert!(h.rated_tbw_bytes.is_some());
    }

    #[test]
    fn nvme_wear_estimates_life() {
        let mut s = SmartData::default();
        s.passed = Some(true);
        s.power_on_hours = Some(1000);
        s.life_percent = Some(90); // 10% used over 1000h
        let h = evaluate(&ssd("Samsung SSD 980 500GB", 500_000_000_000), &s);
        assert_eq!(h.wear_used_percent, Some(10));
        // 10%/1000h -> 90% remaining -> 9000h -> 375 days @24/7
        assert_eq!(h.remaining_poh, Some(9000));
        assert_eq!(h.days_247, Some(375));
        assert_eq!(h.confidence, Confidence::High);
    }

    #[test]
    fn low_wear_lowers_confidence() {
        let mut s = SmartData::default();
        s.passed = Some(true);
        s.power_on_hours = Some(100);
        s.life_percent = Some(99);
        let h = evaluate(&ssd("Samsung SSD 980 500GB", 500_000_000_000), &s);
        assert_eq!(h.confidence, Confidence::Medium);
    }

    #[test]
    fn young_idle_disk_is_not_flagged_implausible() {
        let mut s = SmartData::default();
        s.passed = Some(true);
        s.power_on_hours = Some(200);
        s.lba_written = Some(1_000); // ~0.5 MB
        let h = evaluate(&ssd("SSD 4TB", 4_000_000_000_000), &s);
        assert!(h.notes.iter().all(|n| !n.contains("implausibly")));
    }

    #[test]
    fn failing_smart_is_replace() {
        let mut s = SmartData::default();
        s.passed = Some(false);
        let h = evaluate(&ssd("SSD 1TB", 1_000_000_000_000), &s);
        assert_eq!(h.verdict, Verdict::Replace);
    }

    #[test]
    fn rated_tbw_known_and_override() {
        // Built-in table.
        assert_eq!(rated_tbw_table("kingston sa400s37", 240_000_000_000), Some(80_000_000_000_000));
        assert_eq!(rated_tbw_table("unknown model", 240_000_000_000), None);
        // User override takes precedence (pure matcher).
        let ov = vec![("sa400".to_string(), 999.0)];
        assert_eq!(match_override("kingston sa400s37 240g", &ov), Some(999.0));
        assert_eq!(match_override("samsung ssd 980", &ov), None);
    }

    #[test]
    fn failing_attribute_triggers_backup() {
        let mut s = SmartData::default();
        s.passed = Some(true);
        s.attributes.push(crate::smartctl::SmartAttribute {
            id: 5,
            name: "Reallocated_Sector_Ct".into(),
            value: 5,
            worst: 5,
            threshold: 10,
            raw: 100,
            prefailure: true,
            online: true,
        });
        let h = evaluate(&ssd("SSD 1TB", 1_000_000_000_000), &s);
        assert_eq!(h.verdict, Verdict::BackupNow);
    }
}
