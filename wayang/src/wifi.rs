//! WiFi runtime support for `wayang wifi`.
//!
//! Lists wireless interfaces, parses `iw dev <if> scan`, generates
//! `/data/etc/wpa_supplicant.conf` and connects (regdomain + wpa_supplicant +
//! DHCP + primary). Missing `iw`/`wpa_supplicant` or no wireless NIC yields a
//! readable error instead of a panic.

use std::fs;
use std::path::Path;

use crate::net::{self, NetChoice};
use crate::paths;
use crate::sys;

/// A wireless interface from `/sys/class/net`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiIface {
    pub name: String,
    pub driver: String,
    pub mac: String,
}

/// One scan result (an access point).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bss {
    pub bssid: String,
    pub ssid: String,
    /// dBm; `None` when the scan did not report it.
    pub signal: Option<i32>,
    pub security: String,
}

impl Bss {
    /// Sort key: strongest first, then SSID.
    pub fn signal_or_default(&self) -> i32 {
        self.signal.unwrap_or(i32::MIN)
    }
}

/// Wireless interfaces (`/sys/class/net/*/wireless`), link-up first.
pub fn ifaces() -> Vec<WifiIface> {
    ifaces_at(Path::new("/sys/class/net"))
}

/// Pure form of [`ifaces`] for tests.
pub fn ifaces_at(root: &Path) -> Vec<WifiIface> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<WifiIface> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_string_lossy().into_owned();
            if name == "lo" || !path.join("wireless").exists() {
                return None;
            }
            let mac = read_trim(&path.join("address")).unwrap_or_default();
            let driver = fs::read_link(path.join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "-".into());
            Some(WifiIface { name, driver, mac })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn read_trim(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Run `iw dev <iface> scan` and parse it.
pub fn scan(iface: &str) -> Result<Vec<Bss>, String> {
    if !sys::which("iw") {
        return Err("`iw` is not installed (see docs/NETWORK.md wifi prerequisites)".into());
    }
    let out = sys::run("iw", &["dev", iface, "scan"])
        .map_err(|e| format!("scan on {iface} failed (needs root): {e}"))?;
    Ok(parse_scan(&out))
}

/// Parser scratch state while walking one `BSS` block.
struct Partial {
    bssid: String,
    ssid: String,
    signal: Option<i32>,
    rsn: bool,
    wpa: bool,
    privacy: bool,
}

/// Parse `iw dev <if> scan` output into access points, strongest first.
pub fn parse_scan(text: &str) -> Vec<Bss> {
    let mut out: Vec<Bss> = Vec::new();
    let mut cur: Option<Partial> = None;

    fn flush(cur: &mut Option<Partial>, out: &mut Vec<Bss>) {
        if let Some(p) = cur.take() {
            let security = if p.rsn {
                "WPA2".to_string()
            } else if p.wpa {
                "WPA".to_string()
            } else if p.privacy {
                "WEP".to_string()
            } else {
                "OPEN".to_string()
            };
            out.push(Bss { bssid: p.bssid, ssid: p.ssid, signal: p.signal, security });
        }
    }

    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("BSS ") {
            flush(&mut cur, &mut out);
            let bssid = rest.split(['(', ' ']).next().unwrap_or("").to_string();
            cur = Some(Partial { bssid, ssid: String::new(), signal: None, rsn: false, wpa: false, privacy: false });
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        if let Some(v) = t.strip_prefix("SSID:") {
            c.ssid = v.trim().to_string();
        } else if let Some(v) = t.strip_prefix("signal:") {
            c.signal = v
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<f32>().ok())
                .map(|f| f as i32);
        } else if t.starts_with("RSN:") {
            c.rsn = true;
        } else if t.starts_with("WPA:") {
            c.wpa = true;
        } else if t.starts_with("capability:") && t.contains("Privacy") {
            c.privacy = true;
        }
    }
    flush(&mut cur, &mut out);

    out.sort_by(|a, b| b.signal_or_default().cmp(&a.signal_or_default()).then_with(|| a.ssid.cmp(&b.ssid)));
    out
}

/// `/data/etc/wpa_supplicant.conf` contents for a WPA-PSK network.
pub fn wpa_conf(ssid: &str, psk: &str) -> String {
    format!(
        "network={{\n    ssid=\"{}\"\n    psk=\"{}\"\n    key_mgmt=WPA-PSK\n}}\n",
        escape(ssid),
        escape(psk)
    )
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Basic sanity for an SSID/passphrase pair.
pub fn valid_credentials(ssid: &str, psk: &str) -> Result<(), String> {
    if ssid.trim().is_empty() {
        return Err("pick a network first".into());
    }
    if ssid.contains('\n') {
        return Err("SSID must not contain a newline".into());
    }
    if psk.is_empty() {
        return Err("enter the passphrase (WPA-PSK)".into());
    }
    if psk.len() < 8 && psk.len() != 64 {
        return Err("passphrase must be 8-63 characters (or 64 hex)".into());
    }
    Ok(())
}

/// Persist the wpa_supplicant config and connect: regdomain, wpa_supplicant,
/// DHCP, primary.
pub fn connect(iface: &str, ssid: &str, psk: &str, country: Option<&str>, demo: bool) -> Result<String, String> {
    valid_credentials(ssid, psk)?;
    let country = country.map(str::trim).filter(|c| !c.is_empty());
    if let Some(cc) = country {
        if cc.len() != 2 || !cc.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(format!("country code '{cc}' must be two letters, e.g. GB"));
        }
    }
    if demo {
        return Ok(format!("wifi: connected to {ssid} on {iface} (demo, not applied)"));
    }
    if !sys::which("wpa_supplicant") {
        return Err("`wpa_supplicant` is not installed (see docs/NETWORK.md wifi prerequisites)".into());
    }

    let conf = paths::wpa_conf_file();
    if let Some(parent) = conf.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(&conf, wpa_conf(ssid, psk)).map_err(|e| format!("{}: {e}", conf.display()))?;

    if let Some(cc) = country {
        if sys::which("iw") {
            sys::run("iw", &["reg", "set", cc]).map_err(|e| format!("iw reg set {cc}: {e}"))?;
        }
    }

    sys::run("ip", &["link", "set", iface, "up"])
        .map_err(|e| format!("cannot bring {iface} up: {e}"))?;
    sys::run("wpa_supplicant", &["-B", "-i", iface, "-c", &conf.to_string_lossy()])
        .map_err(|e| format!("wpa_supplicant failed on {iface}: {e}"))?;
    sys::run(
        "udhcpc",
        &["-n", "-q", "-t", "5", "-T", "3", "-i", iface, "-s", "/etc/udhcpc.script"],
    )
    .map_err(|e| format!("no DHCP lease on {iface}: {e}"))?;

    // pin the wifi interface; boot uses MODE=dhcp on it
    NetChoice { iface: Some(iface.to_string()), mode: net::Mode::Dhcp, ..Default::default() }
        .write_persisted()?;
    Ok(format!("wifi: {ssid} on {iface} (dhcp)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SAMPLE: &str = "\
BSS 00:11:22:33:44:55(on wlan0)
\tfreq: 2412
\tsignal: -45.00 dBm
\tcapability: ESS Privacy ShortSlotTime (0x0411)
\tSSID: CoffeeShop
\tRSN:\t* Version: 1
\t\t* Group cipher: CCMP

BSS 66:77:88:99:aa:bb(on wlan0)
\tsignal: -72.30 dBm
\tcapability: ESS (0x0401)
\tSSID: OpenCafe

BSS aa:bb:cc:dd:ee:ff(on wlan0)
\tsignal: -60.10 dBm
\tcapability: ESS Privacy (0x0411)
\tSSID: OldRouter
\tWPA:\t* Version: 1
";

    fn tmpdir() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "wayang-wifi-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_scan_strongest_first() {
        let bss = parse_scan(SAMPLE);
        assert_eq!(bss.len(), 3);
        assert_eq!(bss[0].ssid, "CoffeeShop");
        assert_eq!(bss[0].signal, Some(-45));
        assert_eq!(bss[0].security, "WPA2");
        assert_eq!(bss[0].bssid, "00:11:22:33:44:55");
        assert_eq!(bss[1].ssid, "OldRouter");
        assert_eq!(bss[1].security, "WPA");
        assert_eq!(bss[2].ssid, "OpenCafe");
        assert_eq!(bss[2].security, "OPEN");
    }

    #[test]
    fn parses_empty_scan() {
        assert!(parse_scan("").is_empty());
    }

    #[test]
    fn wpa_conf_shape() {
        assert_eq!(
            wpa_conf("CoffeeShop", "hunter2hunter2"),
            "network={\n    ssid=\"CoffeeShop\"\n    psk=\"hunter2hunter2\"\n    key_mgmt=WPA-PSK\n}\n"
        );
    }

    #[test]
    fn wpa_conf_escapes_quotes() {
        let c = wpa_conf("a\"b", "p\\q12345678");
        assert!(c.contains(r#"ssid="a\"b""#));
        assert!(c.contains(r#"psk="p\\q12345678""#));
    }

    #[test]
    fn credentials_validate() {
        assert!(valid_credentials("net", "12345678").is_ok());
        assert!(valid_credentials("", "12345678").is_err());
        assert!(valid_credentials("net", "short").is_err());
        assert!(valid_credentials("net", "").is_err());
        let hex64 = "a".repeat(64);
        assert!(valid_credentials("net", &hex64).is_ok());
    }

    #[test]
    fn lists_wireless_ifaces_only() {
        let root = tmpdir().join("class/net");
        for (name, wireless, driver) in [("wlan0", true, "rtl8xxxu"), ("eth0", false, "e1000e")] {
            let d = root.join(name);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("address"), "00:11:22:33:44:55\n").unwrap();
            if wireless {
                fs::create_dir_all(d.join("wireless")).unwrap();
                fs::create_dir_all(d.join("device")).unwrap();
                #[cfg(unix)]
                std::os::unix::fs::symlink(format!("/sys/bus/usb/drivers/{driver}"), d.join("device/driver")).unwrap();
            }
        }
        let w = ifaces_at(&root);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].name, "wlan0");
        assert_eq!(w[0].driver, "rtl8xxxu");
        let _ = fs::remove_dir_all(root.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn missing_wireless_root_is_empty() {
        assert!(ifaces_at(Path::new("/nonexistent-wayang-wifi")).is_empty());
    }
}
