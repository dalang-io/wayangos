//! State backend resolution for A/B fallback state.
//!
//! x86 uses the GRUB environment block (`<boot>/grub/grubenv`); ARM has no
//! GRUB and uses a plain `key=value` file (`<boot>/wayang/vars`). Both carry
//! the same keys (`wayang_slot`, `wayang_good`, `wayang_attempts`,
//! `wayang_prev`). Resolution order (frozen in `docs/UPDATE-DESIGN.md`):
//!
//! 1. `<boot>/grub/grubenv` exists → GRUB env;
//! 2. else `<boot>/wayang/vars` exists → plain vars;
//! 3. else error with a clear message.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::grubenv::GrubEnv;

/// Read-only view of a key/value state backend (implemented by [`GrubEnv`],
/// [`VarsFile`] and [`EnvStore`]) so callers stay backend-agnostic.
pub trait EnvView {
    fn get(&self, key: &str) -> Option<&str>;
}

impl EnvView for GrubEnv {
    fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }
}

/// `<boot>/wayang/vars`: `key=value\n` lines, no padding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VarsFile {
    pub map: BTreeMap<String, String>,
}

impl VarsFile {
    pub fn from_bytes(buf: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(buf).map_err(|_| "non-UTF-8 vars file".to_string())?;
        let mut map = BTreeMap::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.split_once('=') {
                Some((k, v)) if !k.is_empty() => {
                    map.insert(k.to_string(), v.to_string());
                }
                _ => return Err(format!("malformed vars entry on line {}", n + 1)),
            }
        }
        Ok(VarsFile { map })
    }

    /// Serialise as `key=value\n` lines (sorted, no padding).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in &self.map {
            out.extend_from_slice(k.as_bytes());
            out.push(b'=');
            out.extend_from_slice(v.as_bytes());
            out.push(b'\n');
        }
        out
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_bytes(&data)
    }

    /// Full write, then fsync (same durability story as the GRUB block).
    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut f = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.write_all(&self.to_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        f.sync_all().map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.map.insert(key.to_string(), value.to_string());
    }
}

impl EnvView for VarsFile {
    fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }
}

/// A resolved state backend plus the path to persist it to.
#[derive(Debug, Clone)]
pub enum EnvStore {
    Grub { path: PathBuf, env: GrubEnv },
    Vars { path: PathBuf, vars: VarsFile },
    /// No backend found; used for status display only (never saves).
    Empty,
}

impl EnvStore {
    pub fn grubenv_path(boot: &Path) -> PathBuf {
        boot.join("grub/grubenv")
    }

    pub fn vars_path(boot: &Path) -> PathBuf {
        boot.join("wayang/vars")
    }

    /// Resolve strictly per the spec; errors when neither file exists.
    pub fn open(boot: &Path) -> Result<EnvStore, String> {
        let grub = Self::grubenv_path(boot);
        if grub.exists() {
            let env = GrubEnv::read(&grub)?;
            return Ok(EnvStore::Grub { path: grub, env });
        }
        let vars_path = Self::vars_path(boot);
        if vars_path.exists() {
            let vars = VarsFile::read(&vars_path)?;
            return Ok(EnvStore::Vars { path: vars_path, vars });
        }
        Err(format!(
            "no update state backend under {}: expected grub/grubenv or wayang/vars",
            boot.display()
        ))
    }

    /// Status-only resolution: a fresh system with neither file still lists.
    pub fn open_or_empty(boot: &Path) -> EnvStore {
        Self::open(boot).unwrap_or(EnvStore::Empty)
    }

    pub fn backend(&self) -> &'static str {
        match self {
            EnvStore::Grub { .. } => "grubenv",
            EnvStore::Vars { .. } => "vars",
            EnvStore::Empty => "none",
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        match self {
            EnvStore::Grub { env, .. } => env.get(key),
            EnvStore::Vars { vars, .. } => vars.get(key),
            EnvStore::Empty => None,
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        match self {
            EnvStore::Grub { env, .. } => env.set(key, value),
            EnvStore::Vars { vars, .. } => vars.set(key, value),
            EnvStore::Empty => {}
        }
    }

    pub fn save(&self) -> Result<(), String> {
        match self {
            EnvStore::Grub { path, env } => env.write(path),
            EnvStore::Vars { path, vars } => vars.write(path),
            EnvStore::Empty => Ok(()),
        }
    }
}

impl EnvView for EnvStore {
    fn get(&self, key: &str) -> Option<&str> {
        EnvStore::get(self, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "wayang-state-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn vars_missing_file_errors() {
        let d = tmp();
        assert!(VarsFile::read(&d.join("vars")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn vars_existing_file_parses() {
        let d = tmp();
        let p = d.join("vars");
        std::fs::write(&p, b"wayang_slot=B\nwayang_good=A\nwayang_attempts=2\n").unwrap();
        let v = VarsFile::read(&p).unwrap();
        assert_eq!(v.get("wayang_slot"), Some("B"));
        assert_eq!(v.get("wayang_good"), Some("A"));
        assert_eq!(v.get("wayang_attempts"), Some("2"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn vars_updates_roundtrip() {
        let d = tmp();
        let p = d.join("wayang/vars");
        let mut v = VarsFile::default();
        v.set("wayang_slot", "B");
        v.set("wayang_prev", "A");
        v.set("wayang_attempts", "0");
        v.write(&p).unwrap();
        let raw = std::fs::read(&p).unwrap();
        assert!(raw.starts_with(b"wayang_"));
        assert!(raw.ends_with(b"\n"));
        assert_eq!(raw.len(), VarsFile::read(&p).unwrap().to_bytes().len());
        let mut back = VarsFile::read(&p).unwrap();
        assert_eq!(back.get("wayang_slot"), Some("B"));
        back.set("wayang_slot", "A");
        back.write(&p).unwrap();
        assert_eq!(VarsFile::read(&p).unwrap().get("wayang_slot"), Some("A"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn vars_rejects_malformed() {
        assert!(VarsFile::from_bytes(b"no-equals\n").is_err());
    }

    #[test]
    fn resolves_grub_first() {
        let d = tmp();
        std::fs::create_dir_all(d.join("grub")).unwrap();
        std::fs::create_dir_all(d.join("wayang")).unwrap();
        std::fs::write(d.join("wayang/vars"), b"wayang_slot=B\n").unwrap();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "A");
        env.write(&d.join("grub/grubenv")).unwrap();

        let store = EnvStore::open(&d).unwrap();
        assert_eq!(store.backend(), "grubenv");
        assert_eq!(store.get("wayang_slot"), Some("A"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resolves_vars_when_no_grub() {
        let d = tmp();
        std::fs::create_dir_all(d.join("wayang")).unwrap();
        std::fs::write(d.join("wayang/vars"), b"wayang_slot=B\n").unwrap();

        let store = EnvStore::open(&d).unwrap();
        assert_eq!(store.backend(), "vars");
        assert_eq!(store.get("wayang_slot"), Some("B"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn errors_when_neither_exists() {
        let d = tmp();
        assert!(EnvStore::open(&d).is_err());
        // status tolerates it with an empty store
        assert_eq!(EnvStore::open_or_empty(&d).backend(), "none");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_store_never_saves() {
        let mut e = EnvStore::Empty;
        e.set("wayang_slot", "B");
        assert!(e.save().is_ok());
        assert!(e.get("wayang_slot").is_none());
    }
}
