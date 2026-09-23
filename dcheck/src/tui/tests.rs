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
    for (i, title) in [(0, "STORAGE ARRAY"), (1, "MEMORY BANK"), (2, "PROCESSOR"), (3, "SESSION")] {
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
