//! The install itself, on a worker thread reporting to the UI.
//!
//! Disk layout (GPT):
//!   p1  512 MiB  EFI system, FAT32 `WAYANGBOOT`: GRUB at the UEFI fallback
//!                path \EFI\BOOT\BOOTX64.EFI plus the A/B payload:
//!                /boot/grub/grub.cfg, /boot/grub/grubenv, /boot/A/vmlinuz,
//!                /boot/A/initramfs.img and /boot/var/meta-A.json. Slot B is
//!                empty until `wayang` stages an update into it.
//!   p2  rest     Linux, ext4 `WAYANGDATA`: mounted at /data by the installed
//!                system; holds hostname, authorized_keys and SSH host keys
//! WayangOS itself still runs from RAM.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::disks::Disk;
use crate::net;
use crate::sha256;
use crate::sys;

pub const PAYLOAD: &str = "/usr/share/wayang-install";

/// Version recorded in `/boot/var/meta-A.json` (the rootfs ships the same value
/// in `/etc/wayang/version`). Overridable at build time with `WAYANG_VERSION`,
/// otherwise the installer crate version.
pub const VERSION: &str = match option_env!("WAYANG_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Edition recorded in `/boot/var/meta-A.json`. Overridable with
/// `WAYANG_EDITION`, otherwise `generic`.
pub const EDITION: &str = match option_env!("WAYANG_EDITION") {
    Some(e) => e,
    None => "generic",
};

/// Files copied verbatim from the payload: (payload path, path on the ESP).
const BOOT_FILES: [(&str, &str); 4] = [
    ("BOOTX64.EFI", "EFI/BOOT/BOOTX64.EFI"),
    ("grub.cfg", "boot/grub/grub.cfg"),
    ("A/vmlinuz", "boot/A/vmlinuz"),
    ("A/initramfs.img", "boot/A/initramfs.img"),
];

/// GRUB fallback state, written fresh on every install.
const GRUBENV: &str = "boot/grub/grubenv";
/// Metadata for slot A, written with hashes of the copied payload.
const META_A: &str = "boot/var/meta-A.json";
/// GRUB environment block is always exactly this many bytes.
const GRUBENV_SIZE: usize = 1024;
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
    pub net: net::NetChoice,
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
    let state = write_boot_state(mnt);
    let _ = sys::run("sync", &[]);
    sys::run("umount", &[mnt])?;
    copied?;
    state?;

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
    for (src_rel, dest_rel) in BOOT_FILES {
        let src_path = Path::new(PAYLOAD).join(src_rel);
        let dest = Path::new(mnt).join(dest_rel);
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        log(tx, format!("copy {src_rel} -> /{dest_rel}"));
        let mut src = File::open(&src_path).map_err(|e| format!("{}: {e}", src_path.display()))?;
        let mut dst = File::create(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        loop {
            let n = src.read(&mut buf).map_err(|e| format!("{src_rel}: {e}"))?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf[..n])
                .map_err(|e| format!("{src_rel}: {e}"))?;
            done += n as u64;
            let _ = tx.send(Msg::Copy(done as f64 / total as f64));
        }
        dst.sync_all().map_err(|e| format!("{src_rel}: {e}"))?;
    }
    Ok(())
}

/// The GRUB environment block that boots slot A and counts boot attempts.
///
/// GRUB's 1024-byte format: `# GRUB Environment Block\n`, then `key=value\n`
/// lines, then `#` padding to the end. GRUB scans back over the trailing `#`
/// run and appends before it, so the final byte stays `#`.
pub fn grubenv() -> Vec<u8> {
    let mut buf = Vec::with_capacity(GRUBENV_SIZE);
    buf.extend_from_slice(b"# GRUB Environment Block\n");
    buf.extend_from_slice(b"wayang_slot=A\n");
    buf.extend_from_slice(b"wayang_good=A\n");
    buf.extend_from_slice(b"wayang_attempts=0\n");
    buf.resize(GRUBENV_SIZE, b'#');
    buf
}

/// Write the fallback state (`grubenv`) and slot-A metadata to the ESP.
fn write_boot_state(mnt: &str) -> Result<(), String> {
    write_sync(&Path::new(mnt).join(GRUBENV), &grubenv())?;
    let meta = meta_a_json()?;
    write_sync(&Path::new(mnt).join(META_A), meta.as_bytes())
}

/// `meta-A.json` for the freshly installed slot, hashing the payload bytes.
fn meta_a_json() -> Result<String, String> {
    let kernel = sha256_hex(&Path::new(PAYLOAD).join("A/vmlinuz"))?;
    let initramfs = sha256_hex(&Path::new(PAYLOAD).join("A/initramfs.img"))?;
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // The installer runs the same initramfs that is installed, so its
    // /etc/wayang/version is the authoritative version for slot A.
    let version = installed_version();
    Ok(format!(
        "{{\"version\":\"{version}\",\"arch\":\"x86_64\",\"edition\":\"{EDITION}\",\"kernel_sha256\":\"{kernel}\",\"initramfs_sha256\":\"{initramfs}\",\"time\":{time}}}\n"
    ))
}

/// `/etc/wayang/version` of the running (and to-be-installed) rootfs, else the
/// build-time [`VERSION`].
fn installed_version() -> String {
    fs::read_to_string("/etc/wayang/version")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| VERSION.to_string())
}

fn sha256_hex(path: &Path) -> Result<String, String> {
    let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(sha256::sha256(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn write_sync(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut f = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(data)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.sync_all()
        .map_err(|e| format!("{}: {e}", path.display()))?;
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

    write_network(mnt, &plan.net, tx)
}

/// Persist the uplink choice to the target `/data` (the ext4 `WAYANGDATA`
/// partition, mounted at `mnt`). Auto mode writes nothing, so the installed
/// system keeps probing every wired NIC.
fn write_network(mnt: &str, net: &net::NetChoice, tx: &Sender<Msg>) -> Result<(), String> {
    let Some(iface) = net.pinned() else {
        log(tx, "network: auto (no primary pinned)");
        return Ok(());
    };
    let primary = Path::new(mnt).join(net::PRIMARY_REL);
    let config = Path::new(mnt).join(net::CONFIG_REL);
    log(
        tx,
        format!("network: primary {iface} ({})", net.mode.config()),
    );
    write_sync(&primary, net.primary_text().as_bytes())?;
    write_sync(&config, net.config_text().as_bytes())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grubenv_is_a_valid_1024_byte_block() {
        let env = grubenv();
        assert_eq!(env.len(), GRUBENV_SIZE);
        assert!(env.starts_with(b"# GRUB Environment Block\n"));
        assert_eq!(env.last(), Some(&b'#'), "block must end with # padding");
        let text = String::from_utf8_lossy(&env);
        assert!(text.contains("\nwayang_slot=A\n"));
        assert!(text.contains("\nwayang_good=A\n"));
        assert!(text.contains("\nwayang_attempts=0\n"));
        // exactly one signature line, and no stray newline after the padding
        assert_eq!(
            text.matches("# GRUB Environment Block\n").count(),
            1,
            "signature must be the first line"
        );
    }

    #[test]
    fn boot_files_use_the_ab_layout() {
        let dests: Vec<&str> = BOOT_FILES.iter().map(|(_, d)| *d).collect();
        assert!(dests.contains(&"boot/A/vmlinuz"));
        assert!(dests.contains(&"boot/A/initramfs.img"));
        assert!(dests.contains(&"boot/grub/grub.cfg"));
        assert!(dests.contains(&"EFI/BOOT/BOOTX64.EFI"));
        // the old flat layout must be gone
        assert!(!dests.contains(&"boot/vmlinuz"));
        assert!(!dests.contains(&"boot/initramfs.img"));
    }

    #[test]
    fn write_network_persists_only_when_pinned() {
        let dir = std::env::temp_dir().join(format!("wayang-net-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mnt = dir.to_str().unwrap();
        let (tx, _rx) = mpsc::channel();

        // auto: no primary, nothing written
        write_network(mnt, &net::NetChoice::default(), &tx).unwrap();
        assert!(!dir.join(net::PRIMARY_REL).exists());
        assert!(!dir.join(net::CONFIG_REL).exists());

        // pinned: primary + config land under /data/etc/network
        let choice = net::NetChoice {
            iface: Some("eth0".into()),
            mode: net::Mode::Static,
            family: net::Family::Ipv4,
            ipv4_address: "10.0.0.2/24".into(),
            ..Default::default()
        };
        write_network(mnt, &choice, &tx).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(net::PRIMARY_REL)).unwrap(),
            "eth0\n"
        );
        let cfg = fs::read_to_string(dir.join(net::CONFIG_REL)).unwrap();
        assert!(cfg.contains("MODE=static\n"));
        assert!(cfg.contains("IPV4_ADDRESS=10.0.0.2/24\n"));

        let _ = fs::remove_dir_all(&dir);
    }
}
