//! Writing a verified update into the idle slot and maintaining GRUB state.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::error::{AppError, Result};
use crate::grubenv::GrubEnv;
use crate::manifest::SlotMeta;
use crate::mount::BootRoot;
use crate::slot::{self, Slot};

fn grubenv_path(boot: &BootRoot) -> std::path::PathBuf {
    boot.path.join("grub/grubenv")
}

fn write_file_sync(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| AppError::err(format!("{}: {e}", dir.display())))?;
    }
    let mut f = File::create(path).map_err(|e| AppError::err(format!("{}: {e}", path.display())))?;
    f.write_all(data).map_err(|e| AppError::err(format!("{}: {e}", path.display())))?;
    f.sync_all().map_err(|e| AppError::err(format!("{}: {e}", path.display())))?;
    Ok(())
}

/// Write `vmlinuz`/`initramfs.img`/`meta` into the idle slot, then point GRUB
/// at it (`wayang_slot`, `wayang_attempts=0`) and remember the previous slot.
pub fn stage(boot: &BootRoot, kernel: &[u8], initramfs: &[u8], meta: &SlotMeta) -> Result<Slot> {
    let mut env = GrubEnv::read(&grubenv_path(boot)).unwrap_or_default();
    let active = slot::staged_slot(&env);
    let target = active.idle();

    write_file_sync(&boot.path.join(target.as_str()).join("vmlinuz"), kernel)?;
    write_file_sync(&boot.path.join(target.as_str()).join("initramfs.img"), initramfs)?;
    write_file_sync(
        &boot.path.join("var").join(format!("meta-{}.json", target.as_str())),
        meta.to_json().as_bytes(),
    )?;

    env.set("wayang_prev", active.as_str());
    env.set("wayang_slot", target.as_str());
    env.set("wayang_attempts", "0");
    env.write(&grubenv_path(boot)).map_err(AppError::err)?;
    Ok(target)
}

/// `wayang mark-ok`: confirm the running slot and clear the attempt counter.
pub fn mark_ok(boot: &BootRoot) -> Result<Slot> {
    let path = grubenv_path(boot);
    let mut env = GrubEnv::read(&path).unwrap_or_default();
    let active = slot::boot_slot(&env);
    env.set("wayang_good", active.as_str());
    env.set("wayang_slot", active.as_str());
    env.set("wayang_attempts", "0");
    env.write(&path).map_err(AppError::err)?;
    Ok(active)
}

/// `wayang update --rollback`: stage the previously saved slot for the next boot.
pub fn rollback(boot: &BootRoot) -> Result<Slot> {
    let path = grubenv_path(boot);
    let mut env = GrubEnv::read(&path).unwrap_or_default();
    let target = env
        .get("wayang_prev")
        .and_then(Slot::parse)
        .or_else(|| env.get("wayang_good").and_then(Slot::parse))
        .ok_or_else(|| AppError::err("no previous slot recorded in grubenv"))?;
    let current = slot::staged_slot(&env);
    env.set("wayang_prev", current.as_str());
    env.set("wayang_slot", target.as_str());
    env.set("wayang_attempts", "0");
    env.write(&path).map_err(AppError::err)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot_dir() -> (std::path::PathBuf, BootRoot) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "wayang-stage-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("grub")).unwrap();
        let boot = BootRoot { path: dir.clone(), mounted: None };
        (dir, boot)
    }

    fn meta() -> SlotMeta {
        SlotMeta {
            version: "1.4.1".into(),
            channel: "stable".into(),
            arch: "x86_64".into(),
            time: 1760000000,
            kernel_sha256: "aa".into(),
            keyid: "release".into(),
        }
    }

    #[test]
    fn stage_writes_idle_slot_and_grubenv() {
        let (dir, boot) = boot_dir();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "A");
        env.write(&dir.join("grub/grubenv")).unwrap();

        let target = stage(&boot, b"k", b"i", &meta()).unwrap();
        assert_eq!(target, Slot::B);
        assert_eq!(std::fs::read(dir.join("B/vmlinuz")).unwrap(), b"k");
        assert_eq!(std::fs::read(dir.join("B/initramfs.img")).unwrap(), b"i");
        assert!(dir.join("var/meta-B.json").exists());

        let env = GrubEnv::read(&dir.join("grub/grubenv")).unwrap();
        assert_eq!(env.get("wayang_slot"), Some("B"));
        assert_eq!(env.get("wayang_prev"), Some("A"));
        assert_eq!(env.get("wayang_attempts"), Some("0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_ok_recovers_fallback_then_clears() {
        let (dir, boot) = boot_dir();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "B");
        env.set("wayang_good", "A");
        env.set("wayang_attempts", "3");
        env.write(&dir.join("grub/grubenv")).unwrap();

        let active = mark_ok(&boot).unwrap();
        assert_eq!(active, Slot::A);
        let env = GrubEnv::read(&dir.join("grub/grubenv")).unwrap();
        assert_eq!(env.get("wayang_good"), Some("A"));
        assert_eq!(env.get("wayang_slot"), Some("A"));
        assert_eq!(env.get("wayang_attempts"), Some("0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_swaps_to_previous() {
        let (dir, boot) = boot_dir();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "B");
        env.set("wayang_prev", "A");
        env.set("wayang_good", "A");
        env.write(&dir.join("grub/grubenv")).unwrap();

        let target = rollback(&boot).unwrap();
        assert_eq!(target, Slot::A);
        let env = GrubEnv::read(&dir.join("grub/grubenv")).unwrap();
        assert_eq!(env.get("wayang_slot"), Some("A"));
        assert_eq!(env.get("wayang_prev"), Some("B"));
        assert_eq!(env.get("wayang_attempts"), Some("0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rollback_errors_without_target() {
        let (dir, boot) = boot_dir();
        assert_eq!(rollback(&boot).unwrap_err().code, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
