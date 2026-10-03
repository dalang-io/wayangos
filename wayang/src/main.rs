//! `wayang` — WayangOS A/B updater CLI.
//!
//! See `docs/UPDATE-DESIGN.md` for the frozen interfaces and exit codes.

mod api;
mod api_routes;
mod arch;
mod bundle;
mod cli;
mod edgerouter;
mod error;
mod esp;
mod fetch;
mod grubenv;
mod hash;
mod help;
mod hud;
mod input;
mod keys;
mod manifest;
mod mount;
mod net;
mod netui;
mod paths;
mod review;
mod screen;
mod sema;
mod sign;
mod slot;
mod sshkeys;
mod sshkeysui;
mod staging;
mod state;
mod status;
mod sys;
mod trusted;
mod tui;
mod ui;
mod update;
mod verify;
mod version;
mod wifi;
mod wifiui;

use std::io::IsTerminal;
use std::process::ExitCode;

use cli::Command;
use error::Result;

const HELP: &str = "\
wayang — WayangOS updater

usage:
  wayang                         interactive HUD (on a TTY)
  wayang status [--json]
  wayang update  [--check] [--from FILE.wup] [--channel C] [--esp DEV] [--reboot]
  wayang upgrade [--check] [--from FILE.wup] [--esp DEV] [--reboot]
  wayang update --rollback [--esp DEV] [--reboot]
  wayang update --boot-other [--esp DEV] [--reboot]   boot the idle slot next (one-shot)
  wayang update --fallback [--esp DEV] [--reboot]     boot the last good slot next (exit 1 if running it)
  wayang net                            runtime uplink HUD (TTY only)
  wayang wifi                           runtime wifi HUD (TTY only)
  wayang edgerouter status               wayangi EdgeRouter agent/token/tunnel state
  wayang edgerouter start                start the wayangi agent (brings up the tunnel)
  wayang edgerouter stop                 stop the running wayangi agent
  wayang edgerouter restart              stop then start the wayangi agent
  wayang edgerouter enroll TOKEN         save the dashboard device token (survives updates)
  wayang edgerouter clear                forget the saved token
  wayang edgerouter apply BUNDLE [--force]  install a wayangi Edge bundle (dir or .tar.gz)
  wayang keygen --out DIR [--keyid NAME]
  wayang sign   --key FILE [--keyid NAME] MANIFEST.json
  wayang verify FILE.wup [--esp DEV]
  wayang mark-ok [--esp DEV]
  wayang reset [--yes]               reset config to defaults (router/fw/wayangi/network; clears a pending update)
  wayang addkey github:USER | gitlab:USER | FILE | 'ssh-ed25519 AAAA... comment'
  wayang api [--listen ADDR] [--rw] [--token-file F] [--tls-cert F --tls-key F [--client-ca F]]
                                     HTTP/JSON API (read-only unless --rw; default 127.0.0.1:8632)
  wayang api --gen-token [--scope ro|rw|admin] [--label WORD] [--append]   write an API token
  wayang --demo [--screens DIR [--svg]] [--size COLSxROWS]   render HUD screens (text, SVG)

env:
  WAYANG_ESP        ESP partition device override
  WAYANG_ARCH       override the host arch token (x86_64 | arm64)
  WAYANG_REPO_URL   release base URL (default: GitHub releases)
  WAYANG_ROOT       relocate /etc/wayang and /boot (testing)

exit codes: 0 ok · 1 error · 2 no update · 3 verify failure · 4 incompatible";

const NET_HELP: &str = "\
wayang net — runtime uplink configuration (interactive)

usage:
  wayang net        open the network HUD (requires a TTY)

Lists interfaces from /sys/class/net, lets you pick DHCP or a static
IPv4/IPv6/both configuration, then writes /data/etc/network/primary and
/data/etc/network/config and applies it immediately.

When stdout is not a TTY this help is printed instead.";

const WIFI_HELP: &str = "\
wayang wifi — runtime wireless configuration (interactive)

usage:
  wayang wifi            open the wifi HUD (requires a TTY)
  wayang wifi detect     list WiFi hardware (works without a bound driver)
  wayang wifi detect --json

Lists wireless interfaces, scans with `iw`, and connects to a chosen SSID,
persisting /data/etc/wpa_supplicant.conf and pinning the interface as primary.

`detect` reads /sys (and /sys/bus/usb) to show the interface/driver and the
USB vendor:product of unbound adapters, with a driver+firmware suggestion.
Set WAYANG_SYS to a fake sysfs root to inspect an offline image.

Requires `iw` and `wpa_supplicant` for connect (see docs/NETWORK.md).";

const EDGEROUTER_HELP: &str = "\
wayang edgerouter — enrol this box on the wayangi dashboard

usage:
  wayang edgerouter status        show the agent, token and tunnel state
  wayang edgerouter start         start the wayangi agent (brings up the tunnel)
  wayang edgerouter stop          stop the running wayangi agent
  wayang edgerouter restart       stop then start the wayangi agent
  wayang edgerouter enroll TOKEN  save the per-device token from the dashboard
  wayang edgerouter clear         forget the saved token
  wayang edgerouter apply BUNDLE [--force]
                                  install a wayangi Edge bundle: a directory or a
                                  .tar.gz with router.toml + fw.toml (and maybe a
                                  token/install.sh). Writes /data/etc/router/config.toml
                                  and /data/etc/fw/config.toml (mode 0644; refuses to
                                  overwrite without --force, keeping a .bak when forced)
                                  and enrols the bundle token. The configs are NOT
                                  applied: wayang-router/fw use commit-confirm, so run
                                  their `check` then `commit` yourself.

The token is written to /data/etc/wayangi/token (mode 600) and survives OS
updates; it is never baked into the image. The agent runs with its state dir on
/data (`--conf-dir /data/etc/wayangi`) and reads the token from the environment
(never argv, so it can't leak through `ps`). `start`/`stop`/`restart` run in the
background and print the resulting state. The boot step in /etc/init.d/edgerouter
starts the same agent when both the binary and the token are present, so the
wayangi0 tunnel comes up on its own after a reboot. Status reads the agent's own
state under /data/etc/wayangi/ (device, plan, addresses, handshake) and reports
a missing/rejected token or a down tunnel with a clear next step; no secrets are
shown. See docs/EDGEROUTER.md §C.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if let Some(code) = demo_mode(&args) {
        return code;
    }

    // No subcommand on a TTY opens the HUD; otherwise print help (pipeable).
    if args.is_empty() {
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return match tui::run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("wayang: {e}");
                    ExitCode::from(1)
                }
            };
        }
        restore_sigpipe();
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }

    // CLI subcommands write to stdout/stderr, which may be a pipe. Restore the
    // default SIGPIPE disposition (Rust ignores it) so `wayang status | head`
    // exits cleanly instead of aborting. The HUD above keeps Rust's behaviour.
    restore_sigpipe();

    let command = match cli::parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("wayang: {e}");
            return ExitCode::from(1);
        }
    };

    let result: Result<i32> = match command {
        Command::Help => {
            println!("{HELP}");
            Ok(0)
        }
        Command::PrintVersion => {
            println!("{}", version_line());
            Ok(0)
        }
        Command::Version => version::read().map(|v| {
            println!("{v}");
            0
        }),
        Command::Status { json } => status::run(json, None),
        Command::Update(a) => update::run(false, &a),
        Command::Upgrade(a) => update::run(true, &a),
        Command::Net => run_screen(NET_HELP, netui::run),
        Command::Wifi => run_screen(WIFI_HELP, wifiui::run),
        Command::WifiDetect { json } => wifi::detect_cmd(json),
        Command::EdgeRouter(cli::EdgeRouterAction::Help) => {
            println!("{EDGEROUTER_HELP}");
            Ok(0)
        }
        Command::EdgeRouter(action) => edgerouter::run(action),
        Command::Keygen { out, keyid } => keys::keygen(&out, &keyid),
        Command::Sign {
            key,
            keyid,
            manifest,
        } => keys::sign_file(&key, keyid.as_deref(), &manifest),
        Command::Verify { bundle, esp } => verify::run(&bundle, esp.as_deref()),
        Command::MarkOk { esp } => run_mark_ok(esp.as_deref()),
        Command::Reset { yes } => run_reset(yes),
        Command::AddKey { spec } => sshkeys::addkey_cmd(&spec).map_err(error::AppError::err),
        Command::Api(a) => api::run(&a).map_err(error::AppError::err),
    };

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("wayang: {e}");
            ExitCode::from(e.code as u8)
        }
    }
}

/// The `wayang --version` line: the crate semver plus the CLI build marker
/// (`ux-p1`). The crate version mirrors the OS product, so the marker is how a
/// HUD/CLI revision is identified without touching it.
fn version_line() -> String {
    format!(
        "wayang {} ({})",
        env!("CARGO_PKG_VERSION"),
        version::BUILD_MARKER
    )
}

/// Rust ignores `SIGPIPE`, so a write to a closed pipe makes the process abort
/// with a "Broken pipe" error (`wayang status | head`). Restore the default
/// disposition on the CLI subcommand paths (never the HUD, which owns the
/// terminal) so piping behaves like every other Unix tool.
#[cfg(unix)]
fn restore_sigpipe() {
    // SAFETY: installing the default handler is async-signal-safe and there is
    // no handler to race with.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe() {}

/// `--demo` / `--screens DIR` render the HUD to text without a terminal.
fn demo_mode(args: &[String]) -> Option<ExitCode> {
    let flag = |f: &str| args.iter().any(|a| a == f);
    let value = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let size = value("--size").unwrap_or_else(|| "100x32".into());

    let result = if flag("--screens") {
        restore_sigpipe();
        match value("--screens") {
            Some(dir) => tui::dump_screens(&dir, &size, flag("--svg")),
            None => Err("--screens needs a directory".to_string()),
        }
    } else if flag("--demo") {
        restore_sigpipe();
        tui::dump_stdout(&size)
    } else {
        return None;
    };

    Some(match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wayang: {e}");
            ExitCode::from(1)
        }
    })
}

fn run_mark_ok(esp: Option<&str>) -> Result<i32> {
    let boot = mount::open(esp)?;
    let active = staging::mark_ok(&boot)?;
    println!("Good slot: {} (attempts reset)", active.as_str());
    Ok(0)
}

/// `wayang reset`: return this box's configuration to defaults. Removes the
/// imported router/firewall/wayangi/network config and clears any pending OS
/// update (the box boots the slot it is running). Never touches /data/bin,
/// /data/var (history) or the SSH keys. Requires the literal `yes` unless
/// `--yes`/`-y`/`--force` was given.
fn run_reset(yes: bool) -> Result<i32> {
    if !yes {
        use std::io::{BufRead, Write};
        eprintln!("This resets WayangOS config on this box to defaults:");
        eprintln!("  - removes /data/etc/router, /data/etc/fw, /data/etc/wayangi,");
        eprintln!("    /data/etc/network");
        eprintln!("  - clears any pending OS update (boots the running slot)");
        eprintln!("Kept: /data/bin, /data/var (history), SSH keys.");
        eprint!("Type 'yes' to continue: ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        if !line.trim().eq_ignore_ascii_case("yes") {
            println!("aborted (nothing changed)");
            return Ok(1);
        }
    }
    let report = reset_config();
    let removed = report.iter().filter(|r| r.starts_with("removed ")).count();
    for r in &report {
        if r.starts_with("warning:") {
            eprintln!("{r}");
        } else {
            println!("{r}");
        }
    }
    if removed == 0 {
        println!("Nothing to reset (already default).");
    } else {
        println!("Reset to defaults. Reboot to apply.");
    }
    Ok(0)
}

/// The mutating half of `run_reset`, shared with the HUD's REVIEW flow: remove
/// the config dirs and clear a pending update, collecting a report. Failures
/// become `warning:` lines (reset never aborts half-way), as the CLI always did.
pub(crate) fn reset_config() -> Vec<String> {
    let mut report = Vec::new();
    for d in [
        paths::router_dir(),
        paths::fw_dir(),
        paths::wayangi_dir(),
        paths::data_network_dir(),
    ] {
        if d.exists() {
            match std::fs::remove_dir_all(&d) {
                Ok(()) => report.push(format!("removed {}", d.display())),
                Err(e) => report.push(format!("warning: {}: {e}", d.display())),
            }
        }
    }
    match mount::open(None) {
        Ok(boot) => match staging::reset_to_running(&boot) {
            Ok(s) => report.push(format!("pending update cleared; boots slot {}", s.as_str())),
            Err(e) => report.push(format!("warning: update state: {e}")),
        },
        Err(e) => report.push(format!("warning: boot state not available: {e}")),
    }
    report
}

/// Run an interactive screen on a TTY, or print its usage when piped so
/// scripts are unaffected.
fn run_screen(help: &str, run: fn() -> std::io::Result<()>) -> Result<i32> {
    if !std::io::stdout().is_terminal() {
        println!("{help}");
        return Ok(0);
    }
    run().map_err(|e| error::AppError::err(e.to_string()))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_identifies_the_build() {
        let s = version_line();
        assert_eq!(
            s,
            format!("wayang {} (ux-p1)", env!("CARGO_PKG_VERSION")),
            "{s}"
        );
        assert!(s.starts_with("wayang 0.1.0 ("), "{s}");
    }
}
