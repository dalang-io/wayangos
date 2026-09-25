//! The install itself, on a worker thread reporting to the UI.
//!
//! Disk layout (GPT):
//!   p1  512 MiB  EFI system, FAT32 `WAYANGBOOT`: GRUB at the UEFI fallback
//!                path \EFI\BOOT\BOOTX64.EFI + kernel + initramfs
//!   p2  rest     Linux, ext4 `WAYANGDATA`: mounted at /data by the installed
//!                system; holds hostname, authorized_keys and SSH host keys
//! WayangOS itself still runs from RAM.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use crate::disks::Disk;
use crate::sys;

pub const PAYLOAD: &str = "/usr/share/wayang-install";
/// (payload file, directory on the ESP)
const BOOT_FILES: [(&str, &str); 4] = [
    ("BOOTX64.EFI", "EFI/BOOT"),
    ("grub.cfg", "boot/grub"),
    ("vmlinuz", "boot"),
    ("initramfs.img", "boot"),
];
// full-featured tools shipped with the installer; BusyBox's mke2fs (earlier
// in init's PATH) can't make ext4
const SFDISK: &str = "/usr/sbin/sfdisk";
const MKE2FS: &str = "/usr/sbin/mke2fs";
const LOCK: &str = "/tmp/wayang-installer.lock";

pub const STEPS: [&str; 6] = [
    "RELEASE DISK",
    "PARTITION  GPT",
    "FORMAT BOOT  FAT32",
    "FORMAT DATA  EXT4",
    "COPY SYSTEM",
    "WRITE SETTINGS",
];

#[derive(Debug, Clone)]
pub struct Plan {
    pub disk: Disk,
    pub hostname: String,
    pub keys: Vec<String>,
}

#[derive(Debug)]
pub enum Msg {
    Step(usize),
    /// Progress within COPY SYSTEM, 0.0..=1.0.
    Copy(f64),
    Log(String),
    Done,
    Failed(String),
}

/// Payload files missing from this image (empty when it can install).
pub fn missing_payload(demo: bool) -> Vec<&'static str> {
    if demo {
        return Vec::new();
    }
    BOOT_FILES
        .iter()
        .map(|(f, _)| *f)
        .chain([SFDISK, MKE2FS])
        .filter(|f| !Path::new(PAYLOAD).join(f).exists() && !Path::new(f).exists())
        .collect()
}

pub fn start(plan: Plan, demo: bool) -> Receiver<Msg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = if demo { fake(&tx) } else { real(&plan, &tx) };
        let _ = tx.send(match result {
            Ok(()) => Msg::Done,
            Err(e) => {
                let _ = fs::remove_file(LOCK);
                Msg::Failed(e)
            }
        });
    });
    rx
}

fn log(tx: &Sender<Msg>, s: impl Into<String>) {
    let _ = tx.send(Msg::Log(s.into()));
}

fn real(plan: &Plan, tx: &Sender<Msg>) -> Result<(), String> {
    // another session (console vs SSH) may be installing too
    File::options()
        .write(true)
        .create_new(true)
        .open(LOCK)
        .map_err(|_| "another session is already installing".to_string())?;

    let disk = &plan.disk;
    let dev = disk.path();
    let (boot, data) = (disk.part_path(1), disk.part_path(2));

    let _ = tx.send(Msg::Step(0));
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    for line in mounts.lines() {
        let mut w = line.split_whitespace();
        if let (Some(src), Some(mnt)) = (w.next(), w.next()) {
            // the disk itself or one of its partitions (sda1, nvme0n1p1), not sdaa
            let rest = src.strip_prefix(&dev).unwrap_or("x");
            if rest
                .trim_start_matches('p')
                .chars()
                .all(|c| c.is_ascii_digit())
            {
                log(tx, format!("umount {mnt}"));
                sys::run("umount", &[mnt])?;
            }
        }
    }

    let _ = tx.send(Msg::Step(1));
    log(tx, format!("sfdisk {dev}: GPT, 512M EFI + Linux"));
    let script = "label: gpt\nsize=512MiB, type=uefi, name=\"WAYANGBOOT\"\ntype=linux, name=\"WAYANGDATA\"\n";
    sys::run_input(
        SFDISK,
        &[
            "--quiet",
            "--wipe",
            "always",
            "--wipe-partitions",
            "always",
            &dev,
        ],
        Some(script),
    )?;
    let mut waited = 0;
    while !(Path::new(&boot).exists() && Path::new(&data).exists()) {
        if waited >= 50 {
            return Err(format!("partitions {boot} / {data} did not appear"));
        }
        thread::sleep(Duration::from_millis(200));
        waited += 1;
    }

    let _ = tx.send(Msg::Step(2));
    log(tx, format!("mkfs.vfat {boot}"));
    sys::run("mkfs.vfat", &["-n", "WAYANGBOOT", &boot])?;

    let _ = tx.send(Msg::Step(3));
    log(tx, format!("mke2fs -t ext4 {data}"));
    sys::run(
        MKE2FS,
        &["-F", "-q", "-t", "ext4", "-L", "WAYANGDATA", &data],
    )?;

    let _ = tx.send(Msg::Step(4));
    let mnt = "/tmp/wayang-target";
    fs::create_dir_all(mnt).map_err(|e| format!("{mnt}: {e}"))?;
    sys::run("mount", &["-t", "vfat", &boot, mnt])?;
    let copied = copy_boot_files(mnt, tx);
    let _ = sys::run("sync", &[]);
    sys::run("umount", &[mnt])?;
    copied?;

    let _ = tx.send(Msg::Step(5));
    sys::run("mount", &["-t", "ext4", &data, mnt])?;
    let written = write_settings(mnt, plan, tx);
    let _ = sys::run("sync", &[]);
    sys::run("umount", &[mnt])?;
    written
}

fn copy_boot_files(mnt: &str, tx: &Sender<Msg>) -> Result<(), String> {
    let total: u64 = BOOT_FILES
        .iter()
        .map(|(f, _)| {
            fs::metadata(Path::new(PAYLOAD).join(f))
                .map(|m| m.len())
                .unwrap_or(0)
        })
        .sum::<u64>()
        .max(1);
    let mut done = 0u64;
    let mut buf = vec![0u8; 1 << 20];
    for (file, dir) in BOOT_FILES {
        let dest_dir = Path::new(mnt).join(dir);
        fs::create_dir_all(&dest_dir).map_err(|e| format!("{}: {e}", dest_dir.display()))?;
        log(tx, format!("copy {file} -> /{dir}/"));
        let src_path = Path::new(PAYLOAD).join(file);
        let mut src = File::open(&src_path).map_err(|e| format!("{}: {e}", src_path.display()))?;
        let dest_path = dest_dir.join(file);
        let mut dst =
            File::create(&dest_path).map_err(|e| format!("{}: {e}", dest_path.display()))?;
        loop {
            let n = src.read(&mut buf).map_err(|e| format!("{file}: {e}"))?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])
                .map_err(|e| format!("{file}: {e}"))?;
            done += n as u64;
            let _ = tx.send(Msg::Copy(done as f64 / total as f64));
        }
        dst.sync_all().map_err(|e| format!("{file}: {e}"))?;
    }
    Ok(())
}

fn write_settings(mnt: &str, plan: &Plan, tx: &Sender<Msg>) -> Result<(), String> {
    let etc = format!("{mnt}/etc");
    let ssh = format!("{etc}/ssh");
    fs::create_dir_all(&ssh).map_err(|e| format!("{ssh}: {e}"))?;
    fs::create_dir_all(format!("{etc}/dropbear")).map_err(|e| format!("{etc}/dropbear: {e}"))?;
    sys::chmod(&ssh, 0o700);
    sys::chmod(&format!("{etc}/dropbear"), 0o700);

    log(tx, format!("hostname {}", plan.hostname));
    fs::write(format!("{etc}/hostname"), format!("{}\n", plan.hostname))
        .map_err(|e| format!("hostname: {e}"))?;

    let keys = format!("{ssh}/authorized_keys");
    log(tx, format!("{} SSH key(s) for root", plan.keys.len()));
    let mut text = plan.keys.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    fs::write(&keys, text).map_err(|e| format!("authorized_keys: {e}"))?;
    sys::chmod(&keys, 0o600);
    Ok(())
}

fn fake(tx: &Sender<Msg>) -> Result<(), String> {
    for (i, step) in STEPS.iter().enumerate() {
        let _ = tx.send(Msg::Step(i));
        log(tx, format!("{} ...", step.to_lowercase()));
        if i == 4 {
            for n in 1..=40 {
                thread::sleep(Duration::from_millis(40));
                let _ = tx.send(Msg::Copy(n as f64 / 40.0));
            }
        } else {
            thread::sleep(Duration::from_millis(500));
        }
    }
    Ok(())
}
