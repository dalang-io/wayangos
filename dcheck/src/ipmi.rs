//! Native IPMI (BMC: iDRAC, iLO, XClarity, …) through `/dev/ipmi0`, without
//! ipmitool. Read-only commands only:
//!
//! - Get Device ID (BMC firmware version)
//! - SDR repository (sensor records) + Get Sensor Reading: fans, voltages,
//!   temperatures, power supplies, intrusion … with their status
//! - SEL (system event log): recent hardware events
//!
//! Parsers are plain functions over bytes (tested); the ioctl transport is
//! Linux-only.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// Sensor health as the BMC reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Ok,
    Warn,
    Crit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sensor {
    pub name: String,
    /// Sensor type (IPMI table 42-3): 0x01 temp, 0x02 voltage, 0x04 fan,
    /// 0x08 power supply, …
    pub kind: u8,
    /// Entity (id, instance): what the sensor belongs to (CPU 1, PSU 2 …).
    pub entity: (u8, u8),
    /// Converted reading and its unit, for threshold sensors.
    pub value: Option<(f64, &'static str)>,
    /// Discrete state in words ("present", "AC lost", …).
    pub state: Option<String>,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Unix time (0 = before the BMC clock was set).
    pub time: u32,
    pub sensor: String,
    pub text: String,
    pub status: Status,
}

#[derive(Debug, Clone, Default)]
pub struct Ipmi {
    pub bmc_firmware: Option<String>,
    pub sensors: Vec<Sensor>,
    /// Most recent events, newest last.
    pub events: Vec<Event>,
    pub sel_entries: u16,
    pub error: Option<String>,
}

/// A sensor record from the SDR repository.
#[derive(Debug, Clone, PartialEq)]
pub struct Sdr {
    pub number: u8,
    pub owner: u8,
    pub lun: u8,
    pub kind: u8,
    pub event_type: u8,
    pub entity: (u8, u8),
    pub name: String,
    /// Full records with a linear analog reading: (M, B, B exponent, R
    /// exponent, analog format, unit).
    pub conv: Option<(i32, i32, i32, i32, u8, &'static str)>,
}

fn nibble_signed(v: u8) -> i32 {
    let v = (v & 0x0F) as i32;
    if v >= 8 {
        v - 16
    } else {
        v
    }
}

fn ten_bit_signed(ls: u8, ms: u8) -> i32 {
    let v = ((ms as i32 & 0xC0) << 2) | ls as i32;
    if v & 0x200 != 0 {
        v - 0x400
    } else {
        v
    }
}

fn unit_name(code: u8) -> &'static str {
    match code {
        1 => "°C",
        2 => "°F",
        4 => "V",
        5 => "A",
        6 => "W",
        18 => "RPM",
        19 => "Hz",
        _ => "",
    }
}

/// Parse one SDR record (header included). Only full (0x01) and compact
/// (0x02) sensor records are kept.
pub fn parse_sdr(rec: &[u8]) -> Option<Sdr> {
    if rec.len() < 20 {
        return None;
    }
    let kind_rec = rec[3];
    let name_at = match kind_rec {
        0x01 => 47,
        0x02 => 31,
        _ => return None,
    };
    let len = (*rec.get(name_at)? & 0x1F) as usize;
    let name = rec
        .get(name_at + 1..name_at + 1 + len)
        .map(|b| String::from_utf8_lossy(b).trim_matches(char::from(0)).trim().to_string())
        .unwrap_or_default();
    let mut s = Sdr {
        number: rec[7],
        owner: rec[5],
        lun: rec[6] & 0x03,
        kind: rec[12],
        event_type: rec[13],
        entity: (rec[8], rec[9] & 0x7F),
        name,
        conv: None,
    };
    if kind_rec == 0x01 && rec.len() > 30 {
        let format = rec[20] >> 6;
        let linear = rec[23] & 0x7F == 0;
        if format != 3 && linear {
            let m = ten_bit_signed(rec[24], rec[25]);
            let b = ten_bit_signed(rec[26], rec[27]);
            let (rexp, bexp) = (nibble_signed(rec[29] >> 4), nibble_signed(rec[29]));
            // Units 1 bit 0: the reading is a percentage.
            let unit = if rec[20] & 1 != 0 { "%" } else { unit_name(rec[21]) };
            s.conv = Some((m, b, bexp, rexp, format, unit));
        }
    }
    Some(s)
}

/// Raw reading → value, per the record's conversion factors.
pub fn convert(raw: u8, conv: (i32, i32, i32, i32, u8, &'static str)) -> f64 {
    let (m, b, bexp, rexp, format, _) = conv;
    let x = match format {
        1 => {
            // one's complement
            if raw & 0x80 != 0 {
                -((!raw) as i32)
            } else {
                raw as i32
            }
        }
        2 => raw as i8 as i32,
        _ => raw as i32,
    } as f64;
    (m as f64 * x + b as f64 * 10f64.powi(bexp)) * 10f64.powi(rexp)
}

/// Discrete sensor states that matter, from the state bits of a reading.
pub fn discrete_state(kind: u8, event_type: u8, bits: u16) -> Option<(String, Status)> {
    let on = |b: u16| bits & (1 << b) != 0;
    let pick = |list: &[(u16, &str, Status)]| {
        let hits: Vec<&(u16, &str, Status)> = list.iter().filter(|(b, _, _)| on(*b)).collect();
        let status = hits.iter().map(|h| h.2).max()?;
        Some((hits.iter().map(|h| h.1).collect::<Vec<_>>().join(", "), status))
    };
    match (event_type, kind) {
        // Redundancy (generic 0x0B).
        (0x0B, _) => pick(&[
            (0, "fully redundant", Status::Ok),
            (1, "redundancy lost", Status::Warn),
            (2, "redundancy degraded", Status::Warn),
            (3, "non-redundant", Status::Warn),
        ]),
        (0x6F, 0x08) => pick(&[
            (0, "present", Status::Ok),
            (1, "failure detected", Status::Crit),
            (2, "predictive failure", Status::Warn),
            (3, "AC lost", Status::Crit),
            (4, "AC lost or out of range", Status::Crit),
            (5, "AC out of range", Status::Warn),
            (6, "configuration error", Status::Warn),
        ]),
        (0x6F, 0x05) => pick(&[(0, "chassis opened", Status::Warn), (4, "LAN leash lost", Status::Warn)]),
        (0x6F, 0x07) => pick(&[
            (0, "IERR", Status::Crit),
            (1, "thermal trip", Status::Crit),
            (5, "throttled", Status::Warn),
            (7, "present", Status::Ok),
            (8, "disabled", Status::Warn),
            (10, "throttled", Status::Warn),
        ]),
        (0x6F, 0x0C) => pick(&[
            (0, "correctable ECC", Status::Warn),
            (1, "uncorrectable ECC", Status::Crit),
            (3, "memory scrub failed", Status::Warn),
            (5, "ECC logging limit reached", Status::Warn),
            (6, "present", Status::Ok),
            (8, "spare", Status::Ok),
            (10, "critical overtemperature", Status::Crit),
        ]),
        (0x6F, 0x0D) => pick(&[(0, "drive present", Status::Ok), (1, "drive fault", Status::Crit), (2, "predictive failure", Status::Warn)]),
        (0x6F, 0x29) => pick(&[(0, "battery low", Status::Warn), (1, "battery failed", Status::Crit), (2, "battery present", Status::Ok)]),
        (0x6F, 0x09) => pick(&[
            (4, "AC lost", Status::Crit),
            (5, "soft power control failure", Status::Warn),
            (6, "power unit failure", Status::Crit),
            (7, "predictive failure", Status::Warn),
        ]),
        _ => None,
    }
}

/// "CPU 1", "PSU 2" … for an entity.
pub fn entity_name(e: (u8, u8)) -> String {
    let what = match e.0 {
        0x03 => "CPU",
        0x04 => "disk bay",
        0x07 => "board",
        0x0A => "PSU",
        0x0B => "card",
        0x13 => "power unit",
        0x14 => "cooling",
        0x17 => "chassis",
        0x1D => "fan",
        0x20 => "DIMM",
        _ => return format!("entity {}.{}", e.0, e.1),
    };
    format!("{what} {}", e.1)
}

/// Append the entity to sensor names that appear more than once.
pub fn disambiguate(sensors: &mut [Sensor]) {
    let names: Vec<String> = sensors.iter().map(|s| s.name.clone()).collect();
    for s in sensors.iter_mut() {
        if names.iter().filter(|n| **n == s.name).count() > 1 {
            s.name = format!("{} ({})", s.name, entity_name(s.entity));
        }
    }
}

/// Status of a threshold reading from the comparison byte.
pub fn threshold_status(state: u8) -> Status {
    if state & 0b0011_0110 != 0 {
        Status::Crit // lower/upper critical or non-recoverable
    } else if state & 0b0000_1001 != 0 {
        Status::Warn // lower/upper non-critical
    } else {
        Status::Ok
    }
}

fn sensor_type_name(kind: u8) -> &'static str {
    match kind {
        0x01 => "Temperature",
        0x02 => "Voltage",
        0x03 => "Current",
        0x04 => "Fan",
        0x05 => "Chassis intrusion",
        0x07 => "Processor",
        0x08 => "Power supply",
        0x09 => "Power unit",
        0x0C => "Memory",
        0x0D => "Drive slot",
        0x0F => "POST error",
        0x10 => "Event log",
        0x12 => "System event",
        0x13 => "Critical interrupt",
        0x19 => "Chipset",
        0x1D => "System boot",
        0x21 => "Slot / connector",
        0x23 => "Watchdog",
        0x29 => "Battery",
        _ => "Sensor",
    }
}

/// Decode a 16-byte SEL record.
pub fn parse_sel(rec: &[u8], names: &dyn Fn(u8) -> Option<String>) -> Option<Event> {
    if rec.len() < 16 || rec[2] != 0x02 {
        return None; // OEM records
    }
    let time = u32::from_le_bytes(rec[3..7].try_into().ok()?);
    let (kind, num, dir_type) = (rec[10], rec[11], rec[12]);
    let deassert = dir_type & 0x80 != 0;
    let offset = rec[13] & 0x0F;
    let sensor = names(num).unwrap_or_else(|| sensor_type_name(kind).to_string());
    let (mut text, mut status) = match dir_type & 0x7F {
        0x01 => {
            let what = match offset {
                0 => "lower non-critical going low",
                1 => "lower non-critical going high",
                2 => "lower critical going low",
                4 => "lower non-recoverable going low",
                7 => "upper non-critical going high",
                9 => "upper critical going high",
                11 => "upper non-recoverable going high",
                _ => "threshold crossed",
            };
            let st = if matches!(offset, 2 | 4 | 9 | 11) { Status::Crit } else { Status::Warn };
            (what.to_string(), st)
        }
        t => match discrete_state(kind, t, 1u16 << offset) {
            Some((s, st)) => (s, st),
            None => match (kind, offset) {
                (0x10, 2) => ("log cleared".into(), Status::Ok),
                (0x12, _) => ("system event".into(), Status::Ok),
                (0x13, _) => ("critical interrupt (PCIe / NMI)".into(), Status::Crit),
                (0x1D, _) => ("boot".into(), Status::Ok),
                (0x0F, _) => ("POST error".into(), Status::Warn),
                _ => (format!("{} event (offset {offset})", sensor_type_name(kind)), Status::Ok),
            },
        },
    };
    if deassert {
        text.push_str(" — cleared");
        status = Status::Ok;
    }
    Some(Event { time, sensor, text, status })
}

// ─── transport ──────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
mod dev {
    use std::fs::{File, OpenOptions};
    use std::os::fd::AsRawFd;

    #[repr(C)]
    struct IpmiMsg {
        netfn: u8,
        cmd: u8,
        data_len: u16,
        data: *mut u8,
    }

    #[repr(C)]
    struct IpmiReq {
        addr: *mut u8,
        addr_len: u32,
        msgid: i64,
        msg: IpmiMsg,
    }

    #[repr(C)]
    struct IpmiRecv {
        recv_type: i32,
        addr: *mut u8,
        addr_len: u32,
        msgid: i64,
        msg: IpmiMsg,
    }

    #[repr(C)]
    struct SysIfAddr {
        addr_type: i32,
        channel: i16,
        lun: u8,
    }

    #[repr(C)]
    struct PollFd {
        fd: i32,
        events: i16,
        revents: i16,
    }

    extern "C" {
        fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
        fn poll(fds: *mut PollFd, n: u64, timeout: i32) -> i32;
    }

    const fn ioc(dir: u64, nr: u64, size: usize) -> std::ffi::c_ulong {
        ((dir << 30) | ((size as u64) << 16) | ((b'i' as u64) << 8) | nr) as std::ffi::c_ulong
    }
    const SEND: std::ffi::c_ulong = ioc(2, 13, std::mem::size_of::<IpmiReq>());
    const RECV: std::ffi::c_ulong = ioc(3, 11, std::mem::size_of::<IpmiRecv>());

    pub struct Bmc {
        f: File,
        id: i64,
    }

    impl Bmc {
        pub fn open() -> Option<Bmc> {
            let f = ["/dev/ipmi0", "/dev/ipmi/0", "/dev/ipmidev/0"]
                .iter()
                .find_map(|p| OpenOptions::new().read(true).write(true).open(p).ok())?;
            Some(Bmc { f, id: 0 })
        }

        /// Send a request; returns the response data after the completion
        /// code, or the non-zero completion code as an error.
        pub fn cmd(&mut self, netfn: u8, cmd: u8, data: &[u8]) -> Result<Vec<u8>, u8> {
            self.cmd_lun(netfn, cmd, data, 0)
        }

        /// As `cmd`, to a LUN of the BMC (sensors can live on LUN 1..3).
        pub fn cmd_lun(&mut self, netfn: u8, cmd: u8, data: &[u8], lun: u8) -> Result<Vec<u8>, u8> {
            let fd = self.f.as_raw_fd();
            self.id += 1;
            let mut addr = SysIfAddr { addr_type: 0x0C, channel: 0x0F, lun };
            let mut payload = data.to_vec();
            let mut req = IpmiReq {
                addr: (&mut addr as *mut SysIfAddr).cast(),
                addr_len: std::mem::size_of::<SysIfAddr>() as u32,
                msgid: self.id,
                msg: IpmiMsg { netfn, cmd, data_len: payload.len() as u16, data: payload.as_mut_ptr() },
            };
            if unsafe { ioctl(fd, SEND, &mut req as *mut IpmiReq) } != 0 {
                return Err(0xFF);
            }
            for _ in 0..8 {
                let mut p = PollFd { fd, events: 1, revents: 0 };
                if unsafe { poll(&mut p, 1, 5000) } <= 0 {
                    return Err(0xFE);
                }
                let mut raddr = [0u8; 32];
                let mut buf = [0u8; 256];
                let mut recv = IpmiRecv {
                    recv_type: 0,
                    addr: raddr.as_mut_ptr(),
                    addr_len: raddr.len() as u32,
                    msgid: 0,
                    msg: IpmiMsg { netfn: 0, cmd: 0, data_len: buf.len() as u16, data: buf.as_mut_ptr() },
                };
                if unsafe { ioctl(fd, RECV, &mut recv as *mut IpmiRecv) } != 0 {
                    return Err(0xFD);
                }
                if recv.msgid != self.id {
                    continue; // a stale response
                }
                let n = (recv.msg.data_len as usize).min(buf.len());
                if n == 0 {
                    return Err(0xFC);
                }
                return if buf[0] == 0 { Ok(buf[1..n].to_vec()) } else { Err(buf[0]) };
            }
            Err(0xFB)
        }
    }
}

/// Read everything the BMC offers (None: no IPMI device).
#[cfg(target_os = "linux")]
pub fn read() -> Option<Ipmi> {
    let mut bmc = dev::Bmc::open()?;
    let mut out = Ipmi::default();
    if let Ok(id) = bmc.cmd(0x06, 0x01, &[]) {
        if id.len() >= 4 {
            out.bmc_firmware = Some(format!("{}.{:02x}", id[2] & 0x7F, id[3]));
        }
    }
    // SDR repository.
    let mut sdrs = Vec::new();
    let resv = bmc.cmd(0x0A, 0x22, &[]).ok().filter(|r| r.len() >= 2).map(|r| [r[0], r[1]]).unwrap_or([0, 0]);
    let mut id: u16 = 0;
    for _ in 0..512 {
        let hdr = match bmc.cmd(0x0A, 0x23, &[resv[0], resv[1], id as u8, (id >> 8) as u8, 0, 5]) {
            Ok(h) if h.len() >= 7 => h,
            Ok(_) => break,
            Err(e) => {
                out.error.get_or_insert(format!("SDR read failed (code {e:#04x})"));
                break;
            }
        };
        let next = u16::from_le_bytes([hdr[0], hdr[1]]);
        let len = hdr[6] as usize;
        let mut rec = hdr[2..7].to_vec();
        let mut off = 5usize;
        while off < len + 5 {
            let n = (len + 5 - off).min(16) as u8;
            match bmc.cmd(0x0A, 0x23, &[resv[0], resv[1], id as u8, (id >> 8) as u8, off as u8, n]) {
                Ok(d) if d.len() > 2 => {
                    rec.extend(&d[2..]);
                    off += d.len() - 2;
                }
                _ => break,
            }
        }
        if let Some(s) = parse_sdr(&rec) {
            sdrs.push(s);
        }
        if next == 0xFFFF || next == id {
            break;
        }
        id = next;
    }
    // Same-named sensors (Dell: "Status", "Temp") get their entity appended.
    // Readings (sensors owned by the BMC itself, LUN 0..3).
    for s in &sdrs {
        if s.owner != 0x20 {
            continue;
        }
        let Ok(r) = bmc.cmd_lun(0x04, 0x2D, &[s.number], s.lun) else { continue };
        if r.len() < 2 || r[1] & 0x20 != 0 || r[1] & 0x40 == 0 {
            continue; // reading unavailable / scanning disabled
        }
        if s.event_type == 0x01 {
            let value = s.conv.map(|c| (convert(r[0], c), c.5));
            let status = r.get(2).map_or(Status::Ok, |b| threshold_status(*b));
            out.sensors.push(Sensor { name: s.name.clone(), kind: s.kind, entity: s.entity, value, state: None, status });
        } else {
            let bits = u16::from_le_bytes([*r.get(2).unwrap_or(&0), *r.get(3).unwrap_or(&0) & 0x7F]);
            if let Some((state, status)) = discrete_state(s.kind, s.event_type, bits) {
                out.sensors.push(Sensor { name: s.name.clone(), kind: s.kind, entity: s.entity, value: None, state: Some(state), status });
            }
        }
    }
    disambiguate(&mut out.sensors);
    // SEL: walk the whole log, keep the last 30 events.
    if let Ok(info) = bmc.cmd(0x0A, 0x40, &[]) {
        if info.len() >= 3 {
            out.sel_entries = u16::from_le_bytes([info[1], info[2]]);
        }
    }
    let name_of = |n: u8| sdrs.iter().find(|s| s.number == n).map(|s| s.name.clone());
    let mut rid: u16 = 0;
    let mut events = Vec::new();
    for _ in 0..4096 {
        let Ok(r) = bmc.cmd(0x0A, 0x43, &[0, 0, rid as u8, (rid >> 8) as u8, 0, 0xFF]) else { break };
        if r.len() < 18 {
            break;
        }
        if let Some(e) = parse_sel(&r[2..18], &name_of) {
            events.push(e);
        }
        let next = u16::from_le_bytes([r[0], r[1]]);
        if next == 0xFFFF || next == rid {
            break;
        }
        rid = next;
    }
    let skip = events.len().saturating_sub(30);
    out.events = events.split_off(skip);
    Some(out)
}

#[cfg(not(target_os = "linux"))]
pub fn read() -> Option<Ipmi> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full sensor record: "Fan1 RPM", fan, threshold, unsigned, RPM,
    /// M = 120, B = 0, exponents 0.
    fn fan_sdr() -> Vec<u8> {
        let mut r = vec![0u8; 48 + 8];
        r[3] = 0x01;
        r[4] = (r.len() - 5) as u8;
        r[5] = 0x20;
        r[7] = 0x30;
        r[12] = 0x04;
        r[13] = 0x01;
        r[20] = 0x00;
        r[21] = 18;
        r[24] = 120;
        r[47] = 0xC0 | 8;
        r[48..56].copy_from_slice(b"Fan1 RPM");
        r
    }

    #[test]
    fn parses_full_record_and_converts() {
        let s = parse_sdr(&fan_sdr()).unwrap();
        assert_eq!(s.name, "Fan1 RPM");
        assert_eq!((s.number, s.kind, s.event_type), (0x30, 0x04, 0x01));
        let c = s.conv.unwrap();
        assert_eq!(c.5, "RPM");
        assert_eq!(convert(32, c), 3840.0);
        // Voltage: M = 2, R exp = -2 (0xE0), unsigned: raw 60 → 1.20 V.
        let v = (2, 0, 0, -2, 0u8, "V");
        assert!((convert(60, v) - 1.2).abs() < 1e-9);
        // Two's-complement temperature, M = 1: raw 0xF6 → -10 °C.
        assert_eq!(convert(0xF6, (1, 0, 0, 0, 2, "°C")), -10.0);
        // 10-bit signed M/B.
        assert_eq!(ten_bit_signed(0xFF, 0xC0), -1);
        assert_eq!(nibble_signed(0x0E), -2);
    }

    #[test]
    fn parses_compact_record() {
        let mut r = vec![0u8; 32 + 5];
        r[3] = 0x02;
        r[5] = 0x20;
        r[7] = 0x62;
        r[12] = 0x08;
        r[13] = 0x6F;
        r[31] = 0xC0 | 5;
        r[32..37].copy_from_slice(b"PS1 S");
        let s = parse_sdr(&r).unwrap();
        assert_eq!(s.name, "PS1 S");
        assert!(s.conv.is_none());
        // Present + AC lost → critical, both named.
        let (text, st) = discrete_state(0x08, 0x6F, 0b1001).unwrap();
        assert_eq!(st, Status::Crit);
        assert_eq!(text, "present, AC lost");
        assert_eq!(discrete_state(0x08, 0x6F, 0b1).unwrap().1, Status::Ok);
        assert_eq!(discrete_state(0x0B, 0x0B, 0b10).unwrap(), ("redundancy lost".to_string(), Status::Warn));
    }

    #[test]
    fn duplicate_names_get_their_entity() {
        let mk = |name: &str, e: (u8, u8)| Sensor { name: name.into(), kind: 0x08, entity: e, value: None, state: None, status: Status::Ok };
        let mut v = vec![mk("Status", (0x0A, 1)), mk("Status", (0x0A, 2)), mk("Temp", (0x03, 1)), mk("Inlet Temp", (0x07, 1))];
        disambiguate(&mut v);
        let names: Vec<&str> = v.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Status (PSU 1)", "Status (PSU 2)", "Temp", "Inlet Temp"]);
    }

    #[test]
    fn threshold_bits() {
        assert_eq!(threshold_status(0), Status::Ok);
        assert_eq!(threshold_status(0b0000_1000), Status::Warn); // upper non-critical
        assert_eq!(threshold_status(0b0001_0000), Status::Crit); // upper critical
        assert_eq!(threshold_status(0b0000_0010), Status::Crit); // lower critical
    }

    #[test]
    fn decodes_sel_records() {
        let names = |n: u8| (n == 0x62).then(|| "PS2 Status".to_string());
        // 2026-06-09, PSU sensor 0x62, sensor-specific, offset 3 (AC lost).
        let mut r = [0u8; 16];
        r[2] = 0x02;
        r[3..7].copy_from_slice(&1_780_992_000u32.to_le_bytes());
        r[10] = 0x08;
        r[11] = 0x62;
        r[12] = 0x6F;
        r[13] = 0x03;
        let e = parse_sel(&r, &names).unwrap();
        assert_eq!(e.sensor, "PS2 Status");
        assert_eq!(e.text, "AC lost");
        assert_eq!(e.status, Status::Crit);
        // Deassertion → cleared.
        r[12] = 0xEF;
        let e = parse_sel(&r, &names).unwrap();
        assert!(e.text.ends_with("cleared"));
        assert_eq!(e.status, Status::Ok);
        // Threshold: upper critical going high on an unnamed fan.
        let mut t = [0u8; 16];
        t[2] = 0x02;
        t[10] = 0x04;
        t[11] = 0x33;
        t[12] = 0x01;
        t[13] = 0x09;
        let e = parse_sel(&t, &names).unwrap();
        assert_eq!((e.sensor.as_str(), e.status), ("Fan", Status::Crit));
        // OEM records are skipped.
        let mut o = [0u8; 16];
        o[2] = 0xC0;
        assert!(parse_sel(&o, &names).is_none());
    }
}
