//! Core data types shared across dcheck modules.

use std::fmt;

/// Physical transport / bus the device is attached through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Sata,
    Nvme,
    Usb,
    Mmc,
    Virtio,
    Scsi,
    Unknown,
}

impl fmt::Display for Bus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Bus::Sata => "SATA",
            Bus::Nvme => "NVMe",
            Bus::Usb => "USB",
            Bus::Mmc => "MMC",
            Bus::Virtio => "VirtIO",
            Bus::Scsi => "SCSI",
            Bus::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// Rotational vs solid-state classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Ssd,
    Hdd,
    Nvme,
    Unknown,
}

impl fmt::Display for MediaKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            MediaKind::Ssd => "SSD",
            MediaKind::Hdd => "HDD",
            MediaKind::Nvme => "NVMe",
            MediaKind::Unknown => "?",
        };
        f.write_str(s)
    }
}

/// A partition (or other child block device) of a disk.
#[derive(Debug, Clone)]
pub struct Partition {
    pub path: String,
    pub size_bytes: u64,
    pub mountpoint: Option<String>,
    pub filesystem: Option<String>,
}

/// A whole-disk block device discovered from `/sys/block`.
#[derive(Debug, Clone)]
pub struct Device {
    /// Kernel name, e.g. `sda`, `nvme0n1`.
    pub name: String,
    /// Device node, e.g. `/dev/sda`.
    pub path: String,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub serial: Option<String>,
    pub bus: Bus,
    pub kind: MediaKind,
    pub size_bytes: u64,
    pub logical_block_size: u64,
    pub removable: bool,
    /// Pre-known SMART pass/fail from the platform (e.g. macOS `diskutil`),
    /// used when no smartctl/native reader is available.
    pub smart_status: Option<bool>,
    pub partitions: Vec<Partition>,
}

impl Device {
    /// Human label for the device: "Vendor Model" or the kernel name.
    pub fn label(&self) -> String {
        match (&self.vendor, &self.model) {
            (Some(v), Some(m)) if !v.is_empty() && !m.is_empty() => format!("{v} {m}"),
            (None, Some(m)) if !m.is_empty() => m.clone(),
            (Some(v), None) if !v.is_empty() => v.clone(),
            _ => self.name.clone(),
        }
    }
}
