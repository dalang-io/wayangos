//! `wayang` — WayangOS A/B updater CLI.
//!
//! See `docs/UPDATE-DESIGN.md` for the frozen interfaces and exit codes.

mod arch;
mod bundle;
mod cli;
mod error;
mod esp;
mod fetch;
mod grubenv;
mod hash;
mod hud;
mod input;
mod keys;
mod manifest;
mod mount;
mod net;
mod netui;
mod paths;
mod pos;
mod posui;
mod screen;
mod sema;
mod sign;
mod slot;
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
  wayang net                            runtime uplink HUD (TTY only)
  wayang wifi                           runtime wifi HUD (TTY only)
  wayang pos [status|start|stop|restart|enable|disable|log]
                                        point-of-sale kiosk service
                                        (also in the HUD: POS screen)
  wayang keygen --out DIR [--keyid NAME]
  wayang sign   --key FILE [--keyid NAME] MANIFEST.json
  wayang verify FILE.wup [--esp DEV]
  wayang mark-ok [--esp DEV]
  wayang --demo [--screens DIR] [--size COLSxROWS]   render HUD screens to text

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
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }

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
            println!("wayang {}", env!("CARGO_PKG_VERSION"));
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
        Command::Pos { action } => run_pos(&action),
        Command::Keygen { out, keyid } => keys::keygen(&out, &keyid),
        Command::Sign { key, keyid, manifest } => keys::sign_file(&key, keyid.as_deref(), &manifest),
        Command::Verify { bundle, esp } => verify::run(&bundle, esp.as_deref()),
        Command::MarkOk { esp } => run_mark_ok(esp.as_deref()),
    };

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("wayang: {e}");
            ExitCode::from(e.code as u8)
        }
    }
}

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
        match value("--screens") {
            Some(dir) => tui::dump_screens(&dir, &size),
            None => Err("--screens needs a directory".to_string()),
        }
    } else if flag("--demo") {
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

/// `wayang pos <action>` runs the rootfs service script, which owns the
/// supervisor, the autostart setting (/data/etc/pos.conf) and the log.
/// The HUD's POS screen shares this module (`crate::pos`).
fn run_pos(action: &str) -> Result<i32> {
    pos::run_action(action).map_err(error::AppError::err)
}

fn run_mark_ok(esp: Option<&str>) -> Result<i32> {
    let boot = mount::open(esp)?;
    let active = staging::mark_ok(&boot)?;
    println!("Good slot: {} (attempts reset)", active.as_str());
    Ok(0)
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
