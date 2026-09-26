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

/// Live association state of a wireless interface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkStatus {
    pub connected: bool,
    pub ssid: String,
    pub bssid: String,
    pub signal: Option<i32>,
}

impl LinkStatus {
    /// Short human summary: the SSID (with signal) or "not connected".
    pub fn summary(&self) -> String {
        if self.connected {
            let ssid = if self.ssid.is_empty() { "<hidden>" } else { &self.ssid };
            match self.signal {
                Some(s) => format!("connected: {ssid} ({s} dBm)"),
                None => format!("connected: {ssid}"),
            }
        } else if !self.ssid.is_empty() {
            format!("not connected (saved: {})", self.ssid)
        } else {
            "not connected".into()
        }
    }
}

/// Parse `iw dev <if> link` output.
pub fn parse_link(text: &str) -> LinkStatus {
    let mut st = LinkStatus::default();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Connected to ") {
            st.connected = true;
            st.bssid = rest.split_whitespace().next().unwrap_or("").to_string();
        } else if let Some(rest) = l.strip_prefix("SSID:") {
            st.ssid = rest.trim().to_string();
        } else if let Some(rest) = l.strip_prefix("signal:") {
            st.signal = rest.split_whitespace().next().and_then(|s| s.parse().ok());
        }
    }
    st
}

/// Current association of `iface` (`iw dev <if> link`), falling back to the
/// persisted SSID so a configured-but-not-yet-associated box still shows it.
pub fn link_status(iface: &str) -> LinkStatus {
    let mut st = LinkStatus::default();
    if sys::which("iw") {
        if let Ok(out) = sys::run("iw", &["dev", iface, "link"]) {
            st = parse_link(&out);
        }
    }
    if !st.connected {
        if let Some(s) = configured_ssid() {
            st.ssid = s;
        }
    }
    st
}

/// SSID from the persisted `/data/etc/wpa_supplicant.conf`, if any.
pub fn configured_ssid() -> Option<String> {
    let text = fs::read_to_string(paths::wpa_conf_file()).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("ssid=") {
            let s = rest.trim().trim_matches('"').to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// Bring a wireless interface up (and unblock rfkill) before using it. `iw
/// scan` fails with "Network is down" on a down interface, and the boot network
/// init only raises wired NICs.
fn bring_up(iface: &str) {
    if sys::which("rfkill") {
        let _ = sys::run("rfkill", &["unblock", "wifi"]);
    }
    if sys::which("ip") {
        let _ = sys::run("ip", &["link", "set", iface, "up"]);
    } else if sys::which("ifconfig") {
        let _ = sys::run("ifconfig", &[iface, "up"]);
    }
}

/// Run `iw dev <iface> scan` and parse it.
pub fn scan(iface: &str) -> Result<Vec<Bss>, String> {
    if !sys::which("iw") {
        return Err("`iw` is not installed (see docs/NETWORK.md wifi prerequisites)".into());
    }
    bring_up(iface);
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

    if sys::which("rfkill") {
        let _ = sys::run("rfkill", &["unblock", "wifi"]);
    }
    sys::run("ip", &["link", "set", iface, "up"])
        .map_err(|e| format!("cannot bring {iface} up: {e}"))?;
    sys::run("wpa_supplicant", &["-B", "-i", iface, "-c", &conf.to_string_lossy()])
        .map_err(|e| format!("wpa_supplicant failed on {iface}: {e}"))?;

    // DHCP cannot run until the link has associated (carrier up); wait for it.
    let assoc = wait_assoc(iface, 20);
    sys::run(
        "udhcpc",
        &["-n", "-q", "-t", "5", "-T", "3", "-i", iface, "-s", "/etc/udhcpc.script"],
    )
    .map_err(|e| {
        if assoc {
            format!("no DHCP lease on {iface}: {e}")
        } else {
            format!("{iface} did not associate (check SSID/passphrase/signal); no DHCP lease")
        }
    })?;
    // keep renewing while the box runs
    let _ = sys::spawn("udhcpc", &["-b", "-i", iface, "-s", "/etc/udhcpc.script"]);

    // pin the wifi interface; boot uses MODE=dhcp on it
    NetChoice { iface: Some(iface.to_string()), mode: net::Mode::Dhcp, ..Default::default() }
        .write_persisted()?;
    Ok(format!("wifi: {ssid} on {iface} (dhcp)"))
}

/// Poll until the interface is associated, up to `tries * 500 ms`.
fn wait_assoc(iface: &str, tries: u32) -> bool {
    for _ in 0..tries {
        if link_status(iface).connected {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    false
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

// ---------------------------------------------------------------------------
// Hardware detection (`wayang wifi detect`) — works even without a bound
// driver, so you can see which chipset is present before bundling firmware.
// ---------------------------------------------------------------------------

/// A WiFi-related device found via sysfs (bound interface or raw USB function).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HwDevice {
    pub iface: String,
    pub bus: String,
    pub id: String,
    pub driver: String,
    pub name: String,
    pub hint: String,
    pub bound: bool,
}

/// (usb id lowercase, suggested driver, suggested firmware) — best effort.
const KNOWN_USB_WIFI: &[(&str, &str, &str)] = &[
    ("0bda:8179", "rtl8xxxu / r8188eu", "rtlwifi/rtl8188eufw.bin"),
    ("0bda:8178", "rtl8xxxu", "rtlwifi/rtl8192cufw.bin"),
    ("0bda:c811", "rtw88 (rtl8821cu)", "rtw88/rtw8821c_fw.bin"),
    ("0bda:c820", "rtw88 (rtl8821cu)", "rtw88/rtw8821c_fw.bin"),
    ("0bda:b812", "rtw88 (rtl8821cu)", "rtw88/rtw8821c_fw.bin"),
    ("0bda:8812", "rtw88 (rtl88xxau)", "rtw88/rtw8812a_fw.bin"),
    ("148f:5370", "rt2800usb", "rt2870.bin"),
    ("148f:7601", "mt7601u", "mt7601u.bin"),
    ("0e8d:7601", "mt7601u", "mt7601u.bin"),
    ("0cf3:9271", "ath9k_htc", "htc_9271.fw"),
];

fn suggest(id: &str) -> String {
    for (k, drv, fw) in KNOWN_USB_WIFI {
        if id == *k {
            return format!("driver: {drv}; firmware: {fw}");
        }
    }
    "unknown — enable configs/defconfig-wifi and add the vendor firmware".into()
}

/// Hint for a wireless interface that already has a driver bound.
fn suggest_bound(driver: &str, id: &str) -> String {
    if id != "-" {
        return suggest(id);
    }
    match driver {
        "iwlwifi" => "Intel WiFi (iwlwifi) — firmware bundled; no action needed".into(),
        "-" => "unknown — enable configs/defconfig-wifi and add the vendor firmware".into(),
        d => format!("driver: {d} — bound; add vendor firmware if association fails"),
    }
}

/// Detect WiFi hardware under the running sysfs.
pub fn detect() -> Vec<HwDevice> {
    let sys = std::env::var("WAYANG_SYS").unwrap_or_else(|_| "/sys".into());
    detect_at(Path::new(&sys))
}

/// Pure form of [`detect`] for tests.
pub fn detect_at(sys: &Path) -> Vec<HwDevice> {
    let mut out: Vec<HwDevice> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // Bound wireless interfaces.
    if let Ok(entries) = fs::read_dir(sys.join("class/net")) {
        for e in entries.flatten() {
            let path = e.path();
            if !path.join("wireless").exists() {
                continue;
            }
            let iface = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dev = fs::canonicalize(path.join("device")).unwrap_or_else(|_| path.join("device"));
            let dpath = dev.to_string_lossy();
            let id = usb_id(&dev).unwrap_or_else(|| "-".into());
            if id != "-" {
                seen.insert(id.clone());
            }
            let bus = if id != "-" {
                "usb"
            } else if dpath.contains("/pci") {
                "pci"
            } else {
                "platform"
            };
            let driver = fs::read_link(path.join("device/driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "-".into());
            let name = read_trim(&dev.join("product"))
                .or_else(|| read_trim(&dev.join("manufacturer")))
                .unwrap_or_default();
            out.push(HwDevice {
                iface,
                bus: bus.into(),
                id: id.clone(),
                driver: driver.clone(),
                name,
                hint: suggest_bound(&driver, &id),
                bound: true,
            });
        }
    }

    // USB functions that look like WiFi but have no interface bound yet.
    if let Ok(entries) = fs::read_dir(sys.join("bus/usb/devices")) {
        for e in entries.flatten() {
            let d = e.path();
            let (Some(vid), Some(pid)) = (read_trim(&d.join("idVendor")), read_trim(&d.join("idProduct"))) else {
                continue;
            };
            let id = format!("{}:{}", vid.to_lowercase(), pid.to_lowercase());
            if seen.contains(&id) {
                continue;
            }
            let name = read_trim(&d.join("product")).unwrap_or_default();
            let driver = usb_driver(&d);
            // Class 0xe0 ("wireless controller") covers both WiFi and Bluetooth;
            // a device already bound to a driver (e.g. btusb) is not a WiFi
            // adapter waiting for a driver, so only treat *unbound* class-e0
            // functions as WiFi candidates.
            let wireless = KNOWN_USB_WIFI.iter().any(|(k, _, _)| id == *k)
                || is_wifi_name(&name)
                || (interface_is_wireless(&d) && driver.is_none());
            if !wireless {
                continue;
            }
            out.push(HwDevice {
                iface: "-".into(),
                bus: "usb".into(),
                id: id.clone(),
                driver: driver.unwrap_or_else(|| "-".into()),
                name,
                hint: suggest(&id),
                bound: false,
            });
        }
    }

    out.sort_by(|a, b| a.iface.cmp(&b.iface).then(a.id.cmp(&b.id)));
    out
}

/// Walk up from a device path to the USB device that has idVendor/idProduct.
fn usb_id(start: &Path) -> Option<String> {
    for anc in start.ancestors() {
        if let (Some(v), Some(p)) = (read_trim(&anc.join("idVendor")), read_trim(&anc.join("idProduct"))) {
            return Some(format!("{}:{}", v.to_lowercase(), p.to_lowercase()));
        }
    }
    None
}

fn interface_is_wireless(usb_dev: &Path) -> bool {
    let Ok(entries) = fs::read_dir(usb_dev) else {
        return false;
    };
    for e in entries.flatten() {
        let class = e.path().join("bInterfaceClass");
        if read_trim(&class).as_deref() == Some("e0") {
            return true;
        }
    }
    false
}

fn usb_driver(usb_dev: &Path) -> Option<String> {
    let entries = fs::read_dir(usb_dev).ok()?;
    for e in entries.flatten() {
        if let Ok(p) = fs::read_link(e.path().join("driver")) {
            return p.file_name().map(|n| n.to_string_lossy().into_owned());
        }
    }
    None
}

fn is_wifi_name(name: &str) -> bool {
    let n = name.to_lowercase();
    ["wifi", "wlan", "wireless", "802.11", "802.11n", "ac600", "ac1200"]
        .iter()
        .any(|k| n.contains(k))
}

/// Print the detection table (or JSON). Part of `wayang wifi detect`.
pub fn detect_cmd(json: bool) -> crate::error::Result<i32> {
    let devs = detect();
    if json {
        let arr: Vec<serde_json::Value> = devs
            .iter()
            .map(|d| {
                serde_json::json!({
                    "iface": d.iface, "bus": d.bus, "id": d.id,
                    "driver": d.driver, "name": d.name, "hint": d.hint, "bound": d.bound,
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(arr));
        return Ok(0);
    }
    if devs.is_empty() {
        println!("no WiFi hardware detected (no wireless interface and no matching USB device)");
        println!("if it is a USB adapter, make sure it is plugged in; then re-run `wayang wifi detect`");
        return Ok(2);
    }
    println!(
        "{:<10} {:<4} {:<10} {:<16} {:<24} {}",
        "IFACE", "BUS", "ID", "DRIVER", "NAME", "SUGGESTION"
    );
    for d in &devs {
        let iface = if d.bound { d.iface.clone() } else { "usb".into() };
        let drv = if d.driver == "-" { "(none)".to_string() } else { d.driver.clone() };
        println!(
            "{:<10} {:<4} {:<10} {:<16} {:<24} {}",
            iface, d.bus, d.id, drv, trunc(&d.name, 24), d.hint
        );
    }
    if devs.iter().any(|d| !d.bound) {
        println!();
        println!("devices marked `usb` have no driver bound: use configs/defconfig-wifi + firmware");
    }
    Ok(0)
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod detect_tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn write(p: &Path, v: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, v).unwrap();
    }

    #[test]
    fn detects_bound_and_unbound_wifi() {
        let root = std::env::temp_dir().join(format!("wayang-detect-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        // bound: wlan0 -> device with driver + usb ids + product
        let net = root.join("class/net/wlan0");
        fs::create_dir_all(net.join("wireless")).unwrap();
        let dev = net.join("device");
        fs::create_dir_all(&dev).unwrap();
        let drv = root.join("drivers/rtw88_8821cu");
        fs::create_dir_all(&drv).unwrap();
        symlink(&drv, dev.join("driver")).unwrap();
        write(&dev.join("idVendor"), "0bda\n");
        write(&dev.join("idProduct"), "c811\n");
        write(&dev.join("product"), "Realtek 802.11ac NIC\n");

        // unbound: USB function with wireless class
        let usb = root.join("bus/usb/devices/1-2");
        write(&usb.join("idVendor"), "0e8d\n");
        write(&usb.join("idProduct"), "7601\n");
        write(&usb.join("product"), "802.11 n WLAN\n");
        write(&usb.join("1-2:1.0/bInterfaceClass"), "e0\n");

        // bound Bluetooth (class 0xe0 too) must NOT be reported as WiFi
        let bt = root.join("bus/usb/devices/2-1");
        write(&bt.join("idVendor"), "8087\n");
        write(&bt.join("idProduct"), "0aa7\n");
        write(&bt.join("2-1:1.0/bInterfaceClass"), "e0\n");
        let btdrv = root.join("drivers/btusb");
        fs::create_dir_all(&btdrv).unwrap();
        symlink(&btdrv, bt.join("2-1:1.0/driver")).unwrap();

        let devs = detect_at(&root);
        let bound = devs.iter().find(|d| d.iface == "wlan0").expect("bound wlan0");
        assert_eq!(bound.bus, "usb");
        assert_eq!(bound.id, "0bda:c811");
        assert_eq!(bound.driver, "rtw88_8821cu");
        assert!(bound.hint.contains("rtw88"));
        assert!(bound.bound);

        let free = devs.iter().find(|d| d.iface == "-").expect("unbound usb");
        assert_eq!(free.id, "0e8d:7601");
        assert!(free.hint.contains("mt7601u"));
        assert!(!free.bound);

        assert!(devs.iter().all(|d| d.id != "8087:0aa7"), "btusb must not be a WiFi candidate");

        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn parses_iw_link() {
        let s = "Connected to 10:f0:05:63:4b:7e (on wlan0)\n\tSSID: MyNet\n\tsignal: -45 dBm\n";
        let st = parse_link(s);
        assert!(st.connected);
        assert_eq!(st.ssid, "MyNet");
        assert_eq!(st.signal, Some(-45));
        assert!(st.summary().contains("MyNet"));

        let down = parse_link("Not connected.\n");
        assert!(!down.connected);
        assert_eq!(down.summary(), "not connected");
    }
}
