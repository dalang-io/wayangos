//! Health verdict and remaining-life estimation from SMART data.

use crate::model::Device;
use crate::smartctl::SmartData;

const DAYS_247_PER_DAY: u64 = 24;
const DAYS_87_PER_DAY: u64 = 8; // office: 8h/day, 7 days/week

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
    if let Some(t) = smart.temperature_c {
        if t >= 60 {
            issues.push(format!("temperature high ({t}°C)"));
        }
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
        (Some(tbw), Some(poh)) if poh >= 24 => tbw >= device.size_bytes / 100,
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

    let confidence = if smart.life_percent.is_some() {
        Confidence::High
    } else if rated.is_some() && rate_plausible {
        Confidence::Medium
    } else {
        Confidence::Low
    };

    let verdict = if smart.error.is_some() || smart.passed.is_none() {
        Verdict::Unknown
    } else if smart.passed == Some(false) {
        Verdict::Replace
    } else if wear_used.map(|w| w >= 90).unwrap_or(false)
        || smart.pending.unwrap_or(0) > 0
        || smart.uncorrectable.unwrap_or(0) > 0
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
        days_247: remaining_poh.map(|h| h / DAYS_247_PER_DAY),
        days_87: remaining_poh.map(|h| h / DAYS_87_PER_DAY),
        confidence,
        notes,
    }
}

/// Best-effort rated endurance (TBW) lookup for common consumer SSDs.
/// Returns bytes. `None` when the model is unknown.
fn rated_tbw_bytes(model: &str, capacity_bytes: u64) -> Option<u64> {
    let m = model.to_ascii_lowercase();
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
mod tests {
    use super::*;
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
            partitions: vec![],
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
    fn failing_smart_is_replace() {
        let mut s = SmartData::default();
        s.passed = Some(false);
        let h = evaluate(&ssd("SSD 1TB", 1_000_000_000_000), &s);
        assert_eq!(h.verdict, Verdict::Replace);
    }
}
