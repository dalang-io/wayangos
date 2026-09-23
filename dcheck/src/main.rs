//! dcheck — device health check.
//!
//! M1: CLI skeleton, block-device enumeration (identity + capacity).
//! Storage health (native SMART) and the TUI arrive in later milestones.
//! See `docs/DCHECK.md`.

mod bench;
mod config;
mod enumerate;
mod health;
mod json;
mod model;
mod monitor;
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
                run_tui(false, None)
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
        Some("check") => check_cmd(&args[1..], false),
        Some("watch") => watch_cmd(&args[1..], false),
        Some("prometheus") => prometheus_cmd(&args[1..], false),
        Some("tui") => run_tui(false, tui_theme_arg(&args[1..])),
        Some("demo") => {
            if interactive() {
                run_tui(true, tui_theme_arg(&args[1..]))
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

fn run_tui(force_demo: bool, light_override: Option<bool>) -> i32 {
    let light = config::resolve_light(light_override);
    let devices = load_devices(force_demo);
    match tui::run(devices, light) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("dcheck: TUI error: {err}");
            1
        }
    }
}

/// Parse `--light` / `--dark` / `--theme light|dark` for the TUI.
fn tui_theme_arg(args: &[String]) -> Option<bool> {
    let mut light = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--light" => light = Some(true),
            "--dark" => light = Some(false),
            "--theme" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("light") => light = Some(true),
                    Some("dark") => light = Some(false),
                    _ => {}
                }
            }
            _ => {}
        }
        i += 1;
    }
    light
}

fn print_help() {
    println!(
        "dcheck {VERSION} — device health check

USAGE:
    dcheck                  Interactive menu (TUI on a terminal)
    dcheck storage          List attached storage devices
    dcheck storage <dev>    Report for one device (e.g. /dev/nvme0n1)
    dcheck storage <dev> --bench           Read-only speed benchmark
    dcheck storage <dev> --test short|long Start a SMART self-test
    dcheck tui              Force the terminal UI
    dcheck check            One-shot health gate (exit code = worst verdict)
    dcheck watch            Monitor + alert (--interval, --webhook, --json)
    dcheck prometheus       Prometheus metrics for scrapers
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
    let mut bench = false;
    let mut test: Option<native::SelfTest> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => demo = true,
            "--json" => json = true,
            "--bench" => bench = true,
            "--test" => {
                i += 1;
                test = match args.get(i).map(String::as_str) {
                    Some("short") => Some(native::SelfTest::Short),
                    Some("long") => Some(native::SelfTest::Long),
                    _ => {
                        eprintln!("dcheck: --test needs 'short' or 'long'");
                        return 2;
                    }
                };
            }
            "-h" | "--help" => {
                println!(
                    "Usage: dcheck storage [<device>] [--demo] [--json] [--bench] [--test short|long]"
                );
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
        i += 1;
    }

    let devices = load_devices(demo);

    if let Some(sel) = selector {
        let dev = match enumerate::find_device(&devices, &sel) {
            Some(dev) => dev,
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
                return 1;
            }
        };

        if let Some(kind) = test {
            return run_selftest(&dev, kind);
        }
        if bench {
            return run_bench(&dev);
        }
        if json {
            println!("{}", report::device_json(&dev).to_string());
        } else {
            report::print_report(&dev);
        }
        return 0;
    }

    if test.is_some() || bench {
        eprintln!("dcheck: --test/--bench need a device (e.g. `dcheck storage /dev/sda --bench`)");
        return 2;
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

/// Prometheus metrics output.
fn prometheus_cmd(args: &[String], session_demo: bool) -> i32 {
    let mut demo = session_demo;
    for arg in args {
        match arg.as_str() {
            "--demo" => demo = true,
            "-h" | "--help" => {
                println!("Usage: dcheck prometheus [--demo]");
                return 0;
            }
            flag if flag.starts_with('-') => {
                eprintln!("dcheck: unknown option '{flag}'");
                return 2;
            }
            _ => {}
        }
    }
    let devices = load_devices(demo);
    print!("{}", report::prometheus(&devices));
    0
}

/// One-shot health gate: exit code = 0 ok, 1 unknown, 2 monitor, 3 backup/replace.
fn check_cmd(args: &[String], session_demo: bool) -> i32 {
    let mut demo = session_demo;
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "--demo" => demo = true,
            "-h" | "--help" => {
                println!("Usage: dcheck check [--json] [--demo]");
                return 0;
            }
            flag if flag.starts_with('-') => {
                eprintln!("dcheck: unknown option '{flag}'");
                return 2;
            }
            _ => {}
        }
    }
    let devices = load_devices(demo);
    monitor::check(&devices, json)
}

/// Watch loop with alerts.
fn watch_cmd(args: &[String], session_demo: bool) -> i32 {
    let mut demo = session_demo;
    let mut json = false;
    let mut quiet = false;
    let mut interval: u64 = config::load().watch_interval;
    let mut webhook: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--quiet" => quiet = true,
            "--demo" => demo = true,
            "--interval" => {
                i += 1;
                interval = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(60);
            }
            "--webhook" => {
                i += 1;
                webhook = args.get(i).cloned();
            }
            "-h" | "--help" => {
                println!(
                    "Usage: dcheck watch [--interval S] [--json] [--quiet] [--webhook URL] [--demo]"
                );
                return 0;
            }
            flag if flag.starts_with('-') => {
                eprintln!("dcheck: unknown option '{flag}'");
                return 2;
            }
            _ => {}
        }
        i += 1;
    }
    let devices = load_devices(demo);
    monitor::watch(&devices, interval, json, quiet, webhook.as_deref())
}

fn run_selftest(dev: &model::Device, kind: native::SelfTest) -> i32 {
    if let Some(msg) = native::selftest(dev, kind) {
        println!("{msg}");
        println!(
            "Note: the result appears in the SMART self-test log once complete (re-run the report)."
        );
        return 0;
    }
    // Fall back to smartctl for NVMe/SCSI or when the native path fails.
    let t = match kind {
        native::SelfTest::Short => "short",
        native::SelfTest::Long => "long",
    };
    match std::process::Command::new("smartctl")
        .arg("-t")
        .arg(t)
        .arg(&dev.path)
        .status()
    {
        Ok(s) if s.success() => {
            println!("self-test {t} started via smartctl");
            0
        }
        _ => {
            eprintln!("dcheck: could not start self-test (run as root, or install smartmontools)");
            1
        }
    }
}

fn run_bench(dev: &model::Device) -> i32 {
    eprintln!(
        "WARNING: read-only benchmark on {} — no data is written.",
        dev.path
    );
    match bench::run(&dev.path, dev.size_bytes) {
        Some(summary) => {
            println!("Benchmark {}: {summary}", dev.path);
            0
        }
        None => {
            eprintln!("dcheck: benchmark unavailable (needs Linux and read access to the raw device)");
            1
        }
    }
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
