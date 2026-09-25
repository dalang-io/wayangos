//! `*.wup` bundle reader: a gzip tar with `manifest.json`, `vmlinuz`,
//! `initramfs.img` and `manifest.json.sig` at the archive root.

use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use tar::Archive;

use crate::error::{AppError, Result};
use crate::hash;
use crate::manifest::Manifest;
use crate::sign;
use crate::trusted::TrustedKeys;

#[derive(Debug)]
pub struct Bundle {
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub kernel: Vec<u8>,
    pub initramfs: Vec<u8>,
    pub signature: Vec<u8>,
}

impl Bundle {
    /// Verify `manifest.json.sig` against the trusted public key for the
    /// manifest's `keyid`.
    pub fn verify_signature(&self, keys: &TrustedKeys) -> Result<()> {
        let key = keys.get(&self.manifest.keyid).ok_or_else(|| {
            AppError::verify(format!("no trusted key for keyid '{}'", self.manifest.keyid))
        })?;
        sign::verify_pub(key, &self.manifest_bytes, &self.signature)
            .map_err(|e| AppError::verify(format!("manifest signature: {e}")))
    }
}

fn take_member<R: std::io::Read>(
    entry: &mut tar::Entry<'_, R>,
    slot: &mut Option<Vec<u8>>,
) -> Result<()> {
    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|e| AppError::err(format!("reading bundle member: {e}")))?;
    *slot = Some(buf);
    Ok(())
}

/// Read and integrity-check a bundle (sha256 of kernel/initramfs). Signature
/// verification is separate so callers can report the exact failure.
pub fn read(path: &Path) -> Result<Bundle> {
    let file = std::fs::File::open(path).map_err(|e| AppError::err(format!("{}: {e}", path.display())))?;
    let mut archive = Archive::new(GzDecoder::new(file));

    let mut manifest_bytes = None;
    let mut kernel = None;
    let mut initramfs = None;
    let mut signature = None;
    let entries = archive
        .entries()
        .map_err(|e| AppError::err(format!("reading bundle: {e}")))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| AppError::err(format!("reading bundle: {e}")))?;
        let name = entry
            .path()
            .map_err(|e| AppError::err(format!("bad bundle entry: {e}")))?
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        match name.as_str() {
            "manifest.json" => take_member(&mut entry, &mut manifest_bytes)?,
            "vmlinuz" => take_member(&mut entry, &mut kernel)?,
            "initramfs.img" => take_member(&mut entry, &mut initramfs)?,
            "manifest.json.sig" => take_member(&mut entry, &mut signature)?,
            _ => {}
        }
    }

    let manifest_bytes = manifest_bytes.ok_or_else(|| AppError::err("bundle is missing manifest.json"))?;
    let manifest = Manifest::from_slice(&manifest_bytes)?;
    let kernel = kernel.ok_or_else(|| AppError::err("bundle is missing vmlinuz"))?;
    let initramfs = initramfs.ok_or_else(|| AppError::err("bundle is missing initramfs.img"))?;
    let signature = signature.ok_or_else(|| AppError::verify("bundle is missing manifest.json.sig"))?;

    let kh = hash::hex32(&hash::sha256_bytes(&kernel));
    if !kh.eq_ignore_ascii_case(&manifest.kernel_sha256) {
        return Err(AppError::verify(format!(
            "vmlinuz sha256 mismatch (got {kh}, want {})",
            manifest.kernel_sha256
        )));
    }
    let ih = hash::hex32(&hash::sha256_bytes(&initramfs));
    if !ih.eq_ignore_ascii_case(&manifest.initramfs_sha256) {
        return Err(AppError::verify(format!(
            "initramfs.img sha256 mismatch (got {ih}, want {})",
            manifest.initramfs_sha256
        )));
    }

    Ok(Bundle { manifest, manifest_bytes, kernel, initramfs, signature })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bundle(dir: &Path, keyid: &str) -> (std::path::PathBuf, [u8; 32], Vec<u8>) {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let sk = sign::generate();
        let vk = sk.verifying_key().to_bytes();
        let kernel = b"kernel-bytes".to_vec();
        let initramfs = b"initramfs-bytes".to_vec();
        let manifest = format!(
            r#"{{"product":"wayangos","channel":"stable","version":"1.4.1","major":1,
                 "arch":"x86_64","kernel_sha256":"{}","initramfs_sha256":"{}","keyid":"{keyid}"}}"#,
            hash::hex32(&hash::sha256_bytes(&kernel)),
            hash::hex32(&hash::sha256_bytes(&initramfs)),
        );
        let sig = sign::sign_bytes(&sk, manifest.as_bytes());

        let path = dir.join("t.wup");
        let f = std::fs::File::create(&path).unwrap();
        let mut enc = GzEncoder::new(f, Compression::fast());
        {
            let mut b = tar::Builder::new(&mut enc);
            let mut add = |name: &str, data: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, data).unwrap();
            };
            add("manifest.json", manifest.as_bytes());
            add("vmlinuz", &kernel);
            add("initramfs.img", &initramfs);
            add("manifest.json.sig", &sig);
        }
        enc.finish().unwrap();
        (path, vk, manifest.into_bytes())
    }

    #[test]
    fn reads_and_verifies() {
        let dir = std::env::temp_dir().join(format!("wayang-bundle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, vk, _) = write_bundle(&dir, "release");
        let b = read(&path).unwrap();
        assert_eq!(b.kernel, b"kernel-bytes");
        let mut keys = TrustedKeys::new();
        keys.insert("release".into(), vk);
        assert!(b.verify_signature(&keys).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_wrong_key() {
        let dir = std::env::temp_dir().join(format!("wayang-bundle2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, _vk, _) = write_bundle(&dir, "release");
        let b = read(&path).unwrap();
        let mut keys = TrustedKeys::new();
        keys.insert("release".into(), sign::generate().verifying_key().to_bytes());
        assert_eq!(b.verify_signature(&keys).unwrap_err().code, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
