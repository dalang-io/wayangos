//! `wayang edgerouter` — enrol this box on the wayangi dashboard and inspect
//! the agent/tunnel state (docs/EDGEROUTER.md §B–C).
//!
//! The device token lives in `/data/etc/wayangi/token` (mode 600), so enrolment
//! survives OS updates and is never baked into the image. The bundled `wayangi`
//! agent stores its own state (`state.json`, mode 600, holds the token and WG
//! private key; `status.json`, mode 644, no secrets) in the same directory —
//! here we only read it to report the tunnel and the last bootstrap. Starting
//! the agent (the boot step) is deliberately out of scope for this command.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cli::EdgeRouterAction;
use crate::error::{AppError, Result};
use crate::paths;

/// The tunnel interface the agent brings up.
const IFACE: &str = "wayangi0";
const TOKEN_FILE: &str = "token";
const STATE_FILE: &str = "state.json";
const STATUS_FILE: &str = "status.json";

/// `/data/etc/wayangi/token` — our persisted enrolment token.
pub fn token_path() -> PathBuf {
    paths::wayangi_dir().join(TOKEN_FILE)
}

/// The bundled agent, if present (the image installs it to `/usr/sbin`, a
/// deployed copy lives in `/data/bin` and survives updates).
pub fn agent_binary() -> Option<PathBuf> {
    [
        PathBuf::from("/usr/sbin/wayangi"),
        paths::data_dir().join("bin/wayangi"),
        PathBuf::from("/usr/bin/wayangi"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Light shape check for a dashboard device token: one non-empty line of
/// printable ASCII, long enough to be a real token and short enough to be sane.
/// The hub is the authority; this only catches a bad paste before we store it.
pub fn validate_token(raw: &str) -> std::result::Result<String, String> {
    let token = raw.trim();
    if token.is_empty() {
        return Err("paste the per-device token from the wayangi dashboard".into());
    }
    if token.len() < 16 {
        return Err("that looks too short to be a device token".into());
    }
    if token.len() > 512 {
        return Err("that looks too long to be a device token".into());
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the token must be a single line without spaces".into());
    }
    Ok(token.to_string())
}

/// Save the token to `/data/etc/wayangi/token` (mode 600). Running the agent
/// itself is the boot step's job, not this command's.
pub fn enroll(token: &str) -> std::result::Result<(), String> {
    enroll_at(&paths::wayangi_dir(), token)
}

fn enroll_at(base: &Path, token: &str) -> std::result::Result<(), String> {
    let token = validate_token(token)?;
    write_secret(&base.join(TOKEN_FILE), &token)
        .map_err(|e| format!("{}: {e}", base.join(TOKEN_FILE).display()))
}

/// Whether a non-empty token file is present.
fn token_present_at(base: &Path) -> bool {
    read_token_at(base).is_some()
}

fn read_token_at(base: &Path) -> Option<String> {
    fs::read_to_string(base.join(TOKEN_FILE))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Forget the saved token. Returns whether one was removed; the agent's own
/// `state.json` is left for the agent to manage.
pub fn clear() -> std::result::Result<bool, String> {
    clear_at(&paths::wayangi_dir())
}

fn clear_at(base: &Path) -> std::result::Result<bool, String> {
    match fs::remove_file(base.join(TOKEN_FILE)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", base.join(TOKEN_FILE).display())),
    }
}

fn write_secret(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(contents.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

// ---- status ------------------------------------------------------------

/// What we know about the EdgeRouter agent on this box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The bundled agent binary, when installed.
    pub binary: Option<PathBuf>,
    /// A token is saved in `/data/etc/wayangi/token`.
    pub token: bool,
    /// `wayangi0` exists and is administratively up (`/sys/class/net`).
    pub tunnel_up: bool,
    /// The agent published a live `status.json` snapshot.
    pub running: bool,
    /// The agent reports a recent WireGuard handshake.
    pub connected: bool,
    pub version: Option<String>,
    /// Unix seconds of the last handshake, if the snapshot has one.
    pub last_handshake: Option<i64>,
    /// Last bootstrap identity (from `status.json` or the agent's `state.json`).
    pub device: Option<String>,
    pub device_id: Option<String>,
    pub account: Option<String>,
    pub plan: Option<String>,
    pub endpoint: Option<String>,
    pub addresses: Vec<String>,
}

impl Status {
    /// True when a bootstrap response has been recorded at least once.
    pub fn known(&self) -> bool {
        self.device.is_some()
            || self.device_id.is_some()
            || self.account.is_some()
            || self.plan.is_some()
            || !self.addresses.is_empty()
    }
}

/// Gather the current state. The persistent dir is authoritative; the agent's
/// default `/etc/wayangi` is read too, in case it runs without a conf-dir
/// override.
pub fn status() -> Status {
    let sys = std::env::var_os("WAYANG_SYS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/sys"));
    status_at(&paths::wayangi_dir(), &sys)
}

fn status_at(base: &Path, sys: &Path) -> Status {
    let mut st = Status {
        binary: agent_binary(),
        token: token_present_at(base),
        tunnel_up: tunnel_up_at(sys),
        ..Default::default()
    };
    for dir in [base.to_path_buf(), PathBuf::from("/etc/wayangi")] {
        merge_snapshot(&mut st, &read_json(&dir.join(STATUS_FILE)));
        if let Some(v) = read_json(&dir.join(STATE_FILE)) {
            merge_state(&mut st, &v);
        }
    }
    st
}

/// `wayangi0` is up: the interface exists and its flags carry IFF_UP (or, with
/// no flags file, its operstate is not `down`). Pure form for tests.
pub fn tunnel_up_at(sys: &Path) -> bool {
    let iface = sys.join("class/net").join(IFACE);
    if !iface.exists() {
        return false;
    }
    if let Ok(raw) = fs::read_to_string(iface.join("flags")) {
        if let Ok(flags) = u32::from_str_radix(raw.trim().trim_start_matches("0x"), 16) {
            return flags & 0x1 != 0;
        }
    }
    match fs::read_to_string(iface.join("operstate")) {
        Ok(s) => s.trim() != "down",
        Err(_) => true,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn str_list(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// Merge the running agent's world-readable `status.json`.
fn merge_snapshot(st: &mut Status, v: &Option<Value>) {
    let Some(v) = v else { return };
    st.running |= v.get("running").and_then(Value::as_bool).unwrap_or(false);
    st.connected |= v.get("connected").and_then(Value::as_bool).unwrap_or(false);
    st.version = str_field(v, "version").or_else(|| st.version.clone());
    if let Some(h) = v.get("last_handshake").and_then(Value::as_i64).filter(|h| *h > 0) {
        st.last_handshake = Some(h);
    }
    st.device = str_field(v, "device").or_else(|| st.device.clone());
    st.device_id = str_field(v, "device_id").or_else(|| st.device_id.clone());
    st.account = str_field(v, "account").or_else(|| st.account.clone());
    st.plan = str_field(v, "plan").or_else(|| st.plan.clone());
    st.endpoint = str_field(v, "endpoint").or_else(|| st.endpoint.clone());
    let addrs = str_list(v, "addresses");
    if !addrs.is_empty() {
        st.addresses = addrs;
    }
}

/// Merge the agent's `state.json`, whose `bootstrap` object carries the
/// original signed response even before a snapshot exists.
fn merge_state(st: &mut Status, v: &Value) {
    st.version = str_field(v, "version").or_else(|| st.version.clone());
    let Some(b) = v.get("bootstrap") else { return };
    st.device = str_field(b, "name").or_else(|| st.device.clone());
    st.device_id = str_field(b, "device_id").or_else(|| st.device_id.clone());
    st.account = str_field(b, "account_email").or_else(|| st.account.clone());
    st.plan = str_field(b, "plan").or_else(|| st.plan.clone());
    st.endpoint = str_field(b, "hub_endpoint").or_else(|| st.endpoint.clone());
    let addrs = str_list(b, "addresses");
    if !addrs.is_empty() {
        st.addresses = addrs;
    }
}

impl Status {
    /// Human-readable `wayang edgerouter status` output.
    pub fn print_human(&self) {
        let agent = self.binary.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not installed".into());
        println!("agent:     {agent}");
        println!("token:     {}", if self.token { format!("saved ({})", token_path().display()) } else { "none".into() });
        println!("tunnel:    {IFACE} {}", if self.tunnel_up { "up" } else { "down" });
        if self.running || self.connected || self.last_handshake.is_some() {
            let hs = self.last_handshake.map(|s| format!("{s}")).unwrap_or_else(|| "-".into());
            println!("runtime:   {} (last handshake {hs})", if self.connected { "connected" } else if self.running { "running" } else { "not running" });
        }
        if let Some(v) = &self.version {
            println!("version:   {v}");
        }
        if self.known() {
            if let Some(d) = &self.device {
                let id = self.device_id.as_deref().map(|i| format!(" (id {i})")).unwrap_or_default();
                println!("device:    {d}{id}");
            }
            if let Some(a) = &self.account {
                println!("account:   {a}");
            }
            if let Some(p) = &self.plan {
                println!("plan:      {p}");
            }
            if !self.addresses.is_empty() {
                println!("addresses: {}", self.addresses.join(", "));
            }
            if let Some(e) = &self.endpoint {
                println!("endpoint:  {e}");
            }
        }
    }
}

/// Dispatch the parsed `edgerouter` action (help is printed by `main`).
pub fn run(action: EdgeRouterAction) -> Result<i32> {
    match action {
        EdgeRouterAction::Status => {
            status().print_human();
            Ok(0)
        }
        EdgeRouterAction::Enroll(token) => {
            enroll(&token).map_err(AppError::err)?;
            println!("Saved the device token to {}.", token_path().display());
            println!("The agent will pick it up on the next boot (or run `wayangi start`).");
            Ok(0)
        }
        EdgeRouterAction::Clear => {
            let removed = clear().map_err(AppError::err)?;
            if removed {
                println!("Cleared the saved device token.");
            } else {
                println!("No saved token to clear.");
            }
            Ok(0)
        }
        EdgeRouterAction::Help => Ok(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "wayang-edgerouter-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn token_shape_is_checked_lightly() {
        assert!(validate_token("").is_err());
        assert!(validate_token("   ").is_err());
        assert!(validate_token("short").is_err());
        assert!(validate_token("has a space in it 0123456789").is_err());
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(validate_token(&format!("  {hex}  ")).unwrap(), hex);
    }

    #[test]
    fn enroll_write_read_clear_round_trip() {
        let dir = tmp();
        let hex = "deadbeefdeadbeefdeadbeefdeadbeef";
        assert!(!token_present_at(&dir));
        enroll_at(&dir, hex).unwrap();
        assert!(token_present_at(&dir));
        assert_eq!(read_token_at(&dir).as_deref(), Some(hex));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join(TOKEN_FILE)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the token file must stay private");
        }
        assert!(clear_at(&dir).unwrap());
        assert!(!token_present_at(&dir));
        assert!(!clear_at(&dir).unwrap(), "clearing twice is a no-op");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn enroll_rejects_a_bad_token_without_writing() {
        let dir = tmp();
        assert!(enroll_at(&dir, "").is_err());
        assert!(!dir.join(TOKEN_FILE).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn enroll_uses_wayang_root() {
        let dir = tmp();
        std::env::set_var("WAYANG_ROOT", &dir);
        assert_eq!(paths::wayangi_dir(), dir.join("data/etc/wayangi"));
        let hex = "0123456789abcdef0123456789abcdef";
        enroll(hex).unwrap();
        assert!(token_path().ends_with("data/etc/wayangi/token"));
        assert!(token_path().is_file());
        assert_eq!(read_token_at(&paths::wayangi_dir()).as_deref(), Some(hex));
        assert!(clear().unwrap());
        assert!(read_token_at(&paths::wayangi_dir()).is_none());
        std::env::remove_var("WAYANG_ROOT");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_merges_snapshot_and_state() {
        let dir = tmp();
        let sys = tmp();
        // no interface: tunnel down
        let st = status_at(&dir, &sys);
        assert!(!st.token && !st.tunnel_up && !st.known());

        // a token + a bootstrap in the agent state (no snapshot yet)
        enroll_at(&dir, "0123456789abcdef0123456789abcdef").unwrap();
        fs::write(
            dir.join(STATE_FILE),
            r#"{"bootstrap":{"name":"naga","device_id":"01HX","account_email":"ops@dalang.io","plan":"edge","addresses":["2a06:98c0::5/128"]}}"#,
        )
        .unwrap();
        let st = status_at(&dir, &sys);
        assert!(st.token);
        assert_eq!(st.device.as_deref(), Some("naga"));
        assert_eq!(st.account.as_deref(), Some("ops@dalang.io"));
        assert_eq!(st.addresses, vec!["2a06:98c0::5/128"]);
        assert!(!tunnel_up_at(&sys));

        // a live snapshot overrides the runtime bits
        fs::write(
            dir.join(STATUS_FILE),
            r#"{"running":true,"connected":true,"last_handshake":42,"endpoint":"203.0.113.9:443"}"#,
        )
        .unwrap();
        let st = status_at(&dir, &sys);
        assert!(st.running && st.connected);
        assert_eq!(st.last_handshake, Some(42));
        assert_eq!(st.endpoint.as_deref(), Some("203.0.113.9:443"));

        // an up interface is detected from sysfs flags
        let net = sys.join("class/net/wayangi0");
        fs::create_dir_all(&net).unwrap();
        fs::write(net.join("flags"), "0x1003").unwrap();
        assert!(tunnel_up_at(&sys));
        fs::write(net.join("flags"), "0x1002").unwrap();
        assert!(!tunnel_up_at(&sys), "IFF_UP clear means down");

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&sys);
    }

    #[test]
    fn invalid_json_is_ignored() {
        let dir = tmp();
        let sys = tmp();
        fs::write(dir.join(STATUS_FILE), "not json").unwrap();
        fs::write(dir.join(STATE_FILE), "{broken").unwrap();
        let st = status_at(&dir, &sys);
        assert!(!st.running && !st.connected && !st.known());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&sys);
    }
}