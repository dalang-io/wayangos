//! Native SMART readers (no external tools).
//!
//! Linux only at runtime: ATA via `HDIO_DRIVE_CMD`, NVMe via the admin command
//! ioctl. The pure byte parsers below are platform-independent and unit tested
//! everywhere.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::model::Device;
use crate::smartctl::SmartData;

/// Read SMART natively. Returns `None` on non-Linux or when the ioctl path
/// fails (no privileges, unsupported device).
pub fn read(device: &Device) -> Option<SmartData> {
    #[cfg(target_os = "linux")]
    {
        if device.kind == crate::model::MediaKind::Nvme || device.name.starts_with("nvme") {
            linux::nvme_read(device)
        } else {
            linux::ata_read(device)
        }
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
        let value = data[off + 3] as u64;
        let raw = le_u48(&data[off + 5..off + 11]);
        match id {
            5 => s.reallocated = Some(raw),
            9 => s.power_on_hours = Some(raw),
            12 => s.power_cycles = Some(raw),
            194 => s.temperature_c = Some((raw & 0xff) as i64),
            197 => s.pending = Some(raw),
            198 => s.uncorrectable = Some(raw),
            199 => s.crc_errors = Some(raw),
            231 | 233 => s.life_percent = Some(value),
            241 => s.lba_written = Some(raw),
            242 => s.lba_read = Some(raw),
            _ => {}
        }
    }
    s
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

    // +32 data units read, +48 data units written (units of 1000*512 bytes).
    let read_units = to_u64_sat(le_u128(&data[32..]));
    let written_units = to_u64_sat(le_u128(&data[48..]));
    s.lba_read = Some(read_units.saturating_mul(1000));
    s.lba_written = Some(written_units.saturating_mul(1000));

    s.power_cycles = Some(to_u64_sat(le_u128(&data[112..])));
    s.power_on_hours = Some(to_u64_sat(le_u128(&data[128..])));
    s.passed = Some(critical_warning == 0);

    s
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::io::AsRawFd;

    use super::{parse_ata_smart, parse_nvme_health};
    use crate::model::Device;
    use crate::smartctl::SmartData;

    type CInt = i32;
    type CULong = u64;

    extern "C" {
        fn ioctl(fd: CInt, request: CULong, ...) -> CInt;
    }

    // Old-style ATA taskfile ioctl: buf = [command, sector_number, feature,
    // sector_count] followed by up to 512 data bytes. Widely supported by
    // libata and far simpler than SG_IO ATA PASS-THROUGH.
    const HDIO_DRIVE_CMD: CULong = 0x031f;
    const WIN_SMART: u8 = 0xB0;
    const SMART_READ_DATA: u8 = 0xD0;
    const SMART_ENABLE: u8 = 0xD8;
    const SMART_RETURN_STATUS: u8 = 0xDA;

    // _IOWR('N', 0x41, struct nvme_admin_cmd) with sizeof == 72.
    const NVME_IOCTL_ADMIN_CMD: CULong = 0xC048_4E41;

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
        buf[1] = 0; // sector number
        buf[2] = feature;
        buf[3] = count;
        unsafe { ioctl(fd, HDIO_DRIVE_CMD, buf.as_mut_ptr()) }
    }

    pub fn ata_read(device: &Device) -> Option<SmartData> {
        let file = std::fs::File::open(&device.path).ok()?;
        let fd = file.as_raw_fd();

        let mut buf = [0u8; 516];
        if hdio_cmd(fd, WIN_SMART, SMART_READ_DATA, 1, &mut buf) != 0
            || !buf[4..].iter().any(|b| *b != 0)
        {
            // Try to enable SMART, then retry.
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

        let mut status = [0u8; 516];
        if hdio_cmd(fd, WIN_SMART, SMART_RETURN_STATUS, 0, &mut status) == 0 {
            let ata_status = status[0];
            // The drive status byte: ERR bit (0x01) clear means healthy.
            if ata_status != 0 {
                smart.passed = Some(ata_status & 0x01 == 0);
            }
        }
        Some(smart)
    }

    pub fn nvme_read(device: &Device) -> Option<SmartData> {
        let controller = controller_path(&device.name)?;
        let file = std::fs::File::open(&controller).ok()?;
        let fd = file.as_raw_fd();

        let mut data = [0u8; 512];
        let mut cmd = NvmeAdminCmd {
            opcode: 0x02, // Get Log Page
            flags: 0,
            command_id: 1,
            nsid: 0xffff_ffff,
            cdw2: 0,
            cdw3: 0,
            metadata: 0,
            addr: data.as_mut_ptr() as u64,
            metadata_len: 0,
            data_len: data.len() as u32,
            cdw10: 0x02, // LOG_ID 0x02 (SMART/Health), NUMD 0
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

    fn controller_path(name: &str) -> Option<String> {
        let rest = name.strip_prefix("nvme")?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            None
        } else {
            Some(format!("/dev/nvme{digits}"))
        }
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
        put(2, 194, 100, 33);
        put(3, 241, 99, 500_000);
        put(4, 199, 100, 0);

        let s = parse_ata_smart(&data);
        assert_eq!(s.reallocated, Some(0));
        assert_eq!(s.power_on_hours, Some(12345));
        assert_eq!(s.temperature_c, Some(33));
        assert_eq!(s.lba_written, Some(500_000));
        assert_eq!(s.crc_errors, Some(0));
    }

    #[test]
    fn parses_nvme_health_log() {
        let mut data = vec![0u8; 512];
        data[0] = 0; // critical_warning
        data[1..3].copy_from_slice(&311u16.to_le_bytes()); // 311K -> 38C
        data[5] = 7; // percentage_used
        data[48..64].copy_from_slice(&2_000_000u128.to_le_bytes()); // data units written
        data[128..144].copy_from_slice(&4321u128.to_le_bytes()); // power_on_hours

        let s = parse_nvme_health(&data);
        assert_eq!(s.passed, Some(true));
        assert_eq!(s.temperature_c, Some(38));
        assert_eq!(s.life_percent, Some(93));
        assert_eq!(s.power_on_hours, Some(4321));
        assert_eq!(s.bytes_written(), Some(2_000_000u64 * 1000 * 512));
    }
}
