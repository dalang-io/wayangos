//! Block-device discovery on Linux.
//!
//! Everything is read from `/sys` and `/proc` so the tool works without
//! `lsblk`/`udev` on a minimal rootfs (e.g. WayangOS). Devices are listed as
//! soon as they are *attached*, whether or not they are mounted.
//!
//! The filesystem root can be overridden with `DCHECK_SYS_ROOT`, which is used
//! by the test fixture and is also handy for inspecting a chroot/offline image.

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

fn sys_block(root: &Path) -> PathBuf {
    root.join("sys/block")
}

/// Enumerate whole-disk block devices.
pub fn list_devices() -> Vec<Device> {
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

fn is_ignored(name: &str) -> bool {
    const PREFIXES: [&str; 6] = ["loop", "ram", "zram", "dm-", "md", "sr"];
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
        partitions,
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
    } else if path.contains("/scsi") {
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
pub fn has_sysfs() -> bool {
    sys_block(&root()).is_dir()
}

/// Built-in sample devices, used for demos on hosts without sysfs (e.g. macOS)
/// and by `dcheck demo`. Not read from disk.
pub fn demo_devices() -> Vec<Device> {
    fn part(path: &str, size: u64, mount: Option<&str>, fs: Option<&str>) -> Partition {
        Partition {
            path: path.to_string(),
            size_bytes: size,
            mountpoint: mount.map(str::to_string),
            filesystem: fs.map(str::to_string),
        }
    }
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
            partitions,
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

#[cfg(test)]
mod tests {
    use super::*;

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
