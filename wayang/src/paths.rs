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

/// `/data/etc/network` (persisted uplink choice; see `docs/NETWORK.md`).
pub fn data_network_dir() -> PathBuf {
    data_dir().join("etc/network")
}

/// `/data/etc/network/primary` — the chosen uplink interface name.
pub fn primary_file() -> PathBuf {
    data_network_dir().join("primary")
}

/// `/data/etc/network/config` — shell-sourceable MODE/FAMILY/… snippet.
pub fn network_config_file() -> PathBuf {
    data_network_dir().join("config")
}

/// `/data/etc/wpa_supplicant.conf` — wifi credentials.
pub fn wpa_conf_file() -> PathBuf {
    data_dir().join("etc/wpa_supplicant.conf")
}

/// `/data/etc/ssh` — root's persisted SSH keys (survives updates).
pub fn ssh_dir() -> PathBuf {
    data_dir().join("etc/ssh")
}

/// `/data/etc/ssh/authorized_keys` — the persistent source of truth; the boot
/// script appends it to `/root/.ssh/authorized_keys`.
pub fn ssh_authorized_keys() -> PathBuf {
    ssh_dir().join("authorized_keys")
}

/// `/root/.ssh/authorized_keys` — the live file sshd reads this boot.
pub fn root_authorized_keys() -> PathBuf {
    under_root("root/.ssh/authorized_keys", "/root/.ssh/authorized_keys")
}

/// `/etc/resolv.conf` (relocated under `WAYANG_ROOT` for tests).
pub fn resolv_conf() -> PathBuf {
    under_root("etc/resolv.conf", "/etc/resolv.conf")
}

/// `/var/run` (relocated under `WAYANG_ROOT` for tests).
pub fn run_dir() -> PathBuf {
    under_root("var/run", "/var/run")
}

fn under_root(rel: &str, abs: &str) -> PathBuf {
    match root() {
        Some(r) => r.join(rel),
        None => PathBuf::from(abs),
    }
}
