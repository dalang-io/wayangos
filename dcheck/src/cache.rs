//! Cache for SMART reads.
//!
//! Some drives answer slowly (a SAS disk behind a PERC takes 0.3–0.75 s per
//! SCSI command, ~3 s per report), and the TUI used to read the same disk at
//! scan time and again every time its report was opened. Two layers:
//!
//! - in-process: a read is reused for the lifetime of the process;
//! - on disk (TTL, default 10 min, `cache_ttl_secs`): `/var/cache/dcheck`
//!   for root, else `$XDG_CACHE_HOME/dcheck` or `~/.cache/dcheck`. Entries
//!   are keyed by device path + model + serial + size, so a swapped disk is
//!   never served stale data. The file is 0600 (it contains serials).
//!
//! Monitoring (`check`, `watch`, `prometheus`, `--json`) and `--fresh` /
//! `DCHECK_NO_CACHE=1` bypass both layers and always read the hardware.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{self, Json};
use crate::model::Device;
use crate::smartctl::SmartData;

static FRESH: AtomicBool = AtomicBool::new(false);
static DISK_LOCK: Mutex<()> = Mutex::new(());

/// When this process started: in fresh mode only reads made since then are
/// reused (so one command never reads the same disk twice).
fn process_start() -> u64 {
    static START: OnceLock<u64> = OnceLock::new();
    *START.get_or_init(now)
}

/// Always read the hardware (monitoring, `--fresh`).
pub fn set_fresh(fresh: bool) {
    FRESH.store(fresh, Ordering::Relaxed);
}

fn fresh() -> bool {
    FRESH.load(Ordering::Relaxed) || std::env::var_os("DCHECK_NO_CACHE").is_some()
}

#[derive(Clone)]
struct Entry {
    at: u64,
    smart: Option<SmartData>,
}

fn memory() -> &'static Mutex<HashMap<String, Entry>> {
    static MEM: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    MEM.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Identity of a physical disk at a path: a swap changes the key. The WWID
/// (when sysfs has one) also tells apart two disks of the same model/size.
fn key(d: &Device) -> String {
    let wwid = ["device/wwid", "wwid"]
        .iter()
        .find_map(|f| std::fs::read_to_string(format!("/sys/block/{}/{f}", d.name)).ok())
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    format!(
        "{}|{}|{}|{}|{}",
        d.path,
        d.model.as_deref().unwrap_or(""),
        d.serial.as_deref().unwrap_or(""),
        d.size_bytes,
        wwid
    )
}

fn ttl() -> u64 {
    crate::config::load().cache_ttl_secs
}

fn cache_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DCHECK_CACHE_DIR") {
        return Some(PathBuf::from(p).join("smart.json"));
    }
    let dir = if crate::native::is_root() {
        PathBuf::from("/var/cache/dcheck")
    } else if let Some(x) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        PathBuf::from(x).join("dcheck")
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".cache/dcheck")
    };
    Some(dir.join("smart.json"))
}

fn load_disk() -> HashMap<String, Entry> {
    let mut out = HashMap::new();
    let Some(path) = cache_file() else { return out };
    let Ok(text) = std::fs::read_to_string(path) else { return out };
    let Some(Json::Obj(map)) = Json::parse(&text) else { return out };
    for (k, v) in map {
        let Some(at) = v.get("at").and_then(Json::as_u64) else { continue };
        let smart = match v.get("smart") {
            Some(Json::Null) | None => None,
            Some(s) => match SmartData::from_json(s) {
                Some(s) => Some(s),
                None => continue,
            },
        };
        out.insert(k, Entry { at, smart });
    }
    out
}

fn save_disk(entries: &HashMap<String, Entry>) {
    let Some(path) = cache_file() else { return };
    let limit = now().saturating_sub(ttl());
    let obj: Vec<(&str, Json)> = entries
        .iter()
        .filter(|(_, e)| e.at >= limit)
        .map(|(k, e)| {
            let smart = e.smart.as_ref().map(SmartData::to_json).unwrap_or(Json::Null);
            (k.as_str(), json::object(vec![("at", json::num(e.at as f64)), ("smart", smart)]))
        })
        .collect();
    let text = json::object(obj).to_string();
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Write-then-rename so concurrent readers never see a half file.
    let tmp = dir.join(format!(".smart.json.{}", std::process::id()));
    if std::fs::write(&tmp, text).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

/// SMART for `d`: from the cache when allowed and young enough, otherwise
/// `read()` (the result is then cached).
pub fn smart(d: &Device, read: impl FnOnce() -> Option<SmartData>) -> Option<SmartData> {
    let k = key(d);
    let start = process_start();
    if let Some(e) = memory().lock().ok().and_then(|m| m.get(&k).cloned()) {
        if !fresh() || e.at >= start {
            return e.smart;
        }
    }
    if !fresh() {
        let ttl = ttl();
        if ttl > 0 {
            let cached = {
                let _guard = DISK_LOCK.lock();
                load_disk().remove(&k)
            };
            if let Some(e) = cached.filter(|e| now().saturating_sub(e.at) <= ttl) {
                if let Ok(mut m) = memory().lock() {
                    m.insert(k, e.clone());
                }
                return e.smart;
            }
        }
    }
    let value = read();
    let entry = Entry { at: now(), smart: value.clone() };
    if let Ok(mut m) = memory().lock() {
        m.insert(k.clone(), entry.clone());
    }
    // Persist real reads (including "no SMART" results) for later runs.
    if ttl() > 0 && !crate::enumerate::is_demo() {
        let _guard = DISK_LOCK.lock();
        let mut disk = load_disk();
        disk.insert(k, entry);
        save_disk(&disk);
    }
    value
}

/// Seconds since the cached SMART data for `d` was read, if cached.
pub fn age(d: &Device) -> Option<u64> {
    let m = memory().lock().ok()?;
    m.get(&key(d)).map(|e| now().saturating_sub(e.at))
}

/// Forget `d` so the next read goes to the hardware (TUI `r`).
pub fn invalidate(d: &Device) {
    if let Ok(mut m) = memory().lock() {
        m.remove(&key(d));
    }
    let _guard = DISK_LOCK.lock();
    let mut disk = load_disk();
    if disk.remove(&key(d)).is_some() {
        save_disk(&disk);
    }
}

/// "just now", "42 s", "3 min", "2 h".
pub fn fmt_age(secs: u64) -> String {
    match secs {
        0..=4 => "just now".into(),
        5..=59 => format!("{secs} s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        _ => format!("{} h ago", secs / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_age() {
        assert_eq!(fmt_age(2), "just now");
        assert_eq!(fmt_age(42), "42 s ago");
        assert_eq!(fmt_age(180), "3 min ago");
        assert_eq!(fmt_age(7300), "2 h ago");
    }

    #[test]
    fn key_changes_when_the_disk_is_swapped() {
        let mut d = crate::enumerate::demo_devices().remove(0);
        let a = key(&d);
        d.serial = Some("OTHER".into());
        assert_ne!(a, key(&d));
    }
}
