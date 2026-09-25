//! Render tests against ratatui's in-memory `TestBackend` using demo data.

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;
use ratatui::Terminal;

use super::*;
use crate::ram::RamModule;

fn ram() -> RamInfo {
    let module = |slot: &str| RamModule {
        locator: slot.into(),
        size_bytes: 16_000_000_000,
        kind: "DDR5".into(),
        speed_mts: Some(4800),
        configured_mts: Some(4800),
        manufacturer: Some("Kingston".into()),
        part_number: Some("KF548C38".into()),
        serial: None,
        rank: Some(1),
    };
    RamInfo {
        total_bytes: 32_000_000_000,
        available_bytes: 12_000_000_000,
        swap_total_bytes: 2_000_000_000,
        swap_free_bytes: 1_500_000_000,
        modules: vec![module("DIMM0"), module("DIMM1")],
        slots_total: 4,
        ram_temp_c: Some(44),
        source: "test".into(),
        ..Default::default()
    }
}

fn cpu() -> CpuInfo {
    CpuInfo {
        model: "Test CPU 9000 @ 3.2GHz".into(),
        vendor: Some("GenuineIntel".into()),
        sockets: 1,
        cores: 8,
        threads: 16,
        mhz: Some(3200.0),
        max_mhz: Some(4500.0),
        cache_kb: Some(32_768),
        temp_c: Some(55),
        sensors: vec![crate::cpu::CpuSensor {
            label: "Package id 0".into(),
            temp_c: 55,
            high_c: Some(77),
            crit_c: Some(87),
        }],
        load1: Some(3.4),
        source: "test".into(),
    }
}

/// App with every background read already completed (no threads).
fn app(pal: Palette, plain: bool) -> App {
    let devices = crate::enumerate::demo_devices();
    let mut a = App::new(devices, pal, Ui { plain }, true, false, false);
    a.health = a
        .devices
        .iter()
        .map(|d| DevHealth::from_metrics(report::device_metrics(d).as_ref()))
        .collect();
    let r = ram();
    a.ram_lines = report::ram_report_lines(&r);
    a.ram = Some(r);
    let c = cpu();
    a.cpu_lines = report::cpu_report_lines(&c);
    a.cpu = Some(c);
    let b = crate::board::demo();
    a.board_lines = report::board_report_lines(&b);
    a.board = Some(b);
    a
}

fn neon() -> Palette {
    Palette::new(ColorMode::Neon, false, false)
}

fn open_report(a: &mut App, index: usize) {
    let (lines, metrics) = report::device_report(&a.devices[index]);
    a.report_dev = Some(index);
    a.report_lines = lines;
    a.report_metrics = metrics;
    a.screen = Screen::Report;
}

fn render(a: &mut App, w: u16, h: u16) -> Buffer {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| views::draw(f, a)).unwrap();
    t.backend().buffer().clone()
}

fn text(buf: &Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn screens() -> Vec<(Screen, &'static [&'static str])> {
    vec![
        (Screen::Menu, &["MODULES", "STORAGE", "PROCESSOR"]),
        (Screen::Storage, &["STORAGE ARRAY", "/dev/nvme0n1", "BACK UP NOW"]),
        (Screen::Ram, &["MEMORY BANK", "USED", "SLOTS"]),
        (Screen::Cpu, &["PROCESSOR CORE", "LOAD", "THREADS"]),
        (Screen::Board, &["MOTHERBOARD", "PowerEdge R630", "MONITOR"]),
    ]
}

#[test]
fn every_screen_renders_key_content() {
    for (w, h) in [(80, 24), (140, 40)] {
        for (screen, expect) in screens() {
            let mut a = app(neon(), false);
            a.screen = screen;
            let t = text(&render(&mut a, w, h));
            for e in expect {
                assert!(t.contains(e), "{screen:?} at {w}x{h} missing {e:?}:\n{t}");
            }
        }
        let mut a = app(neon(), false);
        open_report(&mut a, 2); // HDD with bad sectors
        let t = text(&render(&mut a, w, h));
        for e in ["VITALS", "TELEMETRY LOG", "BACK UP NOW", "reallocated"] {
            assert!(t.contains(e), "report at {w}x{h} missing {e:?}:\n{t}");
        }
    }
}

#[test]
fn report_shows_life_and_endurance_gauges() {
    let mut a = app(neon(), false);
    open_report(&mut a, 0); // NVMe, 6% wear
    let t = text(&render(&mut a, 140, 40));
    assert!(t.contains("94% left"), "{t}");
    assert!(t.contains("ENDURANCE"), "{t}");
}

#[test]
fn menu_cards_follow_selection() {
    for (i, title) in [(0, "STORAGE ARRAY"), (1, "MEMORY BANK"), (2, "PROCESSOR"), (3, "MOTHERBOARD"), (4, "SESSION")] {
        let mut a = app(neon(), false);
        a.menu.select(Some(i));
        let t = text(&render(&mut a, 120, 30));
        assert!(t.contains(title), "card {i} missing {title:?}:\n{t}");
    }
}

#[test]
fn splash_and_help_render() {
    let mut a = app(neon(), false);
    a.screen = Screen::Splash;
    assert!(text(&render(&mut a, 80, 24)).contains("DEVICE HEALTH SYSTEM"));

    let mut a = app(neon(), false);
    a.help = true;
    assert!(text(&render(&mut a, 80, 24)).contains("COMMAND REFERENCE"));
}

#[test]
fn tiny_terminals_do_not_panic() {
    for (w, h) in [(1, 1), (20, 6), (40, 10), (59, 15)] {
        for screen in [Screen::Splash, Screen::Menu, Screen::Storage, Screen::Ram, Screen::Cpu] {
            let mut a = app(neon(), false);
            a.screen = screen;
            a.help = true;
            render(&mut a, w, h);
        }
        let mut a = app(neon(), false);
        open_report(&mut a, 0);
        render(&mut a, w, h);
    }
}

#[test]
fn plain_mode_is_ascii_only() {
    let mut cases: Vec<App> = Vec::new();
    for i in 0..MENU_ITEMS {
        let mut a = app(Palette::new(ColorMode::Ansi, false, false), true);
        a.menu.select(Some(i));
        cases.push(a);
    }
    let mut a = app(Palette::new(ColorMode::Ansi, false, false), true);
    a.screen = Screen::Storage;
    cases.push(a);
    let mut a = app(Palette::new(ColorMode::Ansi, false, false), true);
    a.help = true;
    cases.push(a);

    for mut a in cases {
        for (w, h) in [(80, 24), (140, 40)] {
            let t = text(&render(&mut a, w, h));
            let bad: String = t.chars().filter(|c| !c.is_ascii()).collect();
            assert!(bad.is_empty(), "non-ASCII {bad:?} in {:?}:\n{t}", a.screen);
        }
    }
}

#[test]
fn no_color_uses_no_colors() {
    let mono = Palette::new(ColorMode::Mono, false, false);
    for (screen, _) in screens() {
        let mut a = app(mono.clone(), false);
        a.screen = screen;
        let buf = render(&mut a, 120, 30);
        for cell in buf.content() {
            assert!(matches!(cell.fg, Color::Reset), "{screen:?} fg {:?}", cell.fg);
            assert!(matches!(cell.bg, Color::Reset), "{screen:?} bg {:?}", cell.bg);
        }
    }
}

#[test]
fn neon_paints_rgb_background() {
    let mut a = app(neon(), false);
    let buf = render(&mut a, 80, 24);
    assert!(matches!(buf[(40, 12)].bg, Color::Rgb(..)));
}

#[test]
fn keys_navigate_between_screens() {
    let mut a = app(neon(), false);
    a.screen = Screen::Splash;
    assert!(!handle_key(&mut a, KeyCode::Char('x')));
    assert_eq!(a.screen, Screen::Menu);

    assert!(!handle_key(&mut a, KeyCode::Char('?')));
    assert!(a.help);
    assert!(!handle_key(&mut a, KeyCode::Esc));
    assert!(!a.help);
    assert_eq!(a.screen, Screen::Menu);

    handle_key(&mut a, KeyCode::Char('1'));
    assert_eq!(a.screen, Screen::Storage);
    handle_key(&mut a, KeyCode::Down);
    assert_eq!(a.table.selected(), Some(1));
    handle_key(&mut a, KeyCode::Enter);
    assert_eq!(a.screen, Screen::Report);
    assert_eq!(a.report_dev, Some(1));
    assert!(a.report_rx.is_some());
    handle_key(&mut a, KeyCode::Esc);
    assert_eq!(a.screen, Screen::Storage);
    handle_key(&mut a, KeyCode::Char('b'));
    assert_eq!(a.screen, Screen::Menu);
    assert!(handle_key(&mut a, KeyCode::Char('q')));
}

#[test]
fn scrolling_is_clamped_to_content() {
    let mut a = app(neon(), false);
    a.screen = Screen::Ram;
    a.ram_lines = (0..100).map(|i| format!("line {i}")).collect();
    render(&mut a, 80, 24);
    a.scroll_by(10_000);
    let max = a.max_scroll();
    assert!(max > 0);
    assert_eq!(a.scroll, max);
    a.scroll_by(-10_000);
    assert_eq!(a.scroll, 0);
}

#[test]
fn base64_matches_reference() {
    assert_eq!(base64(b"dcheck"), "ZGNoZWNr");
    assert_eq!(base64(b"ab"), "YWI=");
    assert_eq!(base64(b"a"), "YQ==");
}

/// Dev helper: print every screen as text.
/// `cargo test dump_screens -- --ignored --nocapture`
#[test]
#[ignore]
fn dump_screens() {
    let (w, h) = (120, 32);
    for (screen, _) in screens() {
        let mut a = app(neon(), false);
        a.screen = screen;
        println!("{}", text(&render(&mut a, w, h)));
    }
    for i in [0usize, 2] {
        let mut a = app(neon(), false);
        open_report(&mut a, i);
        println!("{}", text(&render(&mut a, w, h)));
    }
    let mut a = app(neon(), false);
    a.menu.select(Some(2));
    println!("{}", text(&render(&mut a, w, h)));
    let mut a = app(neon(), false);
    a.help = true;
    println!("{}", text(&render(&mut a, w, h)));
    let mut a = app(neon(), false);
    a.screen = Screen::Splash;
    a.started = std::time::Instant::now() - std::time::Duration::from_millis(450);
    println!("{}", text(&render(&mut a, 80, 24)));
    let mut a = app(neon(), false);
    open_report(&mut a, 0);
    println!("{}", text(&render(&mut a, 80, 24)));
}

/// Drain background work until `done` holds (or fail after a few seconds).
fn wait(a: &mut App, done: impl Fn(&App) -> bool) {
    let start = Instant::now();
    while !done(a) {
        assert!(start.elapsed() < Duration::from_secs(20), "timed out");
        a.drain();
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn assert_ascii(t: &str) {
    for ch in t.chars() {
        assert!(ch.is_ascii(), "non-ASCII {ch:?} in plain mode:\n{t}");
    }
}

#[test]
fn recover_screen_shows_chance_steps_and_map() {
    for (row, chance) in [(2usize, "MEDIUM"), (0, "ALMOST NONE")] {
        let mut a = app(neon(), false);
        a.screen = Screen::Storage;
        a.table.select(Some(row));
        handle_key(&mut a, KeyCode::Char('u'));
        assert_eq!(a.screen, Screen::Recover);
        wait(&mut a, |a| a.recover_map.is_some());
        let t = text(&render(&mut a, 140, 44));
        for e in ["RECOVERY", chance, "DISK MAP", "free space still holds old data", "WHAT TO DO", "ddrescue"] {
            assert!(t.contains(e), "row {row} missing {e:?}:\n{t}");
        }
        // Small terminals and plain mode still render.
        let _ = render(&mut a, 60, 16);
        // Plain mode: the panels are ASCII (the log text follows
        // DCHECK_PLAIN, which `--plain` sets for the whole process).
        a.ui = Ui { plain: true };
        let t = text(&render(&mut a, 100, 40));
        assert_ascii(t.split("WHAT TO DO").next().unwrap());
        handle_key(&mut a, KeyCode::Esc);
        assert_eq!(a.screen, Screen::Storage);
    }
}

#[test]
fn verify_flow_plan_run_result() {
    // Demo row 3 is the USB "Flash Disk", simulated as a counterfeit stick;
    // row 1 (SATA SSD) as a genuine drive.
    for (row, code, expect) in [(3usize, 3, "Real size"), (1, 0, "PASS")] {
        let mut a = app(neon(), false);
        a.screen = Screen::Storage;
        a.table.select(Some(row));
        handle_key(&mut a, KeyCode::Char('v'));
        assert_eq!(a.screen, Screen::Verify);
        let t = text(&render(&mut a, 120, 36));
        for e in ["CAPACITY TEST", "back up first", "press y to start"] {
            assert!(t.contains(e), "plan missing {e:?}:\n{t}");
        }
        // Enter alone never starts a test that writes.
        handle_key(&mut a, KeyCode::Enter);
        assert!(matches!(a.verify, VerifyState::Plan { .. }));
        handle_key(&mut a, KeyCode::Char('y'));
        assert!(matches!(a.verify, VerifyState::Running { .. }));
        let t = text(&render(&mut a, 120, 36));
        assert!(t.contains("WRITE") && t.contains("READ BACK"), "{t}");
        // Quitting mid-test is refused.
        assert!(!handle_key(&mut a, KeyCode::Char('q')));
        wait(&mut a, |a| matches!(a.verify, VerifyState::Done { .. }));
        assert!(matches!(a.verify, VerifyState::Done { code: c } if c == code));
        let t = text(&render(&mut a, 120, 36));
        assert!(t.contains(expect), "row {row}:\n{t}");
        handle_key(&mut a, KeyCode::Esc);
        assert_eq!(a.screen, Screen::Storage);
    }
}

#[test]
fn verify_can_be_stopped_and_refuses_dead_ports() {
    let mut a = app(neon(), false);
    a.screen = Screen::Storage;
    a.table.select(Some(1));
    handle_key(&mut a, KeyCode::Char('v'));
    handle_key(&mut a, KeyCode::Char('y'));
    handle_key(&mut a, KeyCode::Esc);
    wait(&mut a, |a| matches!(a.verify, VerifyState::Done { .. }));
    assert!(a.verify_lines.iter().any(|l| l.contains("ABORTED")), "{:?}", a.verify_lines);
    // The unresponsive SATA port has nothing to test.
    let dead = 4;
    a.devices[dead].failure = Some("link reset failed".into());
    a.verify = VerifyState::Idle;
    a.screen = Screen::Storage;
    a.table.select(Some(dead));
    handle_key(&mut a, KeyCode::Char('v'));
    let t = text(&render(&mut a, 120, 30));
    assert!(t.contains("dead port"), "{t}");
    handle_key(&mut a, KeyCode::Char('y'));
    assert!(matches!(a.verify, VerifyState::Plan { .. }));
}

#[test]
fn undelete_screen_demo_flow_with_block_map() {
    let mut a = app(neon(), false);
    a.screen = Screen::Storage;
    a.table.select(Some(2));
    handle_key(&mut a, KeyCode::Char('u'));
    wait(&mut a, |a| a.recover.is_some());
    handle_key(&mut a, KeyCode::Char('d'));
    assert_eq!(a.screen, Screen::Undelete);
    wait(&mut a, |a| a.undel.is_some());
    let t = text(&render(&mut a, 140, 44));
    for e in ["BLOCK MAP", "deleted:", "intact", "Laporan Keuangan 2026.xlsx", "PARTLY REUSED", "OVERWRITTEN"] {
        assert!(t.contains(e), "missing {e:?}:\n{t}");
    }
    // Mark all intact files, open the prompt, type a folder, write (demo).
    handle_key(&mut a, KeyCode::Char('a'));
    assert_eq!(a.undel_marked.len(), 4);
    handle_key(&mut a, KeyCode::Char('w'));
    assert!(a.undel_prompt.is_some());
    for c in "/mnt/usb/rescue".chars() {
        handle_key(&mut a, KeyCode::Char(c));
    }
    // 'q' while typing is text, not quit.
    assert!(!handle_key(&mut a, KeyCode::Char('q')));
    handle_key(&mut a, KeyCode::Backspace);
    let t = text(&render(&mut a, 140, 44));
    assert!(t.contains("RECOVER 4 FILE(S) TO") && t.contains("/mnt/usb/rescue"), "{t}");
    handle_key(&mut a, KeyCode::Enter);
    assert_eq!(a.undel_log.len(), 4);
    assert!(a.undel_log[0].contains("nothing written"));
    let t = text(&render(&mut a, 140, 44));
    assert!(t.contains("RECOVERED"), "{t}");
    handle_key(&mut a, KeyCode::Char('x'));
    assert!(a.undel_log.is_empty());
    // Small and plain terminals.
    let _ = render(&mut a, 60, 16);
    a.ui = Ui { plain: true };
    assert_ascii(&text(&render(&mut a, 120, 40)));
    handle_key(&mut a, KeyCode::Esc);
    assert_eq!(a.screen, Screen::Recover);
}

#[test]
fn undelete_screen_recovers_real_files_from_an_image() {
    // A FAT32 image made on Linux (see testdata/undelete), as the "disk".
    let dir = std::env::temp_dir().join(format!("dcheck-tui-undel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("fat32.img");
    let text_img = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/undelete/fat32.sparse")).unwrap();
    let mut lines = text_img.lines();
    let size: usize = lines.next().unwrap().rsplit(' ').next().unwrap().parse().unwrap();
    let mut bytes = vec![0u8; size];
    for l in lines {
        let p: Vec<&str> = l.split(' ').collect();
        let s: usize = p[0].parse().unwrap();
        if p[1] == "H" {
            for k in 0..512 {
                bytes[s * 512 + k] = u8::from_str_radix(&p[2][2 * k..2 * k + 2], 16).unwrap();
            }
        }
    }
    std::fs::write(&img, &bytes).unwrap();
    let mut a = app(neon(), false);
    a.demo = false;
    a.devices[0].path = img.to_string_lossy().into_owned();
    a.tool_dev = Some(0);
    a.screen = Screen::Recover;
    handle_key(&mut a, KeyCode::Char('d'));
    wait(&mut a, |a| a.undel.is_some());
    let n = a.undel.as_ref().unwrap().files.len();
    assert_eq!(n, 3);
    handle_key(&mut a, KeyCode::Char('a'));
    handle_key(&mut a, KeyCode::Char('w'));
    let out = dir.join("out");
    for c in out.to_string_lossy().chars() {
        handle_key(&mut a, KeyCode::Char(c));
    }
    handle_key(&mut a, KeyCode::Enter);
    wait(&mut a, |a| !a.undel_log.is_empty());
    assert!(a.undel_log.iter().all(|l| l.starts_with("recovered")), "{:?}", a.undel_log);
    let big: Vec<u8> = (0..20000u32).map(|i| ((i * 7 + 3) % 251) as u8).collect();
    assert_eq!(std::fs::read(out.join("big.bin")).unwrap(), big);
    assert!(out.join("Dokumen Kantor/Laporan Keuangan 2026.xlsx").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn motherboard_menu_item_card_and_screen() {
    let mut a = app(neon(), false);
    a.menu.select(Some(3));
    let t = text(&render(&mut a, 120, 36));
    for e in ["MOTHERBOARD", "PowerEdge R630", "FANS", "no AC input"] {
        assert!(t.contains(e), "card missing {e:?}:\n{t}");
    }
    handle_key(&mut a, KeyCode::Enter);
    assert_eq!(a.screen, Screen::Board);
    let t = text(&render(&mut a, 140, 44));
    for e in ["BIOS", "2.19.0", "BMC", "POWER", "ALERTS", "redundancy lost", "BOARD LOG", "EVENT LOG"] {
        assert!(t.contains(e), "screen missing {e:?}:\n{t}");
    }
    handle_key(&mut a, KeyCode::Esc);
    assert_eq!(a.screen, Screen::Menu);
    handle_key(&mut a, KeyCode::Char('4'));
    assert_eq!(a.screen, Screen::Board);
    // Plain mode stays ASCII on the card and the screen.
    a.ui = Ui { plain: true };
    let t = text(&render(&mut a, 140, 44));
    assert_ascii(t.split("BOARD LOG").next().unwrap());
    a.screen = Screen::Menu;
    a.menu.select(Some(3));
    assert_ascii(&text(&render(&mut a, 120, 36)));
    // Exit is now the 5th item.
    a.menu.select(Some(4));
    assert!(handle_key(&mut a, KeyCode::Enter));
}

#[test]
fn virtual_disk_rows_stay_virtual() {
    let mut a = app(neon(), false);
    a.devices[1].name = "vda".into();
    a.devices[1].bus = crate::model::Bus::Virtio;
    let d = a.devices[1].clone();
    assert_eq!(DevHealth::for_device(&d, None).label, "VIRTUAL");
    assert_eq!(DevHealth::for_device(&d, None).sev, 0);
    // A real disk without SMART stays UNKNOWN.
    assert_eq!(DevHealth::for_device(&a.devices[3], None).label, "UNKNOWN");
    // Opening its report (no SMART) keeps the row VIRTUAL.
    a.report_dev = Some(1);
    a.health[1] = DevHealth::for_device(&d, None);
    a.report_rx = Some({
        let (tx, rx) = mpsc::channel();
        tx.send((vec!["x".to_string()], None)).unwrap();
        rx
    });
    a.drain();
    assert_eq!(a.health[1].label, "VIRTUAL");
    a.screen = Screen::Report;
    let t = text(&render(&mut a, 120, 30));
    assert!(t.contains("VIRTUAL"), "{t}");
}
