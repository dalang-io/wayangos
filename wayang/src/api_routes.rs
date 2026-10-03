//! The routes of `wayang api`: the WayangOS CLI's operations as JSON.
//!
//! Every handler calls what the CLI/HUD calls (`status::gather`,
//! `update::plan`/`stage_plan`, `staging::*`, `edgerouter::*`, `sshkeys::*`,
//! `net::list`, `reset_config`): the API cannot do anything the CLI cannot.
//!
//! Reads need any token. Everything that changes the box needs scope `admin`
//! on a server started with `--rw` — an OS update, a slot switch, a reset, an
//! Edge bundle, an SSH key are all things that decide who can reach the box.
//! None of them reboots: the API stages and arms, a person (or an orchestrator
//! that knows the box is safe to lose for a minute) reboots.

use std::path::PathBuf;

use serde_json::{Value, json};
use wayang_api::{App, Need, Principal, Request, Response, Route, err, ok_envelope};

use crate::cli::UpdateArgs;
use crate::edgerouter;
use crate::error::AppError;
use crate::mount;
use crate::net;
use crate::paths;
use crate::sema::Decision;
use crate::slot::Slot;
use crate::sshkeys::{self, SshKey};
use crate::status;
use crate::update;
use crate::version;

/// The second key a reset needs on top of the `admin` scope.
pub const RESET_CONFIRM: &str = "yes-reset";
/// At most one public key line.
const MAX_KEY_LINE: usize = 4096;

pub struct WayangApp {
    /// `Some(root)` = a boot tree under `<root>/boot` (tests); `None` = the
    /// real ESP, discovered like the CLI does.
    pub boot_root: Option<PathBuf>,
    pub tls: bool,
    pub mtls: bool,
    pub rw: bool,
}

impl WayangApp {
    pub fn new(tls: bool, mtls: bool, rw: bool) -> WayangApp {
        WayangApp {
            boot_root: paths::root(),
            tls,
            mtls,
            rw,
        }
    }

    fn open_boot(&self) -> Result<mount::BootRoot, AppError> {
        mount::open_with(self.boot_root.clone(), None)
    }

    fn snapshot(&self) -> Result<status::Status, AppError> {
        let boot = self.open_boot()?;
        Ok(status::gather(
            &boot.path,
            version::read().ok(),
            version::channel_or("stable"),
            paths::data_dir().exists(),
        ))
    }
}

fn app_err(e: AppError, status: u16) -> Response {
    let code = match e.code {
        3 => "verify_failed",
        4 => "incompatible",
        _ => "failed",
    };
    let status = match e.code {
        3 => 422,
        4 => 409,
        _ => status,
    };
    err(status, code, e.msg)
}

/// A staged update (or a slot switch) is waiting for a reboot: the next boot
/// is not the slot that is running.
fn update_pending(st: &status::Status) -> bool {
    st.boot_next != st.active
}

fn slot_json(s: Slot) -> &'static str {
    s.as_str()
}

fn uptime_secs() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/uptime").ok()?;
    s.split_whitespace().next()?.split('.').next()?.parse().ok()
}

/// `wayang-fw --version` and friends, best effort: a missing tool is `null`.
fn tool_versions() -> Value {
    let one = |bin: &str| -> Value {
        crate::sys::run(bin, &["--version"])
            .ok()
            .map(|o| o.trim().to_string())
            .filter(|s| !s.is_empty())
            .map_or(Value::Null, Value::String)
    };
    json!({ "wayang-fw": one("wayang-fw"), "wayang-router": one("wayang-router") })
}

fn ok(data: Value) -> Response {
    ok_envelope(data, None, None)
}

fn read_status(app: &WayangApp) -> Response {
    let st = match app.snapshot() {
        Ok(s) => s,
        Err(e) => return app_err(e, 409),
    };
    let mut data: Value = serde_json::from_str(&st.to_json()).unwrap_or(Value::Null);
    data["update_pending"] = Value::Bool(update_pending(&st));
    data["uptime_s"] = uptime_secs().map_or(Value::Null, Value::from);
    data["tools"] = tool_versions();
    data["net"] = net_summary();
    data["wayang"] = Value::String(env!("CARGO_PKG_VERSION").into());
    ok(data)
}

fn net_summary() -> Value {
    let ifaces = net::list(net::primary_iface().as_deref());
    json!({
        "primary": net::primary_iface(),
        "interfaces": ifaces.iter().filter(|i| i.link).count(),
        "addresses": ifaces.iter().flat_map(|i| i.ipv4.clone()).collect::<Vec<_>>(),
    })
}

fn read_net() -> Response {
    let primary = net::primary_iface();
    let ifaces: Vec<Value> = net::list(primary.as_deref())
        .iter()
        .map(|i| {
            json!({
                "name": i.name,
                "link": i.link,
                "driver": i.driver,
                "mac": i.mac,
                "wireless": i.wireless,
                "primary": i.primary,
                "ipv4": i.ipv4,
            })
        })
        .collect();
    // the persisted choice is a shell snippet of KEY=VALUE lines (no secrets)
    let choice: serde_json::Map<String, Value> =
        std::fs::read_to_string(paths::network_config_file())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| {
                (
                    k.trim().to_string(),
                    Value::String(v.trim().trim_matches('"').into()),
                )
            })
            .collect();
    ok(json!({
        "primary": primary,
        "interfaces": ifaces,
        "config": choice,
        "also": net::read_also(),
    }))
}

fn update_check(app: &WayangApp) -> Response {
    let _ = app;
    let a = UpdateArgs {
        check: true,
        ..UpdateArgs::default()
    };
    match update::plan(false, &a) {
        Ok(p) => ok(json!({
            "installed": p.installed.to_string(),
            "channel": p.channel,
            "arch": p.host,
            "bundle": p.manifest.version,
            "available": p.decision == Decision::Available,
            "notes": p.manifest.notes,
        })),
        Err(e) => app_err(e, 502),
    }
}

fn update_stage(app: &WayangApp) -> Response {
    let st = match app.snapshot() {
        Ok(s) => s,
        Err(e) => return app_err(e, 409),
    };
    if update_pending(&st) {
        return err(
            423,
            "locked",
            format!(
                "an update is already staged (the next boot uses slot {}): reboot into it, \
                 or POST /v1/update/rollback first",
                st.boot_next.as_str()
            ),
        );
    }
    let a = UpdateArgs::default();
    let plan = match update::plan(false, &a) {
        Ok(p) => p,
        Err(e) => return app_err(e, 502),
    };
    if plan.decision == Decision::NoUpdate {
        return err(
            409,
            "conflict",
            format!(
                "no update available: installed {} is up to date (bundle {})",
                plan.installed, plan.manifest.version
            ),
        );
    }
    let from = plan.installed.to_string();
    match update::stage_plan(plan, || app.open_boot(), None) {
        Ok((version, slot)) => ok(json!({
            "staged": version,
            "from": from,
            "slot": slot_json(slot),
            "rebooted": false,
            "next": "reboot to run it (the previous slot stays as the fallback)",
        })),
        Err(e) => app_err(e, 502),
    }
}

fn slot_action(
    app: &WayangApp,
    what: &str,
    f: impl FnOnce(&mount::BootRoot) -> Result<Slot, AppError>,
) -> Response {
    let boot = match app.open_boot() {
        Ok(b) => b,
        Err(e) => return app_err(e, 409),
    };
    match f(&boot) {
        Ok(s) => ok(json!({ "action": what, "slot": slot_json(s), "rebooted": false })),
        Err(e) => app_err(e, 409),
    }
}

fn reset(req: &Request) -> Response {
    if req.query.get("confirm").map(String::as_str) != Some(RESET_CONFIRM) {
        return err(
            400,
            "bad_request",
            format!("a reset needs a second key: ?confirm={RESET_CONFIRM}"),
        );
    }
    let report = crate::reset_config();
    let removed = report.iter().filter(|r| r.starts_with("removed ")).count();
    let warnings: Vec<&String> = report
        .iter()
        .filter(|r| r.starts_with("warning:"))
        .collect();
    ok(json!({
        "removed": removed,
        "report": report,
        "warnings": warnings.len(),
        "next": "reboot to apply",
    }))
}

fn edgerouter_status() -> Response {
    let st = edgerouter::status();
    let h = st.health();
    ok(json!({
        "health": h.label(),
        "message": h.message(),
        "installed": st.binary.is_some(),
        "token": st.token,
        "tunnel_up": st.tunnel_up,
        "running": st.running,
        "connected": st.connected,
        "version": st.version,
        "last_handshake": st.last_handshake,
        "device": st.device,
        "account": st.account,
        "plan": st.plan,
        "endpoint": st.endpoint,
        "addresses": st.addresses,
        "error": st.error,
    }))
}

/// A temp path the bundle body is written to (0600), removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        Scratch(std::env::temp_dir().join(format!(
            "wayang-api-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `POST /v1/edgerouter/apply[?force=1]` — the body is the Edge bundle, as a
/// `.tar.gz` (what `wayang edgerouter apply` takes) or as JSON
/// `{"router_toml": "...", "fw_toml": "...", "token": "..."?}`. Like the CLI it
/// writes the configs and enrols the token and **applies nothing**: the tools'
/// own commit-confirm is the next step.
fn edgerouter_apply(req: &Request) -> Response {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if req.body.is_empty() {
        return err(
            400,
            "bad_request",
            "the body must be an Edge bundle (.tar.gz or JSON)",
        );
    }
    let force = req
        .query
        .get("force")
        .is_some_and(|v| v == "1" || v == "true");
    let scratch = Scratch::new("bundle");
    let target: PathBuf = if req.body.first() == Some(&b'{') {
        let v: Value = match serde_json::from_slice(&req.body) {
            Ok(v) => v,
            Err(e) => {
                return err(
                    400,
                    "bad_request",
                    format!("the JSON body does not parse: {e}"),
                );
            }
        };
        let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(router), Some(fw)) = (field("router_toml"), field("fw_toml")) else {
            return err(
                400,
                "bad_request",
                "the JSON body needs router_toml and fw_toml",
            );
        };
        let dir = &scratch.0;
        if let Err(e) = std::fs::create_dir_all(dir) {
            return err(500, "internal", format!("{}: {e}", dir.display()));
        }
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        let mut files = vec![("router.toml", router), ("fw.toml", fw)];
        if let Some(t) = field("token") {
            files.push(("token", t));
        }
        for (name, text) in files {
            if let Err(e) = std::fs::write(dir.join(name), text) {
                return err(500, "internal", format!("{name}: {e}"));
            }
        }
        dir.clone()
    } else {
        // the extension tells the unpacker what it is; the content must be gzip
        if req.body.get(..2) != Some(&[0x1f, 0x8b]) {
            return err(
                400,
                "bad_request",
                "the body is neither JSON nor a gzip (.tar.gz) bundle",
            );
        }
        let path = scratch.0.with_extension("tar.gz");
        let wrote = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, &req.body));
        if let Err(e) = wrote {
            return err(500, "internal", format!("{}: {e}", path.display()));
        }
        path
    };
    let result = edgerouter::apply(&target, force);
    let _ = std::fs::remove_file(&target);
    match result {
        Ok(r) => ok(json!({
            "router_config": r.router_config,
            "fw_config": r.fw_config,
            "router_backup": r.router_backup,
            "fw_backup": r.fw_backup,
            "enrolled": r.enrolled,
            "wg_keys_written": r.keys_written.len(),
            "applied": false,
            "next": "wayang-router check/commit and wayang-fw check/commit (commit-confirm), then `wayang edgerouter start`",
        })),
        // a refusal to overwrite, a bad config: the bundle is not usable as sent
        Err(e) => err(409, "conflict", e),
    }
}

fn list_ssh_keys() -> Response {
    let keys: Vec<Value> = sshkeys::load()
        .iter()
        .map(|k| {
            json!({
                "kind": k.kind(),
                "algo": k.algo,
                "fingerprint": k.fingerprint(),
                "comment": k.comment,
                "source": k.source,
            })
        })
        .collect();
    ok(json!({ "keys": keys }))
}

fn add_ssh_key(req: &Request) -> Response {
    let Ok(text) = std::str::from_utf8(&req.body) else {
        return err(
            400,
            "bad_request",
            "the body must be one public key line (UTF-8)",
        );
    };
    let line = text.trim();
    if line.is_empty() || line.len() > MAX_KEY_LINE || line.contains('\n') {
        return err(
            400,
            "bad_request",
            "the body must be exactly one public key line",
        );
    }
    let Some(key) = SshKey::parse(line, "api") else {
        return err(422, "validation", "not a valid SSH public key line");
    };
    match sshkeys::save_add(std::slice::from_ref(&key)) {
        Ok(added) => ok(json!({ "added": added, "fingerprint": key.fingerprint() })),
        Err(e) => err(500, "internal", e),
    }
}

/// `POST /v1/ssh-keys/remove` — the body is a fingerprint (`SHA256:…`). Refuses
/// to remove the last key (SSH is how the box is reached) unless
/// `?allow_empty=yes`.
fn remove_ssh_key(req: &Request) -> Response {
    let fp = String::from_utf8_lossy(&req.body).trim().to_string();
    if !fp.starts_with("SHA256:") || fp.len() > 128 {
        return err(
            400,
            "bad_request",
            "the body must be a fingerprint, SHA256:…",
        );
    }
    let keys = sshkeys::load();
    let Some(key) = keys.iter().find(|k| k.fingerprint() == fp) else {
        return err(404, "not_found", format!("no authorized key {fp}"));
    };
    let others = keys.iter().filter(|k| k.blob != key.blob).count();
    if others == 0 && req.query.get("allow_empty").map(String::as_str) != Some("yes") {
        return err(
            409,
            "conflict",
            "that is the last authorized key: removing it would lock SSH out (?allow_empty=yes to do it anyway)",
        );
    }
    match sshkeys::remove(&key.blob) {
        Ok(removed) => ok(json!({ "removed": removed, "fingerprint": fp, "remaining": others })),
        Err(e) => err(500, "internal", e),
    }
}

impl App for WayangApp {
    fn name(&self) -> &str {
        "wayang"
    }

    fn health(&self) -> Value {
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "os": version::read().ok(),
            "tls": self.tls,
            "mtls": self.mtls,
            "rw": self.rw,
        })
    }

    fn route(&self, method: &str, path: &str) -> Route {
        let (want, need) = match path {
            "/v1/status" | "/v1/update/check" | "/v1/net" | "/v1/edgerouter/status" => {
                ("GET", Need::Read)
            }
            "/v1/ssh-keys" => {
                return match method {
                    "GET" => Route::Known(Need::Read),
                    "POST" => Route::Known(Need::Admin),
                    _ => Route::WrongMethod("GET, POST"),
                };
            }
            "/v1/update"
            | "/v1/update/boot-other"
            | "/v1/update/confirm"
            | "/v1/update/rollback"
            | "/v1/reset"
            | "/v1/edgerouter/apply"
            | "/v1/ssh-keys/remove" => ("POST", Need::Admin),
            _ => return Route::Unknown,
        };
        if method == want {
            Route::Known(need)
        } else {
            Route::WrongMethod(want)
        }
    }

    fn handle(&self, req: &Request, _who: &Principal) -> Response {
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/v1/status") => read_status(self),
            ("GET", "/v1/update/check") => update_check(self),
            ("GET", "/v1/net") => read_net(),
            ("GET", "/v1/ssh-keys") => list_ssh_keys(),
            ("GET", "/v1/edgerouter/status") => edgerouter_status(),
            ("POST", "/v1/update") => update_stage(self),
            ("POST", "/v1/update/boot-other") => {
                slot_action(self, "boot-other", crate::staging::stage_other)
            }
            ("POST", "/v1/update/confirm") => slot_action(self, "confirm", crate::staging::mark_ok),
            ("POST", "/v1/update/rollback") => {
                slot_action(self, "rollback", crate::staging::rollback)
            }
            ("POST", "/v1/reset") => reset(req),
            ("POST", "/v1/edgerouter/apply") => edgerouter_apply(req),
            ("POST", "/v1/ssh-keys") => add_ssh_key(req),
            ("POST", "/v1/ssh-keys/remove") => remove_ssh_key(req),
            _ => err(404, "not_found", format!("no route for {}", req.path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grubenv::GrubEnv;
    use std::io::{Read, Write};
    use std::sync::{Arc, MutexGuard};
    use wayang_api::{Options, Outcome, Scope, Server, TokenStore};

    /// A throwaway `WAYANG_ROOT` with a boot tree and an installed version,
    /// holding the process-wide env lock for as long as it lives.
    struct Fx {
        _lock: MutexGuard<'static, ()>,
        dir: PathBuf,
    }

    impl Fx {
        fn new(tag: &str) -> Fx {
            let lock = crate::paths::TEST_ENV
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir()
                .join(format!("wayang-api-routes-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("boot/grub")).unwrap();
            std::fs::create_dir_all(dir.join("boot/var")).unwrap();
            std::fs::create_dir_all(dir.join("etc/wayang")).unwrap();
            std::fs::create_dir_all(dir.join("data")).unwrap();
            std::fs::write(dir.join("etc/wayang/version"), "1.0.30\n").unwrap();
            let mut env = GrubEnv::default();
            env.set("wayang_slot", "A");
            env.set("wayang_good", "A");
            env.set("wayang_attempts", "0");
            env.write(&dir.join("boot/grub/grubenv")).unwrap();
            // SAFETY: the TEST_ENV lock serialises every test that touches the
            // process environment; Drop restores it.
            unsafe { std::env::set_var("WAYANG_ROOT", &dir) };
            Fx { _lock: lock, dir }
        }

        fn app(&self) -> WayangApp {
            WayangApp::new(false, false, true)
        }

        fn grub(&self, key: &str) -> Option<String> {
            let b = std::fs::read(self.dir.join("boot/grub/grubenv")).unwrap();
            GrubEnv::from_bytes(&b).unwrap().map.get(key).cloned()
        }
    }

    impl Drop for Fx {
        fn drop(&mut self) {
            // SAFETY: see Fx::new.
            unsafe {
                std::env::remove_var("WAYANG_ROOT");
                std::env::remove_var("WAYANG_REPO_URL");
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn admin() -> Principal {
        Principal {
            label: "t".into(),
            scope: Scope::Admin,
        }
    }

    fn call(app: &WayangApp, req: Request) -> Response {
        app.handle(&req, &admin())
    }

    const ED1: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPX2zdvjV01sS6kSNEuFCraBHx+RtIL5q/nnODxc+Dys alice@box";
    const ED2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKDgLaeocpxjyYcVfzkVhNoYwyzzw0TqveMdqGKqXB6 bob@laptop";
    const ROUTER_TOML: &str = "[[interface]]\nname = \"lan\"\n[[policy]]\nname = \"lan-out\"\n";
    const FW_TOML: &str = "[[zone]]\nname = \"lan\"\n[[policy]]\nname = \"lan-out\"\n";

    #[test]
    fn every_route_declares_what_it_needs() {
        let a = WayangApp {
            boot_root: None,
            tls: false,
            mtls: false,
            rw: true,
        };
        for p in [
            "/v1/status",
            "/v1/update/check",
            "/v1/net",
            "/v1/edgerouter/status",
        ] {
            assert_eq!(a.route("GET", p), Route::Known(Need::Read), "{p}");
            assert_eq!(a.route("POST", p), Route::WrongMethod("GET"), "{p}");
        }
        assert_eq!(a.route("GET", "/v1/ssh-keys"), Route::Known(Need::Read));
        assert_eq!(a.route("POST", "/v1/ssh-keys"), Route::Known(Need::Admin));
        assert_eq!(
            a.route("PUT", "/v1/ssh-keys"),
            Route::WrongMethod("GET, POST")
        );
        for p in [
            "/v1/update",
            "/v1/update/boot-other",
            "/v1/update/confirm",
            "/v1/update/rollback",
            "/v1/reset",
            "/v1/edgerouter/apply",
            "/v1/ssh-keys/remove",
        ] {
            assert_eq!(
                a.route("POST", p),
                Route::Known(Need::Admin),
                "{p}: changing the box is admin"
            );
            assert_eq!(a.route("GET", p), Route::WrongMethod("POST"), "{p}");
        }
        assert_eq!(a.route("GET", "/v1/nope"), Route::Unknown);
    }

    #[test]
    fn status_is_the_cli_status_plus_the_host_facts() {
        let fx = Fx::new("status");
        let r = call(&fx.app(), Request::new("GET", "/v1/status"));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let d = &r.json()["data"];
        assert_eq!(d["version"], "1.0.30");
        assert_eq!(d["active_slot"], "A");
        assert_eq!(d["update_pending"], false);
        assert!(d["slots"].is_array() && d["tools"].is_object() && d["net"].is_object());
        assert_eq!(d["wayang"], env!("CARGO_PKG_VERSION"));
        let h = fx.app().health();
        assert_eq!(
            (h["rw"].as_bool(), h["tls"].as_bool()),
            (Some(true), Some(false))
        );
    }

    #[test]
    fn a_staged_update_is_a_boot_next_that_is_not_the_running_slot() {
        let mut st = status::unavailable();
        assert!(!update_pending(&st));
        st.boot_next = st.active.idle();
        assert!(update_pending(&st));
    }

    #[test]
    fn slot_actions_use_the_cli_staging_and_never_reboot() {
        let fx = Fx::new("slots");
        let app = fx.app();
        let r = call(&app, Request::new("POST", "/v1/update/boot-other"));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.json()["data"]["slot"], "B");
        assert_eq!(r.json()["data"]["rebooted"], false);
        assert_eq!(fx.grub("wayang_slot").as_deref(), Some("B"));
        // rolling back returns to the slot that was running before
        let r = call(&app, Request::new("POST", "/v1/update/rollback"));
        assert_eq!(
            (r.status, r.json()["data"]["slot"].as_str()),
            (200, Some("A"))
        );
        assert_eq!(fx.grub("wayang_slot").as_deref(), Some("A"));
        let r = call(&app, Request::new("POST", "/v1/update/confirm"));
        assert_eq!(r.status, 200);
        assert_eq!(r.json()["data"]["action"], "confirm");
        // no boot tree at all: a plain 409, not a crash
        let none = WayangApp {
            boot_root: Some(fx.dir.join("nowhere")),
            tls: false,
            mtls: false,
            rw: true,
        };
        assert_eq!(
            call(&none, Request::new("POST", "/v1/update/boot-other")).status,
            409
        );
    }

    #[test]
    fn ssh_keys_list_add_remove_and_the_last_one_is_protected() {
        let fx = Fx::new("ssh");
        let app = fx.app();
        let keys = |app: &WayangApp| {
            call(app, Request::new("GET", "/v1/ssh-keys")).json()["data"]["keys"]
                .as_array()
                .unwrap()
                .clone()
        };
        assert!(keys(&app).is_empty());
        // add: one line, validated
        let r = call(&app, Request::new("POST", "/v1/ssh-keys").body(ED1));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.json()["data"]["added"], 1);
        let fp1 = r.json()["data"]["fingerprint"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(fp1.starts_with("SHA256:"));
        assert_eq!(
            call(&app, Request::new("POST", "/v1/ssh-keys").body(ED1)).json()["data"]["added"],
            0,
            "already there"
        );
        assert_eq!(
            call(&app, Request::new("POST", "/v1/ssh-keys").body("not a key")).status,
            422
        );
        assert_eq!(
            call(
                &app,
                Request::new("POST", "/v1/ssh-keys").body(format!("{ED1}\n{ED2}"))
            )
            .status,
            400
        );
        assert_eq!(
            call(&app, Request::new("POST", "/v1/ssh-keys").body("")).status,
            400
        );
        let l = keys(&app);
        assert_eq!(l.len(), 1);
        assert_eq!(
            (l[0]["kind"].as_str(), l[0]["comment"].as_str()),
            (Some("ED25519"), Some("alice@box"))
        );
        assert!(l[0].get("blob").is_none(), "only the fingerprint is listed");
        // the last key cannot be removed by accident
        let r = call(
            &app,
            Request::new("POST", "/v1/ssh-keys/remove").body(fp1.clone()),
        );
        assert_eq!(r.status, 409);
        assert!(
            r.json()["error"]["message"]
                .as_str()
                .unwrap()
                .contains("lock SSH out")
        );
        assert_eq!(keys(&app).len(), 1);
        // with a second key the first can go
        call(&app, Request::new("POST", "/v1/ssh-keys").body(ED2));
        let r = call(
            &app,
            Request::new("POST", "/v1/ssh-keys/remove").body(fp1.clone()),
        );
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.json()["data"]["remaining"], 1);
        assert_eq!(keys(&app).len(), 1);
        assert_eq!(
            call(&app, Request::new("POST", "/v1/ssh-keys/remove").body(fp1)).status,
            404
        );
        assert_eq!(
            call(
                &app,
                Request::new("POST", "/v1/ssh-keys/remove").body("nope")
            )
            .status,
            400
        );
        // ...and the very last one only with an explicit say-so
        let fp2 = keys(&app)[0]["fingerprint"].as_str().unwrap().to_string();
        let r = call(
            &app,
            Request::new("POST", "/v1/ssh-keys/remove?allow_empty=yes").body(fp2),
        );
        assert_eq!(r.status, 200);
        assert!(keys(&app).is_empty());
    }

    #[test]
    fn reset_needs_the_second_key_and_then_does_what_the_cli_does() {
        let fx = Fx::new("reset");
        let app = fx.app();
        for d in ["router", "fw", "wayangi", "network"] {
            std::fs::create_dir_all(fx.dir.join("data/etc").join(d)).unwrap();
            std::fs::write(fx.dir.join("data/etc").join(d).join("x"), "x").unwrap();
        }
        std::fs::create_dir_all(fx.dir.join("data/bin")).unwrap();
        for q in [
            "/v1/reset",
            "/v1/reset?confirm=yes",
            "/v1/reset?confirm=yes-reset-please",
        ] {
            assert_eq!(call(&app, Request::new("POST", q)).status, 400, "{q}");
        }
        assert!(
            fx.dir.join("data/etc/router/x").exists(),
            "nothing was removed"
        );
        let r = call(&app, Request::new("POST", "/v1/reset?confirm=yes-reset"));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.json()["data"]["removed"], 4);
        for d in ["router", "fw", "wayangi", "network"] {
            assert!(!fx.dir.join("data/etc").join(d).exists(), "{d}");
        }
        assert!(
            fx.dir.join("data/bin").exists(),
            "/data/bin is kept, like the CLI"
        );
        assert_eq!(
            fx.grub("wayang_slot").as_deref(),
            Some("A"),
            "a pending update is cleared"
        );
    }

    fn tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut t = tar::Builder::new(gz);
        for (name, text) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(text.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            t.append_data(&mut h, name, text.as_bytes()).unwrap();
        }
        t.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn edge_bundles_are_installed_as_files_and_never_applied() {
        let fx = Fx::new("edge");
        let app = fx.app();
        // as a .tar.gz
        let gz = tar_gz(&[
            ("bundle/router.toml", ROUTER_TOML),
            ("bundle/fw.toml", FW_TOML),
        ]);
        let r = call(
            &app,
            Request::new("POST", "/v1/edgerouter/apply").body(gz.clone()),
        );
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let d = &r.json()["data"];
        assert_eq!(d["applied"], false);
        assert_eq!(d["enrolled"], false);
        assert!(d["next"].as_str().unwrap().contains("commit-confirm"));
        let rc = fx.dir.join("data/etc/router/config.toml");
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), ROUTER_TOML);
        assert!(fx.dir.join("data/etc/fw/config.toml").exists());
        // a second bundle is refused, never silently replacing a config...
        let r = call(&app, Request::new("POST", "/v1/edgerouter/apply").body(gz));
        assert_eq!(r.status, 409);
        // ...unless forced, which keeps the old one as .bak
        let json = serde_json::json!({
            "router_toml": "[[interface]]\nname = \"wan\"\n",
            "fw_toml": FW_TOML,
            "token": "0123456789abcdef0123456789abcdef",
        });
        let r = call(
            &app,
            Request::new("POST", "/v1/edgerouter/apply?force=1").body(json.to_string()),
        );
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.json()["data"]["enrolled"], true);
        assert!(std::fs::read_to_string(&rc).unwrap().contains("wan"));
        assert!(fx.dir.join("data/etc/router/config.toml.bak").exists());
        assert!(
            !r.json().to_string().contains("0123456789abcdef"),
            "the token is never echoed"
        );
        // bad bodies
        let bad = |b: &str| call(&app, Request::new("POST", "/v1/edgerouter/apply").body(b));
        assert_eq!(bad("").status, 400);
        assert_eq!(bad("plain text").status, 400);
        assert_eq!(bad("{\"router_toml\": \"x\"}").status, 400);
        assert_eq!(bad("{not json").status, 400);
        let r = call(
            &app,
            Request::new("POST", "/v1/edgerouter/apply?force=1")
                .body(r#"{"router_toml":"nothing","fw_toml":"nothing"}"#),
        );
        assert_eq!(
            r.status, 409,
            "configs that do not look like configs are refused"
        );
    }

    #[test]
    fn edgerouter_status_and_net_answer_without_the_hardware() {
        let fx = Fx::new("reads");
        let app = fx.app();
        let r = call(&app, Request::new("GET", "/v1/edgerouter/status"));
        assert_eq!(r.status, 200);
        let d = &r.json()["data"];
        assert!(d["health"].is_string() && d["message"].is_string());
        assert_eq!(d["token"], false);
        let r = call(&app, Request::new("GET", "/v1/net"));
        assert_eq!(r.status, 200);
        assert!(r.json()["data"]["interfaces"].is_array());
        std::fs::create_dir_all(fx.dir.join("data/etc/network")).unwrap();
        std::fs::write(
            fx.dir.join("data/etc/network/config"),
            "MODE=static\nIPV4_DNS=\"1.1.1.1\"\n",
        )
        .unwrap();
        let r = call(&app, Request::new("GET", "/v1/net"));
        assert_eq!(r.json()["data"]["config"]["MODE"], "static");
        assert_eq!(r.json()["data"]["config"]["IPV4_DNS"], "1.1.1.1");
    }

    /// A one-shot HTTP server answering every request with `manifest`.
    fn channel(manifest: String, requests: usize) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..requests {
                let Ok((mut s, _)) = l.accept() else { return };
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{manifest}",
                    manifest.len()
                );
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn manifest(version: &str) -> String {
        serde_json::json!({
            "product": "wayangos", "channel": "stable", "version": version, "major": 1,
            "arch": crate::arch::host_arch(), "kernel_sha256": "00", "initramfs_sha256": "00",
            "notes": "test channel",
        })
        .to_string()
    }

    #[test]
    fn update_check_reads_the_channel_without_downloading_or_touching_the_slots() {
        let fx = Fx::new("check");
        let app = fx.app();
        // SAFETY: held under the TEST_ENV lock; Fx::drop removes it.
        unsafe { std::env::set_var("WAYANG_REPO_URL", channel(manifest("1.0.32"), 1)) };
        let r = call(&app, Request::new("GET", "/v1/update/check"));
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        let d = &r.json()["data"];
        assert_eq!(
            (d["installed"].as_str(), d["bundle"].as_str()),
            (Some("1.0.30"), Some("1.0.32"))
        );
        assert_eq!(d["available"], true);
        assert_eq!(d["notes"], "test channel");
        assert_eq!(fx.grub("wayang_slot").as_deref(), Some("A"));
        // already current
        unsafe { std::env::set_var("WAYANG_REPO_URL", channel(manifest("1.0.30"), 2)) };
        let r = call(&app, Request::new("GET", "/v1/update/check"));
        assert_eq!(r.json()["data"]["available"], false);
        // and POST /v1/update refuses to stage what is not newer
        let r = call(&app, Request::new("POST", "/v1/update"));
        assert_eq!(
            (r.status, r.json()["error"]["code"].as_str()),
            (409, Some("conflict"))
        );
        // an unreachable channel is a 502, with the reason
        unsafe { std::env::set_var("WAYANG_REPO_URL", "http://127.0.0.1:9") };
        let r = call(&app, Request::new("GET", "/v1/update/check"));
        assert_eq!(r.status, 502);
    }

    // ------------------------------------------------- through the real pipeline

    #[test]
    fn scopes_and_the_write_switch_gate_every_changing_route() {
        let fx = Fx::new("pipeline");
        let tokens = TokenStore::single(Scope::Ro, "dash", "r".repeat(32).as_str())
            .with(Scope::Rw, "ci", "w".repeat(32).as_str())
            .with(Scope::Admin, "ops", "a".repeat(32).as_str());
        let mk = |rw: bool| {
            Server::new(
                Arc::new(WayangApp::new(false, false, rw)),
                Options {
                    tokens: Some(tokens.clone()),
                    rw,
                    rate: None,
                    ..Options::default()
                },
            )
            .unwrap()
        };
        let go = |s: &Server, m: &str, p: &str, tok: &str| match s
            .dispatch(&Request::new(m, p).bearer(tok))
        {
            Outcome::Response(r) => r,
            Outcome::Events { .. } => panic!("stream"),
        };
        let (ro, rw, ad) = ("r".repeat(32), "w".repeat(32), "a".repeat(32));
        let s = mk(true);
        assert_eq!(
            go(&s, "GET", "/v1/status", &ro).status,
            200,
            "a read-only token reads"
        );
        for tok in [&ro, &rw] {
            for p in [
                "/v1/update",
                "/v1/update/boot-other",
                "/v1/reset",
                "/v1/ssh-keys/remove",
                "/v1/edgerouter/apply",
            ] {
                let r = go(&s, "POST", p, tok);
                assert_eq!(
                    (r.status, r.json()["error"]["code"].as_str()),
                    (403, Some("forbidden")),
                    "{p}"
                );
            }
        }
        // admin gets through to the handler: a reset without its second key is a 400
        assert_eq!(go(&s, "POST", "/v1/reset", &ad).status, 400);
        assert_eq!(
            fx.grub("wayang_slot").as_deref(),
            Some("A"),
            "nothing moved"
        );
        // a read-only server refuses even admin
        let ro_server = mk(false);
        let r = go(&ro_server, "POST", "/v1/update/boot-other", &ad);
        assert_eq!(
            (r.status, r.json()["error"]["code"].as_str()),
            (403, Some("read_only"))
        );
        assert_eq!(fx.grub("wayang_slot").as_deref(), Some("A"));
        // health shows the write switch to anyone
        let h = match s.dispatch(&Request::new("GET", "/v1/health")) {
            Outcome::Response(r) => r.json(),
            _ => panic!(),
        };
        assert_eq!(h["data"]["rw"], true);
        assert_eq!(h["data"]["mtls"], false);
    }
}
