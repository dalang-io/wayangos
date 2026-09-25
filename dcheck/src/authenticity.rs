//! Is the drive what its label claims? (fake / rebranded / unbranded drives)
//!
//! No single bit proves a drive is counterfeit. What the firmware reports can
//! be checked for consistency:
//!
//! - **WWN / IEEE OUI.** SATA and SAS drives carry a World Wide Name (NAA 5:
//!   4-bit NAA + 24-bit IEEE OUI of the maker + 36-bit id). Genuine branded
//!   drives use their maker's OUI (Samsung `002538`, Seagate `000c50`, …).
//!   A brand in the model name with another maker's OUI, or an all-zero /
//!   missing WWN, is how cheap clones and rebrands show up.
//! - **NVMe PCI vendor ID.** Some makers only ship their own controllers
//!   (Samsung = `144d`); a "Samsung 980" on a Maxio or Phison controller is
//!   not a Samsung.
//! - **Generic identity.** Model strings like `SSD 1TB` or `NVMe SSD` name no
//!   manufacturer; placeholder serials (`0123456789ABCDEF`, all zeros).
//!
//! A determined forger can fake all of these; a clean result means
//! "consistent", never "genuine". Capacity fraud (a drive that reports more
//! space than it has) cannot be seen from identity at all — only a
//! write-and-verify test finds it.

use crate::json::{self, Json};
use crate::model::{Bus, Device};
use crate::smartctl::SmartData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Identity matches a known maker.
    Consistent,
    /// Nothing contradicts the claim, but nothing confirms it either.
    Unverified,
    /// The drive names no manufacturer at all.
    Unbranded,
    Suspicious,
    LikelyFake,
    /// Not applicable (RAID volume, virtual disk) or no identity to check.
    Unknown,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Consistent => "CONSISTENT",
            Level::Unverified => "UNVERIFIED",
            Level::Unbranded => "UNBRANDED",
            Level::Suspicious => "SUSPICIOUS",
            Level::LikelyFake => "LIKELY FAKE",
            Level::Unknown => "N/A",
        }
    }

    /// 0 = fine … 3 = likely fake (for colors).
    pub fn severity(self) -> u8 {
        match self {
            Level::Consistent | Level::Unknown => 0,
            Level::Unverified => 1,
            Level::Unbranded | Level::Suspicious => 2,
            Level::LikelyFake => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Supports the claimed identity.
    Good,
    Info,
    /// Points at a rebrand / clone.
    Bad,
}

#[derive(Debug, Clone)]
pub struct Authenticity {
    pub level: Level,
    /// Brand named by the model/vendor string.
    pub brand: Option<&'static str>,
    /// Maker the WWN OUI / PCI vendor ID is registered to.
    pub maker: Option<&'static str>,
    /// WWN as reported (16+ hex digits), if any.
    pub wwn: Option<String>,
    pub signals: Vec<(Mark, String)>,
}

impl Authenticity {
    /// One-line summary for lists and the TUI.
    pub fn summary(&self) -> String {
        match (self.level, self.brand, self.maker) {
            (Level::Consistent, _, Some(m)) => format!("identity matches {m}"),
            (Level::LikelyFake, Some(b), Some(m)) => format!("claims {b}, identity belongs to {m}"),
            (Level::LikelyFake, Some(b), None) => format!("claims {b}, controller is not {b}"),
            (Level::Unbranded, _, _) => "firmware names no manufacturer".into(),
            (Level::Suspicious, Some(b), _) => format!("claims {b} but carries no {b} identity"),
            (Level::Unknown, _, _) => "not applicable".into(),
            _ => self
                .signals
                .iter()
                .find(|(m, _)| *m == Mark::Bad)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(|| "no maker ID to compare against".into()),
        }
    }
}

struct Maker {
    name: &'static str,
    /// Upper-case substrings of the model / vendor string naming the brand.
    keywords: &'static [&'static str],
    /// Model prefixes (upper case) of the maker's part numbers.
    prefixes: &'static [&'static str],
    /// NVMe PCI vendor IDs of the maker's own controllers.
    pci: &'static [u64],
    /// The maker only ships its own NVMe controllers: another PCI vendor ID
    /// means the drive is not what it claims.
    own_controllers: bool,
}

/// Maker families. Brands owned by one group share its OUIs (WD / HGST /
/// SanDisk; Toshiba / Kioxia / Fujitsu's HDD line; Micron / Crucial). OUIs
/// come from the IEEE registry (`oui_table.rs`); PCI vendor IDs from
/// pci.ids.
const MAKERS: &[Maker] = &[
    Maker {
        name: "Samsung",
        keywords: &["SAMSUNG"],
        prefixes: &["MZ7", "MZV", "MZQ", "MZ-", "MZI", "MZ1", "MZP", "MZN", "MZW", "MZ9", "HD103", "HD204", "HD502"],
        pci: &[0x144d],
        own_controllers: true,
    },
    Maker {
        name: "Western Digital",
        keywords: &["WDC", "WESTERN DIGITAL", "HGST", "HITACHI", "SANDISK", "ULTRASTAR"],
        prefixes: &["WD", "WUH", "WUS", "HUS", "HUH", "HUA", "HDS", "HTS", "HTE"],
        pci: &[0x15b7, 0x1b96, 0x1c58],
        own_controllers: false,
    },
    Maker {
        name: "Seagate",
        keywords: &["SEAGATE"],
        prefixes: &["ST"],
        pci: &[0x1bb1],
        own_controllers: false,
    },
    Maker {
        name: "Toshiba/Kioxia",
        keywords: &["TOSHIBA", "KIOXIA", "FUJITSU"],
        prefixes: &["THN", "KXG", "KBG", "KCD", "KCM", "MG0", "MG1", "AL1", "MBF", "MQ0", "DT01", "HDW", "MK"],
        pci: &[0x1179, 0x1e0f],
        own_controllers: false,
    },
    Maker {
        name: "Micron/Crucial",
        keywords: &["MICRON", "CRUCIAL"],
        prefixes: &["MTFD", "CT"],
        pci: &[0x1344],
        own_controllers: false,
    },
    Maker {
        name: "Intel/Solidigm",
        keywords: &["INTEL", "SOLIDIGM"],
        prefixes: &["SSDSC", "SSDPE", "SSDPF"],
        pci: &[0x8086, 0x025e],
        own_controllers: false,
    },
    Maker {
        name: "Kingston",
        keywords: &["KINGSTON"],
        prefixes: &["SA400", "SV300", "SUV", "SKC", "SNV", "SA2000", "SEDC", "SNS", "SQ500"],
        pci: &[0x2646],
        own_controllers: false,
    },
    Maker {
        name: "SK hynix",
        keywords: &["HYNIX"],
        prefixes: &["HFS", "HFM", "SHGP", "SHGS"],
        pci: &[0x1c5c],
        own_controllers: true,
    },
];

/// NVMe controller vendors that are not drive brands (for the explanation).
fn controller_vendor(vid: u64) -> Option<&'static str> {
    Some(match vid {
        0x1987 => "Phison",
        0x126f => "Silicon Motion",
        0x1e4b => "Maxio",
        0x10ec => "Realtek",
        0x1dbe => "InnoGrit",
        0x1e49 => "YMTC",
        0x1b4b => "Marvell",
        0x1cc1 => "ADATA",
        0x1d97 => "Longsys",
        0x106b => "Apple",
        _ => return None,
    })
}

/// Maker an IEEE OUI is registered to (full registry extract, see
/// `oui_table.rs` / `scripts/gen-dcheck-oui.sh`).
fn maker_by_oui(oui: &str) -> Option<&'static Maker> {
    let (_, name) = crate::oui_table::OUI_MAKERS.iter().find(|(o, _)| *o == oui)?;
    MAKERS.iter().find(|m| m.name == *name)
}

fn maker_by_pci(vid: u64) -> Option<&'static Maker> {
    MAKERS.iter().find(|m| m.pci.contains(&vid))
}

/// Brand named by the model string (and a SCSI vendor field such as
/// `TOSHIBA`; SATA drives behind libata report vendor `ATA`).
fn claimed_brand(vendor: &str, model: &str) -> Option<&'static Maker> {
    let text = format!("{} {}", vendor, model).to_ascii_uppercase();
    let model_up = model.trim().to_ascii_uppercase();
    // The model's first word (vendor prefixes like "WDC WD10…" come first).
    let words: Vec<&str> = model_up.split_whitespace().collect();
    MAKERS.iter().find(|m| {
        m.keywords.iter().any(|k| text.contains(k))
            || words.iter().any(|w| {
                m.prefixes.iter().any(|p| {
                    w.starts_with(p)
                        // "ST" / "CT" / "WD" / "MK" alone are too short:
                        // require a digit right after them.
                        && (p.len() > 2 || w[p.len()..].starts_with(|c: char| c.is_ascii_digit()))
                })
            })
    })
}

/// Model strings that name no manufacturer: `SSD 1TB`, `NVMe SSD 512GB`,
/// `2.5" SATA SSD`, `Generic`.
pub fn is_generic_model(model: &str) -> bool {
    const WORDS: &[&str] = &[
        "SSD", "SATA", "SATAIII", "SATA3", "NVME", "PCIE", "M.2", "M2", "2.5", "2.5\"", "DISK", "DRIVE", "HDD",
        "FLASH", "GENERIC", "SOLID", "STATE", "INCH", "MSATA", "NGFF", "III", "ATA", "SERIES", "INTERNAL", "USB",
        "MASS", "STORAGE", "DEVICE", "GEN3", "GEN4", "X4", "HARD", "MEMORY", "MINI", "PORTABLE",
    ];
    model
        .to_ascii_uppercase()
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-' || c == '/')
        .filter(|w| !w.is_empty())
        .all(|w| {
            WORDS.contains(&w)
                || w.trim_end_matches(['G', 'B', 'T', 'M']).chars().all(|c| c.is_ascii_digit() || c == '.')
        })
}

/// Serials that are not real serials.
pub fn is_placeholder_serial(serial: &str) -> bool {
    let s = serial.trim().to_ascii_uppercase();
    if s.is_empty() {
        return true;
    }
    let first = s.chars().next().unwrap_or('0');
    s.chars().all(|c| c == first)
        || "0123456789ABCDEFGHIJ".starts_with(&s)
        || s.starts_with("0123456789")
        || (s.starts_with("AA0000000000") && s.len() >= 16)
        || s == "DEFAULT STRING"
        || s == "TO BE FILLED BY O.E.M."
}

/// OUI of a WWN (hex digits after the NAA nibble for NAA 5/6; bytes 0–2 of
/// an EUI-64), or `None` when the WWN is absent or all zeros.
fn wwn_oui(wwn: &str, nvme: bool) -> Option<String> {
    let hex: String = wwn.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_lowercase();
    if hex.len() < 7 || hex.chars().all(|c| c == '0') {
        return None;
    }
    // NVMe EUI-64: the OUI is the first 3 bytes; NAA 5/6: after the NAA nibble.
    Some(if nvme { hex[0..6].to_string() } else { hex[1..7].to_string() })
}

fn is_zero_wwn(wwn: &str) -> bool {
    wwn.chars().filter(|c| c.is_ascii_hexdigit()).all(|c| c == '0')
}

/// What identifies the drive (gathered from sysfs, smartctl and ioctl).
#[derive(Debug, Default, Clone)]
pub struct Evidence {
    pub vendor: String,
    pub model: String,
    pub serial: Option<String>,
    pub nvme: bool,
    /// WWN (hex); `Some("000…")` when the drive reports an empty one.
    pub wwn: Option<String>,
    /// SATA drive whose IDENTIFY carries no WWN (libata then builds a
    /// `t10.ATA …` id instead of `naa.…`).
    pub no_wwn: bool,
    pub pci_vendor: Option<u64>,
    pub in_smartctl_database: Option<bool>,
    /// Behind a RAID volume / virtual disk: the identity is the controller's.
    pub virtual_disk: bool,
}

pub fn assess(e: &Evidence) -> Authenticity {
    let mut signals = Vec::new();
    let brand = claimed_brand(&e.vendor, &e.model);
    let generic = brand.is_none() && is_generic_model(&e.model);
    let mut out = Authenticity { level: Level::Unknown, brand: brand.map(|m| m.name), maker: None, wwn: e.wwn.clone(), signals: Vec::new() };
    if e.virtual_disk || (e.model.trim().is_empty() && e.wwn.is_none() && e.pci_vendor.is_none()) {
        return out;
    }

    let mut bad = 0u32;
    let mut strong = false;
    let mut good = false;

    // WWN / OUI (SATA and SAS; NVMe namespaces use an EUI-64 instead).
    let oui = e.wwn.as_deref().and_then(|w| wwn_oui(w, e.nvme));
    let wwn_maker = oui.as_deref().and_then(maker_by_oui);
    match (&e.wwn, &oui) {
        (Some(w), None) if is_zero_wwn(w) && !e.nvme => {
            bad += 1;
            signals.push((Mark::Bad, "WWN is all zeros — no IEEE-registered manufacturer ID".to_string()));
        }
        (None, _) if e.no_wwn => {
            bad += 1;
            signals.push((Mark::Bad, "the drive reports no WWN (every SATA drive from a real maker has one)".to_string()));
        }
        (Some(_), Some(o)) => match (wwn_maker, brand) {
            (Some(m), Some(b)) if m.name == b.name => {
                good = true;
                signals.push((Mark::Good, format!("WWN OUI {o} is registered to {}, matching the model", m.name)));
            }
            (Some(m), Some(b)) => {
                bad += 1;
                strong = true;
                signals.push((Mark::Bad, format!("model says {} but the WWN OUI {o} belongs to {}", b.name, m.name)));
            }
            (Some(m), None) => {
                good = true;
                signals.push((Mark::Good, format!("WWN OUI {o} is registered to {}", m.name)));
            }
            (None, Some(b)) => {
                bad += 1;
                signals.push((Mark::Bad, format!("WWN OUI {o} is not one {} uses", b.name)));
            }
            (None, None) => signals.push((Mark::Info, format!("WWN OUI {o} (maker not in dcheck's table)"))),
        },
        _ => {}
    }
    out.maker = wwn_maker.map(|m| m.name);

    // NVMe controller vendor.
    if let Some(vid) = e.pci_vendor {
        let owner = maker_by_pci(vid);
        let ctrl = owner.map(|m| m.name).or_else(|| controller_vendor(vid));
        let ctrl_text = ctrl.map_or_else(|| format!("PCI vendor {vid:04x}"), |c| format!("{c} ({vid:04x})"));
        match (brand, owner) {
            (Some(b), Some(o)) if o.name == b.name => {
                good = true;
                signals.push((Mark::Good, format!("controller vendor {ctrl_text} matches the model")));
            }
            (Some(b), _) if b.own_controllers => {
                bad += 1;
                strong = true;
                signals.push((Mark::Bad, format!("{} only uses its own controllers, this one is {ctrl_text}", b.name)));
            }
            _ => signals.push((Mark::Info, format!("controller: {ctrl_text}"))),
        }
        if out.maker.is_none() {
            out.maker = owner.map(|m| m.name);
        }
    }

    if generic {
        let mark = if good { Mark::Info } else { Mark::Bad };
        signals.push((mark, format!("model \"{}\" names no manufacturer", e.model.trim())));
    }
    if let Some(s) = &e.serial {
        if is_placeholder_serial(s) {
            bad += 1;
            signals.push((Mark::Bad, format!("placeholder serial number \"{}\"", s.trim())));
        }
    }
    if e.in_smartctl_database == Some(false) && (generic || bad > 0) {
        signals.push((Mark::Info, "not in smartctl's drive database".to_string()));
    }

    out.level = if strong {
        Level::LikelyFake
    } else if generic && !good {
        Level::Unbranded
    } else if bad > 0 {
        Level::Suspicious
    } else if good {
        Level::Consistent
    } else if brand.is_some() || !e.model.trim().is_empty() {
        Level::Unverified
    } else {
        Level::Unknown
    };
    if generic && good {
        signals.push((Mark::Info, "OEM part: the model is generic but the maker ID is real".to_string()));
    }
    out.signals = signals;
    out
}

/// RAID volumes and virtual disks report the controller's identity.
fn is_virtual(vendor: &str, model: &str) -> bool {
    let t = format!("{vendor} {model}").to_ascii_uppercase();
    [
        "PERC", "LOGICAL VOLUME", "VIRTUAL", "MR9", "MEGARAID", "RAID", "QEMU", "VBOX", "VMWARE", "MSFT", "SMARTARRAY",
        "LSI ", "AVAGO", "BROADCOM", "XEN", "RBD", "DRBD",
    ]
    .iter()
    .any(|k| t.contains(k))
}

/// Gather evidence for `d` from sysfs + SMART and assess it.
pub fn for_device(d: &Device, smart: Option<&SmartData>, model: &str, serial: Option<&str>) -> Authenticity {
    let sys = |f: &str| -> Option<String> {
        if crate::enumerate::is_demo() {
            return None;
        }
        std::fs::read_to_string(format!("/sys/block/{}/{f}", d.name))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let nvme = d.name.starts_with("nvme") || d.bus == Bus::Nvme;
    let wwid = sys("device/wwid").or_else(|| sys("wwid"));
    // sysfs: "naa.5002538e10249cb0", "eui.0025385b71b0c1a2", or for a SATA
    // drive without a WWN "t10.ATA     SSD 1TB   <serial>".
    let sys_wwn = wwid.as_deref().and_then(|w| {
        let (kind, rest) = w.split_once('.')?;
        matches!(kind, "naa" | "eui").then(|| rest.trim().to_ascii_lowercase())
    });
    // udev's /dev/disk/by-id/wwn-0x… (it runs its own INQUIRY when the
    // kernel did not keep the VPD page, e.g. some SAS drives).
    let by_id_wwn = || -> Option<String> {
        if crate::enumerate::is_demo() {
            return None;
        }
        std::fs::read_dir("/dev/disk/by-id").ok()?.flatten().find_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let hex = name.strip_prefix("wwn-0x")?;
            let target = std::fs::read_link(e.path()).ok()?;
            (target.file_name()?.to_str()? == d.name).then(|| hex.to_ascii_lowercase())
        })
    };
    let no_wwn = wwid.as_deref().is_some_and(|w| w.starts_with("t10.ATA"));
    // NVMe: the namespace EUI-64 ("ac e4 2e 00 …"); `wwid` may be an NGUID.
    let nvme_eui = sys("eui").map(|e| e.split_whitespace().collect::<String>()).filter(|e| e.len() == 16);
    let pci_vendor = if nvme {
        sys("device/device/vendor")
            .and_then(|v| u64::from_str_radix(v.trim_start_matches("0x"), 16).ok())
            .or_else(|| smart.and_then(|s| s.pci_vendor))
    } else {
        None
    };
    let vendor = d.vendor.clone().unwrap_or_default();
    let e = Evidence {
        virtual_disk: is_virtual(&vendor, model),
        vendor,
        model: model.to_string(),
        serial: serial.map(str::to_string),
        nvme,
        wwn: if nvme {
            nvme_eui.or(sys_wwn.filter(|w| w.len() == 16))
        } else {
            smart.and_then(|s| s.wwn.clone()).or(sys_wwn).or_else(by_id_wwn)
        },
        no_wwn,
        pci_vendor,
        in_smartctl_database: smart.and_then(|s| s.in_smartctl_database),
    };
    // USB bridges and SD cards report the bridge / card reader identity.
    if matches!(d.bus, Bus::Usb | Bus::Mmc | Bus::Virtio) && e.wwn.is_none() {
        let mut a = assess(&Evidence { no_wwn: false, ..e });
        if a.level == Level::Unverified {
            a.level = Level::Unknown;
        }
        return a;
    }
    assess(&e)
}

pub fn to_json(a: &Authenticity) -> Json {
    let opt = |v: Option<&str>| v.map_or(Json::Null, |s| json::string(s.to_string()));
    json::object(vec![
        ("level", json::string(a.level.label().to_ascii_lowercase().replace(' ', "_"))),
        ("summary", json::string(a.summary())),
        ("brand", opt(a.brand)),
        ("maker", opt(a.maker)),
        ("wwn", opt(a.wwn.as_deref())),
        (
            "signals",
            Json::Arr(
                a.signals
                    .iter()
                    .map(|(m, t)| {
                        let mark = match m {
                            Mark::Good => "good",
                            Mark::Info => "info",
                            Mark::Bad => "bad",
                        };
                        json::object(vec![("mark", json::string(mark.to_string())), ("text", json::string(t.clone()))])
                    })
                    .collect(),
            ),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(model: &str, wwn: Option<&str>) -> Evidence {
        Evidence { model: model.into(), serial: Some("S5CNNA0N209858".into()), wwn: wwn.map(str::to_string), ..Default::default() }
    }

    #[test]
    fn genuine_samsung_is_consistent() {
        let a = assess(&ev("MZ7KH480HAHQ0D3", Some("5002538e10249cb0")));
        assert_eq!(a.level, Level::Consistent);
        assert_eq!(a.brand, Some("Samsung"));
        assert_eq!(a.maker, Some("Samsung"));
        let a = assess(&ev("Samsung SSD 870 EVO 1TB", Some("5002538f41234567")));
        assert_eq!(a.level, Level::Consistent);
    }

    #[test]
    fn brand_with_another_makers_oui_is_likely_fake() {
        let a = assess(&ev("Samsung SSD 870 EVO 1TB", Some("50026b7682a1b2c3")));
        assert_eq!(a.level, Level::LikelyFake);
        assert_eq!(a.maker, Some("Kingston"));
        assert!(a.summary().contains("claims Samsung"));
    }

    #[test]
    fn brand_with_zero_or_missing_wwn_is_suspicious() {
        let a = assess(&ev("Samsung SSD 870 EVO 1TB", Some("0000000000000000")));
        assert_eq!(a.level, Level::Suspicious);
        let a = assess(&Evidence { no_wwn: true, ..ev("Samsung SSD 870 EVO 1TB", None) });
        assert_eq!(a.level, Level::Suspicious);
    }

    #[test]
    fn lab_243_generic_ssd_is_unbranded() {
        let a = assess(&Evidence {
            serial: Some("0926270101311015342".into()),
            in_smartctl_database: Some(false),
            no_wwn: true,
            ..ev("SSD 1TB", None)
        });
        assert_eq!(a.level, Level::Unbranded);
        assert!(a.signals.iter().any(|(_, t)| t.contains("no WWN")));
        assert!(a.signals.iter().any(|(_, t)| t.contains("names no manufacturer")));
        assert!(a.signals.iter().any(|(_, t)| t.contains("smartctl")));
    }

    #[test]
    fn samsung_nvme_on_a_foreign_controller_is_likely_fake() {
        let e = Evidence { nvme: true, pci_vendor: Some(0x1e4b), ..ev("Samsung SSD 980 PRO 1TB", None) };
        let a = assess(&e);
        assert_eq!(a.level, Level::LikelyFake);
        assert!(a.signals.iter().any(|(_, t)| t.contains("Maxio")));
        let a = assess(&Evidence { pci_vendor: Some(0x144d), ..e });
        assert_eq!(a.level, Level::Consistent);
        // Brands that buy controllers are not flagged for it.
        let a = assess(&Evidence { nvme: true, pci_vendor: Some(0x1987), ..ev("Seagate FireCuda 530", None) });
        assert_eq!(a.level, Level::Unverified);
    }

    #[test]
    fn sas_and_hdd_families() {
        // Toshiba (ex-Fujitsu) SAS HDD from the fixture.
        let a = assess(&Evidence { vendor: "TOSHIBA".into(), ..ev("MBF2300RC", Some("50000393e822400c")) });
        assert_eq!(a.level, Level::Consistent);
        let a = assess(&ev("WDC WD10SPZX-00Z10T0", Some("50014ee2b5c3d4e5")));
        assert_eq!(a.level, Level::Consistent);
        // HGST drives carry the WD group's OUIs.
        let a = assess(&ev("HGST HUS726T4TALA6L4", Some("5000cca2a1b2c3d4")));
        assert_eq!(a.level, Level::Consistent);
        let a = assess(&ev("ST4000NM0035-1V4107", Some("5000c500a1b2c3d4")));
        assert_eq!(a.brand, Some("Seagate"));
        assert_eq!(a.level, Level::Consistent);
    }

    #[test]
    fn nvme_eui64_oui() {
        // Dell-branded SK hynix PE8110 (melbicom-ded): EUI-64 ace42e0045470185.
        let a = assess(&Evidence { nvme: true, pci_vendor: Some(0x1c5c), ..ev("DELL NVME ISE PE8110 RI U.2 960GB", Some("ace42e0045470185")) });
        assert_eq!(a.maker, Some("SK hynix"));
        assert_eq!(a.level, Level::Consistent);
    }

    #[test]
    fn oem_generic_model_with_real_oui_is_consistent() {
        let a = assess(&ev("SSD 256GB", Some("500a0751e1a2b3c4")));
        assert_eq!(a.level, Level::Consistent);
        assert_eq!(a.maker, Some("Micron/Crucial"));
    }

    #[test]
    fn detects_generic_models_and_placeholder_serials() {
        for m in ["SSD 1TB", "NVMe SSD 512GB", "2.5\" SATA SSD", "Generic", "SATA SSD", "M.2 NVMe SSD 1TB", "256GB SSD"] {
            assert!(is_generic_model(m), "{m}");
        }
        for m in ["KINGSTON SA400S37240G", "SPCC Solid State Disk", "TEAM T253X2256G", "ST1000LM035"] {
            assert!(!is_generic_model(m), "{m}");
        }
        for s in ["", "0000000000", "0123456789ABCDEF", "AA000000000000000123", "FFFFFFFF"] {
            assert!(is_placeholder_serial(s), "{s}");
        }
        assert!(!is_placeholder_serial("S5CNNA0N209858"));
        assert!(!is_placeholder_serial("0926270101311015342"));
    }

    #[test]
    fn short_prefixes_need_a_digit() {
        assert_eq!(claimed_brand("", "ST1000LM035").map(|m| m.name), Some("Seagate"));
        assert!(claimed_brand("", "STORAGE DEVICE").is_none());
        assert_eq!(claimed_brand("", "CT500MX500SSD1").map(|m| m.name), Some("Micron/Crucial"));
        assert!(claimed_brand("ATA", "SSD 1TB").is_none());
    }

    #[test]
    fn raid_volumes_are_not_judged() {
        assert!(is_virtual("DELL", "PERC H730 Mini"));
        assert!(!is_virtual("ATA", "MZ7KH480HAHQ0D3"));
        let a = assess(&Evidence { virtual_disk: true, ..ev("PERC H730 Mini", Some("6b8ca3a0f1e2d3c4")) });
        assert_eq!(a.level, Level::Unknown);
    }
}
