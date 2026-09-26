//! Version decision: `update` is allowed only within the installed major,
//! `upgrade` is the explicit cross-major path.

use semver::Version;

use crate::error::{AppError, Result};
use crate::manifest::{Manifest, PRODUCT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Available,
    NoUpdate,
}

/// True when bundle and installed share a major version.
pub fn same_major(installed: &Version, bundle: &Version) -> bool {
    bundle.major == installed.major
}

pub fn parse_version(s: &str) -> Result<Version> {
    Version::parse(s.trim()).map_err(|e| AppError::err(format!("invalid version '{s}': {e}")))
}

/// `min_from` is an optional lower bound on the installed version.
pub fn min_from_ok(installed: &Version, min_from: Option<&str>) -> Result<bool> {
    match min_from {
        None => Ok(true),
        Some(s) => {
            let min = parse_version(s)?;
            Ok(installed >= &min)
        }
    }
}

/// Reject bundles built for another arch / requiring a newer base.
pub fn check_compat(installed: &Version, m: &Manifest, host_arch: &str) -> Result<()> {
    if m.product != PRODUCT {
        return Err(AppError::err(format!(
            "bundle product '{}' is not '{PRODUCT}'",
            m.product
        )));
    }
    if m.arch != host_arch {
        return Err(AppError::incompatible(format!(
            "bundle is for arch '{}', host is '{host_arch}'",
            m.arch
        )));
    }
    if !min_from_ok(installed, m.min_from.as_deref())? {
        return Err(AppError::incompatible(format!(
            "bundle requires at least {}, installed is {installed}",
            m.min_from.as_deref().unwrap_or("?")
        )));
    }
    Ok(())
}

/// `upgrade` selects cross-major semantics; `update` refuses a major change.
pub fn decide(upgrade: bool, installed: &Version, m: &Manifest) -> Result<Decision> {
    let bundle = parse_version(&m.version)?;
    if same_major(installed, &bundle) {
        if bundle <= *installed {
            return Ok(Decision::NoUpdate);
        }
        return Ok(Decision::Available);
    }
    if !upgrade {
        return Err(AppError::err(format!(
            "{installed} -> {bundle} is a major change; run `wayang upgrade` (or use --from)"
        )));
    }
    if bundle.major < installed.major {
        return Err(AppError::err(format!(
            "refusing downgrade {installed} -> {bundle}"
        )));
    }
    Ok(Decision::Available)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str, arch: &str, min_from: Option<&str>) -> Manifest {
        Manifest {
            product: "wayangos".into(),
            channel: "stable".into(),
            version: version.into(),
            major: version.split('.').next().unwrap().parse().unwrap(),
            arch: arch.into(),
            edition: None,
            kernel_version: None,
            kernel_sha256: "aa".into(),
            initramfs_sha256: "bb".into(),
            min_from: min_from.map(str::to_string),
            notes: None,
            time: None,
            keyid: "release".into(),
        }
    }

    #[test]
    fn update_within_major() {
        let installed = parse_version("1.2.0").unwrap();
        let m = manifest("1.4.1", "x86_64", None);
        assert_eq!(decide(false, &installed, &m).unwrap(), Decision::Available);
    }

    #[test]
    fn update_noop_on_equal_or_older() {
        let installed = parse_version("1.4.1").unwrap();
        assert_eq!(decide(false, &installed, &manifest("1.4.1", "x86_64", None)).unwrap(), Decision::NoUpdate);
        assert_eq!(decide(false, &installed, &manifest("1.2.0", "x86_64", None)).unwrap(), Decision::NoUpdate);
    }

    #[test]
    fn update_refuses_major_change() {
        let installed = parse_version("1.4.1").unwrap();
        let m = manifest("2.0.0", "x86_64", None);
        assert!(decide(false, &installed, &m).is_err());
        assert_eq!(decide(true, &installed, &m).unwrap(), Decision::Available);
    }

    #[test]
    fn upgrade_refuses_downgrade() {
        let installed = parse_version("2.1.0").unwrap();
        assert!(decide(true, &installed, &manifest("1.9.0", "x86_64", None)).is_err());
    }

    #[test]
    fn compat_checks() {
        let installed = parse_version("1.0.0").unwrap();
        assert!(check_compat(&installed, &manifest("1.4.1", "x86_64", Some("1.0.0")), "x86_64").is_ok());
        assert_eq!(
            check_compat(&installed, &manifest("1.4.1", "arm64", None), "x86_64").unwrap_err().code,
            4
        );
        assert_eq!(
            check_compat(&installed, &manifest("1.4.1", "x86_64", Some("1.2.0")), "x86_64").unwrap_err().code,
            4
        );
    }
}
