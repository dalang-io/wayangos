//! Read SMART health via `smartctl -a -j`.
//!
//! `smartctl` is optional enrichment; native ioctl is the planned baseline
//! (M3). For testing, set `DCHECK_SMART_JSON=/path/to/smartctl.json` to parse a
//! captured file instead of invoking smartctl.

use std::process::Command;

use crate::json::Json;
use crate::model::Bus;

#[derive(Debug, Default, Clone)]
pub struct SmartData {
    /// How the data was obtained: `smartctl` or `native`.
    pub source: String,
    pub passed: Option<bool>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub rotation_rate: Option<u64>,
    pub form_factor: Option<String>,
    pub sata_version: Option<String>,
    pub interface_speed: Option<String>,
    pub temperature_c: Option<i64>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub lba_written: Option<u64>,
    pub lba_read: Option<u64>,
    pub capacity_bytes: Option<u64>,
    pub logical_block_size: Option<u64>,
    pub reallocated: Option<u64>,
    pub pending: Option<u64>,
    pub uncorrectable: Option<u64>,
    pub crc_errors: Option<u64>,
    /// Vendor-programmed "100 = new" wear attributes (231/233), if present.
    pub life_percent: Option<u64>,
    /// Full ATA SMART attribute table (ATA only).
    pub attributes: Vec<SmartAttribute>,
    pub in_smartctl_database: Option<bool>,
    pub smart_available: Option<bool>,
    /// NVMe: media errors, spare, temperature-time counters, error-log entries.
    pub media_errors: Option<u64>,
    pub available_spare: Option<u64>,
    pub available_spare_threshold: Option<u64>,
    pub warning_temp_time: Option<u64>,
    pub critical_temp_time: Option<u64>,
    pub nvme_errors: Option<u64>,
    /// SCSI/SAS: manufacture date (year, week) from the start-stop log page.
    pub manufactured: Option<(u16, u8)>,
    /// SCSI/SAS: rated lifetime start-stop cycles (actual is `power_cycles`).
    pub rated_start_stop: Option<u64>,
    /// SCSI/SAS: head load-unload cycles, actual and rated.
    pub load_unload: Option<u64>,
    pub rated_load_unload: Option<u64>,
    /// SCSI/SAS: non-medium (transport/controller) error count.
    pub non_medium_errors: Option<u64>,
    /// Temperature at which the drive trips (SCSI "drive trip").
    pub trip_temp_c: Option<i64>,
    /// Most recent self-test, e.g. "Foreground short: completed (1 h)".
    pub last_self_test: Option<String>,
    /// SAS phy error counters summed over ports: invalid dword, running
    /// disparity, loss of dword sync, phy reset problems.
    pub phy_errors: Option<[u64; 4]>,
    /// Lifetime power-on resets (ATA device statistics).
    pub power_on_resets: Option<u64>,
    /// Lifetime temperature range and the rated maximum operating temperature.
    pub temp_min_c: Option<i64>,
    pub temp_max_c: Option<i64>,
    pub temp_rated_max_c: Option<i64>,
    /// ATA: hardware resets seen by the drive.
    pub hardware_resets: Option<u64>,
    /// Entries in the drive's SMART error log.
    pub error_log_count: Option<u64>,
    /// Feature state: TRIM supported, volatile write cache enabled.
    pub trim: Option<bool>,
    pub write_cache: Option<bool>,
    /// Non-fatal error reported by smartctl (e.g. permission denied).
    pub error: Option<String>,
}

impl SmartData {
    pub fn block_size(&self) -> u64 {
        self.logical_block_size.unwrap_or(512)
    }

    /// The drive speaks ATA (SATA), even when a RAID/SAS controller presents
    /// it as a SCSI disk.
    pub fn is_ata(&self) -> bool {
        self.sata_version.is_some() || !self.attributes.is_empty() || self.power_on_resets.is_some()
    }

    /// Fill every field still unknown here from `other` (another source for
    /// the same drive), keeping this source's values where both exist.
    pub fn fill_from(&mut self, other: &SmartData) {
        macro_rules! fill {
            ($($f:ident),* $(,)?) => {
                $( if self.$f.is_none() { self.$f = other.$f.clone(); } )*
            };
        }
        fill!(
            passed, model, serial, firmware, rotation_rate, form_factor, sata_version,
            interface_speed, temperature_c, power_on_hours, power_cycles, lba_written,
            lba_read, capacity_bytes, logical_block_size, reallocated, pending,
            uncorrectable, crc_errors, life_percent, in_smartctl_database, smart_available,
            media_errors, available_spare, available_spare_threshold, warning_temp_time,
            critical_temp_time, nvme_errors, manufactured, rated_start_stop, load_unload,
            rated_load_unload, non_medium_errors, trip_temp_c, last_self_test, phy_errors,
            power_on_resets, temp_min_c, temp_max_c, temp_rated_max_c, hardware_resets,
            error_log_count, trim, write_cache,
        );
        if self.attributes.is_empty() {
            self.attributes = other.attributes.clone();
        }
        if !other.source.is_empty() && !self.source.contains(&other.source) {
            self.source = format!("{}+{}", self.source, other.source);
        }
    }

    pub fn bytes_written(&self) -> Option<u64> {
        self.lba_written.map(|l| l.saturating_mul(self.block_size()))
    }

    pub fn bytes_read(&self) -> Option<u64> {
        self.lba_read.map(|l| l.saturating_mul(self.block_size()))
    }
}

/// A single ATA SMART attribute.
#[derive(Debug, Clone)]
pub struct SmartAttribute {
    pub id: u8,
    pub name: String,
    pub value: u8,
    pub worst: u8,
    pub threshold: u8,
    pub raw: u64,
    /// Pre-failure attribute (failure predicted when value <= threshold).
    pub prefailure: bool,
    /// Advisory / online attribute.
    pub online: bool,
}

impl SmartAttribute {
    /// True when a pre-failure attribute has reached its threshold.
    pub fn failing(&self) -> bool {
        self.prefailure && self.threshold > 0 && self.value <= self.threshold
    }

    pub fn status(&self) -> &'static str {
        if self.failing() {
            "FAIL"
        } else if self.prefailure && self.threshold > 0 && self.value < self.threshold + 10 {
            "warn"
        } else {
            "ok"
        }
    }
}

/// Human name for common ATA SMART attribute IDs.
pub fn attr_name(id: u8) -> &'static str {
    match id {
        1 => "Raw_Read_Error_Rate",
        3 => "Spin_Up_Time",
        4 => "Start_Stop_Count",
        5 => "Reallocated_Sector_Ct",
        7 => "Seek_Error_Rate",
        9 => "Power_On_Hours",
        10 => "Spin_Retry_Count",
        11 => "Calibration_Retry_Count",
        12 => "Power_Cycle_Count",
        13 => "Read_Soft_Error_Rate",
        22 => "Helium_Level",
        177 => "Wear_Leveling_Count",
        179 => "Used_Rsvd_Blk_Cnt_Tot",
        180 => "Unused_Rsvd_Blk_Cnt_Tot",
        181 => "Program_Fail_Cnt_Total",
        182 => "Erase_Fail_Count_Total",
        183 => "Runtime_Bad_Block",
        184 => "End-to-End_Error",
        187 => "Reported_Uncorrect",
        188 => "Command_Timeout",
        190 => "Airflow_Temperature_Cel",
        192 => "Power-Off_Retract_Count",
        193 => "Load_Cycle_Count",
        194 => "Temperature_Celsius",
        195 => "Hardware_ECC_Recovered",
        196 => "Reallocated_Event_Count",
        197 => "Current_Pending_Sector",
        198 => "Offline_Uncorrectable",
        199 => "UDMA_CRC_Error_Count",
        200 => "Multi_Zone_Error_Rate",
        231 => "SSD_Life_Left",
        233 => "Media_Wearout_Indicator",
        241 => "Total_LBAs_Written",
        242 => "Total_LBAs_Read",
        _ => "Unknown_Attribute",
    }
}

/// Run (or load) smartctl output and parse it (no bus hint). Used by the
/// FreeBSD backend and available for direct calls.
#[cfg_attr(not(target_os = "freebsd"), allow(dead_code))]
pub fn read_smart(device: &str) -> Option<SmartData> {
    read_smart_impl(device, None)
}

/// Bus-aware read: probes `-d` passthrough types when the default fails
/// (USB-SATA bridges, RAID controllers, ...).
pub fn read_smart_dev(device: &crate::model::Device) -> Option<SmartData> {
    read_smart_impl(&device.path, Some(device.bus))
}

fn read_smart_impl(device: &str, bus: Option<Bus>) -> Option<SmartData> {
    if let Ok(path) = std::env::var("DCHECK_SMART_JSON") {
        if !path.is_empty() {
            let text = std::fs::read_to_string(path).ok()?;
            return Some(parse_smart(&Json::parse(&text)?));
        }
    }

    let mut fallback: Option<SmartData> = None;
    for dtype in d_candidates(bus) {
        if let Some(s) = run_smartctl(device, dtype.as_deref()) {
            if s.error.is_none() && has_smart_data(&s) {
                return Some(s);
            }
            if fallback.is_none() {
                fallback = Some(s);
            }
        }
    }
    fallback
}

fn run_smartctl(device: &str, dtype: Option<&str>) -> Option<SmartData> {
    let mut cmd = Command::new("smartctl");
    if let Some(d) = dtype {
        cmd.arg("-d").arg(d);
    }
    // -x is a superset of -a: adds SAS phy counters and more ATA/SCSI logs.
    let output = cmd.arg("-x").arg("-j").arg(device).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let json = Json::parse(&text)?;
    let mut s = parse_smart(&json);
    if let Some(d) = dtype {
        s.source = format!("smartctl -d {d}");
    }
    Some(s)
}

fn has_smart_data(s: &SmartData) -> bool {
    s.passed.is_some() || s.model.is_some() || !s.attributes.is_empty() || s.lba_written.is_some()
}

/// Candidate `-d` types to try, most likely first. `DCHECK_SMART_ARGS`
/// (e.g. `-d megaraid,0`) forces a single one.
fn d_candidates(bus: Option<Bus>) -> Vec<Option<String>> {
    let explicit = std::env::var("DCHECK_SMART_ARGS")
        .ok()
        .filter(|s| !s.trim().is_empty());
    candidates_for(bus, explicit.as_deref())
}

fn candidates_for(bus: Option<Bus>, explicit: Option<&str>) -> Vec<Option<String>> {
    if let Some(args) = explicit {
        let value = args.trim().trim_start_matches("-d").trim().to_string();
        return vec![Some(value)];
    }
    match bus {
        Some(Bus::Usb) => vec![
            Some("sat".into()),
            Some("usbjmicron".into()),
            Some("usbprolific".into()),
            None,
        ],
        Some(Bus::Scsi) => vec![
            None,
            Some("sat".into()),
            Some("megaraid,0".into()),
            Some("megaraid,1".into()),
            Some("3ware,0".into()),
            Some("areca,0".into()),
            Some("cciss,0".into()),
        ],
        _ => vec![None, Some("sat".into())],
    }
}

fn parse_smart(j: &Json) -> SmartData {
    let mut s = SmartData {
        source: "smartctl".to_string(),
        ..SmartData::default()
    };

    s.model = str_at(j, &["model_name"]).or_else(|| str_at(j, &["scsi_model_name"]));
    s.serial = str_at(j, &["serial_number"]);
    s.firmware = str_at(j, &["firmware_version"]).or_else(|| str_at(j, &["scsi_revision"]));
    s.rotation_rate = j.get("rotation_rate").and_then(Json::as_u64);
    s.form_factor = str_at(j, &["form_factor", "name"]);
    s.sata_version = str_at(j, &["sata_version", "string"]);
    s.in_smartctl_database = j.get("in_smartctl_database").and_then(Json::as_bool);
    s.smart_available = j
        .get("smart_support")
        .and_then(|v| v.get("available"))
        .and_then(Json::as_bool);
    s.passed = j
        .get("smart_status")
        .and_then(|v| v.get("passed"))
        .and_then(Json::as_bool);
    s.temperature_c = j
        .get("temperature")
        .and_then(|v| v.get("current"))
        .and_then(Json::as_i64);
    s.power_on_hours = j
        .get("power_on_time")
        .and_then(|v| v.get("hours"))
        .and_then(Json::as_u64);
    s.power_cycles = j.get("power_cycle_count").and_then(Json::as_u64);
    s.trip_temp_c = j
        .get("temperature")
        .and_then(|v| v.get("drive_trip"))
        .and_then(Json::as_i64);
    parse_scsi(j, &mut s);
    s.logical_block_size = j.get("logical_block_size").and_then(Json::as_u64);
    s.capacity_bytes = j
        .get("user_capacity")
        .and_then(|v| v.get("bytes"))
        .and_then(Json::as_u64);

    if let Some(speed) = j.get("interface_speed") {
        s.interface_speed = speed
            .get("current")
            .and_then(|v| v.get("string"))
            .and_then(Json::as_str)
            .or_else(|| {
                speed
                    .get("max")
                    .and_then(|v| v.get("string"))
                    .and_then(Json::as_str)
            })
            .map(str::to_string);
    }

    if let Some(messages) = j
        .get("smartctl")
        .and_then(|v| v.get("messages"))
        .and_then(Json::as_array)
    {
        for m in messages {
            if m.get("severity").and_then(Json::as_str) == Some("error") {
                s.error = m
                    .get("string")
                    .and_then(Json::as_str)
                    .map(str::to_string)
                    .or_else(|| Some("smartctl reported an error".to_string()));
            }
        }
    }

    if let Some(table) = j
        .get("ata_smart_attributes")
        .and_then(|v| v.get("table"))
        .and_then(Json::as_array)
    {
        for attr in table {
            let id = attr.get("id").and_then(Json::as_u64);
            let raw = attr
                .get("raw")
                .and_then(|v| v.get("value"))
                .and_then(Json::as_u64);
            let value = attr.get("value").and_then(Json::as_u64);
            match id {
                Some(5) => s.reallocated = raw,
                Some(9) => s.power_on_hours = s.power_on_hours.or(raw),
                Some(197) => s.pending = raw,
                Some(198) => s.uncorrectable = raw,
                Some(199) => s.crc_errors = raw,
                // Vendor "100 = new" wear indicators.
                Some(231) | Some(233) => s.life_percent = value,
                Some(241) => s.lba_written = raw,
                Some(242) => s.lba_read = raw,
                _ => {}
            }
            if let Some(id) = id {
                let name = attr.get("name").and_then(Json::as_str).unwrap_or("");
                let worst = attr.get("worst").and_then(Json::as_u64).unwrap_or(0) as u8;
                let thresh = attr.get("thresh").and_then(Json::as_u64).unwrap_or(0) as u8;
                let prefailure = attr
                    .get("flags")
                    .and_then(|f| f.get("prefailure"))
                    .and_then(Json::as_bool)
                    .unwrap_or(false);
                let online = attr
                    .get("flags")
                    .and_then(|f| f.get("advisory"))
                    .and_then(Json::as_bool)
                    .unwrap_or(false);
                let id = id as u8;
                s.attributes.push(SmartAttribute {
                    id,
                    name: if name.is_empty() {
                        attr_name(id).to_string()
                    } else {
                        name.to_string()
                    },
                    value: value.unwrap_or(0) as u8,
                    worst,
                    threshold: thresh,
                    raw: raw.unwrap_or(0),
                    prefailure,
                    online,
                });
            }
        }
    }

    // NVMe health log, when present.
    if let Some(nvme) = j.get("nvme_smart_health_information_log") {
        s.temperature_c = s
            .temperature_c
            .or_else(|| nvme.get("temperature").and_then(Json::as_i64));
        s.power_on_hours = s
            .power_on_hours
            .or_else(|| nvme.get("power_on_hours").and_then(Json::as_u64));
        s.power_cycles = s
            .power_cycles
            .or_else(|| nvme.get("power_cycles").and_then(Json::as_u64));
        if let Some(used) = nvme.get("percentage_used").and_then(Json::as_u64) {
            s.life_percent = Some(100u64.saturating_sub(used));
        }
        if let Some(units) = nvme.get("data_units_written").and_then(Json::as_u64) {
            // NVMe data units are 1000 * 512 bytes.
            s.lba_written = Some(units.saturating_mul(1000));
        }
        if let Some(units) = nvme.get("data_units_read").and_then(Json::as_u64) {
            s.lba_read = Some(units.saturating_mul(1000));
        }
        s.media_errors = nvme.get("media_errors").and_then(Json::as_u64);
        s.available_spare = nvme.get("available_spare").and_then(Json::as_u64);
        s.available_spare_threshold = nvme
            .get("available_spare_threshold")
            .and_then(Json::as_u64);
        s.warning_temp_time = nvme.get("warning_temp_time").and_then(Json::as_u64);
        s.critical_temp_time = nvme.get("critical_comp_time").and_then(Json::as_u64);
        s.nvme_errors = nvme.get("num_err_log_entries").and_then(Json::as_u64);
        if s.logical_block_size.is_none() {
            s.logical_block_size = Some(512);
        }
    }

    // Last, so the standardized statistics win over vendor attributes.
    parse_ata_extras(j, &mut s);
    s
}

/// Apply one ATA Device Statistics entry (log 0x04; page, byte offset, value).
/// Shared by the smartctl JSON parser and the native SMART READ LOG path.
///
/// Page 1 general: 0x08 power-on resets, 0x10 power-on hours, 0x18 / 0x28
/// logical sectors written / read. Page 4: 0x08 reported uncorrectable errors.
/// Page 5 temperature (signed °C): 0x08 current, 0x20 highest, 0x28 lowest,
/// 0x58 specified maximum operating. Page 6: 0x08 hardware resets, 0x18
/// interface CRC errors. Page 7 SSD: 0x08 percentage used endurance indicator.
pub fn apply_device_stat(s: &mut SmartData, page: u8, offset: u16, value: u64) {
    let temp = || (value as u8) as i8 as i64;
    match (page, offset) {
        (1, 0x08) => s.power_on_resets = Some(value),
        (1, 0x10) => {
            s.power_on_hours.get_or_insert(value);
        }
        // Standard units (logical sectors) — preferred over vendor attributes.
        (1, 0x18) => s.lba_written = Some(value),
        (1, 0x28) => s.lba_read = Some(value),
        (4, 0x08) => {
            s.uncorrectable.get_or_insert(value);
        }
        (5, 0x08) => {
            s.temperature_c.get_or_insert(temp());
        }
        (5, 0x20) => s.temp_max_c = Some(temp()),
        (5, 0x28) => s.temp_min_c = Some(temp()),
        (5, 0x58) => s.temp_rated_max_c = Some(temp()),
        (6, 0x08) => s.hardware_resets = Some(value),
        (6, 0x18) => {
            s.crc_errors.get_or_insert(value);
        }
        // ACS "Percentage Used Endurance Indicator" (0 = new, may exceed 100).
        (7, 0x08) => s.life_percent = Some(100u64.saturating_sub(value)),
        _ => {}
    }
}

/// ATA extras of `smartctl -x -j`: device statistics, SCT temperatures,
/// error / self-test logs, TRIM and write cache.
fn parse_ata_extras(j: &Json, s: &mut SmartData) {
    if let Some(pages) = j
        .get("ata_device_statistics")
        .and_then(|v| v.get("pages"))
        .and_then(Json::as_array)
    {
        for page in pages {
            let Some(num) = page.get("number").and_then(Json::as_u64) else { continue };
            let Some(table) = page.get("table").and_then(Json::as_array) else { continue };
            for e in table {
                let valid = e
                    .get("flags")
                    .and_then(|f| f.get("valid"))
                    .and_then(Json::as_bool)
                    .unwrap_or(false);
                let off = e.get("offset").and_then(Json::as_u64);
                // smartctl prints signed temperatures as negative numbers.
                let val = e
                    .get("value")
                    .and_then(|v| v.as_f64())
                    .map(|v| if v < 0.0 { (v as i64 as u8) as u64 } else { v as u64 });
                if let (true, Some(off), Some(val)) = (valid, off, val) {
                    apply_device_stat(s, num as u8, off as u16, val);
                }
            }
        }
    }
    if let Some(t) = j.get("ata_sct_status").and_then(|v| v.get("temperature")) {
        if s.temp_min_c.is_none() {
            s.temp_min_c = t.get("lifetime_min").and_then(Json::as_i64);
        }
        if s.temp_max_c.is_none() {
            s.temp_max_c = t.get("lifetime_max").and_then(Json::as_i64);
        }
        if s.temp_rated_max_c.is_none() {
            s.temp_rated_max_c = t.get("op_limit_max").and_then(Json::as_i64).filter(|v| *v > 0);
        }
    }
    if let Some(log) = j.get("ata_smart_error_log") {
        s.error_log_count = ["extended", "summary"]
            .iter()
            .find_map(|k| log.get(k).and_then(|v| v.get("count")).and_then(Json::as_u64));
    }
    if let Some(log) = j.get("ata_smart_self_test_log") {
        let latest = ["standard", "extended"].iter().find_map(|k| {
            log.get(k)
                .and_then(|v| v.get("table"))
                .and_then(Json::as_array)
                .and_then(|t| t.first())
        });
        if let Some(t) = latest {
            let kind = str_at(t, &["type", "string"]).unwrap_or_else(|| "Self-test".into());
            let status = str_at(t, &["status", "string"]).unwrap_or_else(|| "unknown".into());
            let at = t
                .get("lifetime_hours")
                .and_then(Json::as_u64)
                .map(|h| format!(" (at {h} h)"))
                .unwrap_or_default();
            s.last_self_test = Some(format!("{kind}: {}{at}", status.to_ascii_lowercase()));
        }
    }
    s.trim = j.get("trim").and_then(|v| v.get("supported")).and_then(Json::as_bool);
    s.write_cache = j.get("write_cache").and_then(|v| v.get("enabled")).and_then(Json::as_bool);
}

/// SCSI/SAS sections of `smartctl -x -j`.
fn parse_scsi(j: &Json, s: &mut SmartData) {
    let num = |v: Option<&Json>| -> Option<f64> {
        let v = v?;
        v.as_f64().or_else(|| v.as_str().and_then(|t| t.trim().parse().ok()))
    };
    if let Some(ss) = j.get("scsi_start_stop_cycle_counter") {
        let year = num(ss.get("year_of_manufacture")).map(|v| v as u16);
        let week = num(ss.get("week_of_manufacture")).map(|v| v as u8);
        if let (Some(y), Some(w)) = (year, week) {
            if y > 1990 {
                s.manufactured = Some((y, w));
            }
        }
        s.rated_start_stop = ss
            .get("specified_cycle_count_over_device_lifetime")
            .and_then(Json::as_u64);
        if s.power_cycles.is_none() {
            s.power_cycles = ss.get("accumulated_start_stop_cycles").and_then(Json::as_u64);
        }
        s.rated_load_unload = ss
            .get("specified_load_unload_count_over_device_lifetime")
            .and_then(Json::as_u64);
        s.load_unload = ss.get("accumulated_load_unload_cycles").and_then(Json::as_u64);
    }
    if let Some(g) = j.get("scsi_grown_defect_list").and_then(Json::as_u64) {
        s.reallocated = Some(g);
    }
    if let Some(log) = j.get("scsi_error_counter_log") {
        let bs = s.block_size().max(1) as f64;
        let mut uncorrected = None;
        for (dir, slot) in [("read", &mut s.lba_read), ("write", &mut s.lba_written)] {
            let Some(d) = log.get(dir) else { continue };
            if let Some(gb) = num(d.get("gigabytes_processed")) {
                *slot = Some((gb * 1e9 / bs) as u64);
            }
            if let Some(u) = d.get("total_uncorrected_errors").and_then(Json::as_u64) {
                uncorrected = Some(uncorrected.unwrap_or(0) + u);
            }
        }
        if uncorrected.is_some() {
            s.uncorrectable = uncorrected;
        }
    }
    if let Some(t) = j.get("scsi_self_test_0") {
        let code = str_at(t, &["code", "string"]);
        let result = str_at(t, &["result", "string"]);
        let hours = t
            .get("power_on_time")
            .and_then(|v| v.get("hours"))
            .and_then(Json::as_u64);
        if let (Some(c), Some(r)) = (code, result) {
            let at = hours.map(|h| format!(" (at {h} h)")).unwrap_or_default();
            s.last_self_test = Some(format!("{c}: {}{at}", r.to_ascii_lowercase()));
        }
    }
    let mut phy = [0u64; 4];
    let mut any_phy = false;
    for port in 0..8 {
        let Some(p) = j.get(&format!("scsi_sas_port_{port}")) else { break };
        for n in 0..8 {
            let Some(ph) = p.get(&format!("phy_{n}")) else { break };
            if s.interface_speed.is_none() {
                if let Some(rate) = str_at(ph, &["negotiated_logical_link_rate"]) {
                    // "phy enabled; 6 Gbps" -> "6 Gbps"
                    let rate = rate.rsplit(';').next().unwrap_or(&rate).trim().to_string();
                    if rate.contains("bps") {
                        s.interface_speed = Some(rate);
                    }
                }
            }
            for (i, key) in [
                "invalid_dword_count",
                "running_disparity_error_count",
                "loss_of_dword_synchronization_count",
                "phy_reset_problem_count",
            ]
            .iter()
            .enumerate()
            {
                if let Some(v) = ph.get(key).and_then(Json::as_u64) {
                    phy[i] += v;
                    any_phy = true;
                }
            }
        }
    }
    if any_phy {
        s.phy_errors = Some(phy);
    }
}

fn str_at(j: &Json, path: &[&str]) -> Option<String> {
    let mut cur = j;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sata_ssd_device_statistics() {
        let text = include_str!("../testdata/smart-sata-sm863a.json");
        let s = parse_smart(&Json::parse(text).unwrap());
        assert_eq!(s.manufactured, None, "SATA has no manufacture date");
        assert_eq!(s.power_on_hours, Some(2785));
        assert_eq!(s.power_on_resets, Some(32));
        // Device statistics: 4,016,839,893 sectors written, ~2.06 TB.
        assert_eq!(s.bytes_written().map(|b| b / 1_000_000_000), Some(2056));
        assert_eq!(s.life_percent, Some(98), "standard 2% used wins over attr 233 (1%)");
        assert_eq!((s.temp_min_c, s.temp_max_c, s.temp_rated_max_c), (Some(22), Some(39), Some(70)));
        assert_eq!(s.hardware_resets, Some(7));
        assert_eq!(s.crc_errors, Some(0));
        assert_eq!(s.error_log_count, Some(0));
        assert_eq!((s.trim, s.write_cache), (Some(true), Some(true)));
    }

    #[test]
    fn device_stat_temperatures_are_signed() {
        let mut s = SmartData::default();
        apply_device_stat(&mut s, 5, 0x28, 0xF6); // -10 °C
        assert_eq!(s.temp_min_c, Some(-10));
    }

    #[test]
    fn parses_sas_hdd_from_smartctl_x() {
        let text = include_str!("../testdata/smart-sas-toshiba-mbf2300rc.json");
        let s = parse_smart(&Json::parse(text).unwrap());
        assert_eq!(s.model.as_deref(), Some("TOSHIBA MBF2300RC"));
        assert_eq!(s.firmware.as_deref(), Some("0109"));
        assert_eq!(s.passed, Some(true));
        assert_eq!(s.temperature_c, Some(32));
        assert_eq!(s.trip_temp_c, Some(65));
        assert_eq!(s.power_on_hours, Some(25660));
        assert_eq!(s.rotation_rate, Some(10025));
        assert_eq!(s.manufactured, Some((2012, 12)));
        assert_eq!(s.power_cycles, Some(40));
        assert_eq!(s.rated_start_stop, Some(50000));
        assert_eq!(s.load_unload, Some(1785));
        assert_eq!(s.rated_load_unload, Some(200000));
        assert_eq!(s.reallocated, Some(0));
        assert_eq!(s.uncorrectable, Some(0));
        assert_eq!(s.bytes_read().map(|b| b / 1_000_000_000), Some(458181));
        assert_eq!(s.bytes_written().map(|b| b / 1_000_000_000), Some(58649));
        assert_eq!(s.interface_speed.as_deref(), Some("6 Gbps"));
        assert_eq!(s.phy_errors, Some([724, 715, 181, 4]));
        assert!(s.last_self_test.as_deref().unwrap().starts_with("Foreground short: completed"));
    }

    #[test]
    fn fill_from_keeps_primary_and_fills_gaps() {
        let mut a = SmartData {
            source: "smartctl".into(),
            temperature_c: Some(30),
            ..Default::default()
        };
        let b = SmartData {
            source: "native".into(),
            temperature_c: Some(99),
            non_medium_errors: Some(38),
            ..Default::default()
        };
        a.fill_from(&b);
        assert_eq!(a.temperature_c, Some(30));
        assert_eq!(a.non_medium_errors, Some(38));
        assert_eq!(a.source, "smartctl+native");
    }

    const SAMPLE: &str = r#"{
        "smartctl": {"messages": []},
        "model_name": "SSD 1TB",
        "serial_number": "TEST123",
        "firmware_version": "VE0R6304",
        "rotation_rate": 0,
        "logical_block_size": 512,
        "sata_version": {"string": "SATA 3.0", "value": 63},
        "interface_speed": {"current": {"string": "6.0 Gb/s"}},
        "smart_support": {"available": true, "enabled": true},
        "smart_status": {"passed": true},
        "temperature": {"current": 40},
        "power_on_time": {"hours": 275},
        "power_cycle_count": 38,
        "in_smartctl_database": false,
        "ata_smart_attributes": {"table": [
            {"id": 5,  "name": "Reallocated_Sector_Ct", "value": 100, "raw": {"value": 0}},
            {"id": 9,  "name": "Power_On_Hours",         "value": 100, "raw": {"value": 275}},
            {"id": 241,"name": "Total_LBAs_Written",     "value": 100, "raw": {"value": 807}},
            {"id": 242,"name": "Total_LBAs_Read",        "value": 100, "raw": {"value": 92}}
        ]}
    }"#;

    #[test]
    fn parses_sata_smart() {
        let s = parse_smart(&Json::parse(SAMPLE).unwrap());
        assert_eq!(s.model.as_deref(), Some("SSD 1TB"));
        assert_eq!(s.passed, Some(true));
        assert_eq!(s.temperature_c, Some(40));
        assert_eq!(s.power_on_hours, Some(275));
        assert_eq!(s.interface_speed.as_deref(), Some("6.0 Gb/s"));
        assert_eq!(s.sata_version.as_deref(), Some("SATA 3.0"));
        assert_eq!(s.lba_written, Some(807));
        assert_eq!(s.bytes_written(), Some(807 * 512));
    }

    #[test]
    fn parses_nvme_health() {
        let sample = r#"{
            "model_name": "Samsung SSD 980 500GB",
            "logical_block_size": 512,
            "smart_status": {"passed": true},
            "nvme_smart_health_information_log": {
                "temperature": 31,
                "percentage_used": 7,
                "data_units_written": 1234567,
                "power_on_hours": 900,
                "media_errors": 2,
                "available_spare": 100,
                "available_spare_threshold": 10,
                "warning_temp_time": 5,
                "critical_comp_time": 0,
                "num_err_log_entries": 3
            }
        }"#;
        let s = parse_smart(&Json::parse(sample).unwrap());
        assert_eq!(s.life_percent, Some(93)); // 100 - 7
        assert_eq!(s.power_on_hours, Some(900));
        assert_eq!(s.media_errors, Some(2));
        assert_eq!(s.available_spare, Some(100));
        assert_eq!(s.nvme_errors, Some(3));
        assert_eq!(s.warning_temp_time, Some(5));
        // 1234567 * 1000 * 512 bytes
        assert_eq!(s.bytes_written(), Some(1234567 * 1000 * 512));
    }

    #[test]
    fn passthrough_candidates_per_bus() {
        assert_eq!(candidates_for(None, None), vec![None, Some("sat".into())]);
        assert_eq!(candidates_for(Some(Bus::Usb), None)[0], Some("sat".into()));
        let scsi = candidates_for(Some(Bus::Scsi), None);
        assert_eq!(scsi[0], None);
        assert!(scsi.contains(&Some("megaraid,0".into())));
        assert!(scsi.contains(&Some("areca,0".into())));
        // explicit override wins
        assert_eq!(
            candidates_for(Some(Bus::Scsi), Some("-d megaraid,3")),
            vec![Some("megaraid,3".into())]
        );
    }

    #[test]
    fn detects_usable_smart_data() {
        let mut s = SmartData::default();
        assert!(!has_smart_data(&s));
        s.passed = Some(true);
        assert!(has_smart_data(&s));
    }
}
