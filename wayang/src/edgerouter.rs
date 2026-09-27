//! `wayang edgerouter` — enrol this box on the wayangi dashboard, drive the
//! agent, and inspect the token/tunnel state (docs/EDGEROUTER.md §B–C).
//!
//! The device token lives in `/data/etc/wayangi/token` (mode 600), so enrolment
//! survives OS updates and is never baked into the image. The bundled `wayangi`
//! agent stores its own state (`state.json`, mode 600, holds the token and WG
//! private key; `status.json`, mode 644, no secrets) in the same directory
//! (`--conf-dir /data/etc/wayangi`). We read it to report the tunnel and the
//! last bootstrap, and `start`/`stop`/`restart` drive the agent in the
//! background (bounded, captured output; never blocks a caller for long). The
//! boot step runs the same agent (see `/etc/init.d/edgerouter`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cli::EdgeRouterAction;
use crate::error::{AppError, Result};
use crate::paths;

/// The tunnel interface the agent brings up.
const IFACE: &str = "wayangi0";
const TOKEN_FILE: &str = "token";
const STATE_FILE: &str = "state.json";
const STATUS_FILE: &str = "status.json";
/// The agent's log (its own default path; `wayangi logs` reads it). Consulted
/// for hub rejections the agent doesn't persist into its JSON state.
const AGENT_LOG: &str = "/var/log/wayangi.log";
/// How much of the agent log's tail we scan for a hub rejection.
const LOG_TAIL_BYTES: u64 = 16 * 1024;
/// Bounded wait for `wayangi start` (bootstrap + ~10s preflight) / `stop`.
const START_TIMEOUT: Duration = Duration::from_secs(45);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// `/data/etc/wayangi/token` — our persisted enrolment token.
pub fn token_path() -> PathBuf {
    paths::wayangi_dir().join(TOKEN_FILE)
}

/// The bundled agent, if present. A deployed copy in `/data/bin` (persistent,
/// survives OS updates) wins over the image copy in `/usr/sbin`.
pub fn agent_binary() -> Option<PathBuf> {
    [
        paths::data_dir().join("bin/wayangi"),
        PathBuf::from("/usr/sbin/wayangi"),
        PathBuf::from("/usr/bin/wayangi"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// The agent's log file (`WAYANGI_LOG` overrides it in tests).
pub fn agent_log_path() -> PathBuf {
    std::env::var_os("WAYANGI_LOG").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(AGENT_LOG))
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
    /// A hub/token problem reported by the agent (revoked/expired/plan), from
    /// its JSON state or the tail of its log. No secrets are stored here.
    pub error: Option<String>,
}

/// A one-line health verdict derived from [`Status`], used by both the CLI and
/// the console module so they always agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// No `wayangi` binary on this OS image.
    NotInstalled,
    /// Agent present, but `/data/etc/wayangi/token` is missing.
    NotEnrolled,
    /// The agent reports the token/subscription is rejected (revoked, expired,
    /// plan inactive). The string is already a human-safe message.
    Blocked(String),
    /// Enrolled and `wayangi0` is up with a live handshake.
    TunnelUp,
    /// Enrolled, but the tunnel is down (agent stopped, hub unreachable, …).
    TunnelDown,
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

    /// Classify the EdgeRouter state for display.
    pub fn health(&self) -> Health {
        if self.binary.is_none() {
            return Health::NotInstalled;
        }
        if !self.token {
            return Health::NotEnrolled;
        }
        // A live, handshaking tunnel wins over a stale log/JSON complaint.
        if self.tunnel_up && (self.connected || self.last_handshake.is_none()) {
            return Health::TunnelUp;
        }
        if let Some(e) = &self.error {
            if !e.trim().is_empty() {
                return Health::Blocked(e.clone());
            }
        }
        if self.tunnel_up {
            Health::TunnelUp
        } else {
            Health::TunnelDown
        }
    }
}

impl Health {
    /// Short state label for the console card.
    pub fn label(&self) -> &'static str {
        match self {
            Health::NotInstalled => "NOT INSTALLED",
            Health::NotEnrolled => "NOT ENROLLED",
            Health::Blocked(_) => "TOKEN/PLAN ISSUE",
            Health::TunnelUp => "TUNNEL UP",
            Health::TunnelDown => "TUNNEL DOWN",
        }
    }

    /// A clear, secret-free next step for the operator.
    pub fn message(&self) -> String {
        match self {
            Health::NotInstalled => {
                "wayangi is not on this image. Rebuild with scripts/build-wayangi.sh (or copy it to /data/bin/wayangi).".to_string()
            }
            Health::NotEnrolled => {
                "No device token. Run `wayang edgerouter enroll <token>` with the token from the wayangi dashboard.".to_string()
            }
            Health::Blocked(m) => m.clone(),
            Health::TunnelUp => format!("{IFACE} is up and handshaking; the delegated prefix is routed."),
            Health::TunnelDown => {
                format!("Enrolled, but {IFACE} is down. Start it with `wayang edgerouter start`; if it stays down the hub may be unreachable.")
            }
        }
    }
}

/// Gather the current state. The persistent dir is authoritative; the agent's
/// default `/etc/wayangi` is read too, in case it runs without a conf-dir
/// override.
pub fn status() -> Status {
    let sys = std::env::var_os("WAYANG_SYS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/sys"));
    status_full(&paths::wayangi_dir(), &sys, Some(&agent_log_path()))
}

/// Pure form for tests: no log scan.
#[cfg(test)]
fn status_at(base: &Path, sys: &Path) -> Status {
    status_full(base, sys, None)
}

fn status_full(base: &Path, sys: &Path, log: Option<&Path>) -> Status {
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
    // The agent exits after a rejected bootstrap and only records the hub's
    // reason in its log, so consult the tail (newest-up finding wins).
    if let Some(path) = log {
        if let Some(text) = read_tail(path, LOG_TAIL_BYTES) {
            merge_agent_text(&mut st, &text);
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
    merge_error(st, v);
    let addrs = str_list(v, "addresses");
    if !addrs.is_empty() {
        st.addresses = addrs;
    }
}

/// Merge the agent's `state.json`, whose `bootstrap` object carries the
/// original signed response even before a snapshot exists.
fn merge_state(st: &mut Status, v: &Value) {
    st.version = str_field(v, "version").or_else(|| st.version.clone());
    merge_error(st, v);
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

/// Pick up an agent-published error, if a build ever writes one
/// (`error`/`last_error`/`reason`). Stored as a human message, never verbatim
/// secrets (those live in `state.json`, mode 600, and are not logged).
fn merge_error(st: &mut Status, v: &Value) {
    if st.error.is_some() {
        return;
    }
    for key in ["error", "last_error", "reason"] {
        if let Some(raw) = str_field(v, key) {
            if let Some(msg) = classify_agent_text(&raw) {
                st.error = Some(msg);
            }
            return;
        }
    }
}

/// Read at most `max` bytes from the end of `path`, without failing on a
/// missing/short file. Pure form for tests.
fn read_tail(path: &Path, max: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    if len > max {
        f.seek(SeekFrom::Start(len - max)).ok()?;
    }
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    // A tail may start mid-UTF-8; lossy is fine for scanning ASCII markers.
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Merge a hub rejection the agent only wrote to its log. Latest line wins.
/// Only consulted when the agent is not running: a running agent publishes an
/// authoritative `status.json`, and its log may still hold an old rejection
/// from a previous token.
fn merge_agent_text(st: &mut Status, text: &str) {
    if st.error.is_some() || st.running {
        return;
    }
    for line in text.lines().rev() {
        if let Some(msg) = classify_agent_text(line) {
            st.error = Some(msg);
            return;
        }
    }
}

/// Turn an agent line into a clear, secret-free operator message, or `None`
/// when the line isn't a token/subscription rejection. Matched tightly (the
/// hub writes "invalid or revoked token"; the agent wraps it as `hub attach`).
pub fn classify_agent_text(raw: &str) -> Option<String> {
    let l = raw.to_ascii_lowercase();
    let hub = l.contains("hub attach") || l.contains("hub rejected") || l.contains("bootstrap") || l.contains("token");
    if l.contains("payment required") {
        return Some(
            "The device's wayangi subscription is not active (payment required). Complete checkout in the dashboard, then restart the agent."
                .into(),
        );
    }
    if l.contains("revoked") || l.contains("expired token") {
        return Some(
            "The wayangi hub rejected this device's token (revoked or expired). Rotate it in the dashboard and run `wayang edgerouter enroll <token>`."
                .into(),
        );
    }
    if hub && (l.contains("http 401") || l.contains("http 403") || l.contains("unauthorized") || l.contains("forbidden")) {
        return Some(
            "The wayangi hub rejected the token (unauthorized). Check or rotate it in the dashboard, then re-enrol."
                .into(),
        );
    }
    if l.contains("re-bind") || l.contains("hub rejected device attach") {
        return Some(
            "The wayangi hub has this device attached to another machine. Re-run the agent with --force-rebind or rotate the token in the dashboard."
                .into(),
        );
    }
    None
}

impl Status {
    /// Human-readable `wayang edgerouter status` output.
    pub fn print_human(&self) {
        let agent = self.binary.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "not installed".into());
        let health = self.health();
        println!("agent:     {agent}");
        println!("token:     {}", if self.token { format!("saved ({})", token_path().display()) } else { "none".into() });
        println!("tunnel:    {IFACE} {}", if self.tunnel_up { "up" } else { "down" });
        println!("state:     {}", health.label());
        println!("           {}", health.message());
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

// ---- agent lifecycle ---------------------------------------------------

/// Which lifecycle operation the console/CLI asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentOp {
    Start,
    Stop,
    Restart,
}

impl AgentOp {
    pub fn verb(self) -> &'static str {
        match self {
            AgentOp::Start => "start",
            AgentOp::Stop => "stop",
            AgentOp::Restart => "restart",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AgentOp::Start => "Starting the wayangi agent",
            AgentOp::Stop => "Stopping the wayangi agent",
            AgentOp::Restart => "Restarting the wayangi agent",
        }
    }

    /// Run the operation (blocking, bounded). Suitable for a background
    /// thread; the console polls the result.
    pub fn run(self) -> std::result::Result<String, String> {
        match self {
            AgentOp::Start => start_agent(),
            AgentOp::Stop => stop_agent(),
            AgentOp::Restart => restart_agent(),
        }
    }
}

/// Start the agent, passing the saved token via the environment (never argv, so
/// it can't leak through `ps`) and pointing `--conf-dir` at `/data` so the
/// device identity survives OS updates. Returns once the parent exits (it
/// daemonizes after its preflight).
pub fn start_agent() -> std::result::Result<String, String> {
    let bin = agent_binary().ok_or_else(|| "the wayangi agent is not installed (bundle wayangi)".to_string())?;
    let dir = paths::wayangi_dir();
    let token = read_token_at(&dir);
    if token.is_none() && !dir.join(STATE_FILE).is_file() {
        return Err("no device token: run `wayang edgerouter enroll <token>` first".into());
    }
    let dir = dir.to_string_lossy().to_string();
    let args = ["start", "--conf-dir", dir.as_str()];
    let (code, output) = run_agent(&bin, &args, token.as_deref(), START_TIMEOUT)?;
    if code == Some(0) {
        Ok(format!("wayangi agent started; {IFACE} is coming up (log: {AGENT_LOG})."))
    } else {
        Err(agent_failure(code, &output))
    }
}

/// Stop the running agent (a no-op when none is running).
pub fn stop_agent() -> std::result::Result<String, String> {
    let bin = agent_binary().ok_or_else(|| "the wayangi agent is not installed (bundle wayangi)".to_string())?;
    let dir = paths::wayangi_dir().to_string_lossy().to_string();
    let args = ["stop", "--conf-dir", dir.as_str()];
    let (code, output) = run_agent(&bin, &args, None, STOP_TIMEOUT)?;
    if code == Some(0) {
        Ok("wayangi agent stopped.".into())
    } else {
        Err(agent_failure(code, &output))
    }
}

/// Best-effort stop, then start.
pub fn restart_agent() -> std::result::Result<String, String> {
    let stop = stop_agent().unwrap_or_else(|e| format!("(stop skipped: {e})"));
    let start = start_agent()?;
    Ok(format!("{stop} {start}"))
}

/// Build the operator-facing failure message from the agent's captured output,
/// preferring a specific hub rejection over the bare exit code.
fn agent_failure(code: Option<i32>, output: &str) -> String {
    if let Some(msg) = classify_agent_text(output).or_else(|| merge_find(output)) {
        return msg;
    }
    let how = code.map(|c| format!("exit code {c}")).unwrap_or_else(|| "timeout".into());
    format!("wayangi agent failed ({how}); see {AGENT_LOG}.")
}

/// Scan captured output newest-line-first for a rejection marker.
fn merge_find(output: &str) -> Option<String> {
    output.lines().rev().find_map(classify_agent_text)
}

/// Spawn the agent, capture stdout/stderr to a temp file and wait up to
/// `timeout` (polling, so a wedged preflight can't hang the caller forever).
/// Returns the exit code (`None` on timeout) and the captured output.
fn run_agent(
    bin: &Path,
    args: &[&str],
    token: Option<&str>,
    timeout: Duration,
) -> std::result::Result<(Option<i32>, String), String> {
    let log = std::env::temp_dir().join(format!("wayang-edgerouter-{}.log", std::process::id()));
    let out = fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| format!("{}: {e}", log.display()))?;
    let mut cmd = Command::new(bin);
    cmd.args(args).stdin(Stdio::null()).stdout(out).stderr(err);
    if let Some(t) = token {
        cmd.env("WAYANGI_TOKEN", t);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", bin.display()))?;
    let deadline = Instant::now() + timeout;
    let mut code = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                code = status.code().or(Some(-1));
                break;
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("{}: {e}", bin.display())),
        }
    }
    let text = fs::read_to_string(&log).unwrap_or_default();
    let _ = fs::remove_file(&log);
    Ok((code, text))
}

/// Dispatch the parsed `edgerouter` action (help is printed by `main`).
pub fn run(action: EdgeRouterAction) -> Result<i32> {
    // Resolve the lifecycle op up front, before `action` is consumed.
    let agent_op = match &action {
        EdgeRouterAction::Start => Some(AgentOp::Start),
        EdgeRouterAction::Stop => Some(AgentOp::Stop),
        EdgeRouterAction::Restart => Some(AgentOp::Restart),
        _ => None,
    };
    match action {
        EdgeRouterAction::Status => {
            status().print_human();
            Ok(0)
        }
        EdgeRouterAction::Enroll(token) => {
            enroll(&token).map_err(AppError::err)?;
            println!("Saved the device token to {}.", token_path().display());
            println!("The agent will pick it up on the next boot (or run `wayang edgerouter start`).");
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
        EdgeRouterAction::Start | EdgeRouterAction::Stop | EdgeRouterAction::Restart => {
            let op = agent_op.expect("lifecycle op resolved above");
            // Always show the resulting state, so a failure is actionable
            // (tunnel down, token revoked) — but exit non-zero so scripts and
            // the console can tell.
            let code = match op.run() {
                Ok(msg) => {
                    println!("{msg}");
                    0
                }
                Err(msg) => {
                    eprintln!("wayang: {msg}");
                    1
                }
            };
            status().print_human();
            Ok(code)
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

    #[test]
    fn classify_agent_text_matches_only_token_failures() {
        // the hub's exact revocation wording, wrapped by the agent
        let a = classify_agent_text(
            r#"wayangi: hub attach: HTTP 401: {"error":"invalid or revoked token"}"#,
        )
        .expect("revoked");
        assert!(a.to_lowercase().contains("revoked"), "{a}");
        assert!(classify_agent_text("payment required — checkout at https://x").is_some());
        assert!(classify_agent_text("hub attach: HTTP 401 unauthorized").is_some());
        assert!(classify_agent_text("hub rejected device attach: conflict (re-run with --force-rebind)").is_some());
        // benign agent lines must not be classified (avoid false positives)
        assert!(classify_agent_text("awaitTunDeviceCleared(wayangi0): budget expired").is_none());
        assert!(classify_agent_text("preflight OK · transport=udp").is_none());
        assert!(classify_agent_text("hub registered device \"naga\"").is_none());
    }

    fn edge_status(token: bool, tunnel_up: bool) -> Status {
        Status {
            binary: Some(PathBuf::from("/usr/sbin/wayangi")),
            token,
            tunnel_up,
            connected: tunnel_up,
            ..Default::default()
        }
    }

    #[test]
    fn health_classifies_lifecycle_without_secrets() {
        assert_eq!(Status::default().health(), Health::NotInstalled);
        assert_eq!(edge_status(false, false).health(), Health::NotEnrolled);

        let mut blocked = edge_status(true, false);
        blocked.error = Some(classify_agent_text("hub attach: HTTP 401 invalid or revoked token").unwrap());
        match blocked.health() {
            Health::Blocked(m) => {
                assert!(m.contains("revoked"));
                assert!(!m.contains("HTTP 401"), "raw hub text is not surfaced");
            }
            other => panic!("expected Blocked, got {other:?}"),
        }

        assert_eq!(edge_status(true, true).health(), Health::TunnelUp);
        assert_eq!(edge_status(true, false).health(), Health::TunnelDown);
        // a live tunnel wins over a stale log complaint
        let mut stale = edge_status(true, true);
        stale.error = Some("revoked".into());
        assert_eq!(stale.health(), Health::TunnelUp);
    }

    #[test]
    fn status_reads_the_agent_log_tail_for_a_revocation() {
        let dir = tmp();
        let sys = tmp();
        let log = tmp().join("wayangi.log");
        enroll_at(&dir, "0123456789abcdef0123456789abcdef").unwrap();
        fs::write(&log, "banner\nhub attach: HTTP 401: {\"error\":\"invalid or revoked token\"}\n").unwrap();

        let clean = status_at(&dir, &sys);
        assert!(clean.error.is_none(), "no log scan in the pure form");

        let st = status_full(&dir, &sys, Some(&log));
        assert!(st.error.as_deref().unwrap().contains("revoked"));

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&sys);
    }

    #[test]
    fn read_tail_reads_the_end_and_tolerates_missing_files() {
        let dir = tmp();
        let p = dir.join("log");
        fs::write(&p, "0123456789").unwrap();
        assert_eq!(read_tail(&p, 4).as_deref(), Some("6789"));
        assert_eq!(read_tail(&p, 100).as_deref(), Some("0123456789"));
        assert!(read_tail(&dir.join("nope"), 4).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn error_field_from_agent_state_is_classified() {
        let dir = tmp();
        let sys = tmp();
        enroll_at(&dir, "0123456789abcdef0123456789abcdef").unwrap();
        fs::write(dir.join(STATUS_FILE), r#"{"running":false,"error":"invalid or revoked token"}"#).unwrap();
        let st = status_at(&dir, &sys);
        assert!(st.error.as_deref().unwrap().contains("revoked"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&sys);
    }
}