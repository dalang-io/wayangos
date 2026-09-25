//! `wayang verify FILE.wup` — sha256 + signature against `/etc/wayang/trusted_keys`.

use std::path::Path;

use crate::bundle;
use crate::error::{AppError, Result};
use crate::paths;
use crate::trusted;

pub fn run(file: &Path, _esp: Option<&str>) -> Result<i32> {
    let keys = trusted::load(&paths::trusted_keys_file())
        .map_err(|e| AppError::verify(format!("trusted keys: {e}")))?;
    let b = bundle::read(file)?;
    b.verify_signature(&keys)?;
    let file_hash = crate::hash::sha256_file(file)
        .map(|h| crate::hash::hex32(&h))
        .unwrap_or_else(|_| "?".into());
    println!(
        "OK: {} {} ({}), signed by '{}', sha256 match",
        b.manifest.product, b.manifest.version, b.manifest.arch, b.manifest.keyid
    );
    println!("bundle sha256: {file_hash}");
    Ok(0)
}
