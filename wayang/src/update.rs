//! `wayang update` / `wayang upgrade` / `wayang update --rollback`.

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

fn load_keys() -> Result<TrustedKeys> {
    trusted::load(&paths::trusted_keys_file())
        .map_err(|e| AppError::err(format!("trusted keys: {e}")))
}

pub fn run(upgrade: bool, a: &UpdateArgs) -> Result<i32> {
    if a.rollback {
        if upgrade {
            return Err(AppError::err("--rollback is only valid for `wayang update`"));
        }
        return run_rollback(a);
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
        None => download_and_verify(&channel, host, &manifest.version)?,
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

fn download_and_verify(channel: &str, host: &str, version: &str) -> Result<Bundle> {
    let url = fetch::bundle_url(&fetch::base_url(), channel, host, version);
    let dest = std::env::temp_dir().join(format!(
        "wayang-{}-{}-{}.wup",
        version,
        host,
        std::process::id()
    ));
    fetch::download(&url, &dest)?;
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
}
