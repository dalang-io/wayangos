//! `dcheck recover`: can a deleted file still be recovered, and how?
//!
//! Deleting a file removes its directory entry; the blocks are only marked
//! free. What happens to the contents next depends on the drive:
//!
//! - HDD: the old contents stay until new data lands on them.
//! - SSD with TRIM: the filesystem tells the drive which blocks are free and
//!   the controller erases them (reads then return zeros). With the `discard`
//!   mount option that happens within seconds; otherwise at the next
//!   `fstrim` run (weekly `fstrim.timer` on most distributions).
//!
//! This module is read-only. It gathers those facts, estimates the chance,
//! prints concrete next steps (stop writing, image the drive, run the right
//! tool on the image), and can draw a map of where the disk still holds
//! data by sampling it.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::report::human_size_bin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Chance {
    AlmostNone,
    Low,
    Medium,
    High,
}

impl Chance {
    pub fn label(self) -> &'static str {
        match self {
            Chance::High => "HIGH",
            Chance::Medium => "MEDIUM",
            Chance::Low => "LOW",
            Chance::AlmostNone => "ALMOST NONE",
        }
    }

    fn lower(self) -> Chance {
        match self {
            Chance::High => Chance::Medium,
            Chance::Medium => Chance::Low,
            _ => Chance::AlmostNone,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct Timer {
    pub enabled: bool,
    pub last: Option<String>,
    pub next: Option<String>,
}

/// Everything the assessment looks at, for one filesystem.
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// Block device holding the filesystem (`/dev/sda3`, `/dev/dm-0`).
    pub device: String,
    /// The physical disk under it (`/dev/sda`).
    pub disk: String,
    pub size: u64,
    pub fstype: Option<String>,
    pub mountpoint: Option<String>,
    pub mount_opts: Vec<String>,
    /// `Some(true)` HDD, `Some(false)` SSD / flash.
    pub rotational: Option<bool>,
    /// The device (through any dm / RAID / USB stack) accepts TRIM.
    pub trim: bool,
    pub fstrim_timer: Option<Timer>,
    pub used_pct: Option<f64>,
    /// Holds the running system (/, /var, /home …).
    pub system: bool,
    /// dm-crypt / LUKS in the stack.
    pub encrypted: bool,
    /// A disk the hypervisor provides (inside a VM).
    pub virtual_disk: bool,
}

impl Facts {
    fn discard_mount(&self) -> bool {
        self.mount_opts.iter().any(|o| o == "discard" || o.starts_with("discard="))
            && !self.mount_opts.iter().any(|o| o == "nodiscard")
    }

    fn read_only(&self) -> bool {
        self.mount_opts.iter().any(|o| o == "ro")
    }

    fn fs(&self) -> &str {
        self.fstype.as_deref().unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub struct Assessment {
    pub chance: Chance,
    pub reasons: Vec<String>,
    pub steps: Vec<String>,
}

fn fs_family(fs: &str) -> &'static str {
    match fs {
        "ntfs" | "ntfs3" | "fuseblk" => "ntfs",
        "vfat" | "fat" | "msdos" | "exfat" => "fat",
        "ext2" | "ext3" | "ext4" => "ext",
        "xfs" => "xfs",
        "btrfs" => "btrfs",
        "zfs" | "zfs_member" => "zfs",
        "f2fs" => "f2fs",
        _ => "other",
    }
}

/// The recovery tools for a filesystem, run against `img`, output to `out`.
fn tools(fs: &str, img: &str, out: &str) -> Vec<String> {
    let mut v = Vec::new();
    match fs_family(fs) {
        "ntfs" => {
            v.push(format!("ntfsundelete {img} --scan          # list deleted files with names"));
            v.push(format!("ntfsundelete {img} -u -m '*.xlsx' -d {out}   # undelete by name pattern"));
        }
        "fat" => v.push(format!("testdisk {img}   # Advanced → Undelete (names mostly kept)")),
        "ext" => v.push(format!(
            "ext4magic {img} -r -d {out}   # from the journal: names kept if the journal still has them"
        )),
        "xfs" => v.push(format!("xfs_undelete -o {out} {img}   # github.com/ianka/xfs_undelete")),
        "btrfs" => {
            v.push(format!("btrfs-find-root {img}           # older tree roots"));
            v.push(format!("btrfs restore -v -i -t <root> {img} {out}"));
        }
        "zfs" => v.push("zfs list -t snapshot   # ZFS: snapshots are the recovery path".into()),
        _ => {}
    }
    v.push(format!("photorec /log /d {out} {img}   # carving, any filesystem; names are lost"));
    v
}

pub fn assess(f: &Facts) -> Assessment {
    let mut reasons = Vec::new();
    let fs = f.fs();
    let family = fs_family(fs);
    let ssd = f.rotational == Some(false) && !f.virtual_disk;

    let mut chance = if f.virtual_disk && f.trim && f.discard_mount() {
        reasons.push(format!(
            "virtual disk mounted with `{}`: freed blocks are handed back to the host (thin provisioning) and \
             often read back as zeros — whether they are depends on the provider; the disk map shows what is left",
            f.mount_opts.iter().find(|o| o.starts_with("discard")).map_or("discard", |s| s.as_str())
        ));
        Chance::Low
    } else if ssd && f.trim && f.discard_mount() {
        reasons.push(format!(
            "SSD mounted with `{}`: the drive is told about deleted blocks right away and erases them \
             (reads return zeros) — usually within seconds to minutes",
            f.mount_opts.iter().find(|o| o.starts_with("discard")).map_or("discard", |s| s.as_str())
        ));
        Chance::AlmostNone
    } else {
        let base = match family {
            "ntfs" | "fat" => {
                reasons.push(format!("{fs}: a deleted file's record usually keeps its name and location"));
                Chance::High
            }
            "ext" | "xfs" => {
                reasons.push(format!(
                    "{fs}: deleting clears the block map, so names/folders are usually lost; \
                     contents can be carved, recent ones sometimes rebuilt from the journal"
                ));
                Chance::Medium
            }
            "btrfs" => {
                reasons.push("btrfs: copy-on-write — older tree roots can still point at the file".into());
                Chance::Medium
            }
            "zfs" => {
                reasons.push("ZFS: no undelete tools; snapshots are the way back".into());
                Chance::Low
            }
            _ => {
                reasons.push(format!(
                    "filesystem {}: carving (photorec) works on any filesystem",
                    if fs.is_empty() { "unknown" } else { fs }
                ));
                Chance::Medium
            }
        };
        if ssd && f.trim {
            match &f.fstrim_timer {
                Some(t) if t.enabled => {
                    reasons.push(format!(
                        "SSD with TRIM and fstrim.timer enabled (last run: {}, next: {}): if it ran after \
                         the deletion, the data is gone",
                        t.last.as_deref().unwrap_or("never"),
                        t.next.as_deref().unwrap_or("?")
                    ));
                    base.lower()
                }
                _ => {
                    reasons.push("SSD with TRIM, but no discard mount and no fstrim timer: free blocks are \
                                  only erased when someone runs fstrim"
                        .into());
                    base
                }
            }
        } else {
            if f.virtual_disk {
                reasons.push(
                    "virtual disk: deleted data stays until overwritten, unless the host reclaims free space".into(),
                );
            } else if ssd {
                reasons.push("SSD, but TRIM does not reach it (RAID controller / USB bridge / not supported): \
                              deleted blocks are not erased"
                    .into());
            } else if f.rotational == Some(true) {
                reasons.push("HDD: old contents stay on the platters until they are overwritten".into());
            }
            base
        }
    };
    if let Some(u) = f.used_pct {
        if u >= 90.0 && chance > Chance::AlmostNone {
            reasons.push(format!("{u:.0}% full: new writes quickly reuse the freed blocks"));
            chance = chance.lower();
        }
    }
    if f.system && !f.read_only() && chance > Chance::AlmostNone {
        reasons.push("system disk, mounted read-write: logs and services keep writing to it every minute".into());
    }
    if f.encrypted {
        reasons.push("encrypted (dm-crypt): recover from the unlocked /dev/mapper device, not the raw disk".into());
    }

    // Steps.
    let mut steps = Vec::new();
    let mnt = f.mountpoint.as_deref();
    let mut first = vec!["Trash (~/.local/share/Trash, file manager deletes)".to_string(), "backups".to_string()];
    match family {
        "btrfs" => first.push("snapshots: `btrfs subvolume list -s /` (snapper / timeshift)".into()),
        "zfs" => first.push("snapshots: `zfs list -t snapshot`".into()),
        _ => first.push("LVM / VM snapshots".into()),
    }
    steps.push(format!("Look first in: {}.", first.join(", ")));
    if chance == Chance::AlmostNone {
        steps.push("Carving may still find fragments that were not trimmed yet; the steps below are the \
                    only chance, so do them now or not at all."
            .into());
    }
    if (ssd || f.virtual_disk) && f.trim && f.fstrim_timer.as_ref().is_some_and(|t| t.enabled) {
        steps.push("Stop TRIM now: `sudo systemctl stop fstrim.timer`.".into());
    }
    if f.virtual_disk {
        steps.push(
            "This is a virtual machine: take a snapshot of the VM / its disk in the provider's panel NOW — it \
             freezes the deleted data. Then recover from a clone of that snapshot attached to a rescue VM."
                .into(),
        );
    }
    match (mnt, f.system) {
        (Some(_), true) if f.virtual_disk => steps.push(
            "Stop writing: this disk runs the VM — after the snapshot, shut the VM down or boot it into the \
             provider's rescue mode; do not keep working on it."
                .into(),
        ),
        (Some(_), true) => steps.push(
            "Stop writing: this disk runs the system — shut down cleanly and boot a live USB (WayangOS or \
             any rescue system); do not keep working on this machine."
                .into(),
        ),
        (Some(m), false) => steps.push(format!(
            "Stop writing: `sudo umount {m}` (or `sudo mount -o remount,ro {m}` if busy)."
        )),
        (None, _) => steps.push("It is not mounted — keep it that way.".into()),
    }
    let name = f.device.rsplit('/').next().unwrap_or("disk");
    let img = format!("/mnt/other/{name}.img");
    steps.push(format!(
        "Image it to ANOTHER disk with at least {} free: `sudo ddrescue -d -n {} {img} /mnt/other/{name}.map`.",
        human_size_bin(f.size),
        f.device
    ));
    let mut t = String::from("Recover from the image, into a folder on the other disk:");
    for line in tools(fs, &img, "/mnt/other/recovered") {
        t.push_str("\n      ");
        t.push_str(&line);
    }
    steps.push(t);
    steps.push("Never install tools on, or save recovered files to, the disk you are recovering from.".into());
    Assessment { chance, reasons, steps }
}

/// `systemctl show` output (`Key=value` lines).
pub fn parse_timer(show: &str, enabled: &str) -> Timer {
    let get = |k: &str| {
        show.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .map(str::trim)
            .filter(|v| !v.is_empty() && *v != "n/a" && *v != "0")
            .map(str::to_string)
    };
    Timer {
        enabled: matches!(enabled.trim(), "enabled" | "static" | "enabled-runtime"),
        last: get("LastTriggerUSec"),
        next: get("NextElapseUSecRealtime"),
    }
}

/// What a sampled block of the disk holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    /// All zeros or all 0xFF: never written, or erased by TRIM.
    Empty,
    Data,
    Unreadable,
}

pub fn classify(block: &[u8]) -> Sample {
    if block.iter().all(|b| *b == 0) || block.iter().all(|b| *b == 0xFF) {
        Sample::Empty
    } else {
        Sample::Data
    }
}

/// Share of the free space that still holds old data, from the share of
/// sampled cells with data (`data`) and the filesystem's used share.
pub fn free_space_residue(data: f64, used: f64) -> Option<f64> {
    (used < 0.99).then(|| ((data - used) / (1.0 - used)).clamp(0.0, 1.0))
}

/// One map cell from its samples.
pub fn cell_char(samples: &[Sample], plain: bool) -> char {
    let data = samples.iter().filter(|s| **s == Sample::Data).count();
    let bad = samples.iter().filter(|s| **s == Sample::Unreadable).count();
    match (bad, data, plain) {
        (b, _, false) if b == samples.len() => '×',
        (b, _, true) if b == samples.len() => 'x',
        (_, 0, false) => '·',
        (_, 0, true) => '.',
        (_, d, false) if d == samples.len() => '█',
        (_, d, true) if d == samples.len() => '#',
        (_, _, false) => '▒',
        (_, _, true) => '+',
    }
}

/// Assessment of every filesystem on the target, and its disk.
#[derive(Debug, Clone)]
pub struct Gathered {
    pub fss: Vec<(Facts, Assessment)>,
    pub disk: String,
}

/// Where the disk still holds data, from sampling.
#[derive(Debug, Clone)]
pub struct DiskMap {
    pub disk: String,
    pub cell_bytes: u64,
    pub cells: Vec<Vec<Sample>>,
    /// Filesystem index (into `Gathered::fss`) of each cell, if inside one.
    pub cell_fs: Vec<Option<usize>>,
    /// Per filesystem: (device, share of samples with data, residue).
    pub shares: Vec<(String, f64, Option<f64>)>,
}

/// Per filesystem: device, byte range on the disk, used %.
pub type FsRange = (String, Option<(u64, u64)>, Option<f64>);

impl DiskMap {
    pub fn build(disk: &str, cell_bytes: u64, cells: Vec<Vec<Sample>>, ranges: &[FsRange]) -> DiskMap {
        let mut cell_fs = vec![None; cells.len()];
        let mut shares = Vec::new();
        for (fi, (dev, range, used)) in ranges.iter().enumerate() {
            let Some((start, end)) = *range else { continue };
            let (mut n, mut data) = (0usize, 0usize);
            for (i, samples) in cells.iter().enumerate() {
                let (a, b) = (i as u64 * cell_bytes, (i as u64 + 1) * cell_bytes);
                let mid = (a + b) / 2;
                if mid >= start && mid < end {
                    cell_fs[i] = Some(fi);
                }
                if a >= start && b <= end {
                    n += samples.len();
                    data += samples.iter().filter(|s| **s == Sample::Data).count();
                }
            }
            if n > 0 {
                let d = data as f64 / n as f64;
                shares.push((dev.clone(), d, used.and_then(|u| free_space_residue(d, u / 100.0))));
            }
        }
        DiskMap { disk: disk.to_string(), cell_bytes, cells, cell_fs, shares }
    }
}

#[cfg(target_os = "linux")]
pub use imp::{disk_name, mount_source};
pub use imp::{gather, mounted_rw, sample_map};

/// The assessment as report lines (CLI output, TUI log, clipboard).
pub fn report_lines(g: &Gathered) -> Vec<String> {
    let plain = crate::report::ui_plain();
    let dot = if plain { "-" } else { "·" };
    let mut out = Vec::new();
    for (f, a) in &g.fss {
        out.push(crate::report::section(&format!("RECOVERY {dot} {}", f.device)));
        let media = match (f.rotational, f.trim) {
            _ if f.virtual_disk => "virtual disk",
            (Some(true), _) => "HDD",
            (Some(false), true) => "SSD / flash, TRIM supported",
            (Some(false), false) => "SSD / flash, no TRIM reaches it",
            (None, _) => "unknown",
        };
        out.push(format!("  Disk         : {} ({media})", f.disk));
        out.push(format!(
            "  Filesystem   : {}{}",
            f.fstype.as_deref().unwrap_or("unknown"),
            f.mountpoint.as_deref().map(|m| format!(" on {m}")).unwrap_or_else(|| " (not mounted)".into())
        ));
        let relevant: Vec<&str> = f
            .mount_opts
            .iter()
            .map(String::as_str)
            .filter(|o| *o == "ro" || *o == "rw" || o.contains("discard") || *o == "ssd")
            .collect();
        if !relevant.is_empty() {
            out.push(format!("  Mount opts   : {}", relevant.join(",")));
        }
        if let Some(u) = f.used_pct {
            out.push(format!("  Used         : {u:.0}%"));
        }
        if let (Some(t), Some(false)) = (&f.fstrim_timer, f.rotational) {
            out.push(format!(
                "  fstrim.timer : {}{}",
                if t.enabled { "enabled" } else { "disabled" },
                t.last.as_deref().map(|l| format!(" (last run {l})")).unwrap_or_default()
            ));
        }
        out.push(format!("  Chance       : {}", a.chance.label()));
        for r in &a.reasons {
            out.push(format!("    {dot} {r}"));
        }
        out.push(String::new());
        out.push("  What to do:".into());
        for (i, s) in a.steps.iter().enumerate() {
            for (k, part) in s.split('\n').enumerate() {
                if k == 0 {
                    out.push(format!("   {}. {part}", i + 1));
                } else {
                    out.push(format!("   {part}"));
                }
            }
        }
        out.push(String::new());
    }
    out
}

/// The disk map as text (CLI).
pub fn map_lines(m: &DiskMap, plain: bool) -> Vec<String> {
    let mut out = vec![crate::report::section(&format!("DISK MAP {} {}", if plain { "-" } else { "·" }, m.disk))];
    let mut row = String::new();
    for (i, samples) in m.cells.iter().enumerate() {
        if i % 64 == 0 {
            if !row.is_empty() {
                out.push(std::mem::take(&mut row));
            }
            row = format!("  {:>9} ", human_size_bin(i as u64 * m.cell_bytes));
        }
        row.push(cell_char(samples, plain));
    }
    out.push(row);
    out.push(format!(
        "  {} data   {} empty (zeros: never written, or erased by TRIM)   {} mixed   (1 cell = {})",
        if plain { '#' } else { '█' },
        if plain { '.' } else { '·' },
        if plain { '+' } else { '▒' },
        human_size_bin(m.cell_bytes)
    ));
    out.extend(share_lines(m));
    out.push("  Sampled, not exhaustive: a fragment can survive in an \"empty\" cell and vice versa.".into());
    out
}

/// "data in N% of samples … free space still holds old data" per filesystem.
pub fn share_lines(m: &DiskMap) -> Vec<String> {
    m.shares
        .iter()
        .map(|(dev, d, r)| {
            let mut line = format!("  {dev:<12} data in {:.0}% of samples", d * 100.0);
            if let Some(r) = r {
                line.push_str(&format!(" → ~{:.0}% of the free space still holds old data", r * 100.0));
            }
            line
        })
        .collect()
}

/// Demo data: an ext4 HDD whose free space still holds old data, or (for
/// an NVMe) an SSD mounted with discard.
pub fn demo(d: &crate::model::Device) -> (Gathered, DiskMap) {
    let ssd = d.kind != crate::model::MediaKind::Hdd;
    let part = format!("{}{}", d.path, if d.name.starts_with("nvme") { "p2" } else { "2" });
    let f = Facts {
        device: part.clone(),
        disk: d.path.clone(),
        size: d.size_bytes * 95 / 100,
        fstype: Some(if ssd { "btrfs" } else { "ext4" }.into()),
        mountpoint: Some(if ssd { "/" } else { "/data" }.into()),
        mount_opts: if ssd { vec!["rw".into(), "ssd".into(), "discard=async".into()] } else { vec!["rw".into()] },
        rotational: Some(!ssd),
        trim: ssd,
        fstrim_timer: Some(Timer { enabled: true, last: Some("Mon 2026-09-21 00:39:57".into()), next: None }),
        used_pct: Some(if ssd { 38.0 } else { 63.0 }),
        system: ssd,
        encrypted: false,
        virtual_disk: false,
    };
    let a = assess(&f);
    let cells = 512usize;
    let cell_bytes = d.size_bytes / cells as u64;
    // Deterministic pattern: SSD → data only where used; HDD → nearly full.
    let grid: Vec<Vec<Sample>> = (0..cells)
        .map(|i| {
            (0..4)
                .map(|k| {
                    let h = (i * 7 + k * 13) % 100;
                    let data = if ssd { i < cells * 2 / 5 && h < 92 } else { i > 10 && h < 96 };
                    if data {
                        Sample::Data
                    } else {
                        Sample::Empty
                    }
                })
                .collect()
        })
        .collect();
    let start = d.size_bytes - f.size;
    let ranges = vec![(part, Some((start, d.size_bytes)), f.used_pct)];
    let map = DiskMap::build(&d.path, cell_bytes, grid, &ranges);
    (Gathered { fss: vec![(f, a)], disk: d.path.clone() }, map)
}

pub fn cmd(args: &[String]) -> i32 {
    let usage = "Usage: dcheck recover <disk | partition | path> [--no-map] [--cells N]\n\n\
                 Read-only. Estimates whether deleted files can still be recovered from\n\
                 this drive (HDD vs SSD with TRIM, mount options, fstrim timer, filesystem)\n\
                 and prints the steps: stop writing, image the drive, run the right tool\n\
                 on the image. As root it also samples the disk and draws a map of where\n\
                 it still holds data.\n\n  \
                 --no-map    skip the disk map\n  \
                 --cells N   map resolution (default 512 cells, 4 samples each)";
    let mut target = None;
    let mut map = true;
    let mut cells = 512usize;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{usage}");
                return 0;
            }
            "--no-map" => map = false,
            "--cells" => {
                i += 1;
                match args.get(i).and_then(|v| v.parse().ok()).filter(|n: &usize| (16..=8192).contains(n)) {
                    Some(n) => cells = n,
                    None => {
                        eprintln!("dcheck: --cells needs a number between 16 and 8192");
                        return 2;
                    }
                }
            }
            f if f.starts_with('-') => {
                eprintln!("dcheck: unknown option '{f}'\n\n{usage}");
                return 2;
            }
            v => target = Some(v.to_string()),
        }
        i += 1;
    }
    let Some(target) = target else {
        eprintln!("{usage}");
        return 2;
    };
    imp::run(&target, map, cells)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::fs;
    use std::os::unix::fs::FileExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    const SYSTEM: &[&str] = &["/", "/boot", "/boot/efi", "/usr", "/var", "/home", "/srv", "/opt", "/tmp"];

    struct Mount {
        source: String,
        target: String,
        fstype: String,
        opts: Vec<String>,
    }

    fn mounts() -> Vec<Mount> {
        let unescape = |s: &str| s.replace("\\040", " ").replace("\\011", "\t");
        fs::read_to_string("/proc/mounts")
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let source = it.next()?;
                let target = unescape(it.next()?);
                let fstype = it.next()?.to_string();
                let opts = it.next()?.split(',').map(str::to_string).collect();
                let source = fs::canonicalize(source)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| source.to_string());
                Some(Mount { source, target, fstype, opts })
            })
            .collect()
    }

    fn sys(name: &str, f: &str) -> Option<String> {
        fs::read_to_string(format!("/sys/class/block/{name}/{f}")).ok().map(|s| s.trim().to_string())
    }

    /// Parent disk of a partition, or the physical disk under a dm device.
    fn disk_of(name: &str) -> String {
        if Path::new(&format!("/sys/class/block/{name}/partition")).exists() {
            if let Ok(p) = fs::canonicalize(format!("/sys/class/block/{name}")) {
                if let Some(parent) = p.parent().and_then(|p| p.file_name()) {
                    return parent.to_string_lossy().into_owned();
                }
            }
        }
        if let Some(slave) = fs::read_dir(format!("/sys/class/block/{name}/slaves"))
            .ok()
            .and_then(|mut r| r.next())
            .and_then(|e| e.ok())
        {
            return disk_of(&slave.file_name().to_string_lossy());
        }
        name.to_string()
    }

    fn encrypted(name: &str) -> bool {
        if sys(name, "dm/uuid").is_some_and(|u| u.starts_with("CRYPT-")) {
            return true;
        }
        fs::read_dir(format!("/sys/class/block/{name}/slaves"))
            .map(|r| r.flatten().any(|e| encrypted(&e.file_name().to_string_lossy())))
            .unwrap_or(false)
    }

    fn timer() -> Option<Timer> {
        let enabled = Command::new("systemctl").args(["is-enabled", "fstrim.timer"]).output().ok()?;
        let show = Command::new("systemctl")
            .args(["show", "fstrim.timer", "-p", "LastTriggerUSec", "-p", "NextElapseUSecRealtime"])
            .output()
            .ok()?;
        Some(parse_timer(&String::from_utf8_lossy(&show.stdout), &String::from_utf8_lossy(&enabled.stdout)))
    }

    fn facts(name: &str, all: &[Mount], fstype: Option<String>) -> Facts {
        let dev = format!("/dev/{name}");
        let disk = disk_of(name);
        let m = all.iter().find(|m| m.source == dev);
        let queue = |f: &str| sys(name, &format!("queue/{f}")).or_else(|| sys(&disk, &format!("queue/{f}")));
        let mountpoint = m.map(|m| m.target.clone());
        // Every mount of this device (btrfs subvolumes mount the same one).
        let system = all.iter().filter(|x| x.source == dev).any(|x| SYSTEM.contains(&x.target.as_str()));
        Facts {
            device: dev,
            disk: format!("/dev/{disk}"),
            size: sys(name, "size").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 512,
            fstype: m.map(|m| m.fstype.clone()).or(fstype),
            used_pct: mountpoint.as_deref().and_then(crate::mount::usage).map(|u| u.percent),
            mountpoint,
            mount_opts: m.map(|m| m.opts.clone()).unwrap_or_default(),
            rotational: queue("rotational").map(|r| r == "1"),
            trim: queue("discard_max_bytes").and_then(|v| v.parse::<u64>().ok()).is_some_and(|v| v > 0),
            fstrim_timer: None,
            system,
            encrypted: encrypted(name),
            virtual_disk: disk.starts_with("vd")
                || disk.starts_with("xvd")
                || sys(&disk, "device/vendor").as_deref() == Some("0x1af4")
                || sys(&disk, "device/model").is_some_and(|m| {
                    let m = m.to_ascii_lowercase();
                    m.contains("qemu") || m.contains("virtual") || m.contains("vbox")
                }),
        }
    }

    /// Filesystem type of an unmounted partition from its signature.
    fn signature(name: &str) -> Option<String> {
        use std::io::Read;
        let mut buf = vec![0u8; 72 << 10];
        let n = fs::File::open(format!("/dev/{name}")).and_then(|mut f| f.read(&mut buf)).ok()?;
        let sig = crate::verify::data_signatures(&buf[..n]);
        let first = sig.into_iter().find(|s| !s.contains("partition table"))?;
        Some(
            match first {
                "ext2/3/4" => "ext4",
                "XFS" => "xfs",
                "btrfs" => "btrfs",
                "NTFS" => "ntfs",
                "exFAT" => "exfat",
                "FAT32" | "FAT" => "vfat",
                other => other,
            }
            .to_string(),
        )
    }

    /// Filesystems to assess for the argument, and the disk to map.
    /// (device name, filesystem from its signature) of each filesystem.
    type Fss = Vec<(String, Option<String>)>;

    fn targets(arg: &str, all: &[Mount]) -> Result<(Fss, String), String> {
        let p = PathBuf::from(arg);
        if arg.starts_with("/dev/") {
            let real = fs::canonicalize(&p).map_err(|e| format!("{arg}: {e}"))?;
            let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !Path::new(&format!("/sys/class/block/{name}")).exists() {
                return Err(format!("{arg} is not a block device"));
            }
            let mut parts: Vec<String> = fs::read_dir(format!("/sys/class/block/{name}"))
                .map(|r| {
                    r.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .filter(|n| n.starts_with(&name) && *n != name)
                        .collect()
                })
                .unwrap_or_default();
            parts.sort();
            let disk = disk_of(&name);
            if parts.is_empty() {
                return Ok((vec![(name.clone(), signature(&name))], disk));
            }
            // Partitions, or what is stacked on them (LUKS / LVM).
            let mut out = Vec::new();
            for part in parts {
                let holders: Vec<String> = fs::read_dir(format!("/sys/class/block/{part}/holders"))
                    .map(|r| r.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
                    .unwrap_or_default();
                if holders.is_empty() {
                    let sig = signature(&part);
                    let size = sys(&part, "size").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 512;
                    let mounted = all.iter().any(|m| m.source == format!("/dev/{part}"));
                    // Skip BIOS-boot / reserved partitions: tiny and no filesystem.
                    if sig.is_some() || mounted || size >= 64 << 20 {
                        out.push((part, sig));
                    }
                } else {
                    out.extend(holders.into_iter().map(|h| (h, None)));
                }
            }
            return Ok((out, disk));
        }
        let real = fs::canonicalize(&p).map_err(|e| format!("{arg}: {e}"))?;
        let m = all
            .iter()
            .filter(|m| m.source.starts_with("/dev/") && real.starts_with(&m.target))
            .max_by_key(|m| m.target.len())
            .ok_or_else(|| format!("no block-device filesystem holds {arg}"))?;
        let name = m.source.trim_start_matches("/dev/").to_string();
        let disk = disk_of(&name);
        Ok((vec![(name, None)], disk))
    }

    /// Physical disk name under a block device name (`sda3` → `sda`).
    pub fn disk_name(name: &str) -> String {
        disk_of(name)
    }

    /// Block device holding the filesystem that contains `path`.
    pub fn mount_source(path: &Path) -> Option<String> {
        let real = fs::canonicalize(path).ok()?;
        mounts()
            .into_iter()
            .filter(|m| m.source.starts_with("/dev/") && real.starts_with(&m.target))
            .max_by_key(|m| m.target.len())
            .map(|m| m.source)
    }

    /// Mountpoint if `dev` (or a partition of it) is mounted read-write.
    pub fn mounted_rw(dev: &str) -> Option<String> {
        let real = fs::canonicalize(dev).ok()?.to_string_lossy().into_owned();
        mounts()
            .into_iter()
            .find(|m| (m.source == real || m.source.starts_with(&real)) && m.opts.iter().any(|o| o == "rw"))
            .map(|m| m.target)
    }

    /// Assess every filesystem for the argument (fast; no disk reads
    /// except filesystem signatures).
    pub fn gather(arg: &str) -> Result<Gathered, String> {
        let all = mounts();
        let (targets, disk) = targets(arg, &all)?;
        let timer = timer();
        let fss = targets
            .iter()
            .map(|(n, fs)| {
                let mut f = facts(n, &all, fs.clone());
                f.fstrim_timer = timer.clone();
                let a = assess(&f);
                (f, a)
            })
            .collect();
        Ok(Gathered { fss, disk: format!("/dev/{disk}") })
    }

    /// Sample the disk (root; read-only) into `cells` cells.
    pub fn sample_map(g: &Gathered, cells: usize) -> Result<DiskMap, String> {
        let disk = g.disk.trim_start_matches("/dev/");
        let size = sys(disk, "size").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 512;
        let f = fs::File::open(&g.disk).map_err(|e| format!("cannot open {}: {e}", g.disk))?;
        if size < (cells as u64) * 4096 * 4 {
            return Err("disk too small to map".into());
        }
        let cell_bytes = size / cells as u64;
        let mut buf = vec![0u8; 4096];
        let mut grid = Vec::with_capacity(cells);
        for c in 0..cells as u64 {
            let mut samples = Vec::with_capacity(4);
            for k in 0..4u64 {
                // Spread within the cell, 4 KiB aligned.
                let off = c * cell_bytes + (cell_bytes / 4) * k + (cell_bytes / 8);
                let off = off - off % 4096;
                samples.push(match f.read_exact_at(&mut buf, off) {
                    Ok(()) => classify(&buf),
                    Err(_) => Sample::Unreadable,
                });
            }
            grid.push(samples);
        }
        let ranges = g
            .fss
            .iter()
            .map(|(f, _)| {
                let name = f.device.trim_start_matches("/dev/");
                let start = sys(name, "start").and_then(|s| s.parse::<u64>().ok()).map(|s| s * 512);
                (f.device.clone(), start.map(|s| (s, s + f.size)), f.used_pct)
            })
            .collect::<Vec<_>>();
        Ok(DiskMap::build(&g.disk, cell_bytes, grid, &ranges))
    }

    pub fn run(arg: &str, map: bool, cells: usize) -> i32 {
        let g = match gather(arg) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("dcheck: {e}");
                return 1;
            }
        };
        println!();
        for l in report_lines(&g) {
            println!("{l}");
        }
        if map {
            if crate::native::is_root() {
                let tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
                if tty {
                    eprint!("  sampling {} …", g.disk);
                }
                let m = sample_map(&g, cells);
                if tty {
                    eprint!("\r\x1b[2K");
                }
                match m {
                    Ok(m) => {
                        for l in map_lines(&m, crate::report::ui_plain()) {
                            println!("{l}");
                        }
                    }
                    Err(e) => println!("  (disk map: {e})"),
                }
            } else {
                println!("  (run as root for a map of where {} still holds data)", g.disk);
            }
            println!();
        }
        0
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::{DiskMap, Gathered};

    pub fn gather(_: &str) -> Result<Gathered, String> {
        Err("recover is Linux-only for now".into())
    }

    pub fn mounted_rw(_: &str) -> Option<String> {
        None
    }

    pub fn sample_map(_: &Gathered, _: usize) -> Result<DiskMap, String> {
        Err("recover is Linux-only for now".into())
    }

    pub fn run(_: &str, _: bool, _: usize) -> i32 {
        eprintln!("dcheck: recover is Linux-only for now");
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(fs: &str, rot: Option<bool>, trim: bool, opts: &[&str]) -> Facts {
        Facts {
            device: "/dev/sda3".into(),
            disk: "/dev/sda".into(),
            size: 1 << 40,
            fstype: Some(fs.into()),
            mountpoint: Some("/data".into()),
            mount_opts: opts.iter().map(|s| s.to_string()).collect(),
            rotational: rot,
            trim,
            ..Default::default()
        }
    }

    #[test]
    fn ssd_with_discard_is_almost_none() {
        // lab-243: btrfs on a SATA SSD, discard=async.
        let a = assess(&facts("btrfs", Some(false), true, &["rw", "ssd", "discard=async"]));
        assert_eq!(a.chance, Chance::AlmostNone);
        assert!(a.reasons[0].contains("discard=async"));
    }

    #[test]
    fn hdd_by_filesystem() {
        assert_eq!(assess(&facts("ntfs3", Some(true), false, &["rw"])).chance, Chance::High);
        assert_eq!(assess(&facts("vfat", Some(true), false, &["rw"])).chance, Chance::High);
        // 10.0.0.251: ext4 on an HDD.
        let a = assess(&facts("ext4", Some(true), false, &["rw", "relatime"]));
        assert_eq!(a.chance, Chance::Medium);
        assert!(a.steps.iter().any(|s| s.contains("ext4magic")));
        assert!(a.steps.iter().any(|s| s.contains("photorec")));
        assert!(a.steps.iter().any(|s| s.contains("ddrescue -d -n /dev/sda3")));
    }

    #[test]
    fn fstrim_timer_lowers_and_adds_a_step() {
        let mut f = facts("ext4", Some(false), true, &["rw"]);
        f.fstrim_timer = Some(Timer { enabled: true, last: Some("Mon 2026-09-21".into()), next: None });
        let a = assess(&f);
        assert_eq!(a.chance, Chance::Low);
        assert!(a.steps.iter().any(|s| s.contains("systemctl stop fstrim.timer")));
        // TRIM that never reaches the SSD (behind a RAID controller).
        let a = assess(&facts("ext4", Some(false), false, &["rw"]));
        assert_eq!(a.chance, Chance::Medium);
        assert!(a.reasons.iter().any(|r| r.contains("TRIM does not reach")));
    }

    #[test]
    fn full_and_system_disks() {
        let mut f = facts("ntfs", Some(true), false, &["rw"]);
        f.used_pct = Some(95.0);
        assert_eq!(assess(&f).chance, Chance::Medium);
        let mut f = facts("ext4", Some(true), false, &["rw"]);
        f.system = true;
        let a = assess(&f);
        assert!(a.steps.iter().any(|s| s.contains("live USB")));
        assert!(!a.steps.iter().any(|s| s.contains("umount")));
    }

    #[test]
    fn virtual_disks_get_vm_advice() {
        // idch: virtio disk, ext4 mounted with discard, rotational=1.
        let mut f = facts("ext4", Some(true), true, &["rw", "discard"]);
        f.virtual_disk = true;
        f.system = true;
        let a = assess(&f);
        assert_eq!(a.chance, Chance::Low);
        assert!(a.reasons[0].contains("thin provisioning"), "{:?}", a.reasons);
        assert!(!a.reasons.iter().any(|r| r.contains("platters")));
        assert!(a.steps.iter().any(|s| s.contains("snapshot")));
        assert!(a.steps.iter().any(|s| s.contains("rescue mode")));
        assert!(!a.steps.iter().any(|s| s.contains("live USB")));
    }

    #[test]
    fn parses_systemctl_timer() {
        let t = parse_timer(
            "LastTriggerUSec=Mon 2026-09-21 00:14:02 WIB\nNextElapseUSecRealtime=Mon 2026-09-28 00:00:00 WIB\n",
            "enabled\n",
        );
        assert!(t.enabled);
        assert_eq!(t.last.as_deref(), Some("Mon 2026-09-21 00:14:02 WIB"));
        let t = parse_timer("LastTriggerUSec=n/a\n", "disabled");
        assert!(!t.enabled);
        assert_eq!(t.last, None);
    }

    #[test]
    fn map_cells_and_residue() {
        assert_eq!(classify(&[0u8; 4096]), Sample::Empty);
        assert_eq!(classify(&[0xFFu8; 4096]), Sample::Empty);
        let mut b = [0u8; 4096];
        b[7] = 1;
        assert_eq!(classify(&b), Sample::Data);
        assert_eq!(cell_char(&[Sample::Data; 4], true), '#');
        assert_eq!(cell_char(&[Sample::Empty; 4], true), '.');
        assert_eq!(cell_char(&[Sample::Data, Sample::Empty, Sample::Empty, Sample::Empty], true), '+');
        // 4% used, data in 5% of samples: ~1% of the free space holds data.
        let r = free_space_residue(0.05, 0.04).unwrap();
        assert!((r - 0.0104).abs() < 0.001);
        // HDD: data everywhere, 30% used → all free space holds old data.
        assert_eq!(free_space_residue(1.0, 0.3), Some(1.0));
        assert_eq!(free_space_residue(0.1, 0.3), Some(0.0));
    }
}
