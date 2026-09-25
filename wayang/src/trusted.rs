//! `/etc/wayang/trusted_keys`: lines `<keyid> <64-hex-ed25519-pub>`, `#` comments.

use std::collections::BTreeMap;

pub type TrustedKeys = BTreeMap<String, [u8; 32]>;

pub fn parse(text: &str) -> Result<TrustedKeys, String> {
    let mut out = TrustedKeys::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let keyid = it.next().ok_or_else(|| format!("line {}: missing keyid", n + 1))?;
        let hexs = it.next().ok_or_else(|| format!("line {}: missing key", n + 1))?;
        let bytes = hex::decode(hexs).map_err(|e| format!("line {}: bad hex: {e}", n + 1))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| format!("line {}: key must be 32 bytes", n + 1))?;
        out.insert(keyid.to_string(), arr);
    }
    Ok(out)
}

pub fn load(path: &std::path::Path) -> Result<TrustedKeys, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_ignores_comments() {
        let k = parse("# release keys\nrelease 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\n\n")
            .unwrap();
        assert_eq!(k.len(), 1);
        assert_eq!(k["release"][0], 0);
        assert_eq!(k["release"][31], 0x1f);
    }

    #[test]
    fn rejects_bad_hex_and_length() {
        assert!(parse("release zzzz").is_err());
        assert!(parse("release 00").is_err());
        assert!(parse("release").is_err());
    }
}
