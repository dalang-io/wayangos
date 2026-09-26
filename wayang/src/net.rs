//! Runtime network configuration for `wayang net`.
//!
//! Reads interfaces from `/sys/class/net`, formats the persisted files exactly
//! as frozen in `docs/NETWORK.md` and applies the choice immediately so a
//! running box can be reconfigured without hand-editing files.
//!
//! ```text
//! /data/etc/network/primary     # interface name
//! /data/etc/network/config      # MODE=... FAMILY=... IPV4_... (sh snippet)
//! ```

use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;

use crate::paths;
use crate::sys;

/// How the uplink gets its address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Dhcp,
    Static,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Dhcp => "DHCP",
            Mode::Static => "STATIC",
        }
    }

    pub fn config(self) -> &'static str {
        match self {
            Mode::Dhcp => "dhcp",
            Mode::Static => "static",
        }
    }
}

/// Which families a static configuration covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Family {
    #[default]
    Ipv4,
    Ipv6,
    Both,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Ipv4 => "IPv4",
            Family::Ipv6 => "IPv6",
            Family::Both => "BOTH",
        }
    }

    pub fn config(self) -> &'static str {
        match self {
            Family::Ipv4 => "ipv4",
            Family::Ipv6 => "ipv6",
            Family::Both => "both",
        }
    }

    pub fn has_v4(self) -> bool {
        matches!(self, Family::Ipv4 | Family::Both)
    }

    pub fn has_v6(self) -> bool {
        matches!(self, Family::Ipv6 | Family::Both)
    }

    pub fn next(self) -> Family {
        match self {
            Family::Ipv4 => Family::Ipv6,
            Family::Ipv6 => Family::Both,
            Family::Both => Family::Ipv4,
        }
    }
}

/// An interface as read from sysfs, with the runtime bits the HUD shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub link: bool,
    pub driver: String,
    pub mac: String,
    pub wireless: bool,
    pub primary: bool,
    pub ipv4: Vec<String>,
}

/// The chosen uplink and IP configuration. `iface == None` means auto (nothing
/// persisted; the boot script probes every wired NIC).
#[derive(Debug, Clone, Default)]
pub struct NetChoice {
    pub iface: Option<String>,
    pub mode: Mode,
    pub family: Family,
    pub ipv4_address: String,
    pub ipv4_gateway: String,
    pub ipv4_dns: String,
    pub ipv6_address: String,
    pub ipv6_gateway: String,
    pub ipv6_dns: String,
}

impl NetChoice {
    /// The pinned interface, or `None` in auto mode.
    pub fn pinned(&self) -> Option<&str> {
        self.iface.as_deref()
    }

    /// `/data/etc/network/primary` contents (interface name + newline).
    pub fn primary_text(&self) -> String {
        match self.pinned() {
            Some(i) => format!("{i}\n"),
            None => String::new(),
        }
    }

    /// `/data/etc/network/config` contents: a POSIX `sh` snippet, no spaces
    /// around `=`, DNS lists quoted. Missing keys are ignored by the boot
    /// script, so empty values are simply omitted.
    pub fn config_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("MODE={}\n", self.mode.config()));
        if self.mode == Mode::Static {
            s.push_str(&format!("FAMILY={}\n", self.family.config()));
            if self.family.has_v4() {
                push_kv(&mut s, "IPV4_ADDRESS", &self.ipv4_address);
                push_kv(&mut s, "IPV4_GATEWAY", &self.ipv4_gateway);
                push_quoted(&mut s, "IPV4_DNS", &self.ipv4_dns);
            }
            if self.family.has_v6() {
                push_kv(&mut s, "IPV6_ADDRESS", &self.ipv6_address);
                push_kv(&mut s, "IPV6_GATEWAY", &self.ipv6_gateway);
                push_quoted(&mut s, "IPV6_DNS", &self.ipv6_dns);
            }
        }
        s
    }

    /// One-line description for the notice/log.
    pub fn summary(&self) -> String {
        match self.pinned() {
            None => "auto (probe every wired NIC)".into(),
            Some(i) if self.mode == Mode::Dhcp => format!("{i} dhcp"),
            Some(i) => format!("{i} static {}", self.family.config()),
        }
    }

    /// Static needs an address/prefix for every configured family.
    pub fn validate(&self) -> Result<(), String> {
        if self.mode != Mode::Static {
            return Ok(());
        }
        if self.family.has_v4() {
            if self.ipv4_address.is_empty() {
                return Err("set an IPv4 address/prefix, or use DHCP".into());
            }
            valid_cidr(&self.ipv4_address, false)?;
        }
        if self.family.has_v6() {
            if self.ipv6_address.is_empty() {
                return Err("set an IPv6 address/prefix, or use DHCP".into());
            }
            valid_cidr(&self.ipv6_address, true)?;
        }
        Ok(())
    }

    /// Persist `primary` + `config` under `/data/etc/network`.
    pub fn write_persisted(&self) -> Result<(), String> {
        if self.pinned().is_none() {
            // auto: forget any pinned choice
            let _ = fs::remove_file(paths::primary_file());
            let _ = fs::remove_file(paths::network_config_file());
            return Ok(());
        }
        let dir = paths::data_network_dir();
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let primary = paths::primary_file();
        fs::write(&primary, self.primary_text()).map_err(|e| format!("{}: {e}", primary.display()))?;
        let config = paths::network_config_file();
        fs::write(&config, self.config_text()).map_err(|e| format!("{}: {e}", config.display()))
    }

    /// Persist, then bring the choice up in the running system, mirroring the
    /// boot script: DHCP via `udhcpc`, static via `ip addr add` + default route
    /// + `/etc/resolv.conf`.
    pub fn apply(&self, demo: bool) -> Result<String, String> {
        self.validate()?;
        let Some(iface) = self.pinned() else {
            return Err("pick an interface to apply".into());
        };
        if demo {
            return Ok(format!("network: {} (demo, not applied)", self.summary()));
        }
        self.write_persisted()?;

        // a new primary takes over the default route and DNS
        let _ = sys::run("killall", &["udhcpc"]);
        let _ = sys::run("ip", &["route", "del", "default"]);
        let _ = sys::run("route", &["del", "default"]);
        let run = paths::run_dir();
        let _ = fs::create_dir_all(&run);
        let _ = fs::write(run.join("wayang-primary"), format!("{iface}\n"));

        sys::run("ip", &["link", "set", iface, "up"])
            .map_err(|e| format!("cannot bring {iface} up: {e}"))?;

        match self.mode {
            Mode::Dhcp => {
                sys::run(
                    "udhcpc",
                    &["-n", "-q", "-t", "5", "-T", "3", "-i", iface, "-s", "/etc/udhcpc.script"],
                )
                .map_err(|e| format!("no DHCP lease on {iface}: {e}"))?;
                // keep renewing while the box runs
                let _ = sys::spawn("udhcpc", &["-b", "-i", iface, "-s", "/etc/udhcpc.script"]);
                Ok(format!("network: {iface} dhcp"))
            }
            Mode::Static => {
                if self.family.has_v4() {
                    sys::run("ip", &["addr", "add", &self.ipv4_address, "dev", iface])
                        .map_err(|e| format!("{iface}: {e}"))?;
                    if !self.ipv4_gateway.is_empty() {
                        sys::run(
                            "ip",
                            &["route", "add", "default", "via", &self.ipv4_gateway, "dev", iface],
                        )
                        .map_err(|e| format!("{iface}: {e}"))?;
                    }
                }
                if self.family.has_v6() {
                    sys::run("ip", &["-6", "addr", "add", &self.ipv6_address, "dev", iface])
                        .map_err(|e| format!("{iface}: {e}"))?;
                    if !self.ipv6_gateway.is_empty() {
                        sys::run(
                            "ip",
                            &["-6", "route", "add", "default", "via", &self.ipv6_gateway, "dev", iface],
                        )
                        .map_err(|e| format!("{iface}: {e}"))?;
                    }
                }
                write_resolv_conf(
                    if self.family.has_v4() { &self.ipv4_dns } else { "" },
                    if self.family.has_v6() { &self.ipv6_dns } else { "" },
                )?;
                Ok(format!("network: {iface} static {}", self.family.config()))
            }
        }
    }
}

/// Bring `iface` up or down (link only; no address change).
pub fn set_link(iface: &str, up: bool) -> Result<String, String> {
    if iface.trim().is_empty() {
        return Err("pick an interface".into());
    }
    let state = if up { "up" } else { "down" };
    sys::run("ip", &["link", "set", iface, state])
        .or_else(|_| sys::run("ifconfig", &[iface, state]))
        .map_err(|e| format!("cannot set {iface} {state}: {e}"))?;
    Ok(format!("{iface}: link {state}"))
}

/// Obtain a DHCP lease on `iface` **without** making it the primary uplink.
///
/// `udhcpc.script` only installs the default route + DNS when the interface
/// matches `/var/run/wayang-primary`, so a secondary NIC just gets an address.
/// The lease is renewed in the background. Handy for multi-NIC testing.
pub fn dhcp_now(iface: &str, demo: bool) -> Result<String, String> {
    if iface.trim().is_empty() {
        return Err("pick an interface".into());
    }
    if demo {
        return Ok(format!("{iface}: dhcp (demo, not applied)"));
    }
    // Keep the current primary pinned so this lease can't take over the route.
    let run = paths::run_dir();
    let _ = fs::create_dir_all(&run);
    let runf = run.join("wayang-primary");
    let empty = fs::read_to_string(&runf).map(|s| s.trim().is_empty()).unwrap_or(true);
    if empty {
        if let Some(p) = primary_iface() {
            let _ = fs::write(&runf, format!("{p}\n"));
        }
    }
    sys::run("ip", &["link", "set", iface, "up"])
        .map_err(|e| format!("cannot bring {iface} up: {e}"))?;
    sys::run("udhcpc", &["-n", "-q", "-t", "5", "-T", "3", "-i", iface, "-s", "/etc/udhcpc.script"])
        .map_err(|e| format!("no DHCP lease on {iface}: {e}"))?;
    let _ = sys::spawn("udhcpc", &["-b", "-i", iface, "-s", "/etc/udhcpc.script"]);
    Ok(format!("{iface}: DHCP lease (address only; primary unchanged)"))
}

fn push_kv(out: &mut String, key: &str, value: &str) {
    if !value.is_empty() {
        out.push_str(&format!("{key}={value}\n"));
    }
}

fn push_quoted(out: &mut String, key: &str, value: &str) {
    if !value.is_empty() {
        out.push_str(&format!("{key}=\"{value}\"\n"));
    }
}

fn write_resolv_conf(v4: &str, v6: &str) -> Result<(), String> {
    let mut text = String::new();
    for ns in v4.split_whitespace().chain(v6.split_whitespace()) {
        text.push_str(&format!("nameserver {ns}\n"));
    }
    let path = paths::resolv_conf();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

// ---- validation --------------------------------------------------------

/// `ADDRESS/PREFIX`, e.g. `192.168.1.50/24` or `2001:db8::50/64`.
pub fn valid_cidr(s: &str, v6: bool) -> Result<(), String> {
    let s = s.trim();
    let (addr, prefix) = s
        .split_once('/')
        .ok_or_else(|| "add a prefix, e.g. 192.168.1.50/24".to_string())?;
    let p: u32 = prefix.parse().map_err(|_| format!("prefix '{prefix}' is not a number"))?;
    if v6 {
        addr.parse::<Ipv6Addr>()
            .map_err(|_| format!("'{addr}' is not an IPv6 address"))?;
        if p > 128 {
            return Err("IPv6 prefix must be 0-128".into());
        }
    } else {
        addr.parse::<Ipv4Addr>()
            .map_err(|_| format!("'{addr}' is not an IPv4 address"))?;
        if p > 32 {
            return Err("IPv4 prefix must be 0-32".into());
        }
    }
    Ok(())
}

/// A bare address with no prefix.
pub fn valid_ip(s: &str, v6: bool) -> Result<(), String> {
    let s = s.trim();
    if v6 {
        s.parse::<Ipv6Addr>().map(|_| ()).map_err(|_| format!("'{s}' is not an IPv6 address"))
    } else {
        s.parse::<Ipv4Addr>().map(|_| ()).map_err(|_| format!("'{s}' is not an IPv4 address"))
    }
}

/// A whitespace-separated list of addresses; empty is allowed.
pub fn valid_dns(s: &str, v6: bool) -> Result<(), String> {
    for ns in s.split_whitespace() {
        valid_ip(ns, v6)?;
    }
    Ok(())
}

// ---- interface discovery ----------------------------------------------

/// Interfaces from `/sys/class/net`, primary first, then link-up. `lo` is
/// skipped; wireless interfaces are kept and flagged. IPv4 addresses are read
/// from `ip`.
pub fn list(primary: Option<&str>) -> Vec<Iface> {
    scan_at(Path::new("/sys/class/net"), primary, &read_ipv4)
}

/// Pure form of [`list`] with an explicit sysfs root and address reader, so
/// the listing can be unit-tested against a fake sysfs.
pub fn scan_at(root: &Path, primary: Option<&str>, read_ip: &dyn Fn(&str) -> Vec<String>) -> Vec<Iface> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<Iface> = entries
        .flatten()
        .filter_map(|e| read_iface(&e.path(), primary, read_ip))
        .collect();
    out.sort_by(|a, b| {
        b.primary
            .cmp(&a.primary)
            .then_with(|| b.link.cmp(&a.link))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn read_iface(
    path: &Path,
    primary: Option<&str>,
    read_ip: &dyn Fn(&str) -> Vec<String>,
) -> Option<Iface> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if name == "lo" {
        return None;
    }
    let wireless = path.join("wireless").exists() || path.join("phy80211").exists();
    // keep physical NICs and wireless; bridges, VLANs and tunnels have no
    // `device` symlink and are not uplink candidates
    if !path.join("device").exists() && !wireless {
        return None;
    }
    // Wired: link is the carrier. Wireless: carrier only appears once the
    // interface is associated, so report the administrative state (IFF_UP)
    // instead — an unassociated but enabled wlan0 is "up", not "down".
    let carrier = read_trim(&path.join("carrier")).as_deref() == Some("1");
    let up = read_trim(&path.join("flags"))
        .and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .map(|f| f & 0x1 != 0)
        .unwrap_or(false);
    let link = if wireless { up } else { carrier };
    let mac = read_trim(&path.join("address")).unwrap_or_default();
    let driver = fs::read_link(path.join("device/driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "-".into());
    Some(Iface {
        name: name.clone(),
        link,
        driver,
        mac,
        wireless,
        primary: primary == Some(name.as_str()),
        ipv4: read_ip(&name),
    })
}

fn read_trim(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// `ip -o -4 addr show dev <name>` → `inet 192.168.1.50/24`.
fn read_ipv4(name: &str) -> Vec<String> {
    let Ok(out) = sys::run("ip", &["-o", "-4", "addr", "show", "dev", name]) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            let i = w.iter().position(|x| *x == "inet")?;
            w.get(i + 1).map(|s| s.to_string())
        })
        .collect()
}

/// The currently pinned interface, if any.
pub fn primary_iface() -> Option<String> {
    let s = fs::read_to_string(paths::primary_file()).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn static_choice() -> NetChoice {
        NetChoice {
            iface: Some("eth0".into()),
            mode: Mode::Static,
            family: Family::Both,
            ipv4_address: "192.168.1.50/24".into(),
            ipv4_gateway: "192.168.1.1".into(),
            ipv4_dns: "1.1.1.1 8.8.8.8".into(),
            ipv6_address: "2001:db8::50/64".into(),
            ipv6_gateway: "2001:db8::1".into(),
            ipv6_dns: "2001:4860:4860::8888".into(),
        }
    }

    fn tmpdir() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "wayang-net-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn dhcp_config_has_mode_only() {
        let n = NetChoice { iface: Some("eth0".into()), ..Default::default() };
        assert_eq!(n.primary_text(), "eth0\n");
        assert_eq!(n.config_text(), "MODE=dhcp\n");
    }

    #[test]
    fn static_both_config_matches_spec() {
        let n = static_choice();
        assert_eq!(
            n.config_text(),
            "MODE=static\n\
             FAMILY=both\n\
             IPV4_ADDRESS=192.168.1.50/24\n\
             IPV4_GATEWAY=192.168.1.1\n\
             IPV4_DNS=\"1.1.1.1 8.8.8.8\"\n\
             IPV6_ADDRESS=2001:db8::50/64\n\
             IPV6_GATEWAY=2001:db8::1\n\
             IPV6_DNS=\"2001:4860:4860::8888\"\n"
        );
        assert_eq!(n.primary_text(), "eth0\n");
    }

    #[test]
    fn static_v4_only_omits_ipv6_and_empty_values() {
        let mut n = static_choice();
        n.family = Family::Ipv4;
        n.ipv4_dns.clear();
        let text = n.config_text();
        assert!(text.contains("FAMILY=ipv4\n"));
        assert!(text.contains("IPV4_ADDRESS=192.168.1.50/24\n"));
        assert!(!text.contains("IPV6_"));
        assert!(!text.contains("IPV4_DNS"));
    }

    #[test]
    fn cidr_requires_a_prefix() {
        assert!(valid_cidr("192.168.1.50/24", false).is_ok());
        assert!(valid_cidr("2001:db8::50/64", true).is_ok());
        assert!(valid_cidr("192.168.1.50", false).is_err());
        assert!(valid_cidr("192.168.1.50/33", false).is_err());
        assert!(valid_cidr("2001:db8::50/129", true).is_err());
        assert!(valid_cidr("not-an-ip/24", false).is_err());
    }

    #[test]
    fn validate_needs_addresses_for_selected_families() {
        let mut n = static_choice();
        n.family = Family::Ipv6;
        n.ipv6_address.clear();
        assert!(n.validate().is_err());
        n.ipv6_address = "2001:db8::50/64".into();
        assert!(n.validate().is_ok());
        // DHCP ignores fields entirely
        n.mode = Mode::Dhcp;
        n.ipv6_address.clear();
        assert!(n.validate().is_ok());
    }

    #[test]
    fn addresses_and_dns_validate() {
        assert!(valid_ip("192.168.1.1", false).is_ok());
        assert!(valid_ip("2001:db8::1", true).is_ok());
        assert!(valid_ip("192.168.1.1", true).is_err());
        assert!(valid_ip("2001:db8::1", false).is_err());
        assert!(valid_dns("1.1.1.1 8.8.8.8", false).is_ok());
        assert!(valid_dns("", false).is_ok());
        assert!(valid_dns("1.1.1.1 nope", false).is_err());
    }

    #[test]
    fn family_helpers() {
        assert!(Family::Both.has_v4() && Family::Both.has_v6());
        assert!(!Family::Ipv4.has_v6());
        assert_eq!(Family::Both.next(), Family::Ipv4);
    }

    /// Build a fake `/sys/class/net` tree with one wired and one wireless NIC.
    fn fake_sysfs() -> std::path::PathBuf {
        let root = tmpdir().join("class/net");
        for (name, carrier, driver, wireless) in [
            ("eth0", true, "e1000e", false),
            ("wlan0", false, "rtl8xxxu", true),
            ("lo", true, "", false),
            ("docker0", true, "", false),
        ] {
            let d = root.join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("carrier"), if carrier { "1\n" } else { "0\n" }).unwrap();
            // IFF_UP set (0x1) so wireless reports up regardless of carrier
            fs::write(d.join("flags"), "0x1003\n").unwrap();
            fs::write(d.join("address"), format!("52:54:00:00:00:{:02x}\n", name.len())).unwrap();
            if wireless {
                fs::create_dir_all(d.join("wireless")).unwrap();
            }
            if !driver.is_empty() {
                fs::create_dir_all(d.join("device")).unwrap();
                let link = d.join("device/driver");
                #[cfg(unix)]
                std::os::unix::fs::symlink(format!("/sys/bus/pci/drivers/{driver}"), &link).unwrap();
            }
        }
        root
    }

    #[test]
    fn lists_wired_and_wireless_with_primary_flag() {
        let root = fake_sysfs();
        let ifaces = scan_at(&root, Some("wlan0"), &|name| {
            if name == "eth0" {
                vec!["192.168.1.50/24".into()]
            } else {
                Vec::new()
            }
        });
        let names: Vec<&str> = ifaces.iter().map(|i| i.name.as_str()).collect();
        // lo and docker0 are skipped
        assert_eq!(names, vec!["wlan0", "eth0"]);
        let wlan = &ifaces[0];
        assert!(wlan.wireless);
        assert!(wlan.primary);
        assert!(wlan.link, "enabled wireless reports up even without carrier");
        assert_eq!(wlan.driver, "rtl8xxxu");
        let eth = &ifaces[1];
        assert!(!eth.wireless);
        assert!(!eth.primary);
        assert!(eth.link);
        assert_eq!(eth.driver, "e1000e");
        assert_eq!(eth.ipv4, vec!["192.168.1.50/24".to_string()]);
        let _ = fs::remove_dir_all(root.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn scan_at_missing_root_is_empty() {
        assert!(scan_at(Path::new("/nonexistent-wayang-net"), None, &|_| Vec::new()).is_empty());
    }
}
