//! Runtime paths. `WAYANG_ROOT` (when set) relocates `/etc/wayang`, `/boot`
//! and `/data` under a prefix so tests never touch the real system.

use std::path::PathBuf;

pub fn root() -> Option<PathBuf> {
    std::env::var_os("WAYANG_ROOT")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

pub fn etc_wayang() -> PathBuf {
    under_root("etc/wayang", "/etc/wayang")
}

pub fn data_dir() -> PathBuf {
    under_root("data", "/data")
}

pub fn version_file() -> PathBuf {
    etc_wayang().join("version")
}

pub fn channel_file() -> PathBuf {
    etc_wayang().join("channel")
}

pub fn trusted_keys_file() -> PathBuf {
    etc_wayang().join("trusted_keys")
}

fn under_root(rel: &str, abs: &str) -> PathBuf {
    match root() {
        Some(r) => r.join(rel),
        None => PathBuf::from(abs),
    }
}
