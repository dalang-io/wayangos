//! `dcheck verify`: does the drive really hold what it reports?
//!
//! Counterfeit flash (USB sticks, cheap SSDs) reports a capacity larger than
//! its real flash. Past the real size, writes either wrap around onto
//! earlier addresses or vanish. Identity checks cannot see this; only writing
//! data and reading it back can (the approach of f3 / H2testw).
//!
//! Every 4 KiB block gets a header (magic, run seed, global block number)
//! followed by a pseudo-random pattern derived from (seed, block number), so
//! a bad block tells what happened to it: it holds another block's data
//! (wrap-around), zeros, data from an earlier run, or garbage.
//!
//! The test writes sequentially and, after each region, re-reads samples of
//! earlier regions with the OS cache dropped: a wrapping drive overwrites its
//! first blocks as soon as the writes pass its real capacity, so a fake is
//! caught early instead of after filling the whole drive.
//!
//! Two targets:
//! - free space (default): files in `.dcheck-verify-<pid>/` on a filesystem
//!   of the disk, removed afterwards. Existing files are not touched (on a
//!   fake drive, writes past its real capacity can still land on them).
//! - the raw device (`--destructive`): the whole disk is overwritten. Only
//!   for an empty, unmounted drive; needs the device name typed as
//!   confirmation.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::report::human_size_bin;

pub const BLOCK: usize = 4096;
const CHUNK: usize = 4 << 20;
const MAGIC: &[u8; 8] = b"DCHKVRFY";

static STOP: AtomicBool = AtomicBool::new(false);

/// Fill one block with its header and pattern.
pub fn fill_block(buf: &mut [u8], seed: u64, index: u64) {
    buf[..8].copy_from_slice(MAGIC);
    buf[8..16].copy_from_slice(&seed.to_le_bytes());
    buf[16..24].copy_from_slice(&index.to_le_bytes());
    // xorshift64* seeded per block (splitmix64 of seed and index).
    let mut z = seed ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let mut x = (z ^ (z >> 31)) | 1;
    for word in buf[24..].chunks_mut(8) {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
        word.copy_from_slice(&v[..word.len()]);
    }
}

/// What a block that did not read back correctly contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    /// Data written for another block (address wrap-around: fake capacity).
    Wrapped(u64),
    Zeros,
    /// A block from an earlier run: this write never reached the media.
    Stale,
    Garbage,
}

impl Bad {
    fn describe(self) -> String {
        match self {
            Bad::Wrapped(i) => format!("held the data written for {}", human_size_bin(i * BLOCK as u64)),
            Bad::Zeros => "read back as zeros".into(),
            Bad::Stale => "still held data from an earlier run (the write never landed)".into(),
            Bad::Garbage => "read back corrupted".into(),
        }
    }
}

/// Check one block; `scratch` is a BLOCK-sized work buffer.
pub fn check_block(buf: &[u8], seed: u64, index: u64, scratch: &mut [u8]) -> Option<Bad> {
    fill_block(scratch, seed, index);
    if buf == scratch {
        return None;
    }
    if buf.iter().all(|b| *b == 0) {
        return Some(Bad::Zeros);
    }
    if &buf[..8] == MAGIC {
        let s = u64::from_le_bytes(buf[8..16].try_into().ok()?);
        let i = u64::from_le_bytes(buf[16..24].try_into().ok()?);
        if s != seed {
            return Some(Bad::Stale);
        }
        if i != index {
            return Some(Bad::Wrapped(i));
        }
    }
    Some(Bad::Garbage)
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub checked: u64,
    pub bad: u64,
    pub wrapped: u64,
    /// Smallest (block written, block found) distance seen in wrap-around:
    /// the drive's real size when it maps addresses modulo its flash.
    pub wrap_distance: Option<u64>,
    pub first_bad: Option<(u64, Bad)>,
}

impl Stats {
    /// Add the counts of another pass (spot checks + sampled read-back).
    fn merge(&mut self, o: &Stats) {
        self.checked += o.checked;
        self.bad += o.bad;
        self.wrapped += o.wrapped;
        if let Some(d) = o.wrap_distance {
            self.wrap_distance = Some(self.wrap_distance.map_or(d, |w| w.min(d)));
        }
        if let Some((i, b)) = o.first_bad {
            if self.first_bad.is_none_or(|(j, _)| i < j) {
                self.first_bad = Some((i, b));
            }
        }
    }

    fn check_chunk(&mut self, data: &[u8], seed: u64, first_index: u64, scratch: &mut [u8]) {
        for (k, blk) in data.chunks(BLOCK).enumerate() {
            if blk.len() < BLOCK {
                break;
            }
            let index = first_index + k as u64;
            self.checked += 1;
            if let Some(b) = check_block(blk, seed, index, scratch) {
                self.bad += 1;
                if let Bad::Wrapped(found) = b {
                    self.wrapped += 1;
                    if found > index {
                        let d = found - index;
                        self.wrap_distance = Some(self.wrap_distance.map_or(d, |w| w.min(d)));
                    }
                }
                if self.first_bad.is_none_or(|(i, _)| index < i) {
                    self.first_bad = Some((index, b));
                }
            }
        }
    }
}

/// Where the test data goes.
trait Target {
    fn write_at(&mut self, off: u64, data: &[u8]) -> io::Result<()>;
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> io::Result<()>;
    /// fsync and drop the OS page cache, so reads come from the drive.
    fn sync_drop(&mut self) -> io::Result<()>;
}

fn is_enospc(e: &io::Error) -> bool {
    e.raw_os_error() == Some(28)
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub planned: u64,
    pub written: u64,
    pub read: u64,
    pub write_secs: f64,
    pub read_secs: f64,
    pub stats: Stats,
    /// Stopped writing because early checks already found bad blocks.
    pub early_stop: bool,
    pub aborted: bool,
    pub error: Option<String>,
}

/// Progress on stderr for the CLI (the TUI passes its own callback).
struct Progress {
    tty: bool,
    phase: &'static str,
    /// A `\r` progress line is on screen without a newline.
    open: bool,
    start: Instant,
    last: f64,
}

impl Progress {
    fn new() -> Self {
        Progress { tty: io::stderr().is_terminal(), phase: "", open: false, start: Instant::now(), last: -10.0 }
    }

    fn show(&mut self, phase: &'static str, done: u64, total: u64) {
        if phase != self.phase {
            self.end();
            self.phase = phase;
            self.start = Instant::now();
            self.last = -10.0;
        }
        let t = self.start.elapsed().as_secs_f64();
        let every = if self.tty { 0.5 } else { 30.0 };
        if t - self.last < every && done < total {
            return;
        }
        self.last = t;
        let rate = done as f64 / t.max(0.001);
        let eta = if rate > 0.0 { (total.saturating_sub(done)) as f64 / rate } else { 0.0 };
        let line = format!(
            "  {phase:<8} {} / {}  {:>10}  ETA {}",
            human_size_bin(done),
            human_size_bin(total),
            mbps(done, t),
            fmt_secs(eta)
        );
        if self.tty {
            eprint!("\r\x1b[2K{line}");
            self.open = true;
        } else {
            eprintln!("{line}");
        }
        let _ = io::stderr().flush();
    }

    fn end(&mut self) {
        if self.open {
            eprintln!();
            self.open = false;
        }
    }
}

/// Transfer speed in megabits per second, like an internet connection
/// ("608 Mbps"); `bytes` over `secs`.
pub fn mbps(bytes: u64, secs: f64) -> String {
    format!("{:.0} Mbps", bytes as f64 * 8.0 / secs.max(0.001) / 1e6)
}

fn fmt_secs(s: f64) -> String {
    let s = s as u64;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// Region between early checks: 1/64 of the test, 16 MiB – 1 GiB.
pub fn region_size(total: u64) -> u64 {
    let r = (total / 64).clamp(16 << 20, 1 << 30);
    r - r % CHUNK as u64
}

/// Offsets re-read after region `r` (0-based) has been written: the start of
/// the first region (where a wrapping drive lands first), the middle one,
/// and both ends of the region just written.
pub fn spot_offsets(r: u64, region: u64, written: u64) -> Vec<u64> {
    let chunk = CHUNK as u64;
    let mut v = vec![0, (r / 2) * region, r * region];
    let last = written.saturating_sub(chunk);
    v.push(last - last % BLOCK as u64);
    v.sort_unstable();
    v.dedup();
    v
}

/// Progress callback: (phase "writing" / "reading", done, total bytes).
pub type OnProgress<'a> = &'a mut dyn FnMut(&'static str, u64, u64);

/// Ask a running test to stop (Ctrl-C in the CLI, Esc in the TUI); the
/// test files are still removed.
pub fn request_stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// Clear a stop request before starting a test (not inside `run`, so a stop
/// pressed right after starting is not lost).
pub fn reset_stop() {
    STOP.store(false, Ordering::Relaxed);
}

fn run(t: &mut dyn Target, total: u64, seed: u64, progress: OnProgress) -> Outcome {
    let mut out = Outcome { planned: total, ..Default::default() };
    let region = region_size(total);
    let mut buf = vec![0u8; CHUNK];
    let mut scratch = vec![0u8; BLOCK];
    let start = Instant::now();

    // Write, re-checking samples of what is already written after each region.
    let mut off = 0u64;
    let mut next_check = region;
    let mut spot = Stats::default();
    while off < total {
        if STOP.load(Ordering::Relaxed) {
            out.aborted = true;
            break;
        }
        let len = ((total - off).min(CHUNK as u64) as usize) / BLOCK * BLOCK;
        if len == 0 {
            break;
        }
        for (k, blk) in buf[..len].chunks_mut(BLOCK).enumerate() {
            fill_block(blk, seed, off / BLOCK as u64 + k as u64);
        }
        match t.write_at(off, &buf[..len]) {
            Ok(()) => off += len as u64,
            Err(e) if is_enospc(&e) => break,
            Err(e) => {
                out.error = Some(format!("write at {}: {e}", human_size_bin(off)));
                break;
            }
        }
        out.written = off;
        progress("writing", off, total);
        if off >= next_check || off >= total {
            if let Err(e) = t.sync_drop() {
                out.error = Some(format!("sync: {e}"));
                break;
            }
            let r = (off - 1) / region;
            for o in spot_offsets(r, region, off) {
                let n = ((off - o).min(CHUNK as u64) as usize) / BLOCK * BLOCK;
                if t.read_at(o, &mut buf[..n]).is_ok() {
                    spot.check_chunk(&buf[..n], seed, o / BLOCK as u64, &mut scratch);
                }
            }
            if spot.bad > 0 {
                out.early_stop = true;
                break;
            }
            next_check = off + region;
        }
    }
    if let Err(e) = t.sync_drop() {
        out.error.get_or_insert(format!("sync: {e}"));
    }
    out.write_secs = start.elapsed().as_secs_f64();

    // Read back: everything, or after an early stop one chunk per region
    // (enough to map how far the damage goes).
    let start = Instant::now();
    let mut stats = Stats::default();
    let mut off = 0u64;
    while off < out.written && !STOP.load(Ordering::Relaxed) {
        let n = ((out.written - off).min(CHUNK as u64) as usize) / BLOCK * BLOCK;
        if let Err(e) = t.read_at(off, &mut buf[..n]) {
            out.error.get_or_insert(format!("read at {}: {e}", human_size_bin(off)));
            break;
        }
        stats.check_chunk(&buf[..n], seed, off / BLOCK as u64, &mut scratch);
        out.read += n as u64;
        off += if out.early_stop { region.max(n as u64) } else { n as u64 };
        progress("reading", off.min(out.written), out.written);
    }
    if STOP.load(Ordering::Relaxed) {
        out.aborted = true;
    }
    out.read_secs = start.elapsed().as_secs_f64();
    stats.merge(&spot);
    out.stats = stats;
    out
}

/// Filesystem / partition-table signatures in the first 72 KiB of a device
/// or partition (what `--destructive` would erase).
pub fn data_signatures(head: &[u8]) -> Vec<&'static str> {
    let at = |off: usize, magic: &[u8]| head.get(off..off + magic.len()) == Some(magic);
    let mut v = Vec::new();
    if at(512, b"EFI PART") {
        v.push("GPT partition table");
    } else if at(510, &[0x55, 0xAA]) && head.get(446..510).is_some_and(|t| t.iter().any(|b| *b != 0)) {
        v.push("MBR partition table");
    }
    for (off, magic, name) in [
        (1080usize, &[0x53u8, 0xEF][..], "ext2/3/4"),
        (0, b"XFSB", "XFS"),
        (0x10040, b"_BHRfS_M", "btrfs"),
        (3, b"NTFS    ", "NTFS"),
        (3, b"EXFAT   ", "exFAT"),
        (82, b"FAT32   ", "FAT32"),
        (54, b"FAT1", "FAT"),
        (512, b"LABELONE", "LVM"),
        (0, b"LUKS\xba\xbe", "LUKS"),
        (4086, b"SWAPSPACE2", "swap"),
        (0, b"ZFS", "ZFS"),
    ] {
        if !name.is_empty() && at(off, magic) {
            v.push(name);
        }
    }
    // Leftovers of an earlier dcheck verify are not data.
    if at(0, MAGIC) {
        v.clear();
    }
    v
}

/// A free-space test, before it runs.
#[derive(Debug, Clone)]
pub struct Plan {
    pub device: String,
    pub label: String,
    /// Mounted directory the test files go into.
    pub base: String,
    pub avail: u64,
    /// Free space minus the reserve (1%, min 256 MiB): the most it may write.
    pub room: u64,
    /// A system mountpoint on this drive (then only a limited test).
    pub system: Option<String>,
    /// Demo: simulate instead of writing (true = a counterfeit drive).
    pub simulated: Option<bool>,
}

pub use imp::{execute, plan};

/// Size of the quick test.
pub const QUICK: u64 = 8 << 30;

/// What the test will do (shown before asking to continue).
pub fn plan_lines(p: &Plan, total: u64) -> Vec<String> {
    let mut v = vec![
        format!("  Capacity check of {} ({})", p.device, p.label),
        format!(
            "  Writes {} of test data to {} (free: {}), reads it back,",
            human_size_bin(total),
            p.base,
            human_size_bin(p.avail)
        ),
        "  then deletes it. Existing files are not touched, but the filesystem is".into(),
        "  nearly full while the test runs, and on a counterfeit drive writes past".into(),
        "  its real capacity can damage existing data — back up first.".into(),
        "  It also overwrites the free space, so files deleted earlier can no longer".into(),
        "  be recovered — do not run it while you still want to undelete something.".into(),
    ];
    if total < p.room {
        v.push("  Note: only a full-size run can prove the whole capacity.".into());
    }
    v
}

/// In-memory drive for demos and tests: `mem.len()` real bytes behind a
/// reported size; past it writes wrap around (or vanish with `discard`).
pub struct SimTarget {
    pub mem: Vec<u8>,
    pub reported: u64,
    pub discard: bool,
    /// Pause per call, so a demo run is watchable.
    pub delay: std::time::Duration,
}

impl Target for SimTarget {
    fn write_at(&mut self, off: u64, data: &[u8]) -> io::Result<()> {
        debug_assert!(off + data.len() as u64 <= self.reported);
        std::thread::sleep(self.delay);
        let real = self.mem.len() as u64;
        for (k, b) in data.iter().enumerate() {
            let a = off + k as u64;
            if a < real {
                self.mem[a as usize] = *b;
            } else if !self.discard {
                self.mem[(a % real) as usize] = *b;
            }
        }
        Ok(())
    }
    fn read_at(&mut self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        std::thread::sleep(self.delay);
        let real = self.mem.len() as u64;
        for (k, b) in buf.iter_mut().enumerate() {
            let a = off + k as u64;
            *b = if a < real {
                self.mem[a as usize]
            } else if self.discard {
                0
            } else {
                self.mem[(a % real) as usize]
            };
        }
        Ok(())
    }
    fn sync_drop(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Demo plan (nothing is written): `fake` simulates a counterfeit drive.
pub fn demo_plan(d: &crate::model::Device, fake: bool) -> Plan {
    Plan {
        device: d.path.clone(),
        label: d.label(),
        base: "/media/demo (simulated)".into(),
        avail: 128 << 20,
        room: 128 << 20,
        system: None,
        simulated: Some(fake),
    }
}

/// Run a plan: the real test, or the in-memory simulation for demos.
pub fn run_plan(p: &Plan, total: u64, progress: OnProgress) -> Result<(Outcome, Option<String>), String> {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.as_nanos() as u64)
        .unwrap_or(1);
    match p.simulated {
        Some(fake) => {
            let real = if fake { 32 << 20 } else { total };
            let mut t = SimTarget {
                mem: vec![0; real as usize],
                reported: total,
                discard: false,
                delay: std::time::Duration::from_millis(25),
            };
            Ok((run(&mut t, total, seed, progress), None))
        }
        None => execute(p, total, seed, progress),
    }
}

pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_uppercase();
    let s = s.trim_end_matches("IB").trim_end_matches('B');
    let (num, mul) = match s.chars().last()? {
        'K' => (&s[..s.len() - 1], 1u64 << 10),
        'M' => (&s[..s.len() - 1], 1 << 20),
        'G' => (&s[..s.len() - 1], 1 << 30),
        'T' => (&s[..s.len() - 1], 1 << 40),
        _ => (s, 1),
    };
    let v: f64 = num.trim().parse().ok()?;
    (v > 0.0).then_some((v * mul as f64) as u64)
}

pub fn cmd(args: &[String]) -> i32 {
    let usage = "Usage: dcheck verify <device> [--size SIZE] [--dir DIR] [--yes]\n       \
                 dcheck verify <device> --destructive\n\n\
                 Writes test data and reads it back to prove the drive really stores\n\
                 what it reports (fake-capacity check, like f3/H2testw).\n\n  \
                 default        use free space on a mounted filesystem of the drive;\n                 \
                 existing files are kept, test files are removed afterwards\n  \
                 --size SIZE    write at most SIZE (e.g. 8G); default: all free space\n                 \
                 minus 1% (min 256 MiB)\n  \
                 --full         allow filling the free space of a system disk (/, /var,\n                 \
                 /home, ...); without it a system disk needs --size\n  \
                 --dir DIR      put the test files in DIR (must be on the drive)\n  \
                 --yes          do not ask for confirmation (free-space mode only)\n  \
                 --destructive  overwrite the whole unmounted drive; ERASES ALL DATA;\n                 \
                 shows what is on it and asks you to type the device name\n                 \
                 (or ERASE <name> when it holds partitions / filesystems)\n\n\
                 Exit status: 0 all data read back intact, 3 bad data (fake or failing\n\
                 drive), 1 error or aborted, 2 usage.";
    let mut dev_arg = None;
    let mut size = None;
    let mut dir = None;
    let mut yes = false;
    let mut destructive = false;
    let mut full = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{usage}");
                return 0;
            }
            "--yes" | "-y" => yes = true,
            "--destructive" => destructive = true,
            "--full" => full = true,
            "--verify-capacity" => {}
            "--size" => {
                i += 1;
                match args.get(i).and_then(|s| parse_size(s)) {
                    Some(v) => size = Some(v),
                    None => {
                        eprintln!("dcheck: --size needs a size like 8G or 500M");
                        return 2;
                    }
                }
            }
            "--dir" => {
                i += 1;
                match args.get(i) {
                    Some(d) => dir = Some(d.clone()),
                    None => {
                        eprintln!("dcheck: --dir needs a directory");
                        return 2;
                    }
                }
            }
            f if f.starts_with('-') => {
                eprintln!("dcheck: unknown option '{f}'\n\n{usage}");
                return 2;
            }
            v => dev_arg = Some(v.to_string()),
        }
        i += 1;
    }
    let Some(dev_arg) = dev_arg else {
        eprintln!("{usage}");
        return 2;
    };
    imp::run_cmd(&dev_arg, size, dir, yes, destructive, full)
}

/// Result report lines and the exit status (0 pass, 3 bad data, 1 error).
pub fn outcome_lines(target: &str, o: &Outcome, device_mode: bool) -> (Vec<String>, i32) {
    let mut out = Vec::new();
    out.push(crate::report::section("CAPACITY VERIFICATION"));
    out.push(format!("  Target       : {target}"));
    out.push(format!(
        "  Written      : {} of {} planned  ({})",
        human_size_bin(o.written),
        human_size_bin(o.planned),
        mbps(o.written, o.write_secs)
    ));
    out.push(format!(
        "  Read back    : {}{}  ({})",
        human_size_bin(o.read),
        if o.early_stop { " (sampled after the early stop)" } else { "" },
        mbps(o.read, o.read_secs)
    ));
    let s = &o.stats;
    if s.bad > 0 {
        out.push(format!("  Result       : FAIL — {} of {} checked blocks are bad", s.bad, s.checked));
        if let Some((idx, b)) = s.first_bad {
            out.push(format!("  First bad    : the block at {} {}", human_size_bin(idx * BLOCK as u64), b.describe()));
        }
        if s.wrapped > 0 {
            out.push(format!("  Wrap-around  : {} block(s) hold data written for a higher address", s.wrapped));
            if let (true, Some(d)) = (device_mode, s.wrap_distance) {
                out.push(format!("  Real size    : about {} (addresses repeat every that much)", human_size_bin(d * BLOCK as u64)));
            }
            out.push("  Meaning      : the drive stores less than it reports — counterfeit capacity.".into());
        } else {
            out.push("  Meaning      : data written to the drive does not come back — fake capacity or failing media.".into());
        }
        out.push("                 Do not trust this drive with data.".into());
        return (out, 3);
    }
    if let Some(e) = &o.error {
        out.push(format!("  Result       : ERROR — {e}"));
        return (out, 1);
    }
    if o.aborted {
        out.push("  Result       : ABORTED — no bad blocks in what was checked".into());
        return (out, 1);
    }
    out.push(format!("  Result       : PASS — all {} read back intact", human_size_bin(o.read)));
    if !device_mode && o.written < o.planned {
        out.push("  Note         : the filesystem filled up before the planned size".into());
    }
    (out, 0)
}

fn print_outcome(target: &str, o: &Outcome, device_mode: bool) -> i32 {
    let (lines, code) = outcome_lines(target, o, device_mode);
    println!();
    for l in lines {
        println!("{l}");
    }
    code
}

#[cfg(target_os = "linux")]
mod imp {
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{FileExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::model::Device;

    fn confirm(question: &str) -> bool {
        eprint!("{question} ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        io::stdin().read_line(&mut line).is_ok() && matches!(line.trim(), "y" | "Y" | "yes" | "YES")
    }

    extern "C" {
        fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32;
        fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const POSIX_FADV_DONTNEED: i32 = 4;
    const BLKFLSBUF: std::ffi::c_ulong = 0x1261;
    const O_EXCL: i32 = 0o200;

    extern "C" fn on_signal(_: i32) {
        STOP.store(true, Ordering::Relaxed);
    }

    fn drop_cache(f: &File) {
        unsafe {
            posix_fadvise(f.as_raw_fd(), 0, 0, POSIX_FADV_DONTNEED);
        }
    }

    /// Test files of FILE_BYTES each in a directory.
    struct Files {
        dir: PathBuf,
        files: Vec<File>,
    }
    const FILE_BYTES: u64 = 1 << 30;

    impl Files {
        fn file(&mut self, idx: usize) -> io::Result<&File> {
            while self.files.len() <= idx {
                let p = self.dir.join(format!("{:05}.dat", self.files.len()));
                self.files.push(OpenOptions::new().read(true).write(true).create_new(true).open(p)?);
            }
            Ok(&self.files[idx])
        }
    }

    impl Target for Files {
        fn write_at(&mut self, off: u64, data: &[u8]) -> io::Result<()> {
            // Chunks never cross a file boundary (FILE_BYTES % CHUNK == 0).
            let f = self.file((off / FILE_BYTES) as usize)?;
            f.write_all_at(data, off % FILE_BYTES)
        }
        fn read_at(&mut self, off: u64, buf: &mut [u8]) -> io::Result<()> {
            let f = self.file((off / FILE_BYTES) as usize)?;
            f.read_exact_at(buf, off % FILE_BYTES)
        }
        fn sync_drop(&mut self) -> io::Result<()> {
            for f in &self.files {
                f.sync_all()?;
                drop_cache(f);
            }
            Ok(())
        }
    }

    struct Raw(File);

    impl Target for Raw {
        fn write_at(&mut self, off: u64, data: &[u8]) -> io::Result<()> {
            self.0.write_all_at(data, off)
        }
        fn read_at(&mut self, off: u64, buf: &mut [u8]) -> io::Result<()> {
            self.0.read_exact_at(buf, off)
        }
        fn sync_drop(&mut self) -> io::Result<()> {
            self.0.sync_all()?;
            drop_cache(&self.0);
            unsafe {
                ioctl(self.0.as_raw_fd(), BLKFLSBUF, 0);
            }
            Ok(())
        }
    }

    /// The disk from the enumeration, or any block device (e.g. a
    /// device-mapper test target) by path.
    fn find(arg: &str) -> Option<Device> {
        let devices = crate::enumerate::list_devices();
        if let Some(d) = crate::enumerate::find_device(&devices, arg) {
            return Some(d);
        }
        let real = fs::canonicalize(arg).ok()?;
        let name = real.file_name()?.to_str()?.to_string();
        let sys = Path::new("/sys/class/block").join(&name);
        let sectors: u64 = fs::read_to_string(sys.join("size")).ok()?.trim().parse().ok()?;
        Some(Device {
            name,
            path: real.to_string_lossy().into_owned(),
            vendor: None,
            model: None,
            firmware: None,
            serial: None,
            bus: crate::model::Bus::Unknown,
            kind: crate::model::MediaKind::Unknown,
            size_bytes: sectors * 512,
            logical_block_size: 512,
            removable: false,
            smart_status: None,
            partitions: Vec::new(),
            failure: None,
        })
    }

    /// Why the raw device must not be overwritten, if anything uses it.
    fn in_use(d: &Device) -> Option<String> {
        let mut names = vec![d.name.clone()];
        if let Ok(rd) = fs::read_dir(format!("/sys/class/block/{}", d.name)) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with(&d.name) && n != d.name {
                    names.push(n);
                }
            }
        }
        let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
        let swaps = fs::read_to_string("/proc/swaps").unwrap_or_default();
        for n in &names {
            let dev = format!("/dev/{n}");
            for line in mounts.lines() {
                let mut it = line.split_whitespace();
                let (Some(src), Some(mp)) = (it.next(), it.next()) else { continue };
                let src_real = fs::canonicalize(src).ok().map(|p| p.to_string_lossy().into_owned());
                if src == dev || src_real.as_deref() == Some(dev.as_str()) {
                    return Some(format!("{dev} is mounted on {mp}"));
                }
            }
            if swaps.lines().any(|l| l.split_whitespace().next() == Some(dev.as_str())) {
                return Some(format!("{dev} is used as swap"));
            }
            let holders = fs::read_dir(format!("/sys/class/block/{n}/holders"))
                .map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect::<Vec<_>>())
                .unwrap_or_default();
            if !holders.is_empty() {
                return Some(format!("{dev} is held by {} (LVM / RAID / device-mapper)", holders.join(", ")));
            }
        }
        None
    }

    pub fn run_cmd(arg: &str, size: Option<u64>, dir: Option<String>, yes: bool, destructive: bool, full: bool) -> i32 {
        if !crate::native::is_root() && destructive {
            eprintln!("dcheck: --destructive needs root");
            return 1;
        }
        let Some(d) = find(arg) else {
            eprintln!("dcheck: device '{arg}' not found");
            return 1;
        };
        unsafe {
            signal(2, on_signal as extern "C" fn(i32) as usize);
            signal(15, on_signal as extern "C" fn(i32) as usize);
        }
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_nanos() as u64)
            .unwrap_or(1)
            ^ ((std::process::id() as u64) << 32);
        if destructive {
            destructive_run(&d, size, seed)
        } else {
            free_space_run(&d, size, dir, yes, full, seed)
        }
    }

    /// Partitions and filesystem signatures on `d` (and its partitions).
    fn contents(d: &Device) -> Vec<String> {
        use std::io::Read;
        let head = |path: &str| -> Vec<&'static str> {
            let mut buf = vec![0u8; 72 << 10];
            match File::open(path).and_then(|mut f| f.read(&mut buf)) {
                Ok(n) => data_signatures(&buf[..n]),
                Err(_) => Vec::new(),
            }
        };
        let mut out: Vec<String> = head(&d.path).iter().map(|s| format!("{}: {s}", d.path)).collect();
        let mut parts: Vec<String> = fs::read_dir(format!("/sys/class/block/{}", d.name))
            .map(|r| {
                r.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with(&d.name) && n != &d.name)
                    .collect()
            })
            .unwrap_or_default();
        parts.sort();
        for p in parts {
            let size = fs::read_to_string(format!("/sys/class/block/{p}/size"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|s| human_size_bin(s * 512))
                .unwrap_or_default();
            let sig = head(&format!("/dev/{p}"));
            let what = if sig.is_empty() { "partition".to_string() } else { sig.join(", ") };
            out.push(format!("/dev/{p}: {what} {size}"));
        }
        out
    }

    fn destructive_run(d: &Device, size: Option<u64>, seed: u64) -> i32 {
        if let Some(why) = in_use(d) {
            eprintln!("dcheck: refusing to overwrite {}: {why}", d.path);
            return 1;
        }
        if !io::stdin().is_terminal() {
            eprintln!("dcheck: --destructive needs an interactive terminal to confirm");
            return 1;
        }
        let total = size.unwrap_or(d.size_bytes).min(d.size_bytes);
        let total = total - total % BLOCK as u64;
        eprintln!("\n  !! {} ({}, {}) will be OVERWRITTEN: every partition and file on it is lost.", d.path, d.label(), human_size_bin(d.size_bytes));
        eprintln!("  !! {} will be written, then read back.", human_size_bin(total));
        let found = contents(d);
        if found.is_empty() {
            eprintln!("  No partitions or filesystem signatures found (looks empty).");
        } else {
            eprintln!("  !! THIS DRIVE HOLDS DATA:");
            for f in &found {
                eprintln!("       {f}");
            }
        }
        // An empty drive: its name. A drive with data: "ERASE <name>".
        let want = if found.is_empty() { d.name.clone() } else { format!("ERASE {}", d.name) };
        eprint!("  Type \"{want}\" to continue (anything else cancels): ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err() || line.trim() != want {
            eprintln!("dcheck: not confirmed, nothing written");
            return 1;
        }
        let f = match OpenOptions::new().read(true).write(true).custom_flags(O_EXCL).open(&d.path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("dcheck: cannot open {} exclusively: {e} (in use?)", d.path);
                return 1;
            }
        };
        eprintln!();
        let mut prog = Progress::new();
        let o = run(&mut Raw(f), total, seed, &mut |ph, d, t| prog.show(ph, d, t));
        prog.end();
        print_outcome(&format!("{} (raw device, destructive)", d.path), &o, true)
    }

    /// Where a free-space test would write, and how much room there is.
    pub fn plan(d: &Device, dir: Option<String>) -> Result<Plan, String> {
        // The mounted filesystem of this drive with the most free space.
        let base = match dir {
            Some(p) => PathBuf::from(p),
            None => d
                .partitions
                .iter()
                .filter_map(|p| p.mountpoint.as_ref().map(|m| (m.clone(), crate::mount::usage(m))))
                .filter_map(|(m, u)| u.map(|u| (m, u.avail)))
                .max_by_key(|(_, a)| *a)
                .map(|(m, _)| PathBuf::from(m))
                .ok_or_else(|| {
                    format!(
                        "no mounted filesystem on {}: mount a partition, or (empty drive only) use \
                         `dcheck verify {} --destructive` in a shell",
                        d.path, d.path
                    )
                })?,
        };
        let u = crate::mount::usage(&base.to_string_lossy())
            .ok_or_else(|| format!("cannot read free space of {}", base.display()))?;
        const SYSTEM: &[&str] = &["/", "/boot", "/boot/efi", "/usr", "/var", "/home", "/srv", "/opt", "/tmp"];
        let system = d
            .partitions
            .iter()
            .filter_map(|p| p.mountpoint.as_deref())
            .find(|m| SYSTEM.contains(m))
            .map(str::to_string);
        let reserve = (u.total / 100).max(256 << 20);
        let room = u.avail.saturating_sub(reserve);
        let room = room - room % BLOCK as u64;
        if room < 16 << 20 {
            return Err(format!(
                "only {} free on {} (keeping {} in reserve)",
                human_size_bin(u.avail),
                base.display(),
                human_size_bin(reserve)
            ));
        }
        Ok(Plan {
            device: d.path.clone(),
            label: d.label(),
            base: base.to_string_lossy().into_owned(),
            avail: u.avail,
            room,
            system,
            simulated: None,
        })
    }

    /// Write, read back and delete the test files.
    pub fn execute(plan: &Plan, total: u64, seed: u64, progress: OnProgress) -> Result<(Outcome, Option<String>), String> {
        let tdir = Path::new(&plan.base).join(format!(".dcheck-verify-{}", std::process::id()));
        fs::create_dir(&tdir).map_err(|e| format!("cannot create {}: {e}", tdir.display()))?;
        let mut files = Files { dir: tdir.clone(), files: Vec::new() };
        let o = run(&mut files, total, seed, progress);
        drop(files);
        let cleanup = fs::remove_dir_all(&tdir)
            .err()
            .map(|e| format!("could not remove {}: {e} — delete it by hand", tdir.display()));
        Ok((o, cleanup))
    }

    fn free_space_run(d: &Device, size: Option<u64>, dir: Option<String>, yes: bool, full: bool, seed: u64) -> i32 {
        let plan = match plan(d, dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("dcheck: {e}");
                return 1;
            }
        };
        if let (Some(m), None, false) = (&plan.system, size, full) {
            eprintln!(
                "dcheck: {} is a system disk ({m} is on it). Filling its free space can stop\n\
                 services (databases, logs) while the test runs. Use --size (e.g. --size 10G),\n\
                 or --full when the machine is idle.",
                d.path
            );
            return 1;
        }
        let total = size.unwrap_or(plan.room).min(plan.room);
        let total = total - total % BLOCK as u64;
        eprintln!();
        for l in plan_lines(&plan, total) {
            eprintln!("{l}");
        }
        if !yes {
            if !io::stdin().is_terminal() {
                eprintln!("dcheck: not a terminal; pass --yes to run without confirmation");
                return 1;
            }
            if !confirm("  Continue? [y/N]") {
                eprintln!("dcheck: cancelled");
                return 1;
            }
        }
        eprintln!();
        let mut prog = Progress::new();
        let result = execute(&plan, total, seed, &mut |ph, d, t| prog.show(ph, d, t));
        prog.end();
        match result {
            Ok((o, cleanup)) => {
                let code = print_outcome(&format!("{} (free space, test files)", plan.base), &o, false);
                if let Some(e) = cleanup {
                    eprintln!("dcheck: {e}");
                }
                code
            }
            Err(e) => {
                eprintln!("dcheck: {e}");
                1
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{OnProgress, Outcome, Plan};
    use crate::model::Device;

    pub fn plan(_: &Device, _: Option<String>) -> Result<Plan, String> {
        Err("verify is Linux-only for now".into())
    }

    pub fn execute(_: &Plan, _: u64, _: u64, _: OnProgress) -> Result<(Outcome, Option<String>), String> {
        Err("verify is Linux-only for now".into())
    }

    pub fn run_cmd(_: &str, _: Option<u64>, _: Option<String>, _: bool, _: bool, _: bool) -> i32 {
        eprintln!("dcheck: verify is Linux-only for now");
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(real: u64, reported: u64, discard: bool) -> SimTarget {
        SimTarget { mem: vec![0; real as usize], reported, discard, delay: std::time::Duration::ZERO }
    }

    fn run_quiet(t: &mut SimTarget, total: u64) -> Outcome {
        run(t, total, 1, &mut |_, _, _| {})
    }

    #[test]
    fn blocks_roundtrip_and_diagnose() {
        let mut a = vec![0u8; BLOCK];
        let mut scratch = vec![0u8; BLOCK];
        fill_block(&mut a, 7, 42);
        assert_eq!(check_block(&a, 7, 42, &mut scratch), None);
        assert_eq!(check_block(&a, 7, 41, &mut scratch), Some(Bad::Wrapped(42)));
        assert_eq!(check_block(&a, 8, 42, &mut scratch), Some(Bad::Stale));
        assert_eq!(check_block(&[0u8; BLOCK], 7, 42, &mut scratch), Some(Bad::Zeros));
        a[100] ^= 1;
        assert_eq!(check_block(&a, 7, 42, &mut scratch), Some(Bad::Garbage));
        // Different blocks get different patterns.
        let mut b = vec![0u8; BLOCK];
        fill_block(&mut b, 7, 43);
        assert_ne!(a[24..], b[24..]);
    }

    #[test]
    fn genuine_drive_passes() {
        let mut t = fake(64 << 20, 64 << 20, false);
        let o = run_quiet(&mut t, 64 << 20);
        assert_eq!(o.written, 64 << 20);
        assert_eq!(o.read, 64 << 20);
        assert_eq!(o.stats.bad, 0);
        assert!(!o.early_stop);
    }

    #[test]
    fn wrapping_fake_is_caught_early_with_its_real_size() {
        // Reports 256 MiB, really 48 MiB.
        let mut t = fake(48 << 20, 256 << 20, false);
        let o = run_quiet(&mut t, 256 << 20);
        assert!(o.stats.bad > 0);
        assert!(o.stats.wrapped > 0);
        assert!(o.early_stop);
        assert!(o.written < 128 << 20, "stopped at {}", o.written);
        assert_eq!(o.stats.wrap_distance.map(|d| d * BLOCK as u64), Some(48 << 20));
    }

    #[test]
    fn discarding_fake_reads_zeros() {
        let mut t = fake(40 << 20, 128 << 20, true);
        let o = run_quiet(&mut t, 128 << 20);
        assert!(o.stats.bad > 0);
        assert_eq!(o.stats.first_bad.map(|(_, b)| b), Some(Bad::Zeros));
        let first = o.stats.first_bad.map(|(i, _)| i * BLOCK as u64).unwrap();
        assert!((40 << 20..48 << 20).contains(&first), "{first}");
    }

    #[test]
    fn finds_data_signatures() {
        let mut h = vec![0u8; 72 << 10];
        assert!(data_signatures(&h).is_empty());
        h[512..520].copy_from_slice(b"EFI PART");
        assert_eq!(data_signatures(&h), vec!["GPT partition table"]);
        let mut e = vec![0u8; 72 << 10];
        e[1080] = 0x53;
        e[1081] = 0xEF;
        assert_eq!(data_signatures(&e), vec!["ext2/3/4"]);
        let mut n = vec![0u8; 4096];
        n[3..11].copy_from_slice(b"NTFS    ");
        n[510] = 0x55;
        n[511] = 0xAA;
        assert!(data_signatures(&n).contains(&"NTFS"));
        // A previous dcheck verify run is not "data".
        let mut v = vec![0u8; 72 << 10];
        fill_block(&mut v[..BLOCK], 1, 0);
        assert!(data_signatures(&v).is_empty());
    }

    #[test]
    fn speed_is_in_megabits() {
        // 76 MB/s (lab-243 write speed) = 608 Mbps.
        assert_eq!(mbps(76_000_000, 1.0), "608 Mbps");
        assert_eq!(mbps(4 << 30, 56.5), "608 Mbps");
    }

    #[test]
    fn sizes_and_regions() {
        assert_eq!(parse_size("8G"), Some(8 << 30));
        assert_eq!(parse_size("500MiB"), Some(500 << 20));
        assert_eq!(parse_size("1.5T"), Some((1.5 * (1u64 << 40) as f64) as u64));
        assert_eq!(parse_size("4096"), Some(4096));
        assert_eq!(parse_size("abc"), None);
        assert_eq!(region_size(100 << 20), 16 << 20);
        assert_eq!(region_size(1 << 40), 1 << 30);
        assert_eq!(spot_offsets(0, 16 << 20, 16 << 20), vec![0, (16 << 20) - (4 << 20)]);
    }
}
