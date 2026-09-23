//! Native hardware readers (no external tools).
//!
//! Linux only at runtime:
//!   - SATA: `HDIO_DRIVE_CMD` for IDENTIFY DEVICE + SMART.
//!   - SCSI/SAS: `SG_IO` for INQUIRY/VPD identity (LOG SENSE health where the
//!     transport allows it; some RAID/JBOD controllers block it).
//!   - NVMe: admin command ioctl for the SMART/Health log, sysfs for the link.
//! The pure byte parsers are platform-independent and unit tested everywhere.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::model::Device;
use crate::smartctl::{attr_name, SmartAttribute, SmartData};

/// Identity fields read natively (used to enrich the report when smartctl is
/// absent).
#[derive(Debug, Default, Clone)]
pub struct IdInfo {
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub serial: Option<String>,
}

impl IdInfo {
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.firmware.is_none() && self.serial.is_none()
    }
}

/// Read SMART natively. Returns `None` on non-Linux or when the ioctl path
/// fails (no privileges, unsupported device/transport).
pub fn read(device: &Device) -> Option<SmartData> {
    #[cfg(target_os = "linux")]
    {
        use crate::model::{Bus, MediaKind};
        match (device.kind, device.bus) {
            (MediaKind::Nvme, _) | (_, Bus::Nvme) => linux::nvme_read(device),
            (_, Bus::Scsi) => linux::scsi_read(device),
            _ => linux::ata_read(device),
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
    let mut s = SmartData::default();
    s.source = "native".to_string();
    s.logical_block_size = Some(512);

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
    let mut s = SmartData::default();
    s.source = "native".to_string();
    s.logical_block_size = Some(512);
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
    s.passed = Some(critical_warning == 0);

    s
}

/// Parse SCSI log page parameters (code -> raw data).
pub fn parse_log_params(buf: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 4; // skip 4-byte log page header
    while i + 4 <= buf.len() {
        let code = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let len = buf[i + 3] as usize;
        if i + 4 + len > buf.len() {
            break;
        }
        out.push((code, buf[i + 4..i + 4 + len].to_vec()));
        i += 4 + len;
    }
    out
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    use super::{
        nonempty, parse_ata_smart, parse_log_params, parse_nvme_health, IdInfo, SelfTest, SmartData,
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

    fn hdio_cmd(fd: CInt, command: u8, feature: u8, count: u8, buf: &mut [u8; 516]) -> CInt {
        buf[0] = command;
        buf[1] = 0;
        buf[2] = feature;
        buf[3] = count;
        unsafe { ioctl(fd, HDIO_DRIVE_CMD, buf.as_mut_ptr()) }
    }

    /// Issue a SCSI command returning data. Success = GOOD status.
    fn scsi_cmd(fd: CInt, cdb: &[u8], data: &mut [u8]) -> bool {
        let mut sense = [0u8; 32];
        let mut cdb = cdb.to_vec();
        let mut hdr = SgIoHdr {
            interface_id: 'S' as CInt,
            dxfer_direction: SG_DXFER_FROM_DEV,
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
        rc == 0 && hdr.status == 0 && hdr.driver_status == 0
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

    /// SCSI log page (page control in `pc`: 0x40 = cumulative).
    fn log_page(fd: CInt, page: u8, pc: u8) -> Option<Vec<u8>> {
        let mut buf = [0u8; 252];
        let cdb = [0x4D, pc, page, 0, 0, 0, 0, 0x00, 0xFC, 0x00];
        if scsi_cmd(fd, &cdb, &mut buf) {
            Some(buf.to_vec())
        } else {
            None
        }
    }

    pub fn scsi_read(device: &Device) -> Option<SmartData> {
        let file = fs::File::open(&device.path).ok()?;
        let fd = file.as_raw_fd();
        let mut s = SmartData::default();
        s.source = "native".to_string();
        s.logical_block_size = Some(512);
        let mut found = false;

        // Temperature (log page 0x0D, parameter 0x0000).
        if let Some(buf) = log_page(fd, 0x0D, 0x40) {
            for (code, data) in parse_log_params(&buf) {
                if code == 0x0000 && data.len() >= 2 {
                    s.temperature_c = Some(i16::from_be_bytes([data[0], data[1]]) as i64);
                    found = true;
                }
            }
        }

        // Total uncorrected errors from read (0x03) + write (0x02) counter logs.
        let mut uncorrected = 0u64;
        for page in [0x02u8, 0x03u8] {
            if let Some(buf) = log_page(fd, page, 0x40) {
                for (code, data) in parse_log_params(&buf) {
                    if code == 0x0005 && data.len() >= 4 {
                        uncorrected += u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as u64;
                        found = true;
                    }
                }
            }
        }
        if found {
            s.uncorrectable = Some(uncorrected);
        }

        // Grown defect list (READ DEFECT DATA, GLIST).
        let mut defect = [0u8; 252];
        if scsi_cmd(fd, &[0x37, 0x08, 0, 0, 0, 0, 0, 0x00, 0xFC, 0x00], &mut defect) {
            let len = u16::from_be_bytes([defect[2], defect[3]]) as usize;
            s.reallocated = Some((len / 8) as u64);
            found = true;
        }

        if !found {
            return None;
        }
        s.passed = Some(s.uncorrectable.unwrap_or(0) == 0);
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

    pub fn ata_read(device: &Device) -> Option<SmartData> {
        let file = fs::File::open(&device.path).ok()?;
        let fd = file.as_raw_fd();

        let mut buf = [0u8; 516];
        if hdio_cmd(fd, WIN_SMART, SMART_READ_DATA, 1, &mut buf) != 0
            || !buf[4..].iter().any(|b| *b != 0)
        {
            let mut enable = [0u8; 516];
            let _ = hdio_cmd(fd, WIN_SMART, SMART_ENABLE, 0, &mut enable);
            let mut retry = [0u8; 516];
            if hdio_cmd(fd, WIN_SMART, SMART_READ_DATA, 1, &mut retry) != 0
                || !retry[4..].iter().any(|b| *b != 0)
            {
                return None;
            }
            buf = retry;
        }

        let mut smart = parse_ata_smart(&buf[4..]);
        let mut thr = [0u8; 516];
        if hdio_cmd(fd, WIN_SMART, SMART_READ_THRESHOLDS, 1, &mut thr) == 0
            && thr[4..].iter().any(|b| *b != 0)
        {
            super::apply_thresholds(&mut smart.attributes, &thr[4..]);
        }
        let mut status = [0u8; 516];
        if hdio_cmd(fd, WIN_SMART, SMART_RETURN_STATUS, 0, &mut status) == 0 {
            let ata_status = status[0];
            if ata_status != 0 {
                smart.passed = Some(ata_status & 0x01 == 0);
            }
        }
        Some(smart)
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

        let s = parse_nvme_health(&data);
        assert_eq!(s.passed, Some(true));
        assert_eq!(s.temperature_c, Some(38));
        assert_eq!(s.life_percent, Some(93));
        assert_eq!(s.power_on_hours, Some(4321));
        assert_eq!(s.bytes_written(), Some(2_000_000u64 * 1000 * 512));
    }

    #[test]
    fn parses_scsi_log_parameters() {
        // header (4) + param 0x0000 len2 value 33 + param 0x0001 len1 value 5
        let buf = [0x0d, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x21,
                   0x00, 0x01, 0x00, 0x01, 0x05];
        let params = parse_log_params(&buf);
        assert_eq!(params.len(), 2);
        assert_eq!(params[0].0, 0x0000);
        assert_eq!(params[0].1, vec![0x00, 0x21]);
        assert_eq!(params[1].0, 0x0001);
    }
}
