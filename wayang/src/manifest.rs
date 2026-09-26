//! `manifest.json` schema (both channel manifests and bundle manifests) and the
//! per-slot `meta-<slot>.json` written at staging time.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

pub const PRODUCT: &str = "wayangos";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub product: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    pub version: String,
    pub major: u64,
    pub arch: String,
    #[serde(default)]
    pub edition: Option<String>,
    /// Kernel release the bundle was built from (e.g. "7.2.7"), informational.
    #[serde(default)]
    pub kernel_version: Option<String>,
    pub kernel_sha256: String,
    pub initramfs_sha256: String,
    #[serde(default)]
    pub min_from: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub time: Option<u64>,
    #[serde(default = "default_keyid")]
    pub keyid: String,
}

fn default_channel() -> String {
    "stable".into()
}

fn default_keyid() -> String {
    "release".into()
}

impl Manifest {
    pub fn from_slice(bytes: &[u8]) -> Result<Manifest> {
        serde_json::from_slice(bytes).map_err(|e| AppError::err(format!("invalid manifest.json: {e}")))
    }
}

/// `/boot/var/meta-<slot>.json` — the version installed in a slot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlotMeta {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub kernel_version: Option<String>,
    #[serde(default)]
    pub time: u64,
    #[serde(default)]
    pub kernel_sha256: String,
    #[serde(default)]
    pub keyid: String,
}

impl SlotMeta {
    pub fn read(path: &std::path::Path) -> Option<SlotMeta> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "product": "wayangos",
      "channel": "stable",
      "version": "1.4.1",
      "major": 1,
      "arch": "x86_64",
      "edition": "intel",
      "kernel_sha256": "aa",
      "initramfs_sha256": "bb",
      "min_from": "1.0.0",
      "notes": "text",
      "time": 1760000000,
      "keyid": "release"
    }"#;

    #[test]
    fn parses_sample() {
        let m = Manifest::from_slice(SAMPLE.as_bytes()).unwrap();
        assert_eq!(m.product, PRODUCT);
        assert_eq!(m.version, "1.4.1");
        assert_eq!(m.major, 1);
        assert_eq!(m.arch, "x86_64");
        assert_eq!(m.keyid, "release");
        assert_eq!(m.min_from.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn defaults_for_optional() {
        let raw = r#"{"product":"wayangos","version":"2.0.0","major":2,
                      "arch":"arm64","kernel_sha256":"aa","initramfs_sha256":"bb"}"#;
        let m = Manifest::from_slice(raw.as_bytes()).unwrap();
        assert_eq!(m.channel, "stable");
        assert_eq!(m.keyid, "release");
        assert!(m.min_from.is_none());
        assert!(m.edition.is_none());
    }

    #[test]
    fn rejects_missing_required() {
        let raw = r#"{"product":"wayangos","version":"1.0.0"}"#;
        assert!(Manifest::from_slice(raw.as_bytes()).is_err());
    }
}
