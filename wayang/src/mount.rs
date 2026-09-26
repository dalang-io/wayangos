//! Resolving the ESP to a writable directory and cleaning up temporary mounts.
//!
//! With `WAYANG_ROOT` set, the boot directory is `<root>/boot` and nothing is
//! mounted (tests). Otherwise the ESP is discovered and used where it is
//! already mounted, or mounted read-write at a temp dir and unmounted on drop.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{AppError, Result};
use crate::paths;

pub struct BootRoot {
    pub path: PathBuf,
    pub(crate) mounted: Option<PathBuf>,
}

impl BootRoot {
    pub fn unmount(&mut self) {
        if let Some(mp) = self.mounted.take() {
            let _ = Command::new("umount").arg(&mp).status();
        }
    }
}

impl Drop for BootRoot {
    fn drop(&mut self) {
        self.unmount();
    }
}

pub fn open(esp_override: Option<&str>) -> Result<BootRoot> {
    open_with(paths::root(), esp_override)
}

/// Pure form of [`open`] with an explicit `WAYANG_ROOT` value.
pub fn open_with(root: Option<PathBuf>, esp_override: Option<&str>) -> Result<BootRoot> {
    if let Some(root) = root {
        return Ok(BootRoot { path: root.join("boot"), mounted: None });
    }

    let device = match esp_override {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d),
        _ => crate::esp::discover().ok_or_else(|| {
            AppError::err("no ESP found (set WAYANG_ESP or pass --esp DEV)")
        })?,
    };

    if let Some(mp) = mounted_at(&device) {
        return Ok(BootRoot { path: state_root(&mp), mounted: None });
    }

    let dir = std::env::temp_dir().join(format!("wayang-esp-{}", std::process::id()));
    std::fs::create_dir_all(&dir)
        .map_err(|e| AppError::err(format!("{}: {e}", dir.display())))?;

    let out = Command::new("mount")
        .arg("-o")
        .arg("rw")
        .arg(&device)
        .arg(&dir)
        .output()
        .map_err(|e| AppError::err(format!("failed to run mount: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(AppError::err(format!(
            "mount {} {}: {}",
            device.display(),
            dir.display(),
            err.trim()
        )));
    }
    Ok(BootRoot { path: state_root(&dir), mounted: Some(dir) })
}

/// Directory that holds the A/B state (`grub/grubenv` or `wayang/vars`).
///
/// x86 installs nest everything under `<ESP>/boot` (GRUB prefix), so the state
/// is at `<ESP>/boot/grub/grubenv`; ARM keeps `wayang/vars` at the FAT root.
fn state_root(base: &Path) -> PathBuf {
    let nested = base.join("boot/grub/grubenv").exists()
        || base.join("boot/wayang/vars").exists()
        || base.join("boot/A").exists()
        || base.join("boot/var").exists();
    if nested {
        base.join("boot")
    } else {
        base.to_path_buf()
    }
}

/// Mount point of `device` according to `/proc/mounts`, if any.
fn mounted_at(device: &Path) -> Option<PathBuf> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    let want = std::fs::canonicalize(device).unwrap_or_else(|_| device.to_path_buf());
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let dev = match it.next() {
            Some(d) => d,
            None => continue,
        };
        let mp = match it.next() {
            Some(m) => m,
            None => continue,
        };
        let dev_path = PathBuf::from(dev);
        let same = dev_path == want
            || std::fs::canonicalize(&dev_path).map(|c| c == want).unwrap_or(false);
        if same {
            return Some(PathBuf::from(mp));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayang_root_short_circuits() {
        let b = open_with(Some(PathBuf::from("/tmp/wayang-root-test")), Some("/dev/whatever")).unwrap();
        assert_eq!(b.path, PathBuf::from("/tmp/wayang-root-test/boot"));
        assert!(b.mounted.is_none());
    }

    #[test]
    fn state_root_detects_layout() {
        let base = std::env::temp_dir().join(format!("wayang-sr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("boot/grub")).unwrap();
        std::fs::write(base.join("boot/grub/grubenv"), b"").unwrap();
        assert_eq!(state_root(&base), base.join("boot"));

        let flat = std::env::temp_dir().join(format!("wayang-sr-flat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&flat);
        std::fs::create_dir_all(flat.join("wayang")).unwrap();
        assert_eq!(state_root(&flat), flat);

        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&flat);
    }
}
