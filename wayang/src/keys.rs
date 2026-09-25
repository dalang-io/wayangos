//! `wayang keygen` and `wayang sign`.

use std::path::Path;

use crate::error::{AppError, Result};
use crate::sign;

/// Write `<keyid>.key` (32-byte seed as 64 hex) and `<keyid>.pub` (64 hex).
pub fn keygen(out: &Path, keyid: &str) -> Result<i32> {
    std::fs::create_dir_all(out).map_err(|e| AppError::err(format!("{}: {e}", out.display())))?;
    let sk = sign::generate();
    let public = sign::public_hex(&sk.verifying_key());
    let seed = sign::seed_hex(&sk);

    let key_path = out.join(format!("{keyid}.key"));
    let pub_path = out.join(format!("{keyid}.pub"));
    std::fs::write(&key_path, format!("{seed}\n"))
        .map_err(|e| AppError::err(format!("{}: {e}", key_path.display())))?;
    std::fs::write(&pub_path, format!("{public}\n"))
        .map_err(|e| AppError::err(format!("{}: {e}", pub_path.display())))?;

    println!("wrote {}", key_path.display());
    println!("wrote {}", pub_path.display());
    println!("add to trusted_keys: {keyid} {public}");
    Ok(0)
}

/// Write `<MANIFEST>.sig`: raw 64-byte ed25519 signature over the file bytes.
pub fn sign_file(key: &Path, keyid: Option<&str>, manifest: &Path) -> Result<i32> {
    let hexs = std::fs::read_to_string(key)
        .map_err(|e| AppError::err(format!("{}: {e}", key.display())))?;
    let sk = sign::signing_key_from_hex(&hexs).map_err(AppError::err)?;
    let data = std::fs::read(manifest)
        .map_err(|e| AppError::err(format!("{}: {e}", manifest.display())))?;
    let sig = sign::sign_bytes(&sk, &data);

    let dest = format!("{}.sig", manifest.display());
    std::fs::write(&dest, sig).map_err(|e| AppError::err(format!("{dest}: {e}")))?;
    let who = keyid.unwrap_or("(unnamed)");
    println!("wrote {dest} ({}-byte signature for keyid {who})", sig.len());
    Ok(0)
}
