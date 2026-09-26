//! The Network step: pick a wired uplink and its IP configuration, apply it to
//! the running installer (so key fetching works) and persist it to the target
//! `/data` for the installed system.
//!
//! File formats follow `docs/NETWORK.md` (frozen):
//!
//! ```text
//! /data/etc/network/primary     # interface name
//! /data/etc/network/config      # MODE=... FAMILY=... IPV4_... (sh snippet)
//! ```

use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::process::{Command, Stdio};

use crate::sys;

/// Paths on the target `/data`, relative to its mount point.
pub const PRIMARY_REL: &str = "etc/network/primary";
pub const CONFIG_REL: &str = "etc/network/config";

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

/// A wired interface as read from sysfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub link: bool,
    pub driver: String,
    pub mac: String,
}

/// The chosen uplink and IP configuration. `iface == None` means auto (leave
/// the installed system probing every wired NIC; nothing is persisted).
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

    /// `/data/etc/network/config` contents. A POSIX `sh` snippet, no spaces
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

    /// Bring the choice up in the running installer, mirroring `wayang-net set`
    /// (DHCP via `udhcpc`, static via `ip addr add` + default route + DNS).
    /// Auto is a no-op: the installer's init already probes the NICs.
    pub fn apply_live(&self, demo: bool) -> Result<String, String> {
        let Some(iface) = self.pinned() else {
            return Ok("network: auto - left as-is".into());
        };
        if demo {
            return Ok(format!("network: {} (demo)", self.summary()));
        }

        // a new primary takes over the default route and DNS
        let _ = sys::run("killall", &["udhcpc"]);
        let _ = sys::run("ip", &["route", "del", "default"]);
        let _ = sys::run("route", &["del", "default"]);
        let _ = fs::create_dir_all("/var/run");
        let _ = fs::write("/var/run/wayang-primary", format!("{iface}\n"));

        sys::run("ip", &["link", "set", iface, "up"])
            .map_err(|e| format!("cannot bring {iface} up: {e}"))?;

        match self.mode {
            Mode::Dhcp => {
                sys::run(
                    "udhcpc",
                    &[
                        "-n",
                        "-q",
                        "-t",
                        "5",
                        "-T",
                        "3",
                        "-i",
                        iface,
                        "-s",
                        "/etc/udhcpc.script",
                    ],
                )
                .map_err(|e| format!("no DHCP lease on {iface}: {e}"))?;
                // keep renewing while the installer runs
                let _ = Command::new("udhcpc")
                    .args(["-b", "-i", iface, "-s", "/etc/udhcpc.script"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                Ok(format!("network: {iface} dhcp"))
            }
            Mode::Static => {
                if self.family.has_v4() {
                    if self.ipv4_address.is_empty() {
                        return Err("IPv4 address is empty".into());
                    }
                    sys::run("ip", &["addr", "add", &self.ipv4_address, "dev", iface])
                        .map_err(|e| format!("{iface}: {e}"))?;
                    if !self.ipv4_gateway.is_empty() {
                        sys::run(
                            "ip",
                            &[
                                "route",
                                "add",
                                "default",
                                "via",
                                &self.ipv4_gateway,
                                "dev",
                                iface,
                            ],
                        )
                        .map_err(|e| format!("{iface}: {e}"))?;
                    }
                }
                if self.family.has_v6() {
                    if self.ipv6_address.is_empty() {
                        return Err("IPv6 address is empty".into());
                    }
                    sys::run(
                        "ip",
                        &["-6", "addr", "add", &self.ipv6_address, "dev", iface],
                    )
                    .map_err(|e| format!("{iface}: {e}"))?;
                    if !self.ipv6_gateway.is_empty() {
                        sys::run(
                            "ip",
                            &[
                                "-6",
                                "route",
                                "add",
                                "default",
                                "via",
                                &self.ipv6_gateway,
                                "dev",
                                iface,
                            ],
                        )
                        .map_err(|e| format!("{iface}: {e}"))?;
                    }
                }
                write_resolv_conf(
                    if self.family.has_v4() {
                        &self.ipv4_dns
                    } else {
                        ""
                    },
                    if self.family.has_v6() {
                        &self.ipv6_dns
                    } else {
                        ""
                    },
                )?;
                Ok(format!("network: {iface} static {}", self.family.config()))
            }
        }
    }
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
    fs::write("/etc/resolv.conf", text).map_err(|e| format!("/etc/resolv.conf: {e}"))
}

// ---- validation --------------------------------------------------------

/// `ADDRESS/PREFIX`, e.g. `192.168.1.50/24` or `2001:db8::50/64`.
pub fn valid_cidr(s: &str, v6: bool) -> Result<(), String> {
    let s = s.trim();
    let (addr, prefix) = s
        .split_once('/')
        .ok_or_else(|| "add a prefix, e.g. 192.168.1.50/24".to_string())?;
    let p: u32 = prefix
        .parse()
        .map_err(|_| format!("prefix '{prefix}' is not a number"))?;
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
        s.parse::<Ipv6Addr>()
            .map(|_| ())
            .map_err(|_| format!("'{s}' is not an IPv6 address"))
    } else {
        s.parse::<Ipv4Addr>()
            .map(|_| ())
            .map_err(|_| format!("'{s}' is not an IPv4 address"))
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

/// Wired interfaces from `/sys/class/net`, link-up first. Skips `lo`,
/// wireless and virtual interfaces (no `device` symlink), matching the boot
/// script's `wired()`.
pub fn scan(demo: bool) -> Vec<Iface> {
    if demo {
        return demo_ifaces();
    }
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut out: Vec<Iface> = entries
        .flatten()
        .filter_map(|e| read_iface(&e.path()))
        .collect();
    out.sort_by(|a, b| b.link.cmp(&a.link).then_with(|| a.name.cmp(&b.name)));
    out
}

fn read_iface(path: &std::path::Path) -> Option<Iface> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if name == "lo" || path.join("wireless").exists() || path.join("phy80211").exists() {
        return None;
    }
    // physical NICs expose their driver under device/; bridges, VLANs and
    // tunnels do not, and are not candidates for the uplink
    if !path.join("device").exists() {
        return None;
    }
    let link = read_trim(&path.join("carrier")).as_deref() == Some("1");
    let mac = read_trim(&path.join("address")).unwrap_or_default();
    let driver = fs::read_link(path.join("device/driver"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "-".into());
    Some(Iface {
        name,
        link,
        driver,
        mac,
    })
}

fn read_trim(path: &std::path::Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn demo_ifaces() -> Vec<Iface> {
    vec![
        Iface {
            name: "eth0".into(),
            link: true,
            driver: "e1000e".into(),
            mac: "52:54:00:12:34:56".into(),
        },
        Iface {
            name: "enp0s20u1".into(),
            link: false,
            driver: "r8152".into(),
            mac: "00:e0:4c:68:01:23".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn default_is_auto_and_writes_nothing() {
        let n = NetChoice::default();
        assert_eq!(n.pinned(), None);
        assert!(n.primary_text().is_empty());
        // auto mode: nothing persisted even though config_text is never called
        assert_eq!(n.mode, Mode::Dhcp);
    }

    #[test]
    fn dhcp_config_has_mode_only() {
        let n = NetChoice {
            iface: Some("eth0".into()),
            ..Default::default()
        };
        assert_eq!(n.primary_text(), "eth0\n");
        assert_eq!(n.config_text(), "MODE=dhcp\n");
    }

    #[test]
    fn static_both_config_matches_spec() {
        let n = static_choice();
        let text = n.config_text();
        assert_eq!(
            text,
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
}
