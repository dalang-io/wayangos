//! Writing a verified update into the idle slot and maintaining A/B state.
//!
//! The state backend is resolved by [`EnvStore`] (GRUB env on x86, a plain
//! `wayang/vars` file on ARM); the keys are identical on both.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::error::{AppError, Result};
use crate::manifest::SlotMeta;
use crate::mount::BootRoot;
use crate::slot::{self, Slot};
use crate::state::EnvStore;

fn state_store(boot: &BootRoot) -> Result<EnvStore> {
    EnvStore::open(&boot.path).map_err(AppError::err)
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

/// Write `vmlinuz`/`initramfs.img`/`meta` into the idle slot, then point the
/// bootloader at it (`wayang_slot`, `wayang_attempts=0`) and remember the
/// previous slot.
pub fn stage(boot: &BootRoot, kernel: &[u8], initramfs: &[u8], meta: &SlotMeta) -> Result<Slot> {
    let mut store = state_store(boot)?;
    let active = slot::staged_slot(&store);
    let target = active.idle();

    write_file_sync(&boot.path.join(target.as_str()).join("vmlinuz"), kernel)?;
    write_file_sync(&boot.path.join(target.as_str()).join("initramfs.img"), initramfs)?;
    write_file_sync(
        &boot.path.join("var").join(format!("meta-{}.json", target.as_str())),
        meta.to_json().as_bytes(),
    )?;

    store.set("wayang_prev", active.as_str());
    store.set("wayang_slot", target.as_str());
    store.set("wayang_attempts", "0");
    store.save().map_err(AppError::err)?;
    Ok(target)
}

/// `wayang mark-ok`: confirm the running slot and clear the attempt counter.
pub fn mark_ok(boot: &BootRoot) -> Result<Slot> {
    let mut store = state_store(boot)?;
    let active = slot::boot_slot(&store);
    store.set("wayang_good", active.as_str());
    store.set("wayang_slot", active.as_str());
    store.set("wayang_attempts", "0");
    store.save().map_err(AppError::err)?;
    Ok(active)
}

/// `wayang update --rollback`: stage the previously saved slot for the next boot.
pub fn rollback(boot: &BootRoot) -> Result<Slot> {
    let mut store = state_store(boot)?;
    let target = store
        .get("wayang_prev")
        .and_then(Slot::parse)
        .or_else(|| store.get("wayang_good").and_then(Slot::parse))
        .ok_or_else(|| AppError::err("no previous slot recorded in update state"))?;
    let current = slot::staged_slot(&store);
    store.set("wayang_prev", current.as_str());
    store.set("wayang_slot", target.as_str());
    store.set("wayang_attempts", "0");
    store.save().map_err(AppError::err)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grubenv::GrubEnv;
    use crate::state::VarsFile;

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

    #[test]
    fn stage_and_rollback_on_arm_vars_backend() {
        let (dir, boot) = boot_dir();
        let mut vars = VarsFile::default();
        vars.set("wayang_slot", "A");
        vars.set("wayang_good", "A");
        vars.write(&dir.join("wayang/vars")).unwrap();

        let target = stage(&boot, b"k", b"i", &meta()).unwrap();
        assert_eq!(target, Slot::B);
        let vars = VarsFile::read(&dir.join("wayang/vars")).unwrap();
        assert_eq!(vars.get("wayang_slot"), Some("B"));
        assert_eq!(vars.get("wayang_prev"), Some("A"));

        let target = rollback(&boot).unwrap();
        assert_eq!(target, Slot::A);
        let vars = VarsFile::read(&dir.join("wayang/vars")).unwrap();
        assert_eq!(vars.get("wayang_slot"), Some("A"));
        assert_eq!(vars.get("wayang_prev"), Some("B"));
        assert_eq!(vars.get("wayang_attempts"), Some("0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_errors_without_backend() {
        let (dir, boot) = boot_dir();
        let err = stage(&boot, b"k", b"i", &meta()).unwrap_err();
        assert_eq!(err.code, 1);
        assert!(err.msg.contains("no update state backend"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
