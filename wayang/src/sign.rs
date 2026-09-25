//! ed25519 key generation, signing and verification.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::OsRng;

pub fn generate() -> SigningKey {
    SigningKey::generate(&mut OsRng)
}

pub fn seed_hex(sk: &SigningKey) -> String {
    hex::encode(sk.to_bytes())
}

pub fn public_hex(vk: &VerifyingKey) -> String {
    hex::encode(vk.to_bytes())
}

pub fn signing_key_from_hex(hexstr: &str) -> Result<SigningKey, String> {
    let bytes = hex::decode(hexstr.trim()).map_err(|e| format!("bad hex key: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "signing key must be 32 bytes (64 hex chars)".to_string())?;
    Ok(SigningKey::from_bytes(&arr))
}

pub fn verifying_key_from_bytes(bytes: &[u8; 32]) -> Result<VerifyingKey, String> {
    VerifyingKey::from_bytes(bytes).map_err(|e| format!("bad public key: {e}"))
}

/// Raw 64-byte ed25519 signature over `msg`.
pub fn sign_bytes(sk: &SigningKey, msg: &[u8]) -> [u8; 64] {
    sk.sign(msg).to_bytes()
}

pub fn verify_bytes(vk: &VerifyingKey, msg: &[u8], sig: &[u8]) -> Result<(), String> {
    let arr: [u8; 64] = sig
        .try_into()
        .map_err(|_| "signature must be 64 bytes".to_string())?;
    let sig = Signature::from_bytes(&arr);
    vk.verify_strict(msg, &sig)
        .map_err(|_| "signature verification failed".to_string())
}

/// Verify against a raw 32-byte public key (as stored in `trusted_keys`).
pub fn verify_pub(pub_bytes: &[u8; 32], msg: &[u8], sig: &[u8]) -> Result<(), String> {
    let vk = verifying_key_from_bytes(pub_bytes)?;
    verify_bytes(&vk, msg, sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let sk = generate();
        let vk = sk.verifying_key();
        let msg = b"manifest bytes";
        let sig = sign_bytes(&sk, msg);
        assert_eq!(sig.len(), 64);
        assert!(verify_bytes(&vk, msg, &sig).is_ok());
        assert!(verify_bytes(&vk, b"other", &sig).is_err());
    }

    #[test]
    fn seed_roundtrip_and_pub_matches() {
        let sk = generate();
        let seed = seed_hex(&sk);
        assert_eq!(seed.len(), 64);
        let sk2 = signing_key_from_hex(&seed).unwrap();
        assert_eq!(public_hex(&sk.verifying_key()), public_hex(&sk2.verifying_key()));
        let msg = b"x";
        let sig = sign_bytes(&sk2, msg);
        assert!(verify_pub(&sk.verifying_key().to_bytes(), msg, &sig).is_ok());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(signing_key_from_hex("00").is_err());
        assert!(signing_key_from_hex("zz").is_err());
        let sk = generate();
        let vk = sk.verifying_key();
        assert!(verify_bytes(&vk, b"m", &[0u8; 63]).is_err());
    }
}
