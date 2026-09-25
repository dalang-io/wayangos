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
mod keys;
mod manifest;
mod mount;
mod paths;
mod sema;
mod sign;
mod slot;
mod staging;
mod status;
mod trusted;
mod update;
mod verify;
mod version;

use std::process::ExitCode;

use cli::Command;
use error::Result;

const HELP: &str = "\
wayang — WayangOS updater

usage:
  wayang version
  wayang status [--json]
  wayang update  [--check] [--from FILE.wup] [--channel C] [--esp DEV] [--reboot]
  wayang upgrade [--check] [--from FILE.wup] [--esp DEV] [--reboot]
  wayang update --rollback [--esp DEV] [--reboot]
  wayang keygen --out DIR [--keyid NAME]
  wayang sign   --key FILE [--keyid NAME] MANIFEST.json
  wayang verify FILE.wup [--esp DEV]
  wayang mark-ok [--esp DEV]

env:
  WAYANG_ESP        ESP partition device override
  WAYANG_REPO_URL   release base URL (default: GitHub releases)
  WAYANG_ROOT       relocate /etc/wayang and /boot (testing)

exit codes: 0 ok · 1 error · 2 no update · 3 verify failure · 4 incompatible";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
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

fn run_mark_ok(esp: Option<&str>) -> Result<i32> {
    let boot = mount::open(esp)?;
    let active = staging::mark_ok(&boot)?;
    println!("Good slot: {} (attempts reset)", active.as_str());
    Ok(0)
}
