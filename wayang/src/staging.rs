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

/// Rewrite `<ESP>/boot/grub/grub.cfg` from the shared template, but only when it
/// differs, so updated systems pick up menu changes (per-slot versions,
/// `wayang.slot`, …) without rewriting the ESP on every boot.
fn refresh_grub_cfg(boot: &BootRoot) {
    const TEMPLATE: &str = include_str!("../grub-disk.cfg");
    let cfg = boot.path.join("grub/grub.cfg");
    if std::fs::read_to_string(&cfg).is_ok_and(|cur| cur == TEMPLATE) {
        return;
    }
    if let Some(dir) = cfg.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&cfg, TEMPLATE);
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
    stage_from(boot, slot::running_slot(), kernel, initramfs, meta)
}

/// [`stage`] with the running slot given; never writes into it. `None`
/// (no `wayang.slot=` on the command line) trusts `wayang_slot`.
pub fn stage_from(boot: &BootRoot, running: Option<Slot>, kernel: &[u8], initramfs: &[u8], meta: &SlotMeta) -> Result<Slot> {
    let mut store = state_store(boot)?;
    let active = running.unwrap_or_else(|| slot::staged_slot(&store));
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
    // Record the staged version/kernel so the GRUB menu can show each slot.
    let kver = meta.kernel_version.clone().unwrap_or_else(|| "?".into());
    store.set(&format!("wayang_ver_{}", target.as_str()), &meta.version);
    store.set(&format!("wayang_kver_{}", target.as_str()), &kver);
    store.save().map_err(AppError::err)?;
    Ok(target)
}

/// `wayang mark-ok`: confirm the running slot and clear the attempt counter.
pub fn mark_ok(boot: &BootRoot) -> Result<Slot> {
    mark_ok_from(boot, slot::running_slot())
}

/// [`mark_ok`] with the running slot given. Marking what GRUB *planned* to
/// boot instead of what booted would, after a manual pick of the other menu
/// entry, record a broken slot as good (and then fall back to it forever).
pub fn mark_ok_from(boot: &BootRoot, running: Option<Slot>) -> Result<Slot> {
    let mut store = state_store(boot)?;
    let active = running.unwrap_or_else(|| slot::boot_slot(&store));
    store.set("wayang_good", active.as_str());
    store.set("wayang_slot", active.as_str());
    store.set("wayang_attempts", "0");
    // Refresh the per-slot version/kernel from the on-ESP metadata so the GRUB
    // menu reflects whatever each slot actually holds.
    for s in [Slot::A, Slot::B] {
        let meta = boot.path.join("var").join(format!("meta-{}.json", s.as_str()));
        let Ok(text) = std::fs::read_to_string(&meta) else { continue };
        let Ok(m) = serde_json::from_str::<SlotMeta>(&text) else { continue };
        if m.version.is_empty() {
            continue;
        }
        store.set(&format!("wayang_ver_{}", s.as_str()), &m.version);
        let kver = m.kernel_version.unwrap_or_else(|| "?".into());
        store.set(&format!("wayang_kver_{}", s.as_str()), &kver);
    }
    store.save().map_err(AppError::err)?;
    refresh_grub_cfg(boot);
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
    let current = slot::running_slot().unwrap_or_else(|| slot::staged_slot(&store));
    store.set("wayang_prev", current.as_str());
    store.set("wayang_slot", target.as_str());
    store.set("wayang_attempts", "0");
    store.save().map_err(AppError::err)?;
    Ok(target)
}

/// `wayang update --boot-other`: point the next boot at the idle slot, whatever
/// it holds. This is a one-shot switch: `wayang_good` is untouched (and seeded
/// to the running slot if it was never set), so a slot that never reaches
/// `wayang mark-ok` still falls back after the attempt budget. Nothing is
/// applied or committed.
pub fn stage_other(boot: &BootRoot) -> Result<Slot> {
    let mut store = state_store(boot)?;
    let current = slot::running_slot().unwrap_or_else(|| slot::staged_slot(&store));
    let target = current.idle();
    if store.get("wayang_good").is_none() {
        store.set("wayang_good", current.as_str());
    }
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
            kernel_version: Some("7.2.7".into()),
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
    fn mark_ok_marks_the_slot_that_booted_not_the_planned_one() {
        // B was staged (and freezes); the admin picked A in the GRUB menu
        let (dir, boot) = boot_dir();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "B");
        env.set("wayang_good", "A");
        env.set("wayang_attempts", "1");
        env.write(&dir.join("grub/grubenv")).unwrap();

        assert_eq!(mark_ok_from(&boot, Some(Slot::A)).unwrap(), Slot::A);
        let env = GrubEnv::read(&dir.join("grub/grubenv")).unwrap();
        assert_eq!(env.get("wayang_good"), Some("A"));
        assert_eq!(env.get("wayang_slot"), Some("A"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_never_overwrites_the_running_slot() {
        let (dir, boot) = boot_dir();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "B");
        env.write(&dir.join("grub/grubenv")).unwrap();
        // running A although B is staged: the update must go to B, not A
        assert_eq!(stage_from(&boot, Some(Slot::A), b"k", b"i", &meta()).unwrap(), Slot::B);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn running_slot_from_cmdline() {
        assert_eq!(
            slot::parse_cmdline("BOOT_IMAGE=/boot/A/vmlinuz loglevel=3 wayang.data=LABEL=WAYANGDATA wayang.slot=A"),
            Some(Slot::A)
        );
        assert_eq!(slot::parse_cmdline("console=ttyS0"), None);
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
    fn stage_other_switches_to_the_idle_slot_and_keeps_good() {
        let (dir, boot) = boot_dir();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "A");
        env.set("wayang_good", "A");
        env.set("wayang_attempts", "2");
        env.write(&dir.join("grub/grubenv")).unwrap();

        // no `wayang.slot=` on this host, so the staged slot is the baseline
        assert_eq!(stage_other(&boot).unwrap(), Slot::B);
        let env = GrubEnv::read(&dir.join("grub/grubenv")).unwrap();
        assert_eq!(env.get("wayang_slot"), Some("B"));
        assert_eq!(env.get("wayang_prev"), Some("A"));
        assert_eq!(env.get("wayang_attempts"), Some("0"), "attempt budget resets for the new slot");
        assert_eq!(env.get("wayang_good"), Some("A"), "fallback target is preserved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_other_seeds_good_when_missing() {
        let (dir, boot) = boot_dir();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "A");
        env.write(&dir.join("grub/grubenv")).unwrap();

        assert_eq!(stage_other(&boot).unwrap(), Slot::B);
        let env = GrubEnv::read(&dir.join("grub/grubenv")).unwrap();
        assert_eq!(env.get("wayang_good"), Some("A"), "running slot becomes the fallback");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_other_on_arm_vars_backend() {
        let (dir, boot) = boot_dir();
        let mut vars = VarsFile::default();
        vars.set("wayang_slot", "B");
        vars.set("wayang_good", "A");
        vars.write(&dir.join("wayang/vars")).unwrap();

        assert_eq!(stage_other(&boot).unwrap(), Slot::A);
        let vars = VarsFile::read(&dir.join("wayang/vars")).unwrap();
        assert_eq!(vars.get("wayang_slot"), Some("A"));
        assert_eq!(vars.get("wayang_prev"), Some("B"));
        assert_eq!(vars.get("wayang_good"), Some("A"));
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
