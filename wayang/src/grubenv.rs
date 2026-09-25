//! GRUB environment block (the fallback state).
//!
//! GRUB stores the block as exactly 1024 bytes:
//!
//! ```text
//! # GRUB Environment Block\n
//! key=value\n
//! ...
//! ############...   (padding to 1024 bytes)
//! ```
//!
//! Lines beginning with `#` are padding (`# GRUB Environment Block` itself is
//! the first such line and is not part of the map).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub const BLOCK_SIZE: usize = 1024;
const HEADER: &[u8] = b"# GRUB Environment Block\n";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrubEnv {
    pub map: BTreeMap<String, String>,
}

impl GrubEnv {
    pub fn from_bytes(buf: &[u8]) -> Result<Self, String> {
        if buf.len() < HEADER.len() || &buf[..HEADER.len()] != HEADER {
            return Err("not a GRUB environment block".into());
        }
        let mut map = BTreeMap::new();
        for line in buf[HEADER.len()..].split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            if line[0] == b'#' {
                break;
            }
            let s = std::str::from_utf8(line).map_err(|_| "non-UTF-8 entry in grubenv")?;
            match s.split_once('=') {
                Some((k, v)) => {
                    map.insert(k.to_string(), v.to_string());
                }
                None => return Err(format!("malformed grubenv entry '{s}'")),
            }
        }
        Ok(GrubEnv { map })
    }

    pub fn to_bytes(&self) -> Result<[u8; BLOCK_SIZE], String> {
        let mut buf = Vec::with_capacity(BLOCK_SIZE);
        buf.extend_from_slice(HEADER);
        for (k, v) in &self.map {
            if k.is_empty() || k.contains(['=', '\n', '\r']) || v.contains(['\n', '\r']) {
                return Err(format!("invalid grubenv key/value '{k}'"));
            }
            buf.extend_from_slice(k.as_bytes());
            buf.push(b'=');
            buf.extend_from_slice(v.as_bytes());
            buf.push(b'\n');
        }
        if buf.len() > BLOCK_SIZE {
            return Err("grubenv too large".into());
        }
        buf.resize(BLOCK_SIZE, b'#');
        let mut out = [0u8; BLOCK_SIZE];
        out.copy_from_slice(&buf);
        Ok(out)
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_bytes(&data)
    }

    /// Write atomically enough for a FAT ESP: full block, then fsync.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let bytes = self.to_bytes()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut f = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.write_all(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "B");
        env.set("wayang_good", "A");
        env.set("wayang_attempts", "0");
        let bytes = env.to_bytes().unwrap();
        assert_eq!(bytes.len(), BLOCK_SIZE);
        assert!(bytes.starts_with(HEADER));
        let back = GrubEnv::from_bytes(&bytes).unwrap();
        assert_eq!(back, env);
        assert_eq!(back.get("wayang_slot"), Some("B"));
    }

    #[test]
    fn parses_padded_block() {
        let mut raw = Vec::new();
        raw.extend_from_slice(HEADER);
        raw.extend_from_slice(b"wayang_slot=A\nwayang_attempts=2\n");
        raw.resize(BLOCK_SIZE, b'#');
        let env = GrubEnv::from_bytes(&raw).unwrap();
        assert_eq!(env.get("wayang_slot"), Some("A"));
        assert_eq!(env.get("wayang_attempts"), Some("2"));
        assert_eq!(env.map.len(), 2);
    }

    #[test]
    fn empty_block_ok() {
        let mut raw = HEADER.to_vec();
        raw.resize(BLOCK_SIZE, b'#');
        let env = GrubEnv::from_bytes(&raw).unwrap();
        assert!(env.map.is_empty());
    }

    #[test]
    fn rejects_garbage() {
        assert!(GrubEnv::from_bytes(b"not grub").is_err());
        let mut raw = HEADER.to_vec();
        raw.extend_from_slice(b"oops\n");
        raw.resize(BLOCK_SIZE, b'#');
        assert!(GrubEnv::from_bytes(&raw).is_err());
    }

    #[test]
    fn rejects_oversized() {
        let mut env = GrubEnv::default();
        env.set("big", &"x".repeat(BLOCK_SIZE));
        assert!(env.to_bytes().is_err());
    }
}
