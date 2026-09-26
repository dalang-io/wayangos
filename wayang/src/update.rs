//! `wayang update` / `wayang upgrade` / `wayang update --rollback`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::arch::host_arch;
use crate::bundle::{self, Bundle};
use crate::cli::UpdateArgs;
use crate::error::{AppError, Result};
use crate::fetch;
use crate::manifest::{Manifest, SlotMeta};
use crate::mount;
use crate::paths;
use crate::sema::{self, Decision};
use crate::staging;
use crate::trusted::{self, TrustedKeys};
use crate::version;

/// Lock-free byte counter shared between the download loop (background thread)
/// and the HUD: readers never block the updater.
#[derive(Debug, Default)]
pub struct Progress {
    downloaded: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    pub fn new() -> Arc<Progress> {
        Arc::new(Progress::default())
    }

    pub fn set(&self, downloaded: u64) {
        self.downloaded.store(downloaded, Ordering::Relaxed);
    }

    pub fn set_total(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn downloaded(&self) -> u64 {
        self.downloaded.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

/// Percent complete (0..=100) when `total` is known; `None` means the transfer
/// size is unknown, so the UI stays indeterminate.
pub fn percent(downloaded: u64, total: u64) -> Option<f64> {
    if total == 0 {
        return None;
    }
    Some((downloaded as f64 * 100.0 / total as f64).clamp(0.0, 100.0))
}

/// Transfer rate in MB/s (SI, 10^6 bytes) over `elapsed_secs`.
pub fn mb_per_s(downloaded: u64, elapsed_secs: f64) -> f64 {
    if elapsed_secs <= 0.0 {
        0.0
    } else {
        downloaded as f64 / 1_000_000.0 / elapsed_secs
    }
}

/// `3.4 MB/s` (or KB/s below 1 MB/s) for the gauge label.
pub fn human_rate(mb_per_s: f64) -> String {
    if mb_per_s >= 1.0 {
        format!("{mb_per_s:.1} MB/s")
    } else {
        format!("{:.0} KB/s", mb_per_s * 1000.0)
    }
}

fn load_keys() -> Result<TrustedKeys> {
    trusted::load(&paths::trusted_keys_file())
        .map_err(|e| AppError::err(format!("trusted keys: {e}")))
}

pub fn run(upgrade: bool, a: &UpdateArgs) -> Result<i32> {
    run_with_progress(upgrade, a, None)
}

/// [`run`] with an optional progress hook fed by the bundle download.
pub fn run_with_progress(upgrade: bool, a: &UpdateArgs, progress: Option<Arc<Progress>>) -> Result<i32> {
    if a.rollback {
        if upgrade {
            return Err(AppError::err("--rollback is only valid for `wayang update`"));
        }
        return run_rollback(a);
    }
    if a.boot_other {
        if upgrade {
            return Err(AppError::err("--boot-other is only valid for `wayang update`"));
        }
        return run_boot_other(a);
    }

    let installed = sema::parse_version(&version::read()?)?;
    let channel = a
        .channel
        .clone()
        .or_else(version::read_channel)
        .unwrap_or_else(|| "stable".into());
    let host = host_arch();

    // Obtain the manifest. `--from` also gives us a verified bundle to reuse.
    let mut loaded: Option<Bundle> = None;
    let manifest: Manifest = if let Some(file) = &a.from {
        let keys = load_keys()?;
        let b = bundle::read(file)?;
        b.verify_signature(&keys)?;
        let m = b.manifest.clone();
        loaded = Some(b);
        m
    } else {
        let url = fetch::manifest_url(&fetch::base_url(), &channel, host);
        Manifest::from_slice(&fetch::fetch_bytes(&url)?)?
    };

    sema::check_compat(&installed, &manifest, host)?;
    if let Decision::NoUpdate = sema::decide(upgrade, &installed, &manifest)? {
        crate::outln!(
            "No update available: installed {} is up to date (bundle {}).",
            installed, manifest.version
        );
        return Ok(2);
    }

    if a.check {
        crate::outln!(
            "Update available: {} -> {} ({} / {}).",
            installed, manifest.version, manifest.channel, manifest.arch
        );
        if let Some(n) = &manifest.notes {
            crate::outln!("notes: {n}");
        }
        return Ok(0);
    }

    let bundle = match loaded {
        Some(b) => b,
        None => download_and_verify(&channel, host, &manifest.version, progress.as_deref())?,
    };

    let boot = mount::open(a.esp.as_deref())?;
    let meta = SlotMeta {
        version: bundle.manifest.version.clone(),
        channel: bundle.manifest.channel.clone(),
        arch: bundle.manifest.arch.clone(),
        kernel_version: bundle.manifest.kernel_version.clone(),
        time: bundle.manifest.time.unwrap_or(0),
        kernel_sha256: bundle.manifest.kernel_sha256.clone(),
        keyid: bundle.manifest.keyid.clone(),
    };
    let target = staging::stage(&boot, &bundle.kernel, &bundle.initramfs, &meta)?;
    drop(boot);

    crate::outln!("Staged {} into slot {}.", bundle.manifest.version, target.as_str());
    maybe_reboot(a.reboot)?;
    Ok(0)
}

fn download_and_verify(
    channel: &str,
    host: &str,
    version: &str,
    progress: Option<&Progress>,
) -> Result<Bundle> {
    let url = fetch::bundle_url(&fetch::base_url(), channel, host, version);
    let dest = std::env::temp_dir().join(format!(
        "wayang-{}-{}-{}.wup",
        version,
        host,
        std::process::id()
    ));
    fetch::download_with_progress(&url, &dest, |got, total| {
        if let Some(p) = progress {
            p.set_total(total);
            p.set(got);
        }
    })?;
    let result = (|| {
        let keys = load_keys()?;
        let b = bundle::read(&dest)?;
        b.verify_signature(&keys)?;
        Ok(b)
    })();
    let _ = std::fs::remove_file(&dest);
    result
}

pub fn run_rollback(a: &UpdateArgs) -> Result<i32> {
    let boot = mount::open(a.esp.as_deref())?;
    let target = staging::rollback(&boot)?;
    drop(boot);
    crate::outln!("Rollback staged: next boot uses slot {}.", target.as_str());
    maybe_reboot(a.reboot)?;
    Ok(0)
}

/// Stage a one-shot boot into the idle slot (the one not running). This only
/// moves `wayang_slot`; the anti-lockout fallback (`wayang_good`, attempts)
/// still protects the next boot, and nothing is applied or committed here.
pub fn run_boot_other(a: &UpdateArgs) -> Result<i32> {
    let boot = mount::open(a.esp.as_deref())?;
    let target = staging::stage_other(&boot)?;
    drop(boot);
    crate::outln!("Boot staged: next boot uses slot {}.", target.as_str());
    maybe_reboot(a.reboot)?;
    Ok(0)
}

/// Reboot only for a real invocation that asked for it.
pub fn maybe_reboot(requested: bool) -> Result<()> {
    if !requested {
        crate::outln!("Reboot to apply (or pass --reboot).");
        return Ok(());
    }
    if std::env::var_os("WAYANG_NO_REBOOT").is_some() || paths::root().is_some() {
        crate::outln!("Reboot suppressed (test environment).");
        return Ok(());
    }
    let status = std::process::Command::new("reboot")
        .status()
        .map_err(|e| AppError::err(format!("failed to run reboot: {e}")))?;
    if !status.success() {
        return Err(AppError::err("reboot failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reboot_not_called_in_test_env() {
        std::env::set_var("WAYANG_ROOT", "/tmp/wayang-reboot-test");
        assert!(maybe_reboot(true).is_ok());
        std::env::remove_var("WAYANG_ROOT");
    }

    #[test]
    fn progress_percent_and_rate() {
        assert_eq!(percent(0, 0), None, "unknown total stays indeterminate");
        assert_eq!(percent(50, 100), Some(50.0));
        assert_eq!(percent(25, 100), Some(25.0));
        assert_eq!(percent(150, 100), Some(100.0), "clamped to 100");
        assert_eq!(mb_per_s(2_000_000, 1.0), 2.0);
        assert_eq!(mb_per_s(1_000_000, 0.0), 0.0);
        assert_eq!(human_rate(2.5), "2.5 MB/s");
        assert_eq!(human_rate(0.5), "500 KB/s");
    }

    #[test]
    fn progress_shared_counter() {
        let p = Progress::new();
        assert_eq!(percent(p.downloaded(), p.total()), None);
        p.set_total(1000);
        p.set(250);
        assert_eq!(percent(p.downloaded(), p.total()), Some(25.0));
    }

    #[test]
    fn boot_other_only_valid_for_update() {
        let mut a = UpdateArgs { boot_other: true, ..Default::default() };
        let err = run_with_progress(true, &a, None).unwrap_err();
        assert!(err.msg.contains("--boot-other"), "{}", err.msg);
        a.boot_other = false;
        a.rollback = true;
        let err = run_with_progress(true, &a, None).unwrap_err();
        assert!(err.msg.contains("--rollback"), "{}", err.msg);
    }
}
