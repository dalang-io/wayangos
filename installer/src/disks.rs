//! Disks the installer can target, read from /sys/block: what they are
//! (NVMe / SATA / SAS / RAID / USB / eMMC / virtual, SSD or HDD), what is on
//! them, and whether they may be picked at all.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::sys;

/// Volume label of the installer ISO (see scripts/build-installer-iso.sh).
pub const INSTALLER_LABEL: &str = "WAYANG_INSTALL";
/// ESP (512 MiB) + a usable /data.
pub const MIN_SIZE: u64 = 2_000_000_000;

/// SCSI host drivers that present RAID logical volumes rather than disks.
const RAID_DRIVERS: [&str; 7] = [
    "megaraid_sas",
    "hpsa",
    "smartpqi",
    "aacraid",
    "arcmsr",
    "mpi3mr",
    "3w-sas",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Nvme,
    Sata,
    Sas,
    Raid,
    Usb,
    Mmc,
    Virtual,
    Scsi,
    Other,
}

#[derive(Debug, Clone, Default)]
pub struct Fs {
    pub kind: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Part {
    pub name: String,
    pub size: u64,
    pub fs: Fs,
}

#[derive(Debug, Clone)]
pub struct Disk {
    pub name: String,
    pub model: String,
    pub serial: Option<String>,
    pub size: u64,
    pub bus: Bus,
    pub rotational: bool,
    pub removable: bool,
    pub readonly: bool,
    /// Filesystem on the whole device (no partition table).
    pub whole: Fs,
    pub parts: Vec<Part>,
}

impl Disk {
    pub fn path(&self) -> String {
        format!("/dev/{}", self.name)
    }

    /// Partition device for index 1..: nvme0n1 -> nvme0n1p1, sda -> sda1.
    pub fn part_path(&self, n: u8) -> String {
        let sep = if self.name.ends_with(|c: char| c.is_ascii_digit()) {
            "p"
        } else {
            ""
        };
        format!("/dev/{}{sep}{n}", self.name)
    }

    pub fn kind(&self) -> &'static str {
        match (self.bus, self.rotational) {
            (Bus::Nvme, _) => "NVMe SSD",
            (Bus::Sata, false) => "SATA SSD",
            (Bus::Sata, true) => "SATA HDD",
            (Bus::Sas, false) => "SAS SSD",
            (Bus::Sas, true) => "SAS HDD",
            (Bus::Raid, _) => "RAID VOLUME",
            (Bus::Usb, _) => "USB DRIVE",
            (Bus::Mmc, _) => "eMMC / SD",
            (Bus::Virtual, _) => "VIRTUAL",
            (Bus::Scsi, false) => "SCSI SSD",
            (Bus::Scsi, true) => "SCSI HDD",
            (Bus::Other, _) => "DISK",
        }
    }

    pub fn is_installer(&self) -> bool {
        self.whole.label.as_deref() == Some(INSTALLER_LABEL)
            || self
                .parts
                .iter()
                .any(|p| p.fs.label.as_deref() == Some(INSTALLER_LABEL))
    }

    /// Why this disk can't be the target, if it can't.
    pub fn blocked(&self) -> Option<&'static str> {
        if self.is_installer() {
            Some("installer media")
        } else if self.readonly {
            Some("read-only")
        } else if self.size < MIN_SIZE {
            Some("too small")
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty() && self.whole.kind.is_none()
    }

    /// One-line summary of what's on it: "empty", "ext4 'data'",
    /// "3 partitions · vfat ntfs".
    pub fn contents(&self) -> String {
        if let Some(why) = self.blocked() {
            return why.to_string();
        }
        if self.parts.is_empty() {
            return match (&self.whole.kind, &self.whole.label) {
                (None, _) => "empty".into(),
                (Some(k), Some(l)) => format!("{k} '{l}'"),
                (Some(k), None) => k.clone(),
            };
        }
        let mut kinds: Vec<&str> = Vec::new();
        for p in &self.parts {
            let k = p.fs.kind.as_deref().unwrap_or("raw");
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
        let n = self.parts.len();
        format!(
            "{n} partition{} · {}",
            if n == 1 { "" } else { "s" },
            kinds.join(" ")
        )
    }
}

pub fn scan(demo: bool) -> Vec<Disk> {
    if demo {
        return demo_disks();
    }
    let fs_map = blkid();
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
        .into_iter()
        .filter(|n| !ignored(n))
        .filter_map(|n| read_disk(&n, &fs_map))
        .collect()
}

fn ignored(name: &str) -> bool {
    [
        "loop", "ram", "zram", "sr", "fd", "md", "dm-", "nbd", "mtdblock",
    ]
    .iter()
    .any(|p| name.starts_with(p))
        || name.contains("boot")
        || name.contains("rpmb")
}

fn read(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn read_disk(name: &str, fs_map: &HashMap<String, Fs>) -> Option<Disk> {
    let base = Path::new("/sys/block").join(name);
    let size = read(&base.join("size"))?.parse::<u64>().ok()? * 512;
    if size == 0 {
        return None; // empty card reader slot
    }
    let resolved = fs::canonicalize(&base).ok()?.to_string_lossy().into_owned();
    let dev = base.join("device");
    let vendor = read(&dev.join("vendor")).filter(|v| v != "ATA");
    let model = read(&dev.join("model"))
        .or_else(|| read(&dev.join("name")))
        .unwrap_or_else(|| name.to_string());
    let model = match vendor {
        Some(v) if !model.to_lowercase().starts_with(&v.to_lowercase()) => format!("{v} {model}"),
        _ => model,
    };

    let mut parts = Vec::new();
    if let Ok(entries) = fs::read_dir(&base) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.join("partition").exists() {
                continue;
            }
            let pname = e.file_name().to_string_lossy().into_owned();
            let psize = read(&p.join("size"))
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
                * 512;
            parts.push(Part {
                fs: fs_map.get(&pname).cloned().unwrap_or_default(),
                name: pname,
                size: psize,
            });
        }
    }
    // sda2 before sda10
    parts.sort_by(|a, b| {
        a.name
            .len()
            .cmp(&b.name.len())
            .then_with(|| a.name.cmp(&b.name))
    });

    Some(Disk {
        name: name.to_string(),
        model,
        serial: read(&dev.join("serial")),
        size,
        bus: classify(name, &resolved),
        rotational: read(&base.join("queue/rotational")).as_deref() == Some("1"),
        removable: read(&base.join("removable")).as_deref() == Some("1"),
        readonly: read(&base.join("ro")).as_deref() == Some("1"),
        whole: fs_map.get(name).cloned().unwrap_or_default(),
        parts,
    })
}

fn classify(name: &str, resolved: &str) -> Bus {
    if name.starts_with("nvme") {
        Bus::Nvme
    } else if name.starts_with("mmcblk") {
        Bus::Mmc
    } else if name.starts_with("vd") || name.starts_with("xvd") || resolved.contains("/virtio") {
        Bus::Virtual
    } else if resolved.contains("/usb") {
        Bus::Usb
    } else if resolved.contains("/end_device-") {
        // SAS transport class: host/port-H:N/end_device-H:N/target...
        Bus::Sas
    } else if resolved.contains("/ata") {
        Bus::Sata
    } else if let Some(drv) = scsi_host_driver(resolved) {
        if RAID_DRIVERS.contains(&drv.as_str()) {
            Bus::Raid
        } else {
            Bus::Scsi
        }
    } else {
        Bus::Other
    }
}

/// proc_name of the SCSI host in a sysfs path (`.../host2/target2:0:0/...`).
fn scsi_host_driver(resolved: &str) -> Option<String> {
    let host = resolved.split('/').find(|c| {
        c.strip_prefix("host")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })?;
    read(
        &Path::new("/sys/class/scsi_host")
            .join(host)
            .join("proc_name"),
    )
}

/// busybox blkid: `/dev/sda1: LABEL="x y" UUID="..." TYPE="ext4"`.
fn blkid() -> HashMap<String, Fs> {
    let out = sys::run("blkid", &[]).unwrap_or_default();
    parse_blkid(&out)
}

fn parse_blkid(out: &str) -> HashMap<String, Fs> {
    let mut map = HashMap::new();
    for line in out.lines() {
        let Some((dev, rest)) = line.split_once(": ") else {
            continue;
        };
        let mut fs = Fs::default();
        let mut s = rest;
        while let Some(eq) = s.find("=\"") {
            let key = s[..eq].trim();
            let after = &s[eq + 2..];
            let Some(end) = after.find('"') else { break };
            let val = after[..end].to_string();
            match key {
                "TYPE" => fs.kind = Some(val),
                "LABEL" => fs.label = Some(val),
                _ => {}
            }
            s = &after[end + 1..];
        }
        map.insert(dev.trim_start_matches("/dev/").to_string(), fs);
    }
    map
}

fn demo_disks() -> Vec<Disk> {
    let fs = |k: &str, l: Option<&str>| Fs {
        kind: Some(k.into()),
        label: l.map(Into::into),
    };
    let part = |n: &str, size: u64, f: Fs| Part {
        name: n.into(),
        size,
        fs: f,
    };
    let disk = |name: &str, model: &str, size: u64, bus: Bus, rot: bool| Disk {
        name: name.into(),
        model: model.into(),
        serial: Some(format!("S{}X{}", name.len() * 7919, size % 99991)),
        size,
        bus,
        rotational: rot,
        removable: bus == Bus::Usb,
        readonly: false,
        whole: Fs::default(),
        parts: Vec::new(),
    };
    let mut nvme = disk(
        "nvme0n1",
        "Samsung SSD 970 EVO Plus 500GB",
        500_107_862_016,
        Bus::Nvme,
        false,
    );
    nvme.parts = vec![
        part("nvme0n1p1", 104_857_600, fs("vfat", Some("SYSTEM"))),
        part("nvme0n1p2", 16_777_216, Fs::default()),
        part("nvme0n1p3", 499_000_000_000, fs("ntfs", Some("Windows"))),
    ];
    let ssd = disk(
        "sda",
        "Crucial CT1000MX500SSD1",
        1_000_204_886_016,
        Bus::Sata,
        false,
    );
    let mut sas = disk(
        "sdb",
        "SEAGATE ST4000NM0025",
        4_000_787_030_016,
        Bus::Sas,
        true,
    );
    sas.parts = vec![part("sdb1", 4_000_785_104_896, fs("ext4", Some("backup")))];
    let mut usb = disk("sdc", "SanDisk Ultra Fit", 30_752_000_000, Bus::Usb, false);
    usb.whole = fs("iso9660", Some(INSTALLER_LABEL));
    vec![nvme, ssd, sas, usb]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blkid_lines() {
        let m = parse_blkid(
            "/dev/sda1: LABEL=\"My Data\" UUID=\"1234\" TYPE=\"ntfs\"\n/dev/sr0: UUID=\"2026\" LABEL=\"WAYANG_INSTALL\" TYPE=\"iso9660\"\n",
        );
        assert_eq!(m["sda1"].kind.as_deref(), Some("ntfs"));
        assert_eq!(m["sda1"].label.as_deref(), Some("My Data"));
        assert_eq!(m["sr0"].label.as_deref(), Some(INSTALLER_LABEL));
    }

    #[test]
    fn classifies_buses() {
        assert_eq!(
            classify(
                "nvme0n1",
                "/sys/devices/pci0000:00/0000:00:1d.0/0000:3d:00.0/nvme/nvme0/nvme0n1"
            ),
            Bus::Nvme
        );
        assert_eq!(
            classify(
                "sda",
                "/sys/devices/pci0000:00/0000:00:17.0/ata1/host0/target0:0:0/0:0:0:0/block/sda"
            ),
            Bus::Sata
        );
        assert_eq!(
            classify("sdb", "/sys/devices/pci0000:00/0000:00:03.0/host2/port-2:0/end_device-2:0/target2:0:0/2:0:0:0/block/sdb"),
            Bus::Sas
        );
        assert_eq!(classify("sdc", "/sys/devices/pci0000:00/0000:00:14.0/usb2/2-1/2-1:1.0/host6/target6:0:0/6:0:0:0/block/sdc"), Bus::Usb);
        assert_eq!(
            classify(
                "vda",
                "/sys/devices/pci0000:00/0000:00:05.0/virtio2/block/vda"
            ),
            Bus::Virtual
        );
        assert_eq!(
            classify(
                "mmcblk0",
                "/sys/devices/platform/soc/mmc0/mmc0:0001/block/mmcblk0"
            ),
            Bus::Mmc
        );
    }

    #[test]
    fn partition_paths_and_rules() {
        let d = &demo_disks();
        assert_eq!(d[0].part_path(1), "/dev/nvme0n1p1");
        assert_eq!(d[1].part_path(2), "/dev/sda2");
        assert_eq!(d[3].blocked(), Some("installer media"));
        assert_eq!(d[1].contents(), "empty");
        assert_eq!(d[0].contents(), "3 partitions · vfat raw ntfs");
        assert_eq!(d[2].kind(), "SAS HDD");
    }
}
