//! dcheck — device health check.
//!
//! M1: CLI skeleton, block-device enumeration (identity + capacity).
//! Storage health (native SMART) and the TUI arrive in later milestones.
//! See `docs/DCHECK.md`.

mod authenticity;
mod bench;
mod board;
mod cache;
mod config;
mod cpu;
mod enumerate;
mod health;
mod ipmi;
mod json;
mod kernlog;
mod model;
mod monitor;
mod mount;
mod native;
mod oui_table;
mod ram;
mod recover;
mod report;
mod smartctl;
mod tui;
mod undelete;
mod update;
mod verify;
mod virt;

use std::io::{self, IsTerminal, Write};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    // `dcheck … | head` should end quietly like other CLI tools, not panic
    // on a closed pipe (Rust ignores SIGPIPE by default).
    #[cfg(unix)]
    unsafe {
        extern "C" {
            fn signal(signum: i32, handler: usize) -> usize;
        }
        signal(13, 0); // SIGPIPE → SIG_DFL
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

fn run(args: &[String]) -> i32 {
    // `--fresh` anywhere (e.g. `dcheck tui --fresh`): ignore cached reads.
    if args.iter().any(|a| a == "--fresh") {
        cache::set_fresh(true);
    }
    match args.first().map(String::as_str) {
        None => {
            if interactive() {
                run_tui(false, None, None, None, None)
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
        // Monitoring must reflect the hardware now, never a cached read.
        Some("check") => {
            cache::set_fresh(true);
            check_cmd(&args[1..], false)
        }
        Some("watch") => {
            cache::set_fresh(true);
            watch_cmd(&args[1..], false)
        }
        Some("prometheus") => {
            cache::set_fresh(true);
            prometheus_cmd(&args[1..], false)
        }
        Some("update") | Some("self-update") => update::cmd(&args[1..]),
        Some("snapshot") => snapshot_cmd(&args[1..]),
        Some("tui") => run_tui(
            false,
            tui_theme_arg(&args[1..]),
            tui_mouse_arg(&args[1..]),
            tui_plain_arg(&args[1..]),
            tui_transparent_arg(&args[1..]),
        ),
        Some("demo") => {
            if interactive() {
                run_tui(
                    true,
                    tui_theme_arg(&args[1..]),
                    tui_mouse_arg(&args[1..]),
                    tui_plain_arg(&args[1..]),
                    tui_transparent_arg(&args[1..]),
                )
            } else {
                interactive_menu(true)
            }
        }
        Some("verify") => verify::cmd(&args[1..]),
        Some("recover") => recover::cmd(&args[1..]),
        Some("undelete") => undelete::cmd(&args[1..]),
        Some("ram") => ram_cmd(&args[1..]),
        Some("cpu") => cpu_cmd(&args[1..]),
        Some("board") | Some("motherboard") | Some("mobo") => board_cmd(&args[1..]),
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

/// Parse `--mouse` / `--no-mouse` for the TUI.
fn tui_mouse_arg(args: &[String]) -> Option<bool> {
    let mut mouse = None;
    for arg in args {
        match arg.as_str() {
            "--mouse" => mouse = Some(true),
            "--no-mouse" => mouse = Some(false),
            _ => {}
        }
    }
    mouse
}

/// Parse `--plain` / `--fancy` for the TUI.
fn tui_plain_arg(args: &[String]) -> Option<bool> {
    let mut plain = None;
    for arg in args {
        match arg.as_str() {
            "--plain" => plain = Some(true),
            "--fancy" | "--no-plain" => plain = Some(false),
            _ => {}
        }
    }
    plain
}

/// Parse `--transparent` / `--solid` for the TUI.
fn tui_transparent_arg(args: &[String]) -> Option<bool> {
    let mut t = None;
    for arg in args {
        match arg.as_str() {
            "--transparent" => t = Some(true),
            "--solid" | "--background" => t = Some(false),
            _ => {}
        }
    }
    t
}

fn run_tui(
    force_demo: bool,
    light_override: Option<bool>,
    mouse_override: Option<bool>,
    plain_override: Option<bool>,
    transparent_override: Option<bool>,
) -> i32 {
    let light = config::resolve_light(light_override);
    let mouse = mouse_override.unwrap_or_else(|| config::load().mouse);
    let plain = plain_override.unwrap_or_else(|| {
        config::load().plain || std::env::var_os("DCHECK_PLAIN").is_some()
    });
    let transparent = transparent_override.unwrap_or_else(|| {
        config::load().transparent || std::env::var_os("DCHECK_TRANSPARENT").is_some()
    });
    if plain {
        // Report text (section headers, meters) follows the same glyph set.
        std::env::set_var("DCHECK_PLAIN", "1");
    }
    let splash = config::load().splash && std::env::var_os("DCHECK_NO_SPLASH").is_none();
    let demo = force_demo || std::env::var_os("DCHECK_DEMO").is_some();
    let devices = load_devices(force_demo);
    let opts = tui::Options {
        light,
        demo,
        mouse,
        plain,
        transparent,
        splash,
    };
    match tui::run(devices, opts) {
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
    dcheck storage <dev>    Report for one device (e.g. /dev/nvme0n1; --fresh skips the cache)
    dcheck storage <dev> --bench           Read-only speed benchmark
    dcheck storage <dev> --test short|long Start a SMART self-test
    dcheck verify <dev>     Prove the real capacity: write test data to free
                            space and read it back (fake drives; --help)
    dcheck undelete <dev|image>  List deleted files (NTFS/FAT32/exFAT) and
                            recover them to another disk (--to DIR; --carve)
    dcheck recover <dev|path>  Deleted a file by mistake? Chance of recovery,
                            what to do now, and a map of remaining data
    dcheck tui              Terminal UI (--light|--dark, --mouse, --plain)
    dcheck check            One-shot health gate (exit code = worst verdict)
    dcheck watch            Monitor + alert (--interval, --webhook, --json)
    dcheck prometheus       Prometheus metrics for scrapers
    dcheck update           Update to the latest release (--check, --force)
    dcheck snapshot DIR     Render every TUI screen to SVG (docs/screenshots)
    dcheck demo             Run with built-in sample devices (no sysfs needed)
    dcheck ram | cpu        Memory / CPU report (--json)
    dcheck board            Motherboard: maker, BIOS, PCIe/USB devices, sensors,
                            BMC event log and health (--json; root for IPMI)
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
        println!("  2) RAM");
        println!("  3) CPU");
        println!("  q) Quit");
        print!("Select: ");
        let _ = io::stdout().flush();

        match read_line() {
            Some(input) => match input.as_str() {
                "1" | "storage" | "s" => {
                    storage_cmd(&[], force_demo);
                }
                "2" | "ram" | "r" => {
                    ram_cmd(&[]);
                }
                "3" | "cpu" | "c" => {
                    cpu_cmd(&[]);
                }
                "q" | "quit" | "exit" | "" => return 0,
                other => eprintln!("Unknown choice '{other}'."),
            },
            None => return 0, // EOF
        }
    }
}

fn snapshot_cmd(args: &[String]) -> i32 {
    let usage = "Usage: dcheck snapshot DIR [--demo] [--light] [--host NAME] [--mask-serials] \
                 [--size 120x34] [--report DEV]... [--tools DEV]";
    let mut dir = None;
    let mut opts = tui::snapshot::Options {
        demo: false,
        light: false,
        host: None,
        mask_serials: false,
        width: 120,
        height: 34,
        reports: Vec::new(),
        tools: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => opts.demo = true,
            "--light" => opts.light = true,
            "--mask-serials" => opts.mask_serials = true,
            "--host" => {
                i += 1;
                opts.host = args.get(i).cloned();
            }
            "--report" => {
                i += 1;
                opts.reports.extend(args.get(i).cloned());
            }
            "--tools" => {
                i += 1;
                opts.tools = args.get(i).cloned();
            }
            "--size" => {
                i += 1;
                let parsed = args.get(i).and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                });
                match parsed {
                    Some((w, h)) if w >= 40 && h >= 12 => (opts.width, opts.height) = (w, h),
                    _ => {
                        eprintln!("{usage}");
                        return 2;
                    }
                }
            }
            "-h" | "--help" => {
                println!("{usage}");
                return 0;
            }
            other if !other.starts_with('-') && dir.is_none() => dir = Some(other.to_string()),
            _ => {
                eprintln!("{usage}");
                return 2;
            }
        }
        i += 1;
    }
    let Some(dir) = dir else {
        eprintln!("{usage}");
        return 2;
    };
    let devices = load_devices(opts.demo);
    match tui::snapshot::run(std::path::Path::new(&dir), devices, &opts) {
        Ok(files) => {
            for f in files {
                println!("{}", f.display());
            }
            0
        }
        Err(e) => {
            eprintln!("dcheck: snapshot failed: {e}");
            1
        }
    }
}

fn storage_cmd(args: &[String], session_demo: bool) -> i32 {
    if args.iter().any(|a| a == "--verify-capacity") {
        return verify::cmd(args);
    }
    let mut selector: Option<String> = None;
    let mut demo = session_demo;
    let mut json = false;
    let mut bench = false;
    let mut test: Option<native::SelfTest> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--demo" => demo = true,
            "--json" => {
                json = true;
                cache::set_fresh(true);
            }
            "--fresh" => cache::set_fresh(true),
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
            println!("{}", report::device_json(&dev));
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
        println!("{}", crate::json::Json::Arr(items));
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

fn ram_cmd(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--json") {
        let info = ram::read();
        println!("{}", report::ram_json(&info));
    } else {
        for line in report::ram_report_lines(&ram::read()) {
            println!("{line}");
        }
    }
    0
}

fn cpu_cmd(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--json") {
        let info = cpu::read();
        println!("{}", report::cpu_json(&info));
    } else {
        for line in report::cpu_report_lines(&cpu::read()) {
            println!("{line}");
        }
    }
    0
}

fn board_cmd(args: &[String]) -> i32 {
    let info = board::read();
    if args.iter().any(|a| a == "--json") {
        println!("{}", report::board_json(&info));
    } else {
        for line in report::board_report_lines(&info) {
            println!("{line}");
        }
    }
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
