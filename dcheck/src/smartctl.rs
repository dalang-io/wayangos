//! Read SMART health via `smartctl -a -j`.
//!
//! `smartctl` is optional enrichment; native ioctl is the planned baseline
//! (M3). For testing, set `DCHECK_SMART_JSON=/path/to/smartctl.json` to parse a
//! captured file instead of invoking smartctl.

use std::process::Command;

use crate::json::Json;

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
    /// Non-fatal error reported by smartctl (e.g. permission denied).
    pub error: Option<String>,
}

impl SmartData {
    pub fn block_size(&self) -> u64 {
        self.logical_block_size.unwrap_or(512)
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

/// Run (or load) smartctl output and parse it. Returns `None` only when neither
/// a JSON file nor the `smartctl` binary is available.
pub fn read_smart(device: &str) -> Option<SmartData> {
    let text = match std::env::var("DCHECK_SMART_JSON") {
        Ok(path) if !path.is_empty() => std::fs::read_to_string(path).ok()?,
        _ => {
            let output = Command::new("smartctl")
                .arg("-a")
                .arg("-j")
                .arg(device)
                .output()
                .ok()?;
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
    };

    let json = Json::parse(&text)?;
    Some(parse_smart(&json))
}

fn parse_smart(j: &Json) -> SmartData {
    let mut s = SmartData {
        source: "smartctl".to_string(),
        ..SmartData::default()
    };

    s.model = str_at(j, &["model_name"]);
    s.serial = str_at(j, &["serial_number"]);
    s.firmware = str_at(j, &["firmware_version"]);
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
        if s.logical_block_size.is_none() {
            s.logical_block_size = Some(512);
        }
    }

    s
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
                "power_on_hours": 900
            }
        }"#;
        let s = parse_smart(&Json::parse(sample).unwrap());
        assert_eq!(s.life_percent, Some(93)); // 100 - 7
        assert_eq!(s.power_on_hours, Some(900));
        // 1234567 * 1000 * 512 bytes
        assert_eq!(s.bytes_written(), Some(1234567 * 1000 * 512));
    }
}
