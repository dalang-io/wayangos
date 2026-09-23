//! Block-device discovery on Linux.
//!
//! Everything is read from `/sys` and `/proc` so the tool works without
//! `lsblk`/`udev` on a minimal rootfs (e.g. WayangOS). Devices are listed as
//! soon as they are *attached*, whether or not they are mounted.
//!
//! The filesystem root can be overridden with `DCHECK_SYS_ROOT`, which is used
//! by the test fixture and is also handy for inspecting a chroot/offline image.

// Linux uses the sysfs helpers below; FreeBSD/macOS have their own modules.
#![cfg_attr(
    any(target_os = "freebsd", target_os = "macos"),
    allow(dead_code, unused_imports)
)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::model::{Bus, Device, MediaKind, Partition};

/// Filesystem root that contains `sys/` and `proc/`.
fn root() -> PathBuf {
    match std::env::var_os("DCHECK_SYS_ROOT") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from("/"),
    }
}

/// SATA ports whose drive the kernel gave up on (no block device), from the
/// kernel log. Only for the live system (or `$DCHECK_KMSG` with a fixture).
fn failed_ata_ports(root: &Path, devices: &[Device]) -> Vec<Device> {
    if root != Path::new("/") && std::env::var_os("DCHECK_KMSG").is_none() {
        return Vec::new();
    }
    // Ports that do have a working disk: /sys/block/<dev>/device -> .../ataN/...
    let mut live = std::collections::BTreeSet::new();
    for d in devices {
        let link = sys_block(root).join(&d.name).join("device");
        if let Ok(p) = fs::canonicalize(link) {
            for comp in p.components() {
                let c = comp.as_os_str().to_string_lossy();
                if let Some(n) = c.strip_prefix("ata").and_then(|n| n.parse::<u32>().ok()) {
                    live.insert(n);
                }
            }
        }
    }
    let messages = crate::kernlog::read_messages();
    crate::kernlog::ata_port_states(messages.iter().map(String::as_str))
        .into_iter()
        .filter(|(port, st)| st.failed.is_some() && !live.contains(port))
        .map(|(port, st)| Device {
            name: format!("ata{port}"),
            path: format!("ata{port}"),
            vendor: None,
            model: Some("unresponsive drive".into()),
            firmware: None,
            serial: None,
            bus: Bus::Sata,
            kind: MediaKind::Unknown,
            size_bytes: 0,
            logical_block_size: 512,
            removable: false,
            smart_status: None,
            partitions: Vec::new(),
            failure: st.failed,
        })
        .collect()
}

fn sys_block(root: &Path) -> PathBuf {
    root.join("sys/block")
}

/// Enumerate whole-disk block devices.
#[cfg(not(any(target_os = "freebsd", target_os = "macos")))]
pub fn list_devices() -> Vec<Device> {
    list_devices_sysfs()
}

/// Enumerate disks on FreeBSD (via `sysctl kern.disks` + smartctl identity).
#[cfg(target_os = "freebsd")]
pub fn list_devices() -> Vec<Device> {
    if std::env::var_os("DCHECK_SYS_ROOT").is_some() {
        return list_devices_sysfs();
    }
    freebsd::list_devices()
}

/// Enumerate physical disks on macOS via `diskutil`.
#[cfg(target_os = "macos")]
pub fn list_devices() -> Vec<Device> {
    if std::env::var_os("DCHECK_SYS_ROOT").is_some() {
        return list_devices_sysfs();
    }
    macos::list_devices()
}

fn list_devices_sysfs() -> Vec<Device> {
    let root = root();
    let mut devices = Vec::new();
    let mounts = read_mounts(&root);

    let entries = match fs::read_dir(sys_block(&root)) {
        Ok(e) => e,
        Err(_) => return devices, // not Linux / no sysfs
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    for name in names {
        if is_ignored(&name) {
            continue;
        }
        if let Some(dev) = build_device(&root, &name, &mounts) {
            devices.push(dev);
        }
    }
    devices.extend(failed_ata_ports(&root, &devices));
    devices
}

/// Find a single device by kernel name (`sda`), node (`/dev/sda`) or path.
pub fn find_device(devices: &[Device], needle: &str) -> Option<Device> {
    let want = needle.trim();
    let want_name = want.strip_prefix("/dev/").unwrap_or(want);
    devices
        .iter()
        .find(|d| d.name == want_name || d.path == want || d.name == want)
        .cloned()
}

/// Virtual / layered block devices: no SMART, and they would only add
/// UNKNOWN rows (and a non-zero `dcheck check`). Ceph RBD, DRBD and bcache
/// devices also resolve outside `/devices/virtual`, so match them by name.
fn is_ignored(name: &str) -> bool {
    const PREFIXES: [&str; 11] = [
        "loop", "ram", "zram", "dm-", "md", "sr", "nbd", "zd", "rbd", "drbd", "bcache",
    ];
    PREFIXES.iter().any(|p| name.starts_with(p))
}

fn build_device(root: &Path, name: &str, mounts: &Mounts) -> Option<Device> {
    let base = sys_block(root).join(name);

    // /sys/block/<dev>/size is always expressed in 512-byte sectors.
    let size_sectors = read_u64(base.join("size")).unwrap_or(0);
    let logical_block_size = read_u64(base.join("queue/logical_block_size")).unwrap_or(512);
    let size_bytes = size_sectors.saturating_mul(512);

    let removable = read_str(base.join("removable"))
        .map(|s| s == "1")
        .unwrap_or(false);
    let rotational = read_str(base.join("queue/rotational")).map(|s| s == "1");

    let device_link = base.join("device");
    let resolved = fs::canonicalize(&device_link).unwrap_or(device_link);

    // Skip pure virtual block devices (nbd, zram/zswap, dm, ...).
    if resolved.to_string_lossy().contains("/virtual/") {
        return None;
    }

    let bus = classify_bus(name, &resolved);
    let kind = classify_kind(name, rotational);
    let (vendor, model, firmware, serial) = read_identity(root, name, &base);
    let partitions = read_partitions(name, &base, mounts);

    Some(Device {
        name: name.to_string(),
        path: format!("/dev/{name}"),
        vendor,
        model,
        firmware,
        serial,
        bus,
        kind,
        size_bytes,
        logical_block_size,
        removable,
        smart_status: None,
        partitions,
        failure: None,
    })
}

fn classify_kind(name: &str, rotational: Option<bool>) -> MediaKind {
    if name.starts_with("nvme") {
        MediaKind::Nvme
    } else {
        match rotational {
            Some(true) => MediaKind::Hdd,
            Some(false) => MediaKind::Ssd,
            None => MediaKind::Unknown,
        }
    }
}

fn classify_bus(name: &str, resolved: &Path) -> Bus {
    let path = resolved.to_string_lossy();
    if name.starts_with("nvme") || path.contains("/nvme") {
        Bus::Nvme
    } else if path.contains("/usb") {
        Bus::Usb
    } else if path.contains("/ata") {
        Bus::Sata
    } else if path.contains("/mmc") || name.starts_with("mmcblk") {
        Bus::Mmc
    } else if path.contains("/virtio") {
        Bus::Virtio
    } else if path.contains("/sas") || path.contains("/scsi") {
        Bus::Scsi
    } else if path.contains("/host") || path.contains("/target") {
        // SAS/SCSI HBAs (e.g. mpt3sas) expose hostN/targetN paths without "sas".
        Bus::Scsi
    } else {
        Bus::Unknown
    }
}

fn read_identity(
    root: &Path,
    name: &str,
    base: &Path,
) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let vendor = read_str(base.join("device/vendor"));
    let mut model = read_str(base.join("device/model"));
    let mut firmware = read_str(base.join("device/rev"));
    let mut serial = read_str(base.join("device/serial"));

    // NVMe stores identity on the controller, not the namespace.
    if name.starts_with("nvme") {
        if let Some(ctrl) = nvme_controller(name) {
            let cbase = root.join("sys/class/nvme").join(ctrl);
            if model.is_none() {
                model = read_str(cbase.join("model"));
            }
            if firmware.is_none() {
                firmware = read_str(cbase.join("firmware_rev"));
            }
            if serial.is_none() {
                serial = read_str(cbase.join("serial"));
            }
        }
    }

    // Normalise empty strings to None.
    let norm = |o: Option<String>| o.filter(|s| !s.is_empty());
    (norm(vendor), norm(model), norm(firmware), norm(serial))
}

/// `nvme0n1` -> `nvme0`; leaves other names untouched.
fn nvme_controller(name: &str) -> Option<String> {
    let rest = name.strip_prefix("nvme")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(format!("nvme{digits}"))
    }
}

/// Mount points indexed two ways: by device number and by source path.
///
/// Matching only by `major:minor` misses filesystems whose mountinfo device
/// number differs from the block device node — e.g. btrfs (which reports an
/// anonymous `0:NN` device) and some LVM/dm setups. The source path
/// (`/dev/sda3`) is the reliable fallback.
#[derive(Default)]
struct Mounts {
    by_devnum: HashMap<String, (String, String)>,
    by_source: HashMap<String, (String, String)>,
}

impl Mounts {
    fn lookup(&self, devnum: &str, devpath: &str) -> Option<(String, String)> {
        self.by_devnum
            .get(devnum)
            .or_else(|| self.by_source.get(devpath))
            .cloned()
    }
}

fn read_partitions(name: &str, base: &Path, mounts: &Mounts) -> Vec<Partition> {
    let mut parts = Vec::new();
    let entries = match fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return parts,
    };

    let mut children: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|c| is_partition_of(name, c))
        .collect();
    children.sort();

    for child in children {
        let cbase = base.join(&child);
        let size = read_u64(cbase.join("size"))
            .unwrap_or(0)
            .saturating_mul(512);
        let devpath = format!("/dev/{child}");
        let (mountpoint, filesystem) = read_str(cbase.join("dev"))
            .and_then(|id| mounts.lookup(&id, &devpath))
            .map(|(m, f)| (Some(m), Some(f)))
            .unwrap_or((None, None));

        parts.push(Partition {
            path: devpath,
            size_bytes: size,
            mountpoint,
            filesystem,
        });
    }
    parts
}

/// A child entry is a partition if it is the disk name plus an optional `p`
/// and a run of digits (`sda1`, `mmcblk0p1`, `nvme0n1p1`).
fn is_partition_of(disk: &str, child: &str) -> bool {
    let Some(rest) = child.strip_prefix(disk) else {
        return false;
    };
    let rest = rest.strip_prefix('p').unwrap_or(rest);
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// Read mount points from `<root>/proc/self/mountinfo`.
fn read_mounts(root: &Path) -> Mounts {
    let mut mounts = Mounts::default();
    let data = match fs::read_to_string(root.join("proc/self/mountinfo")) {
        Ok(s) => s,
        Err(_) => return mounts,
    };

    for line in data.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        let Some(sep) = fields.iter().position(|f| *f == "-") else {
            continue;
        };
        let Some(fstype) = fields.get(sep + 1) else {
            continue;
        };
        let entry = (decode_octal(fields[4]), fstype.to_string());

        let devnum = fields[2].to_string();
        mounts.by_devnum.entry(devnum).or_insert_with(|| entry.clone());

        if let Some(source) = fields.get(sep + 2) {
            if source.starts_with("/dev/") {
                mounts
                    .by_source
                    .entry((*source).to_string())
                    .or_insert(entry);
            }
        }
    }
    mounts
}

/// Decode the octal escapes (`\040` etc.) used in mountinfo field 5.
fn decode_octal(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let oct = &s[i + 1..i + 4];
            if let Ok(v) = u8::from_str_radix(oct, 8) {
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn read_str(path: PathBuf) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let cleaned = raw.trim().trim_matches('\0').trim();
    if cleaned.is_empty() {
        None
    } else {
        // SATA vendor/model fields are space padded.
        Some(cleaned.split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

fn read_u64(path: PathBuf) -> Option<u64> {
    read_str(path)?.parse().ok()
}

/// Whether a (real or overridden) sysfs exists on this host.
#[cfg(target_os = "macos")]
pub fn has_sysfs() -> bool {
    true
}

/// Whether a (real or overridden) sysfs exists on this host.
#[cfg(target_os = "freebsd")]
pub fn has_sysfs() -> bool {
    true
}

/// Whether a (real or overridden) sysfs exists on this host.
#[cfg(not(any(target_os = "freebsd", target_os = "macos")))]
pub fn has_sysfs() -> bool {
    sys_block(&root()).is_dir()
}

/// Built-in sample devices, used for demos on hosts without sysfs (e.g. macOS)
/// and by `dcheck demo`. Not read from disk.
static DEMO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True once built-in demo devices are in use; SMART reads then return
/// [`demo_smart`] instead of touching hardware.
pub fn is_demo() -> bool {
    DEMO.load(std::sync::atomic::Ordering::Relaxed)
}

/// Synthetic SMART data for the demo devices (healthy NVMe, worn SATA SSD,
/// HDD with bad sectors, USB/MMC without SMART).
pub fn demo_smart(d: &Device) -> Option<crate::smartctl::SmartData> {
    use crate::smartctl::{SmartAttribute, SmartData};
    let attr = |id, name: &str, value, threshold, raw| SmartAttribute {
        id,
        name: name.to_string(),
        value,
        worst: value,
        threshold,
        raw,
        prefailure: threshold > 0,
        online: true,
    };
    let base = SmartData {
        source: "demo".to_string(),
        passed: Some(true),
        model: d.model.clone(),
        serial: d.serial.clone(),
        firmware: d.firmware.clone(),
        logical_block_size: Some(512),
        ..Default::default()
    };
    match d.name.as_str() {
        "nvme0n1" => Some(SmartData {
            temperature_c: Some(41),
            power_on_hours: Some(3_120),
            power_cycles: Some(812),
            lba_written: Some(35_156_250_000), // 18 TB
            lba_read: Some(52_734_375_000),
            life_percent: Some(94),
            media_errors: Some(0),
            available_spare: Some(100),
            available_spare_threshold: Some(10),
            interface_speed: Some("8.0 GT/s x4".to_string()),
            ..base
        }),
        "sda" => Some(SmartData {
            temperature_c: Some(36),
            power_on_hours: Some(12_400),
            power_cycles: Some(2_210),
            lba_written: Some(113_281_250_000), // 58 TB of 80 TBW
            reallocated: Some(0),
            pending: Some(0),
            interface_speed: Some("6.0 Gb/s".to_string()),
            attributes: vec![
                attr(5, "Reallocated_Sector_Ct", 100, 10, 0),
                attr(9, "Power_On_Hours", 100, 0, 12_400),
                attr(194, "Temperature_Celsius", 64, 0, 36),
                attr(241, "Total_LBAs_Written", 100, 0, 113_281_250_000),
            ],
            ..base
        }),
        "sdb" => Some(SmartData {
            temperature_c: Some(47),
            power_on_hours: Some(28_050),
            power_cycles: Some(4_120),
            rotation_rate: Some(5400),
            reallocated: Some(8),
            pending: Some(2),
            uncorrectable: Some(0),
            interface_speed: Some("6.0 Gb/s".to_string()),
            attributes: vec![
                attr(5, "Reallocated_Sector_Ct", 90, 36, 8),
                attr(9, "Power_On_Hours", 62, 0, 28_050),
                attr(194, "Temperature_Celsius", 103, 0, 47),
                attr(197, "Current_Pending_Sector", 200, 0, 2),
            ],
            ..base
        }),
        _ => None,
    }
}

pub fn demo_devices() -> Vec<Device> {
    DEMO.store(true, std::sync::atomic::Ordering::Relaxed);
    fn part(path: &str, size: u64, mount: Option<&str>, fs: Option<&str>) -> Partition {
        Partition {
            path: path.to_string(),
            size_bytes: size,
            mountpoint: mount.map(str::to_string),
            filesystem: fs.map(str::to_string),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn dev(
        name: &str,
        vendor: Option<&str>,
        model: &str,
        firmware: &str,
        serial: &str,
        bus: Bus,
        kind: MediaKind,
        size: u64,
        removable: bool,
        partitions: Vec<Partition>,
    ) -> Device {
        Device {
            name: name.to_string(),
            path: format!("/dev/{name}"),
            vendor: vendor.map(str::to_string),
            model: Some(model.to_string()),
            firmware: Some(firmware.to_string()),
            serial: Some(serial.to_string()),
            bus,
            kind,
            size_bytes: size,
            logical_block_size: 512,
            removable,
            smart_status: None,
            partitions,
            failure: None,
        }
    }

    vec![
        dev(
            "nvme0n1", None, "Samsung SSD 980 500GB", "2B4QFXO7", "S5GXNX0R123456",
            Bus::Nvme, MediaKind::Nvme, 500_000_000_000, false,
            vec![
                part("/dev/nvme0n1p1", 100_000_000_000, Some("/"), Some("ext4")),
                part("/dev/nvme0n1p2", 300_000_000_000, None, None),
            ],
        ),
        dev(
            "sda", Some("ATA"), "KINGSTON SA400S37", "R0105A", "ABC123",
            Bus::Sata, MediaKind::Ssd, 240_000_000_000, false,
            vec![
                part("/dev/sda1", 1_000_000_000, Some("/boot"), Some("vfat")),
                part("/dev/sda2", 200_000_000_000, None, None),
            ],
        ),
        dev(
            "sdb", Some("ATA"), "WDC WD10SPZX-00Z10T0", "01.01A01", "WD-XYZ",
            Bus::Sata, MediaKind::Hdd, 1_000_000_000_000, false,
            vec![part("/dev/sdb1", 1_000_000_000_000, None, None)],
        ),
        dev(
            "sdc", Some("Generic"), "Flash Disk", "8.07", "USB-0001",
            Bus::Usb, MediaKind::Ssd, 16_000_000_000, true,
            vec![],
        ),
        dev(
            "mmcblk0", None, "SD32G", "0x0", "MMC-0001",
            Bus::Mmc, MediaKind::Ssd, 32_000_000_000, false,
            vec![part("/dev/mmcblk0p1", 32_000_000_000, Some("/media/card"), Some("ext4"))],
        ),
    ]
}

#[cfg(target_os = "macos")]
mod macos {
    use crate::model::{Bus, Device, MediaKind};

    /// List physical whole disks via `diskutil list`, then read each with
    /// `diskutil info` (model, size, SSD/HDD, protocol, SMART status).
    pub fn list_devices() -> Vec<Device> {
        let Ok(out) = std::process::Command::new("diskutil").arg("list").output() else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let mut devices = Vec::new();
        for line in text.lines() {
            if line.starts_with("/dev/disk") && line.contains(", physical)") {
                if let Some(node) = line.split_whitespace().next() {
                    if let Some(name) = node.strip_prefix("/dev/") {
                        devices.push(info(name));
                    }
                }
            }
        }
        devices
    }

    fn info(name: &str) -> Device {
        let node = format!("/dev/{name}");
        let (mut model, mut size, mut ssd, mut removable) = (None, 0u64, false, false);
        let mut protocol = String::new();
        let mut smart = None;

        if let Ok(out) = std::process::Command::new("diskutil")
            .arg("info")
            .arg(&node)
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                let Some((key, value)) = line.split_once(':') else {
                    continue;
                };
                let key = key.trim();
                let value = value.trim();
                match key {
                    "Device / Media Name" => model = nonempty(value),
                    "Disk Size" => size = parse_size_bytes(value).unwrap_or(0),
                    "Solid State" => ssd = value.eq_ignore_ascii_case("yes"),
                    "Protocol" => protocol = value.to_string(),
                    "SMART Status" => {
                        smart = match value {
                            "Verified" => Some(true),
                            "Failing" => Some(false),
                            _ => None,
                        }
                    }
                    "Removable Media" => removable = value.eq_ignore_ascii_case("yes"),
                    _ => {}
                }
            }
        }

        let bus = if protocol.contains("NVMe") || protocol.contains("Fabric") {
            Bus::Nvme
        } else if protocol.contains("SATA") {
            Bus::Sata
        } else if protocol.contains("USB") {
            Bus::Usb
        } else if protocol.contains("SCSI") || protocol.contains("SAS") {
            Bus::Scsi
        } else {
            Bus::Unknown
        };
        let kind = if bus == Bus::Nvme {
            MediaKind::Nvme
        } else if ssd {
            MediaKind::Ssd
        } else {
            MediaKind::Hdd
        };

        Device {
            name: name.to_string(),
            path: node,
            vendor: None,
            model,
            firmware: None,
            serial: None,
            bus,
            kind,
            size_bytes: size,
            logical_block_size: 512,
            removable,
            smart_status: smart,
            partitions: Vec::new(),
            failure: None,
        }
    }

    fn nonempty(value: &str) -> Option<String> {
        let t = value.trim();
        if t.is_empty() || t == "-" {
            None
        } else {
            Some(t.to_string())
        }
    }

    /// Parse `"500.3 GB (500277790720 Bytes)"` -> bytes.
    fn parse_size_bytes(value: &str) -> Option<u64> {
        let start = value.find('(')?;
        let digits: String = value[start + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_diskutil_size() {
            assert_eq!(parse_size_bytes("500.3 GB (500277790720 Bytes)"), Some(500277790720));
            assert_eq!(parse_size_bytes("no size here"), None);
        }
    }
}

#[cfg(target_os = "freebsd")]
mod freebsd {
    use crate::model::{Bus, Device, MediaKind};
    use crate::smartctl;

    /// List disks via `sysctl -n kern.disks`; identity/capacity from smartctl.
    pub fn list_devices() -> Vec<Device> {
        let out = match std::process::Command::new("sysctl")
            .args(["-n", "kern.disks"])
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .filter(|n| !n.starts_with("cd") && !n.starts_with("md"))
            .map(build)
            .collect()
    }

    fn build(name: &str) -> Device {
        let path = format!("/dev/{name}");
        let smart = smartctl::read_smart(&path);
        let (model, serial, firmware, capacity, rota) = match &smart {
            Some(s) => (
                s.model.clone(),
                s.serial.clone(),
                s.firmware.clone(),
                s.capacity_bytes,
                s.rotation_rate,
            ),
            None => (None, None, None, None, None),
        };

        let nvme = name.starts_with("nvme") || name.starts_with("nda");
        let bus = if nvme {
            Bus::Nvme
        } else if name.starts_with("ada") {
            Bus::Sata
        } else if name.starts_with("da") {
            Bus::Scsi
        } else if name.starts_with("mmcsd") {
            Bus::Mmc
        } else {
            Bus::Unknown
        };
        let kind = if nvme {
            MediaKind::Nvme
        } else {
            match rota {
                Some(0) => MediaKind::Ssd,
                Some(_) => MediaKind::Hdd,
                None => MediaKind::Unknown,
            }
        };

        Device {
            name: name.to_string(),
            path,
            vendor: None,
            model,
            firmware,
            serial,
            bus,
            kind,
            size_bytes: capacity.unwrap_or(0),
            logical_block_size: 512,
            removable: false,
            smart_status: None,
            partitions: Vec::new(),
            failure: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_virtual_block_devices() {
        for name in ["rbd0", "rbd63", "drbd1", "bcache0", "dm-3", "loop7", "nbd0", "zd16", "md127"] {
            assert!(is_ignored(name), "{name} should be skipped");
        }
        for name in ["sda", "nvme0n1", "mmcblk0", "vda", "xvda"] {
            assert!(!is_ignored(name), "{name} should be listed");
        }
    }

    #[test]
    fn partitions_are_recognised() {
        assert!(is_partition_of("sda", "sda1"));
        assert!(is_partition_of("sda", "sda15"));
        assert!(is_partition_of("mmcblk0", "mmcblk0p1"));
        assert!(is_partition_of("nvme0n1", "nvme0n1p1"));
        assert!(!is_partition_of("sda", "sda"));
        assert!(!is_partition_of("sda", "sdab"));
        assert!(!is_partition_of("nvme0n1", "nvme0n1"));
        assert!(!is_partition_of("mmcblk0", "mmcblk0boot0"));
    }

    #[test]
    fn nvme_controller_is_extracted() {
        assert_eq!(nvme_controller("nvme0n1").as_deref(), Some("nvme0"));
        assert_eq!(nvme_controller("nvme12n1").as_deref(), Some("nvme12"));
        assert_eq!(nvme_controller("sda"), None);
    }

    #[test]
    fn octal_escapes_are_decoded() {
        assert_eq!(decode_octal("/mnt/My\\040Disk"), "/mnt/My Disk");
        assert_eq!(decode_octal("/plain"), "/plain");
    }

    #[test]
    fn classifies_buses() {
        assert_eq!(
            classify_bus(
                "sda",
                Path::new("/sys/devices/pci0000:00/0000:02:00.0/host0/target0:0:0/0:0:0:0")
            ),
            Bus::Scsi
        );
        assert_eq!(
            classify_bus(
                "sda",
                Path::new("/sys/devices/pci0000:00/ata1/host0/target0:0:0/0:0:0:0")
            ),
            Bus::Sata
        );
        assert_eq!(
            classify_bus(
                "nvme0n1",
                Path::new("/sys/devices/pci0000:00/nvme/nvme0/nvme0n1")
            ),
            Bus::Nvme
        );
    }

    #[test]
    fn mounts_fall_back_to_source_path() {
        // btrfs reports an anonymous 0:NN device; the /dev path must still match.
        let mut mounts = Mounts::default();
        mounts
            .by_source
            .insert("/dev/sda3".to_string(), ("/".to_string(), "btrfs".to_string()));
        assert_eq!(
            mounts.lookup("0:34", "/dev/sda3"),
            Some(("/".to_string(), "btrfs".to_string()))
        );
        assert_eq!(mounts.lookup("8:2", "/dev/sda2"), None);
    }
}
