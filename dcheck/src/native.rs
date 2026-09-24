//! Native hardware readers (no external tools).
//!
//! Linux only at runtime:
//!   - SATA: `HDIO_DRIVE_CMD` for IDENTIFY DEVICE + SMART.
//!   - SCSI/SAS: `SG_IO` for INQUIRY/VPD identity (LOG SENSE health where the
//!     transport allows it; some RAID/JBOD controllers block it).
//!   - NVMe: admin command ioctl for the SMART/Health log, sysfs for the link.
//!
//! The pure byte parsers are platform-independent and unit tested everywhere.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[cfg(all(target_os = "linux", not(target_pointer_width = "64")))]
compile_error!("dcheck's raw ioctl/statvfs structs assume a 64-bit Linux target");

use crate::model::Device;
use crate::smartctl::{attr_name, SmartAttribute, SmartData};

/// Identity fields read natively (used to enrich the report when smartctl is
/// absent).
#[derive(Debug, Default, Clone)]
pub struct IdInfo {
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub serial: Option<String>,
    /// World Wide Name (hex), see `authenticity.rs`.
    pub wwn: Option<String>,
}

impl IdInfo {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.firmware.is_none() && self.serial.is_none()
    }
}

/// Read SMART natively. Returns `None` on non-Linux or when the ioctl path
/// fails (no privileges, unsupported device/transport).
pub fn read(device: &Device) -> Option<SmartData> {
    let mut s = read_raw(device)?;
    // Keep the identity with the SMART data (and so in the cache): a separate
    // INQUIRY/IDENTIFY costs up to ~0.75 s behind some RAID controllers.
    if s.model.is_none() || s.serial.is_none() || s.firmware.is_none() || s.wwn.is_none() {
        let id = identity(device);
        s.model = s.model.or(id.model);
        s.serial = s.serial.or(id.serial);
        s.firmware = s.firmware.or(id.firmware);
        s.wwn = s.wwn.or(id.wwn);
    }
    Some(s)
}

fn read_raw(device: &Device) -> Option<SmartData> {
    #[cfg(target_os = "linux")]
    {
        use crate::model::{Bus, MediaKind};
        let ata_behind_scsi = device.vendor.as_deref().map(str::trim) == Some("ATA");
        let force_sat = std::env::var_os("DCHECK_SAT").is_some();
        match (device.kind, device.bus) {
            (MediaKind::Nvme, _) | (_, Bus::Nvme) => linux::nvme_read(device),
            // DCHECK_SAT=1: use ATA PASS-THROUGH even where HDIO works (testing).
            _ if force_sat => linux::sat_read(device),
            // SATA behind a SAS/RAID HBA (e.g. PERC/MegaRAID JBOD): the
            // controller translates ATA PASS-THROUGH (SAT); SCSI logs are a
            // poor subset for these drives.
            (_, Bus::Scsi) if ata_behind_scsi => {
                linux::sat_read(device).or_else(|| linux::scsi_read(device))
            }
            (_, Bus::Scsi) => linux::scsi_read(device),
            // USB bridges usually speak SAT; HDIO does not reach them.
            (_, Bus::Usb) => linux::ata_read(device).or_else(|| linux::sat_read(device)),
            _ => linux::ata_read(device).or_else(|| linux::sat_read(device)),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device;
        None
    }
}

/// Read identity (model/firmware/serial) natively.
pub fn identity(device: &Device) -> IdInfo {
    #[cfg(target_os = "linux")]
    {
        linux::identity(device)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device;
        IdInfo::default()
    }
}

/// SMART self-test type.
#[derive(Debug, Clone, Copy)]
pub enum SelfTest {
    Short,
    Long,
}

impl SelfTest {
    fn code(self) -> u8 {
        match self {
            SelfTest::Short => 1,
            SelfTest::Long => 2,
        }
    }

    fn label(self) -> &'static str {
        match self {
            SelfTest::Short => "short",
            SelfTest::Long => "long",
        }
    }
}

/// Start a SMART self-test and report the current execution status.
/// Native implementation is ATA-only; callers may fall back to smartctl.
pub fn selftest(device: &Device, kind: SelfTest) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        linux::ata_selftest(device, kind)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device, kind);
        None
    }
}

/// Negotiated interface link speed (SATA `sata_spd`, NVMe PCIe link).
pub fn link_speed(device: &Device) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        linux::link_speed(device)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device;
        None
    }
}

fn le_u48(bytes: &[u8]) -> u64 {
    let mut v = 0u64;
    for (i, b) in bytes.iter().take(6).enumerate() {
        v |= (*b as u64) << (8 * i);
    }
    v
}

fn le_u128(bytes: &[u8]) -> u128 {
    let mut buf = [0u8; 16];
    let n = bytes.len().min(16);
    buf[..n].copy_from_slice(&bytes[..n]);
    u128::from_le_bytes(buf)
}

fn to_u64_sat(v: u128) -> u64 {
    u64::try_from(v).unwrap_or(u64::MAX)
}

/// WWN from ATA IDENTIFY DEVICE words 108–111 (all zeros when the drive
/// reports none).
pub fn ata_wwn(identify: &[u8]) -> Option<String> {
    if identify.len() < 224 {
        return None;
    }
    let word = |w: usize| u16::from_le_bytes([identify[w * 2], identify[w * 2 + 1]]) as u64;
    Some(format!("{:016x}", word(108) << 48 | word(109) << 32 | word(110) << 16 | word(111)))
}

/// NAA designator of the logical unit from SCSI VPD page 0x83 (hex).
pub fn parse_vpd83_naa(page: &[u8]) -> Option<String> {
    if page.len() < 4 || page[1] != 0x83 {
        return None;
    }
    let end = (4 + u16::from_be_bytes([page[2], page[3]]) as usize).min(page.len());
    let mut i = 4;
    while i + 4 <= end {
        let (assoc, kind, len) = ((page[i + 1] >> 4) & 0x3, page[i + 1] & 0xf, page[i + 3] as usize);
        let data = page.get(i + 4..i + 4 + len)?;
        if kind == 3 && assoc == 0 && !data.is_empty() {
            return Some(data.iter().map(|b| format!("{b:02x}")).collect());
        }
        i += 4 + len;
    }
    None
}

fn nonempty(s: String) -> Option<String> {
    let t = s.trim().to_string();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Parse the 512-byte ATA SMART READ DATA structure.
pub fn parse_ata_smart(data: &[u8]) -> SmartData {
    let mut s = SmartData {
        source: "native".to_string(),
        logical_block_size: Some(512),
        ..Default::default()
    };

    let len = data.len();
    for i in 0..30 {
        let off = 2 + i * 12;
        if off + 12 > len {
            break;
        }
        let id = data[off];
        if id == 0 {
            continue; // unused attribute slot
        }
        let flags = u16::from_le_bytes([data[off + 1], data[off + 2]]);
        let value = data[off + 3];
        let worst = data[off + 4];
        let raw = le_u48(&data[off + 5..off + 11]);

        match id {
            5 => s.reallocated = Some(raw),
            9 => s.power_on_hours = Some(raw),
            12 => s.power_cycles = Some(raw),
            194 => s.temperature_c = Some((raw & 0xff) as i64),
            197 => s.pending = Some(raw),
            198 => s.uncorrectable = Some(raw),
            199 => s.crc_errors = Some(raw),
            231 | 233 => s.life_percent = Some(value as u64),
            241 => s.lba_written = Some(raw),
            242 => s.lba_read = Some(raw),
            _ => {}
        }

        s.attributes.push(SmartAttribute {
            id,
            name: attr_name(id).to_string(),
            value,
            worst,
            threshold: 0,
            raw,
            prefailure: flags & 0x0001 != 0,
            online: flags & 0x0002 != 0,
        });
    }
    s
}

/// Apply thresholds (from SMART READ THRESHOLDS) to parsed attributes.
pub fn apply_thresholds(attrs: &mut [SmartAttribute], thresholds: &[u8]) {
    for attr in attrs.iter_mut() {
        for i in 0..30 {
            let off = 2 + i * 12;
            if off + 2 > thresholds.len() {
                break;
            }
            if thresholds[off] == attr.id {
                attr.threshold = thresholds[off + 1];
                break;
            }
        }
    }
}

/// Parse the 512-byte NVMe SMART/Health information log.
pub fn parse_nvme_health(data: &[u8]) -> SmartData {
    let mut s = SmartData {
        source: "native".to_string(),
        logical_block_size: Some(512),
        ..Default::default()
    };
    if data.len() < 200 {
        return s;
    }

    let critical_warning = data[0];
    let temperature_k = u16::from_le_bytes([data[1], data[2]]) as i64;
    if temperature_k > 0 {
        s.temperature_c = Some(temperature_k - 273);
    }

    let percentage_used = data[5] as u64;
    s.life_percent = Some(100u64.saturating_sub(percentage_used));

    let read_units = to_u64_sat(le_u128(&data[32..]));
    let written_units = to_u64_sat(le_u128(&data[48..]));
    s.lba_read = Some(read_units.saturating_mul(1000));
    s.lba_written = Some(written_units.saturating_mul(1000));

    s.power_cycles = Some(to_u64_sat(le_u128(&data[112..])));
    s.power_on_hours = Some(to_u64_sat(le_u128(&data[128..])));
    s.available_spare = Some(data[3] as u64);
    s.available_spare_threshold = Some(data[4] as u64);
    s.media_errors = Some(to_u64_sat(le_u128(&data[160..])));
    s.nvme_errors = Some(to_u64_sat(le_u128(&data[176..])));
    s.warning_temp_time = Some(u32::from_le_bytes([
        data[192], data[193], data[194], data[195],
    ]) as u64);
    s.critical_temp_time = Some(u32::from_le_bytes([
        data[196], data[197], data[198], data[199],
    ]) as u64);
    s.passed = Some(critical_warning == 0);

    s
}

/// Parse SCSI log page parameters (code -> raw data). Stops at the page
/// length from the header so zero padding is not read as parameters.
pub fn parse_log_params(buf: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    if buf.len() < 4 {
        return out;
    }
    let page_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    let end = buf.len().min(4 + page_len);
    let mut i = 4; // skip 4-byte log page header
    while i + 4 <= end {
        let code = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let len = buf[i + 3] as usize;
        if i + 4 + len > end {
            break;
        }
        out.push((code, buf[i + 4..i + 4 + len].to_vec()));
        i += 4 + len;
    }
    out
}

/// LOG SENSE page control: cumulative values (what smartctl reports).
pub const LOG_PC_CUMULATIVE: u8 = 1;

/// LOG SENSE(10) CDB. PC lives in byte 2 bits 7-6 next to the page code;
/// byte 1 only holds SP/PPC (setting it makes drives reject the command).
pub fn log_sense_cdb(page: u8, pc: u8, alloc: u16) -> [u8; 10] {
    let [hi, lo] = alloc.to_be_bytes();
    [0x4D, 0x00, (pc & 0x03) << 6 | (page & 0x3F), 0, 0, 0, 0, hi, lo, 0]
}

/// READ DEFECT DATA(10) for the grown defect list (REQ_GLIST, format 4).
pub fn read_defect_cdb() -> [u8; 10] {
    [0x37, 0x00, 0x08 | 0x04, 0, 0, 0, 0, 0x00, 0xFC, 0x00]
}

/// Big-endian unsigned counter of up to 8 bytes (SCSI log parameters vary).
fn be_uint(data: &[u8]) -> u64 {
    let tail = &data[data.len().saturating_sub(8)..];
    tail.iter().fold(0u64, |acc, b| (acc << 8) | *b as u64)
}

/// Supported page codes from LOG SENSE page 0x00.
pub fn parse_supported_pages(buf: &[u8]) -> Vec<u8> {
    if buf.len() < 4 {
        return Vec::new();
    }
    let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    buf[4..buf.len().min(4 + len)].iter().map(|p| p & 0x3F).collect()
}

/// Fold one SCSI log page into `s`; returns true when it contributed data.
///
/// - 0x02 / 0x03 write / read error counters: 0x0005 bytes processed,
///   0x0006 total uncorrected errors
/// - 0x0D temperature: 0x0000 current °C (byte 1, 0xFF = unknown)
/// - 0x06 non-medium errors: 0x0000 count
/// - 0x0E start-stop cycle counter: 0x0001 manufacture date ("YYYYWW"),
///   0x0003 / 0x0004 rated / accumulated start-stop cycles,
///   0x0005 / 0x0006 rated / accumulated load-unload cycles
/// - 0x10 self-test results: 0x0001 most recent test
/// - 0x18 SAS protocol port: negotiated link rate + phy error counters
/// - 0x15 background scan results: 0x0000 accumulated power-on minutes
/// - 0x2F informational exceptions: 0x0000 ASC/ASCQ (0 = no failure
///   predicted) and most recent temperature
pub fn apply_scsi_log(s: &mut SmartData, page: u8, buf: &[u8]) -> bool {
    let mut found = false;
    for (code, data) in parse_log_params(buf) {
        match (page, code) {
            (0x02 | 0x03, 0x0006) if !data.is_empty() => {
                s.uncorrectable = Some(s.uncorrectable.unwrap_or(0) + be_uint(&data));
                found = true;
            }
            (0x02 | 0x03, 0x0005) if !data.is_empty() => {
                let blocks = be_uint(&data) / s.block_size().max(1);
                if page == 0x02 {
                    s.lba_written = Some(blocks);
                } else {
                    s.lba_read = Some(blocks);
                }
                found = true;
            }
            (0x0D, 0x0000) if data.len() >= 2 && data[1] != 0xFF => {
                s.temperature_c = Some(data[1] as i64);
                found = true;
            }
            (0x06, 0x0000) if !data.is_empty() => {
                s.non_medium_errors = Some(be_uint(&data));
                found = true;
            }
            (0x0E, 0x0001) if data.len() >= 6 && data[..6].is_ascii() => {
                let text = String::from_utf8_lossy(&data[..6]);
                if let (Ok(y), Ok(w)) = (text[..4].trim().parse::<u16>(), text[4..].trim().parse::<u8>()) {
                    if y > 1990 {
                        s.manufactured = Some((y, w));
                    }
                }
            }
            (0x0E, 0x0003) if data.len() >= 4 => s.rated_start_stop = Some(be_uint(&data[..4])),
            (0x0E, 0x0004) if data.len() >= 4 => {
                s.power_cycles = Some(be_uint(&data[..4]));
                found = true;
            }
            (0x0E, 0x0005) if data.len() >= 4 => s.rated_load_unload = Some(be_uint(&data[..4])),
            (0x0E, 0x0006) if data.len() >= 4 => s.load_unload = Some(be_uint(&data[..4])),
            (0x10, 0x0001) if data.len() >= 4 && data.iter().any(|b| *b != 0) => {
                s.last_self_test = Some(self_test_summary(data[0], u16::from_be_bytes([data[2], data[3]])));
            }
            (0x18, _) => {
                if let Some((rate, errs)) = parse_sas_port(&data) {
                    if s.interface_speed.is_none() {
                        s.interface_speed = rate;
                    }
                    let acc = s.phy_errors.get_or_insert([0; 4]);
                    for (a, e) in acc.iter_mut().zip(errs) {
                        *a += e;
                    }
                }
            }
            (0x15, 0x0000) if data.len() >= 4 => {
                s.power_on_hours = Some(be_uint(&data[..4]) / 60);
                found = true;
            }
            (0x2F, 0x0000) if data.len() >= 2 => {
                s.passed = Some(data[0] == 0);
                if s.temperature_c.is_none() && data.len() >= 3 && data[2] != 0 && data[2] != 0xFF {
                    s.temperature_c = Some(data[2] as i64);
                }
                found = true;
            }
            _ => {}
        }
    }
    found
}

/// "Foreground short: completed (at 1 h)" from a self-test result byte
/// (code in bits 7-5, result in bits 3-0) and its power-on-hours stamp.
fn self_test_summary(b0: u8, hours: u16) -> String {
    let code = match b0 >> 5 {
        1 => "Background short",
        2 => "Background extended",
        4 => "Abort background",
        5 => "Foreground short",
        6 => "Foreground extended",
        _ => "Self-test",
    };
    let result = match b0 & 0x0F {
        0 => "completed",
        1 => "aborted by command",
        2 => "aborted by reset",
        3 => "unknown error",
        4 => "FAILED",
        5..=7 => "FAILED (segment)",
        0xF => "in progress",
        _ => "reserved",
    };
    format!("{code}: {result} (at {hours} h)")
}

/// One SAS protocol-specific port parameter (log page 0x18): negotiated link
/// rate of the first active phy and summed phy error counters.
fn parse_sas_port(data: &[u8]) -> Option<(Option<String>, [u64; 4])> {
    if data.len() < 4 || data[0] & 0x0F != 6 {
        return None; // not SAS
    }
    let phys = data[3] as usize;
    let mut rate = None;
    let mut errs = [0u64; 4];
    let mut off = 4;
    for _ in 0..phys {
        if off + 4 > data.len() {
            break;
        }
        let len = 4 + data[off + 3] as usize;
        let desc = &data[off..data.len().min(off + len)];
        if desc.len() >= 48 {
            if rate.is_none() {
                rate = match desc[5] & 0x0F {
                    0x8 => Some("1.5 Gbps"),
                    0x9 => Some("3 Gbps"),
                    0xA => Some("6 Gbps"),
                    0xB => Some("12 Gbps"),
                    0xC => Some("22.5 Gbps"),
                    _ => None,
                }
                .map(str::to_string);
            }
            for (i, e) in errs.iter_mut().enumerate() {
                *e += be_uint(&desc[32 + i * 4..36 + i * 4]);
            }
        }
        off += len;
    }
    Some((rate, errs))
}

/// Entries of the ATA Device Statistics log (0x04): `(page, offset, value)`
/// for every statistic flagged supported + valid. Each 512-byte page starts
/// with a header qword (byte 2 = page number); statistics are little-endian
/// qwords with bit 63 = supported, bit 62 = valid, value in bits 0-47.
pub fn parse_device_stats(log: &[u8]) -> Vec<(u8, u16, u64)> {
    let mut out = Vec::new();
    for page in log.as_chunks::<512>().0 {
        let num = page[2];
        if num == 0 || page.iter().all(|b| *b == 0) {
            continue; // page 0 lists supported pages; empty = unsupported
        }
        for off in (8..512).step_by(8) {
            let q = u64::from_le_bytes(page[off..off + 8].try_into().unwrap());
            if q >> 63 & 1 == 1 && q >> 62 & 1 == 1 {
                out.push((num, off as u16, q & 0xFFFF_FFFF_FFFF));
            }
        }
    }
    out
}

/// ATA PASS-THROUGH(16) CDB (SAT). `protocol` 4 = PIO data-in (512-byte
/// blocks, length in the count field), 3 = non-data with CK_COND so the
/// drive's registers come back in the sense data. `lba` is 24-bit.
pub fn sat_cdb(protocol: u8, command: u8, feature: u8, count: u8, lba: u32) -> [u8; 16] {
    let flags = match protocol {
        4 => 0x0E, // T_DIR = from device, BYT_BLOK = blocks, T_LENGTH = count field
        _ => 0x20, // CK_COND: return ATA registers
    };
    [
        0x85,
        protocol << 1,
        flags,
        0,
        feature,
        0,
        count,
        0,
        lba as u8,
        0,
        (lba >> 8) as u8,
        0,
        (lba >> 16) as u8,
        0, // device
        command,
        0,
    ]
}

/// SMART RETURN STATUS from SAT sense data: LBA mid/high 0x4F/0xC2 = OK,
/// 0xF4/0x2C = threshold exceeded. Handles descriptor (0x72, ATA Status
/// Return descriptor 0x09) and fixed (0x70) sense formats.
pub fn parse_sat_smart_status(sense: &[u8]) -> Option<bool> {
    let (mid, high) = match sense.first()? & 0x7F {
        0x72 | 0x73 => {
            let total = 8 + *sense.get(7)? as usize;
            let mut i = 8;
            let mut regs = None;
            while i + 1 < total.min(sense.len()) {
                let len = sense[i + 1] as usize + 2;
                if sense[i] == 0x09 && i + 13 < sense.len() {
                    regs = Some((sense[i + 9], sense[i + 11]));
                    break;
                }
                i += len;
            }
            regs?
        }
        0x70 | 0x71 => (*sense.get(10)?, *sense.get(11)?),
        _ => return None,
    };
    match (mid, high) {
        (0x4F, 0xC2) => Some(true),
        (0xF4, 0x2C) => Some(false),
        _ => None,
    }
}

/// Effective user id is root (SMART ioctls need it).
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        extern "C" {
            fn geteuid() -> u32;
        }
        unsafe { geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    use crate::smartctl::apply_device_stat;

    use super::{
        apply_scsi_log, parse_device_stats, log_sense_cdb, read_defect_cdb, nonempty, parse_ata_smart, parse_nvme_health,
        parse_supported_pages, IdInfo, SelfTest, SmartData, LOG_PC_CUMULATIVE,
    };
    use crate::model::{Bus, Device, MediaKind};

    type CInt = i32;
    type CULong = u64;

    extern "C" {
        fn ioctl(fd: CInt, request: CULong, ...) -> CInt;
    }

    const HDIO_DRIVE_CMD: CULong = 0x031f;
    const HDIO_GET_IDENTITY: CULong = 0x030d;
    const WIN_SMART: u8 = 0xB0;
    const SMART_READ_DATA: u8 = 0xD0;
    const SMART_READ_THRESHOLDS: u8 = 0xD1;
    const SMART_ENABLE: u8 = 0xD8;
    const SMART_RETURN_STATUS: u8 = 0xDA;
    const SMART_EXECUTE_OFFLINE: u8 = 0xD4;

    const SG_IO: CULong = 0x2285;
    const SG_DXFER_NONE: CInt = -1;
    const SG_DXFER_FROM_DEV: CInt = -3;

    // _IOWR('N', 0x41, struct nvme_admin_cmd) with sizeof == 72.
    const NVME_IOCTL_ADMIN_CMD: CULong = 0xC048_4E41;

    #[repr(C)]
    struct SgIoHdr {
        interface_id: CInt,
        dxfer_direction: CInt,
        cmd_len: u8,
        mx_sb_len: u8,
        iovec_count: u16,
        dxfer_len: u32,
        dxferp: *mut u8,
        cmdp: *mut u8,
        sbp: *mut u8,
        timeout: u32,
        flags: u32,
        pack_id: CInt,
        usr_ptr: *mut u8,
        status: u8,
        masked_status: u8,
        msg_status: u8,
        sb_len_wr: u8,
        host_status: u16,
        driver_status: u16,
        resid: CInt,
        duration: u32,
        info: u32,
    }

    #[repr(C)]
    struct NvmeAdminCmd {
        opcode: u8,
        flags: u8,
        command_id: u16,
        nsid: u32,
        cdw2: u32,
        cdw3: u32,
        metadata: u64,
        addr: u64,
        metadata_len: u32,
        data_len: u32,
        cdw10: u32,
        cdw11: u32,
        cdw12: u32,
        cdw13: u32,
        cdw14: u32,
        cdw15: u32,
        timeout_ms: u32,
        result: u32,
    }

    const SMART_READ_LOG: u8 = 0xD5;

    fn hdio_cmd(fd: CInt, command: u8, feature: u8, count: u8, buf: &mut [u8; 516]) -> CInt {
        buf[0] = command;
        buf[1] = 0;
        buf[2] = feature;
        buf[3] = count;
        unsafe { ioctl(fd, HDIO_DRIVE_CMD, buf.as_mut_ptr()) }
    }

    /// Issue a SCSI command returning data. Success = GOOD status.
    fn scsi_cmd(fd: CInt, cdb: &[u8], data: &mut [u8]) -> bool {
        matches!(sg_io(fd, cdb, data), Some((0, _)))
    }

    /// Raw SG_IO: `(SCSI status, sense bytes)`, or None when the ioctl or
    /// the transport failed. `data` empty = no data phase.
    fn sg_io(fd: CInt, cdb: &[u8], data: &mut [u8]) -> Option<(u8, Vec<u8>)> {
        let mut sense = [0u8; 32];
        let mut cdb = cdb.to_vec();
        let mut hdr = SgIoHdr {
            interface_id: 'S' as CInt,
            dxfer_direction: if data.is_empty() { SG_DXFER_NONE } else { SG_DXFER_FROM_DEV },
            cmd_len: cdb.len() as u8,
            mx_sb_len: sense.len() as u8,
            iovec_count: 0,
            dxfer_len: data.len() as u32,
            dxferp: data.as_mut_ptr(),
            cmdp: cdb.as_mut_ptr(),
            sbp: sense.as_mut_ptr(),
            timeout: 20_000,
            flags: 0,
            pack_id: 0,
            usr_ptr: std::ptr::null_mut(),
            status: 0,
            masked_status: 0,
            msg_status: 0,
            sb_len_wr: 0,
            host_status: 0,
            driver_status: 0,
            resid: 0,
            duration: 0,
            info: 0,
        };
        let rc = unsafe { ioctl(fd, SG_IO, &mut hdr as *mut SgIoHdr) };
        // host_status != 0: the HBA never delivered the command.
        if rc != 0 || hdr.host_status != 0 {
            return None;
        }
        // DRIVER_SENSE (0x08) only says sense data is present.
        if hdr.driver_status & !0x08 != 0 {
            return None;
        }
        let n = (hdr.sb_len_wr as usize).min(sense.len());
        Some((hdr.status, sense[..n].to_vec()))
    }

    /// SAT PIO data-in; accepts GOOD, or CHECK CONDITION that only reports
    /// "ATA pass-through information available" (RECOVERED ERROR 00/1D).
    fn sat_pio_in(fd: CInt, command: u8, feature: u8, count: u8, lba: u32) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; 512 * count as usize];
        let cdb = super::sat_cdb(4, command, feature, count, lba);
        let (status, sense) = sg_io(fd, &cdb, &mut buf)?;
        let ok = status == 0 || (status == 2 && sense_is_passthrough_info(&sense));
        (ok && buf.iter().any(|b| *b != 0)).then_some(buf)
    }

    fn sense_is_passthrough_info(sense: &[u8]) -> bool {
        let (key, asc, ascq) = match sense.first().map(|b| b & 0x7F) {
            Some(0x72 | 0x73) if sense.len() >= 4 => (sense[1] & 0x0F, sense[2], sense[3]),
            Some(0x70 | 0x71) if sense.len() >= 14 => (sense[2] & 0x0F, sense[12], sense[13]),
            _ => return false,
        };
        key == 0x01 && asc == 0x00 && ascq == 0x1D
    }

    /// SMART over SAT: LBA mid/high carry the 0x4F/0xC2 SMART signature.
    const SMART_LBA: u32 = 0xC2_4F00;

    /// SMART over ATA PASS-THROUGH (SAT) — SATA drives behind SAS/RAID HBAs
    /// and USB bridges.
    pub fn sat_read(device: &Device) -> Option<SmartData> {
        let file = fs::File::open(&device.path).ok()?;
        let fd = file.as_raw_fd();
        let smart_in = |feature: u8, count: u8, lba_low: u8| {
            sat_pio_in(fd, WIN_SMART, feature, count, SMART_LBA | lba_low as u32)
        };
        let status = || {
            let cdb = super::sat_cdb(3, WIN_SMART, SMART_RETURN_STATUS, 0, SMART_LBA);
            let (_, sense) = sg_io(fd, &cdb, &mut [])?;
            super::parse_sat_smart_status(&sense)
        };
        let mut smart = ata_smart(smart_in, status)?;
        smart.source = "native (SAT)".to_string();
        Some(smart)
    }

    /// Shared SMART read for any ATA transport: `data_in(feature, sectors,
    /// lba_low)` runs a SMART data-in subcommand, `status()` SMART RETURN
    /// STATUS.
    fn ata_smart(
        data_in: impl Fn(u8, u8, u8) -> Option<Vec<u8>>,
        status: impl Fn() -> Option<bool>,
    ) -> Option<SmartData> {
        let data = data_in(SMART_READ_DATA, 1, 0)?;
        let mut smart = parse_ata_smart(&data);
        if let Some(thr) = data_in(SMART_READ_THRESHOLDS, 1, 0) {
            super::apply_thresholds(&mut smart.attributes, &thr);
        }
        smart.passed = status();
        // Device Statistics (log 0x04, up to 8 pages): standardized writes,
        // endurance used, temperature history, resets.
        if let Some(log) = data_in(SMART_READ_LOG, 8, 0x04).or_else(|| data_in(SMART_READ_LOG, 1, 0x04)) {
            for (page, off, value) in parse_device_stats(&log) {
                apply_device_stat(&mut smart, page, off, value);
            }
        }
        Some(smart)
    }

    /// ATA fixed-length string field. `/sys`/ioctl return the IDENTIFY words in
    /// native endianness such that memory bytes are already in ASCII order.
    fn ata_string(data: &[u8], word: usize, words: usize) -> String {
        let mut out = String::new();
        for i in 0..words {
            let off = (word + i) * 2;
            if off + 1 >= data.len() {
                break;
            }
            out.push(data[off] as char);
            out.push(data[off + 1] as char);
        }
        out
    }

    fn ascii_trim(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes)
            .trim_matches(|c: char| c == '\0' || c.is_whitespace())
            .trim()
            .to_string()
    }

    fn ata_identity(fd: CInt) -> Option<IdInfo> {
        // HDIO_GET_IDENTITY returns the 512-byte IDENTIFY DEVICE structure.
        let mut data = [0u8; 512];
        let rc = unsafe { ioctl(fd, HDIO_GET_IDENTITY, data.as_mut_ptr()) };
        if rc != 0 || !data.iter().any(|b| *b != 0) {
            return None;
        }
        if std::env::var_os("DCHECK_DEBUG").is_some() {
            eprintln!(
                "[id] serial={:?} fw={:?} model={:?}",
                ata_string(&data, 10, 10),
                ata_string(&data, 23, 4),
                ata_string(&data, 27, 20)
            );
        }
        Some(IdInfo {
            model: nonempty(ata_string(&data, 27, 20)),
            firmware: nonempty(ata_string(&data, 23, 4)),
            serial: nonempty(ata_string(&data, 10, 10)),
            wwn: super::ata_wwn(&data),
        })
    }
    fn scsi_identity(fd: CInt) -> Option<IdInfo> {
        let mut info = IdInfo::default();

        let mut inq = [0u8; 96];
        if scsi_cmd(fd, &[0x12, 0x00, 0x00, 0x00, 96, 0x00], &mut inq) {
            let vendor = ascii_trim(&inq[8..16]);
            let product = ascii_trim(&inq[16..32]);
            // SATA disks reached via SCSI INQUIRY report vendor "ATA"; keep the
            // product only, otherwise prepend the real vendor.
            let model = if vendor.is_empty() || vendor == "ATA" {
                product
            } else {
                format!("{vendor} {product}")
            };
            info.model = nonempty(model);
            info.firmware = nonempty(ascii_trim(&inq[32..36]));
        }

        let mut vpd = [0u8; 252];
        if scsi_cmd(fd, &[0x12, 0x01, 0x80, 0x00, 0xFC, 0x00], &mut vpd) {
            let len = vpd[3] as usize;
            if 4 + len <= vpd.len() {
                info.serial = nonempty(ascii_trim(&vpd[4..4 + len]));
            }
        }
        let mut vpd83 = [0u8; 252];
        if scsi_cmd(fd, &[0x12, 0x01, 0x83, 0x00, 0xFC, 0x00], &mut vpd83) {
            info.wwn = super::parse_vpd83_naa(&vpd83);
        }

        if info.is_empty() {
            None
        } else {
            Some(info)
        }
    }

    pub fn identity(device: &Device) -> IdInfo {
        let Ok(file) = fs::File::open(&device.path) else {
            return IdInfo::default();
        };
        let fd = file.as_raw_fd();

        if device.kind == MediaKind::Nvme || device.name.starts_with("nvme") {
            return IdInfo::default(); // sysfs already exposes NVMe identity
        }
        if device.bus == Bus::Scsi {
            return scsi_identity(fd).unwrap_or_default();
        }
        ata_identity(fd)
            .or_else(|| scsi_identity(fd))
            .unwrap_or_default()
    }

    /// Read one SCSI log page (cumulative values).
    fn log_page(fd: CInt, page: u8) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; 1024];
        let cdb = log_sense_cdb(page, LOG_PC_CUMULATIVE, buf.len() as u16);
        scsi_cmd(fd, &cdb, &mut buf).then_some(buf)
    }

    pub fn scsi_read(device: &Device) -> Option<SmartData> {
        let file = fs::File::open(&device.path).ok()?;
        let fd = file.as_raw_fd();
        let mut s = SmartData {
            source: "native".to_string(),
            logical_block_size: Some(device.logical_block_size.max(512)),
            ..Default::default()
        };

        // Only ask for pages the drive lists (page 0x00); fall back to all.
        const PAGES: [u8; 9] = [0x2F, 0x0D, 0x02, 0x03, 0x06, 0x0E, 0x10, 0x15, 0x18];
        let supported = log_page(fd, 0x00).map(|b| parse_supported_pages(&b));
        let mut found = false;
        for page in PAGES {
            if supported.as_ref().is_some_and(|sp| !sp.contains(&page)) {
                continue;
            }
            if let Some(buf) = log_page(fd, page) {
                found |= apply_scsi_log(&mut s, page, &buf);
            }
        }

        // Grown defect list: READ DEFECT DATA(10) with REQ_GLIST and the
        // bytes-from-index format in byte 2 (8-byte entries); the header's
        // list length is valid even when the list itself is truncated.
        let mut defect = [0u8; 252];
        if scsi_cmd(fd, &read_defect_cdb(), &mut defect) {
            let len = u16::from_be_bytes([defect[2], defect[3]]) as usize;
            s.reallocated = Some((len / 8) as u64);
            found = true;
        }

        if !found {
            return None;
        }
        if s.passed.is_none() {
            // No informational-exceptions page: judge by uncorrected errors.
            s.passed = Some(s.uncorrectable.unwrap_or(0) == 0);
        }
        Some(s)
    }

    fn selftest_status(v: u8) -> &'static str {
        match v {
            0 => "completed without error",
            1 => "aborted by host",
            2 => "interrupted by reset",
            3 => "fatal error",
            4 => "unknown failure",
            5 => "completed: failed elements",
            0x0f => "in progress",
            _ => "reserved",
        }
    }

    pub fn ata_selftest(device: &Device, kind: SelfTest) -> Option<String> {
        let file = fs::File::open(&device.path).ok()?;
        let fd = file.as_raw_fd();
        let mut buf = [0u8; 516];
        if hdio_cmd(fd, WIN_SMART, SMART_EXECUTE_OFFLINE, kind.code(), &mut buf) != 0 {
            return None;
        }
        let mut data = [0u8; 516];
        if hdio_cmd(fd, WIN_SMART, SMART_READ_DATA, 1, &mut data) == 0 {
            let status = data[4 + 363] & 0x0f;
            let remaining = data[4 + 364];
            Some(format!(
                "{} self-test started; status: {} ({}% remaining)",
                kind.label(),
                selftest_status(status),
                remaining
            ))
        } else {
            Some(format!("{} self-test started", kind.label()))
        }
    }

    /// SMART through the kernel's HDIO_DRIVE_CMD (directly attached SATA).
    pub fn ata_read(device: &Device) -> Option<SmartData> {
        let file = fs::File::open(&device.path).ok()?;
        let fd = file.as_raw_fd();

        // Some drives ship with SMART disabled: enable once if the first read
        // comes back empty.
        let mut probe = [0u8; 516];
        if hdio_cmd(fd, WIN_SMART, SMART_READ_DATA, 1, &mut probe) != 0
            || !probe[4..].iter().any(|b| *b != 0)
        {
            let mut enable = [0u8; 516];
            let _ = hdio_cmd(fd, WIN_SMART, SMART_ENABLE, 0, &mut enable);
        }
        let data_in = |feature: u8, count: u8, lba_low: u8| -> Option<Vec<u8>> {
            let mut buf = vec![0u8; 4 + 512 * count as usize];
            buf[0] = WIN_SMART;
            buf[1] = lba_low;
            buf[2] = feature;
            buf[3] = count;
            let rc = unsafe { ioctl(fd, HDIO_DRIVE_CMD, buf.as_mut_ptr()) };
            (rc == 0 && buf[4..].iter().any(|b| *b != 0)).then(|| buf.split_off(4))
        };
        let status = || {
            let mut st = [0u8; 516];
            (hdio_cmd(fd, WIN_SMART, SMART_RETURN_STATUS, 0, &mut st) == 0 && st[0] != 0)
                .then(|| st[0] & 0x01 == 0)
        };
        ata_smart(data_in, status)
    }

    pub fn nvme_read(device: &Device) -> Option<SmartData> {
        let controller = controller_path(&device.name)?;
        let file = fs::File::open(&controller).ok()?;
        let fd = file.as_raw_fd();

        let mut data = [0u8; 512];
        let mut cmd = NvmeAdminCmd {
            opcode: 0x02,
            flags: 0,
            command_id: 1,
            nsid: 0xffff_ffff,
            cdw2: 0,
            cdw3: 0,
            metadata: 0,
            addr: data.as_mut_ptr() as u64,
            metadata_len: 0,
            data_len: data.len() as u32,
            cdw10: 0x02,
            cdw11: 0,
            cdw12: 0,
            cdw13: 0,
            cdw14: 0,
            cdw15: 0,
            timeout_ms: 10_000,
            result: 0,
        };
        let rc = unsafe { ioctl(fd, NVME_IOCTL_ADMIN_CMD, &mut cmd as *mut NvmeAdminCmd) };
        if rc != 0 || cmd.result != 0 {
            return None;
        }
        Some(parse_nvme_health(&data))
    }

    pub fn link_speed(device: &Device) -> Option<String> {
        // NVMe: PCIe negotiated link.
        if let Some(ctrl) = controller_name(&device.name) {
            let base = format!("/sys/class/nvme/{ctrl}/device");
            let speed = fs::read_to_string(format!("{base}/current_link_speed")).ok()?;
            let speed = speed.trim();
            if !speed.is_empty() {
                let width = fs::read_to_string(format!("{base}/current_link_width"))
                    .ok()
                    .map(|w| w.trim().to_string())
                    .filter(|w| !w.is_empty());
                return Some(match width {
                    Some(w) => format!("{speed} x{w}"),
                    None => speed.to_string(),
                });
            }
        }

        // SATA: negotiated link speed from the matching ata_link.
        let base = Path::new("/sys/block").join(&device.name);
        let resolved = fs::canonicalize(base.join("device")).ok()?;
        let path = resolved.to_string_lossy();
        if let Some(pos) = path.find("/ata") {
            let digits: String = path[pos + 4..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                let spd =
                    fs::read_to_string(format!("/sys/class/ata_link/link{digits}/sata_spd")).ok()?;
                let spd = spd.trim();
                if !spd.is_empty() && spd != "<unknown>" {
                    return Some(spd.to_string());
                }
            }
        }
        None
    }

    fn controller_name(name: &str) -> Option<String> {
        let rest = name.strip_prefix("nvme")?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            None
        } else {
            Some(format!("nvme{digits}"))
        }
    }

    fn controller_path(name: &str) -> Option<String> {
        controller_name(name).map(|c| format!("/dev/{c}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wwn_from_identify_and_vpd83() {
        let mut id = [0u8; 512];
        for (w, v) in [(108, 0x5002u16), (109, 0x538e), (110, 0x1024), (111, 0x9cb0)] {
            id[w * 2..w * 2 + 2].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(ata_wwn(&id).as_deref(), Some("5002538e10249cb0"));
        assert_eq!(ata_wwn(&[0u8; 512]).as_deref(), Some("0000000000000000"));
        // T10 vendor id designator, then the LU NAA one.
        let mut page = vec![0x00, 0x83, 0x00, 0x00];
        page.extend([0x02, 0x01, 0x00, 0x04, b'A', b'T', b'A', b' ']);
        page.extend([0x01, 0x03, 0x00, 0x08, 0x50, 0x00, 0xc5, 0x00, 0x71, 0x78, 0x1f, 0x63]);
        page[3] = (page.len() - 4) as u8;
        assert_eq!(parse_vpd83_naa(&page).as_deref(), Some("5000c50071781f63"));
        assert_eq!(parse_vpd83_naa(&[0, 0x80, 0, 0]), None);
    }

    #[test]
    fn sat_cdb_layout() {
        // SMART READ DATA: PIO in, blocks, count field, 0xC24F signature.
        let cdb = sat_cdb(4, 0xB0, 0xD0, 1, 0xC2_4F00);
        assert_eq!(cdb[0], 0x85);
        assert_eq!(cdb[1], 4 << 1);
        assert_eq!(cdb[2], 0x0E);
        assert_eq!((cdb[4], cdb[6]), (0xD0, 1));
        assert_eq!((cdb[8], cdb[10], cdb[12]), (0x00, 0x4F, 0xC2));
        assert_eq!(cdb[14], 0xB0);
        // SMART READ LOG 0x04: LBA low carries the log address.
        assert_eq!(sat_cdb(4, 0xB0, 0xD5, 8, 0xC2_4F04)[8], 0x04);
        // Non-data RETURN STATUS asks for registers (CK_COND).
        let st = sat_cdb(3, 0xB0, 0xDA, 0, 0xC2_4F00);
        assert_eq!((st[1], st[2]), (3 << 1, 0x20));
    }

    #[test]
    fn parses_sat_smart_status_from_sense() {
        // Descriptor format: 0x72, RECOVERED ERROR 00/1D, ATA status return
        // descriptor with LBA mid/high at +9/+11.
        let mut desc = vec![0x72, 0x01, 0x00, 0x1D, 0, 0, 0, 14];
        desc.extend([0x09, 0x0C, 0, 0, 0, 0, 0, 0, 0, 0x4F, 0, 0xC2, 0, 0x50]);
        assert_eq!(parse_sat_smart_status(&desc), Some(true));
        desc[8 + 9] = 0xF4;
        desc[8 + 11] = 0x2C;
        assert_eq!(parse_sat_smart_status(&desc), Some(false));
        // Fixed format: LBA mid/high in bytes 10/11.
        let mut fixed = vec![0u8; 18];
        fixed[0] = 0x70;
        fixed[10] = 0x4F;
        fixed[11] = 0xC2;
        assert_eq!(parse_sat_smart_status(&fixed), Some(true));
        assert_eq!(parse_sat_smart_status(&[]), None);
    }

    #[test]
    fn parses_ata_device_statistics_log() {
        let mut log = vec![0u8; 512 * 2];
        // page 1 (general): header + power-on resets = 32, sectors written.
        log[2] = 1;
        let valid = |v: u64| (v | (0b11 << 62)).to_le_bytes();
        log[8..16].copy_from_slice(&valid(32));
        log[24..32].copy_from_slice(&valid(4_016_839_893));
        log[32..40].copy_from_slice(&(7u64 | (1 << 63)).to_le_bytes()); // supported, not valid
        // page 7 (SSD): percentage used = 2.
        log[512 + 2] = 7;
        log[512 + 8..512 + 16].copy_from_slice(&valid(2));
        let stats = parse_device_stats(&log);
        assert_eq!(stats, vec![(1, 0x08, 32), (1, 0x18, 4_016_839_893), (7, 0x08, 2)]);
    }

    #[test]
    fn applies_start_stop_self_test_and_non_medium_pages() {
        let mut s = SmartData::default();
        apply_scsi_log(
            &mut s,
            0x0E,
            &log_page(
                0x0E,
                &[
                    (0x0001, b"201212"),
                    (0x0003, &50_000u32.to_be_bytes()),
                    (0x0004, &40u32.to_be_bytes()),
                    (0x0005, &200_000u32.to_be_bytes()),
                    (0x0006, &1_785u32.to_be_bytes()),
                ],
            ),
        );
        assert_eq!(s.manufactured, Some((2012, 12)));
        assert_eq!((s.power_cycles, s.rated_start_stop), (Some(40), Some(50_000)));
        assert_eq!((s.load_unload, s.rated_load_unload), (Some(1_785), Some(200_000)));

        apply_scsi_log(&mut s, 0x06, &log_page(0x06, &[(0x0000, &[0, 0, 0, 38])]));
        assert_eq!(s.non_medium_errors, Some(38));

        let mut st = [0u8; 16];
        st[0] = 5 << 5; // foreground short, result 0 = completed
        st[3] = 1;
        apply_scsi_log(&mut s, 0x10, &log_page(0x10, &[(0x0001, &st)]));
        assert_eq!(s.last_self_test.as_deref(), Some("Foreground short: completed (at 1 h)"));
    }

    #[test]
    fn parses_sas_port_phy_counters() {
        let mut data = vec![0x06, 0, 2, 1]; // SAS, generation 2, 1 phy
        let mut desc = vec![0u8; 48];
        desc[3] = 44;
        desc[5] = 0x0A; // 6 Gbps
        desc[32..36].copy_from_slice(&724u32.to_be_bytes());
        desc[36..40].copy_from_slice(&715u32.to_be_bytes());
        desc[40..44].copy_from_slice(&181u32.to_be_bytes());
        desc[44..48].copy_from_slice(&4u32.to_be_bytes());
        data.extend(desc);
        let mut s = SmartData::default();
        apply_scsi_log(&mut s, 0x18, &log_page(0x18, &[(0x0001, &data)]));
        assert_eq!(s.interface_speed.as_deref(), Some("6 Gbps"));
        assert_eq!(s.phy_errors, Some([724, 715, 181, 4]));
    }

    /// Build a log page: header + (code, data) parameters.
    fn log_page(page: u8, params: &[(u16, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (code, data) in params {
            body.extend_from_slice(&code.to_be_bytes());
            body.push(0x03); // DU/DS/TSD/ETC/TMC/LBIN/LP flags
            body.push(data.len() as u8);
            body.extend_from_slice(data);
        }
        let mut out = vec![page, 0];
        out.extend_from_slice(&(body.len() as u16).to_be_bytes());
        out.extend(body);
        out.resize(252, 0); // zero padding like a real reply
        out
    }

    #[test]
    fn log_sense_cdb_puts_page_control_in_byte_2() {
        let cdb = log_sense_cdb(0x0D, LOG_PC_CUMULATIVE, 1024);
        assert_eq!(cdb[0], 0x4D);
        assert_eq!(cdb[1], 0x00, "byte 1 is SP/PPC only");
        assert_eq!(cdb[2], 0x40 | 0x0D);
        assert_eq!([cdb[7], cdb[8]], [0x04, 0x00]);
    }

    #[test]
    fn read_defect_cdb_requests_glist_in_byte_2() {
        let cdb = read_defect_cdb();
        assert_eq!(cdb[0], 0x37);
        assert_eq!(cdb[1], 0x00);
        assert_eq!(cdb[2] & 0x08, 0x08);
    }

    #[test]
    fn parses_supported_pages() {
        let buf = [0x00, 0x00, 0x00, 0x04, 0x00, 0x02, 0x0D, 0x2F, 0x00, 0x00];
        assert_eq!(parse_supported_pages(&buf), vec![0x00, 0x02, 0x0D, 0x2F]);
    }

    #[test]
    fn log_params_stop_at_page_length() {
        let buf = log_page(0x0D, &[(0x0000, &[0, 38])]);
        assert_eq!(parse_log_params(&buf).len(), 1);
    }

    #[test]
    fn applies_sas_log_pages() {
        let mut s = SmartData {
            logical_block_size: Some(512),
            ..Default::default()
        };
        assert!(apply_scsi_log(&mut s, 0x2F, &log_page(0x2F, &[(0x0000, &[0x00, 0x00, 36])])));
        assert!(apply_scsi_log(&mut s, 0x0D, &log_page(0x0D, &[(0x0000, &[0, 38]), (0x0001, &[0, 68])])));
        let written = 1_000_000_000_000u64.to_be_bytes();
        assert!(apply_scsi_log(
            &mut s,
            0x02,
            &log_page(0x02, &[(0x0005, &written), (0x0006, &[0, 0, 0, 2])])
        ));
        assert!(apply_scsi_log(&mut s, 0x03, &log_page(0x03, &[(0x0006, &[0, 0, 0, 1])])));
        assert!(apply_scsi_log(
            &mut s,
            0x0E,
            &log_page(0x0E, &[(0x0003, &[0, 0, 0x27, 0x10]), (0x0004, &[0, 0, 0x01, 0x2C])])
        ));
        let minutes = (5000u32 * 60).to_be_bytes();
        assert!(apply_scsi_log(&mut s, 0x15, &log_page(0x15, &[(0x0000, &minutes)])));

        assert_eq!(s.passed, Some(true));
        assert_eq!(s.temperature_c, Some(38));
        assert_eq!(s.uncorrectable, Some(3), "write 2 + read 1, not bytes processed");
        assert_eq!(s.bytes_written(), Some(1_000_000_000_000));
        assert_eq!(s.power_cycles, Some(300));
        assert_eq!(s.power_on_hours, Some(5000));
    }

    #[test]
    fn informational_exception_marks_failure() {
        let mut s = SmartData::default();
        apply_scsi_log(&mut s, 0x2F, &log_page(0x2F, &[(0x0000, &[0x5D, 0x10, 40])]));
        assert_eq!(s.passed, Some(false));
        assert_eq!(s.temperature_c, Some(40));
    }

    #[test]
    fn parses_ata_attributes() {
        let mut data = vec![0u8; 512];
        let mut put = |idx: usize, id: u8, value: u8, raw: u64| {
            let off = 2 + idx * 12;
            data[off] = id;
            data[off + 3] = value;
            for (i, b) in raw.to_le_bytes().iter().take(6).enumerate() {
                data[off + 5 + i] = *b;
            }
        };
        put(0, 5, 100, 0);
        put(1, 9, 99, 12345);
        put(2, 12, 100, 38);
        put(3, 194, 100, 33);
        put(4, 241, 99, 500_000);

        let s = parse_ata_smart(&data);
        assert_eq!(s.power_on_hours, Some(12345));
        assert_eq!(s.power_cycles, Some(38));
        assert_eq!(s.temperature_c, Some(33));
        assert_eq!(s.lba_written, Some(500_000));
    }

    #[test]
    fn parses_attributes_and_thresholds() {
        let mut data = vec![0u8; 512];
        let put = |d: &mut [u8], idx: usize, id: u8, flags: u16, value: u8, worst: u8, raw: u64| {
            let off = 2 + idx * 12;
            d[off] = id;
            d[off + 1] = flags as u8;
            d[off + 2] = (flags >> 8) as u8;
            d[off + 3] = value;
            d[off + 4] = worst;
            for (i, b) in raw.to_le_bytes().iter().take(6).enumerate() {
                d[off + 5 + i] = *b;
            }
        };
        put(&mut data, 0, 5, 0x0013, 90, 95, 3);

        let mut s = parse_ata_smart(&data);
        assert_eq!(s.attributes.len(), 1);
        assert_eq!(s.attributes[0].id, 5);
        assert!(s.attributes[0].prefailure);
        assert!(!s.attributes[0].failing()); // no threshold yet

        let mut thr = vec![0u8; 512];
        thr[2] = 5;
        thr[3] = 100;
        apply_thresholds(&mut s.attributes, &thr);
        assert_eq!(s.attributes[0].threshold, 100);
        assert!(s.attributes[0].failing());
    }

    #[test]
    fn parses_nvme_health_log() {
        let mut data = vec![0u8; 512];
        data[0] = 0;
        data[1..3].copy_from_slice(&311u16.to_le_bytes());
        data[5] = 7;
        data[48..64].copy_from_slice(&2_000_000u128.to_le_bytes());
        data[128..144].copy_from_slice(&4321u128.to_le_bytes());
        data[3] = 100;
        data[4] = 10;
        data[160..176].copy_from_slice(&5u128.to_le_bytes());
        data[176..192].copy_from_slice(&3u128.to_le_bytes());
        data[192..196].copy_from_slice(&7u32.to_le_bytes());

        let s = parse_nvme_health(&data);
        assert_eq!(s.passed, Some(true));
        assert_eq!(s.temperature_c, Some(38));
        assert_eq!(s.life_percent, Some(93));
        assert_eq!(s.power_on_hours, Some(4321));
        assert_eq!(s.available_spare, Some(100));
        assert_eq!(s.media_errors, Some(5));
        assert_eq!(s.nvme_errors, Some(3));
        assert_eq!(s.warning_temp_time, Some(7));
        assert_eq!(s.bytes_written(), Some(2_000_000u64 * 1000 * 512));
    }

    #[test]
    fn parses_scsi_log_parameters() {
        // header (4, page length 11) + param 0x0000 len2 value 33 + param 0x0001 len1 value 5
        let buf = [0x0d, 0x00, 0x00, 0x0B, 0x00, 0x00, 0x00, 0x02, 0x00, 0x21,
                   0x00, 0x01, 0x00, 0x01, 0x05];
        let params = parse_log_params(&buf);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].0, 0x0000);
        assert_eq!(params[0].1, vec![0x00, 0x21]);
        assert_eq!(params[1].0, 0x0001);
    }
}
