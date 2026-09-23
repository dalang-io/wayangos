//! dcheck — device health check.
//!
//! M1: CLI skeleton, block-device enumeration (identity + capacity).
//! Storage health (native SMART) and the TUI arrive in later milestones.
//! See `docs/DCHECK.md`.

mod enumerate;
mod health;
mod json;
mod model;
mod native;
mod report;
mod smartctl;
mod tui;

use std::io::{self, IsTerminal, Write};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None => {
            if interactive() {
                run_tui(false)
            } else {
                interactive_menu(false)
            }
        }
        Some("-h") | Some("--help") | Some("help") => {
            print_help();
            0
        }
        Some("-V") | Some("--version") => {
            println!("dcheck {VERSION}");
            0
        }
        Some("storage") | Some("disk") => storage_cmd(&args[1..], false),
        Some("tui") => run_tui(false),
        Some("demo") => {
            if interactive() {
                run_tui(true)
            } else {
                interactive_menu(true)
            }
        }
        Some("ram") => coming_soon("RAM"),
        Some("cpu") => coming_soon("CPU"),
        Some(other) => {
            eprintln!("dcheck: unknown command '{other}'\n");
            print_help();
            2
        }
    }
}

fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn run_tui(force_demo: bool) -> i32 {
    let devices = load_devices(force_demo);
    match tui::run(devices) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("dcheck: TUI error: {err}");
            1
        }
    }
}

fn print_help() {
    println!(
        "dcheck {VERSION} — device health check

USAGE:
    dcheck                  Interactive menu (TUI on a terminal)
    dcheck storage          List attached storage devices
    dcheck storage <dev>    Report for one device (e.g. /dev/nvme0n1)
    dcheck tui              Force the terminal UI
    dcheck demo             Run with built-in sample devices (no sysfs needed)
    dcheck ram | cpu        Coming soon
    dcheck --version

On hosts without /sys (e.g. macOS) dcheck automatically falls back to demo data.
Storage listing includes devices that are attached but not mounted.
SMART reads require root."
    );
}

fn interactive_menu(force_demo: bool) -> i32 {
    loop {
        println!("\ndcheck {VERSION} — Device Health Check");
        println!("  1) Storage");
        println!("  2) RAM    (coming soon)");
        println!("  3) CPU    (coming soon)");
        println!("  q) Quit");
        print!("Select: ");
        let _ = io::stdout().flush();

        match read_line() {
            Some(input) => match input.as_str() {
                "1" | "storage" | "s" => {
                    storage_cmd(&[], force_demo);
                }
                "2" | "ram" | "r" => {
                    coming_soon("RAM");
                }
                "3" | "cpu" | "c" => {
                    coming_soon("CPU");
                }
                "q" | "quit" | "exit" | "" => return 0,
                other => eprintln!("Unknown choice '{other}'."),
            },
            None => return 0, // EOF
        }
    }
}

fn storage_cmd(args: &[String], session_demo: bool) -> i32 {
    let mut selector: Option<String> = None;
    let mut demo = session_demo;
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--demo" => demo = true,
            "--json" => json = true,
            "-h" | "--help" => {
                println!("Usage: dcheck storage [<device>] [--demo] [--json]");
                return 0;
            }
            flag if flag.starts_with('-') => {
                eprintln!("dcheck: unknown option '{flag}'");
                return 2;
            }
            value => {
                if selector.is_none() {
                    selector = Some(value.to_string());
                }
            }
        }
    }

    let devices = load_devices(demo);

    if let Some(sel) = selector {
        return match enumerate::find_device(&devices, &sel) {
            Some(dev) => {
                if json {
                    println!("{}", report::device_json(&dev).to_string());
                } else {
                    report::print_report(&dev);
                }
                0
            }
            None => {
                if json {
                    println!(
                        "{}",
                        crate::json::object(vec![(
                            "error",
                            crate::json::string(format!("device '{sel}' not found")),
                        )])
                        .to_string()
                    );
                } else {
                    eprintln!("dcheck: device '{sel}' not found. Attached devices:");
                    report::print_list(&devices);
                }
                1
            }
        };
    }

    if json {
        let items: Vec<crate::json::Json> =
            devices.iter().map(report::device_json_basic).collect();
        println!("{}", crate::json::Json::Arr(items).to_string());
        return 0;
    }

    report::print_list(&devices);

    if devices.is_empty() || !io::stdin().is_terminal() {
        return 0;
    }

    prompt_selection(&devices)
}

/// Load real devices, or fall back to built-in demo data when there is no sysfs
/// (e.g. running on macOS) unless explicitly asked otherwise.
fn load_devices(force_demo: bool) -> Vec<model::Device> {
    if force_demo || std::env::var_os("DCHECK_DEMO").is_some() {
        return enumerate::demo_devices();
    }
    let devices = enumerate::list_devices();
    if devices.is_empty() && !enumerate::has_sysfs() {
        eprintln!("note: no /sys on this host — showing built-in demo data (try `dcheck demo`).\n");
        return enumerate::demo_devices();
    }
    devices
}

/// Interactive picker used when `dcheck storage` runs on a terminal.
fn prompt_selection(devices: &[model::Device]) -> i32 {
    loop {
        print!("\nSelect [1-{}], name, or q: ", devices.len());
        let _ = io::stdout().flush();

        let input = match read_line() {
            Some(s) => s,
            None => return 0,
        };
        if input.is_empty() || input == "q" || input == "quit" {
            return 0;
        }

        if let Ok(index) = input.parse::<usize>() {
            if index >= 1 && index <= devices.len() {
                report::print_report(&devices[index - 1]);
                return 0;
            }
            eprintln!("Out of range.");
            continue;
        }

        match enumerate::find_device(devices, &input) {
            Some(dev) => {
                report::print_report(&dev);
                return 0;
            }
            None => eprintln!("No device matched '{input}'."),
        }
    }
}

fn coming_soon(feature: &str) -> i32 {
    println!("{feature} check is coming soon. Storage is the current focus.");
    0
}

fn read_line() -> Option<String> {
    let mut buf = String::new();
    match io::stdin().read_line(&mut buf) {
        Ok(0) => None,
        Ok(_) => Some(buf.trim().to_string()),
        Err(_) => None,
    }
}
