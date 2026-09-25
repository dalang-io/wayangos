//! Running inside a virtual machine.
//!
//! A VM has no physical disks, fans or DIMMs of its own: the hypervisor shows
//! virtual devices (virtio, "QEMU HARDDISK", "VMware Virtual disk" …) without
//! SMART, and the board is an emulated chipset with a firmware image that
//! looks ancient (SeaBIOS from 2014). dcheck says so instead of reporting
//! UNKNOWN disks or a "12-year-old BIOS": the physical health belongs to the
//! host / cloud provider.

use std::path::Path;
use std::sync::OnceLock;

use crate::model::{Bus, Device};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Virt {
    /// "KVM / QEMU", "VMware", "Hyper-V", "Xen", "VirtualBox", …
    pub hypervisor: String,
    /// Cloud when the firmware names one (Amazon EC2, Google, …).
    pub cloud: Option<String>,
}

impl Virt {
    pub fn label(&self) -> String {
        match &self.cloud {
            Some(c) => format!("{} ({c})", self.hypervisor),
            None => self.hypervisor.clone(),
        }
    }
}

/// Identify the hypervisor from DMI strings, `/sys/hypervisor/type` and the
/// CPU `hypervisor` flag.
pub fn detect_from(dmi: &[&str], hyp_type: Option<&str>, cpu_flag: bool) -> Option<Virt> {
    let all = dmi.join(" ").to_ascii_lowercase();
    let has = |k: &str| all.contains(k);
    let cloud = if has("amazon ec2") {
        Some("Amazon EC2")
    } else if has("google compute engine") || has("google") {
        Some("Google Cloud")
    } else if has("digitalocean") {
        Some("DigitalOcean")
    } else if has("hetzner") {
        Some("Hetzner")
    } else if has("alibaba") {
        Some("Alibaba Cloud")
    } else if has("openstack") {
        Some("OpenStack")
    } else {
        None
    }
    .map(str::to_string);
    let hv = if has("vmware") {
        "VMware"
    } else if has("virtualbox") || has("innotek") {
        "VirtualBox"
    } else if has("microsoft corporation") && has("virtual machine") {
        "Hyper-V"
    } else if has("xen") || hyp_type == Some("xen") {
        "Xen"
    } else if has("qemu") || has("kvm") || has("bochs") || has("amazon ec2") || has("google") || has("openstack") {
        "KVM / QEMU"
    } else if has("parallels") {
        "Parallels"
    } else if cpu_flag {
        "virtual machine"
    } else {
        return None;
    };
    Some(Virt { hypervisor: hv.into(), cloud })
}

/// This machine's hypervisor, if it is a VM (read once).
pub fn detect() -> Option<Virt> {
    static V: OnceLock<Option<Virt>> = OnceLock::new();
    V.get_or_init(|| {
        if crate::enumerate::is_demo() {
            return None;
        }
        let root = match std::env::var_os("DCHECK_SYS_ROOT") {
            Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
            _ => std::path::PathBuf::from("/"),
        };
        read_from(&root)
    })
    .clone()
}

pub fn read_from(root: &Path) -> Option<Virt> {
    let rd = |p: &str| std::fs::read_to_string(root.join(p)).ok().map(|s| s.trim().to_string());
    let dmi: Vec<String> = ["sys_vendor", "product_name", "board_vendor", "bios_vendor", "chassis_vendor"]
        .iter()
        .filter_map(|f| rd(&format!("sys/class/dmi/id/{f}")))
        .collect();
    let dmi_refs: Vec<&str> = dmi.iter().map(String::as_str).collect();
    let hyp = rd("sys/hypervisor/type");
    let cpu_flag = rd("proc/cpuinfo").is_some_and(|c| {
        c.lines().any(|l| l.starts_with("flags") && l.split_whitespace().any(|f| f == "hypervisor"))
    });
    detect_from(&dmi_refs, hyp.as_deref(), cpu_flag)
}

/// A disk the hypervisor provides (no SMART, no physical wear).
pub fn is_virtual_disk(d: &Device) -> bool {
    if d.bus == Bus::Virtio || d.name.starts_with("vd") || d.name.starts_with("xvd") {
        return true;
    }
    let id = format!("{} {}", d.vendor.as_deref().unwrap_or(""), d.model.as_deref().unwrap_or("")).to_ascii_lowercase();
    [
        "qemu harddisk",
        "qemu hardisk",
        "qemu",
        "vbox harddisk",
        "vmware virtual",
        "vmware,",
        "virtual disk",
        "msft virtual",
        "xen virtual",
        "amazon elastic block store",
        "google persistentdisk",
        "0x1af4",
    ]
    .iter()
    .any(|k| id.contains(k))
}

/// "VIRT" for virtual disks, else the media kind.
pub fn kind_label(d: &Device) -> String {
    if is_virtual_disk(d) {
        "VIRT".into()
    } else {
        d.kind.to_string()
    }
}

/// Why a virtual disk has no health data.
pub const DISK_NOTE: &str =
    "virtual disk: the hypervisor exposes no SMART; the physical disks belong to the host / provider";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_hypervisors() {
        // idch (IDCloudHost): QEMU i440FX with SeaBIOS.
        let v = detect_from(&["QEMU", "Standard PC (i440FX + PIIX, 1996)", "SeaBIOS"], None, true).unwrap();
        assert_eq!(v.hypervisor, "KVM / QEMU");
        assert_eq!(v.cloud, None);
        assert_eq!(detect_from(&["VMware, Inc.", "VMware Virtual Platform"], None, true).unwrap().hypervisor, "VMware");
        assert_eq!(detect_from(&["Microsoft Corporation", "Virtual Machine"], None, true).unwrap().hypervisor, "Hyper-V");
        assert_eq!(detect_from(&["innotek GmbH", "VirtualBox"], None, true).unwrap().hypervisor, "VirtualBox");
        let ec2 = detect_from(&["Amazon EC2", "t3.micro"], None, true).unwrap();
        assert_eq!(ec2.label(), "KVM / QEMU (Amazon EC2)");
        assert_eq!(detect_from(&[], Some("xen"), false).unwrap().hypervisor, "Xen");
        assert_eq!(detect_from(&["Some Cloud"], None, true).unwrap().hypervisor, "virtual machine");
        // Bare metal.
        assert_eq!(detect_from(&["Dell Inc.", "PowerEdge R630"], None, false), None);
    }

    #[test]
    fn recognises_virtual_disks() {
        let mut d = crate::enumerate::demo_devices().remove(1);
        assert!(!is_virtual_disk(&d));
        d.name = "vda".into();
        assert!(is_virtual_disk(&d));
        assert_eq!(kind_label(&d), "VIRT");
        let mut q = crate::enumerate::demo_devices().remove(1);
        q.vendor = Some("ATA".into());
        q.model = Some("QEMU HARDDISK".into());
        assert!(is_virtual_disk(&q));
        // biznetgio: vendor "QEMU", model "QEMU HARDDISK" — not repeated.
        q.vendor = Some("QEMU".into());
        assert_eq!(q.label(), "QEMU HARDDISK");
        q.vendor = Some("TOSHIBA".into());
        q.model = Some("MBF2300RC".into());
        assert_eq!(q.label(), "TOSHIBA MBF2300RC");
    }
}
