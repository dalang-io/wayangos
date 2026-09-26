//! Root's SSH `authorized_keys`: parsing/validation, GitHub/GitLab fetch and
//! persistence to `/data/etc/ssh/authorized_keys` (survives updates) plus the
//! live `/root/.ssh/authorized_keys`.
//!
//! This is the single implementation of key validation/fetch/format: the
//! console SSH screen (`sshkeysui.rs`) and the `wayang addkey` CLI both use it,
//! and `wayang-addkey` (scripts/build-rootfs.sh) is a thin wrapper over
//! `wayang addkey`. (The installer is a separate pre-boot binary and keeps its
//! own copy.)

use std::fs;
use std::path::Path;

use crate::hash;
use crate::paths;
use crate::sys;

/// Public-key algorithms accepted in an `authorized_keys` line.
const ALGOS: [&str; 8] = [
    "ssh-ed25519",
    "ssh-rsa",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ssh-ed25519@openssh.com",
    "sk-ecdsa-sha2-nistp256@openssh.com",
    "ssh-dss",
];

/// CA bundle shipped in the rootfs; the static curl has none built in.
const CA_BUNDLE: &str = "/etc/ssl/certs/ca-certificates.crt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshKey {
    pub algo: String,
    pub blob: String,
    pub comment: String,
    /// Where it came from, e.g. `github:alice`, `data`, `typed`.
    pub source: String,
}

impl SshKey {
    /// Parse one `authorized_keys` / `.pub` line. Leading options
    /// (`no-pty,from=...`) are skipped; the blob must decode and name the same
    /// algorithm. `None` means "not a valid public key".
    pub fn parse(line: &str, source: &str) -> Option<SshKey> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|w| ALGOS.contains(w))?;
        let algo = words[at];
        let blob = *words.get(at + 1)?;
        let raw = base64_decode(blob)?;
        // the blob starts with the algorithm name as an SSH string
        let n = u32::from_be_bytes(raw.get(..4)?.try_into().ok()?) as usize;
        if raw.get(4..4 + n)? != algo.as_bytes() {
            return None;
        }
        Some(SshKey {
            algo: algo.to_string(),
            blob: blob.to_string(),
            comment: words[at + 2..].join(" "),
            source: source.to_string(),
        })
    }

    pub fn line(&self) -> String {
        if self.comment.is_empty() {
            format!("{} {}", self.algo, self.blob)
        } else {
            format!("{} {} {}", self.algo, self.blob, self.comment)
        }
    }

    pub fn fingerprint(&self) -> String {
        let raw = base64_decode(&self.blob).unwrap_or_default();
        format!("SHA256:{}", base64_nopad(&hash::sha256_bytes(&raw)))
    }

    pub fn kind(&self) -> &'static str {
        match self.algo.as_str() {
            "ssh-ed25519" => "ED25519",
            "ssh-rsa" => "RSA",
            "ssh-dss" => "DSA",
            "sk-ssh-ed25519@openssh.com" => "ED25519-SK",
            "sk-ecdsa-sha2-nistp256@openssh.com" => "ECDSA-SK",
            _ => "ECDSA",
        }
    }
}

pub fn parse_all(text: &str, source: &str) -> Vec<SshKey> {
    text.lines().filter_map(|l| SshKey::parse(l, source)).collect()
}

/// Add keys not already present (same blob). Returns how many were new.
pub fn merge(into: &mut Vec<SshKey>, new: Vec<SshKey>) -> usize {
    let mut added = 0;
    for k in new {
        if !into.iter().any(|e| e.blob == k.blob) {
            into.push(k);
            added += 1;
        }
    }
    added
}

/// `alice`, `github:alice` or `gitlab:alice` -> (host, user).
pub fn remote_spec(spec: &str) -> Result<(&'static str, String), String> {
    let spec = spec.trim();
    let (host, user) = match spec.split_once(':') {
        Some(("github", u)) => ("github.com", u),
        Some(("gitlab", u)) => ("gitlab.com", u),
        Some((h, _)) => return Err(format!("unknown site '{h}' - use github:USER or gitlab:USER")),
        None => ("github.com", spec),
    };
    let ok = !user.is_empty()
        && user.len() <= 64
        && user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        return Err(format!("'{user}' is not a valid user name"));
    }
    Ok((host, user.to_string()))
}

/// Keys published at https://github.com/USER.keys (or gitlab.com).
pub fn fetch_remote(spec: &str, demo: bool) -> Result<Vec<SshKey>, String> {
    let (host, user) = remote_spec(spec)?;
    let source = format!("{}:{user}", host.trim_end_matches(".com"));
    if demo {
        std::thread::sleep(std::time::Duration::from_millis(900));
        return Ok(parse_all(&demo_keys(&user), &source));
    }
    let url = format!("https://{host}/{user}.keys");
    let mut args = vec!["-fsSL", "--max-time", "20"];
    if Path::new(CA_BUNDLE).exists() {
        args.extend(["--cacert", CA_BUNDLE]);
    }
    args.push(&url);
    let body = sys::run("curl", &args).map_err(|e| {
        if e.contains("(22)") {
            format!("{host} has no user '{user}'")
        } else if e.contains("(6)") || e.contains("(7)") || e.contains("(28)") {
            format!("cannot reach {host} - is the network up?")
        } else {
            e
        }
    })?;
    let keys = parse_all(&body, &source);
    if keys.is_empty() {
        return Err(format!("{user} has no SSH keys on {host}"));
    }
    Ok(keys)
}

/// Every authorized key, from `/data/etc/ssh/authorized_keys` and the live
/// `/root/.ssh/authorized_keys`, deduplicated by blob.
pub fn load() -> Vec<SshKey> {
    let mut keys = Vec::new();
    for (path, label) in [
        (paths::ssh_authorized_keys(), "data"),
        (paths::root_authorized_keys(), "live"),
    ] {
        if let Ok(text) = fs::read_to_string(&path) {
            merge(&mut keys, parse_all(&text, label));
        }
    }
    keys
}

/// Append `keys` to both files (deduplicated by blob); returns how many were
/// new in the persistent `/data` copy.
pub fn save_add(keys: &[SshKey]) -> Result<usize, String> {
    let added = append_file(&paths::ssh_authorized_keys(), keys)?;
    append_file(&paths::root_authorized_keys(), keys)?;
    Ok(added)
}

const ADDKEY_HELP: &str = "\
wayang-addkey — let an SSH public key log in as root.
  wayang-addkey github:USER | gitlab:USER    keys published on GitHub/GitLab
  wayang-addkey FILE                          a .pub or authorized_keys file
  wayang-addkey 'ssh-ed25519 AAAA... you@laptop'";

/// Resolve a `wayang addkey` argument to validated keys without writing:
/// `github:USER` / `gitlab:USER`, an existing FILE, or a literal key line.
pub fn resolve_addkey(spec: &str) -> Result<Vec<SshKey>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("no SSH public key found".into());
    }
    if spec.starts_with("github:") || spec.starts_with("gitlab:") {
        fetch_remote(spec, false)
    } else if Path::new(spec).is_file() {
        let text = fs::read_to_string(spec).map_err(|e| format!("{spec}: {e}"))?;
        Ok(parse_all(&text, "file"))
    } else {
        Ok(parse_all(spec, "typed"))
    }
}

/// `wayang addkey`: the one implementation behind `wayang-addkey` and the SSH
/// screen. Returns a process exit code.
pub fn addkey_cmd(spec: &str) -> Result<i32, String> {
    let spec = spec.trim();
    if spec.is_empty() || matches!(spec, "-h" | "--help") {
        println!("{ADDKEY_HELP}");
        return Ok(if spec.is_empty() { 1 } else { 0 });
    }
    let keys = resolve_addkey(spec)?;
    if keys.is_empty() {
        return Err("no SSH public key found".into());
    }
    let added = save_add(&keys)?;
    if added == 0 {
        crate::outln!("already authorized");
    } else if data_persistent() {
        crate::outln!("authorized {added} new key(s), saved in /data");
    } else {
        crate::outln!("authorized {added} new key(s) until reboot (no /data)");
    }
    Ok(0)
}

/// Whether `/data` is a separate persistent mount (not the tmpfs root, where a
/// key would only last until reboot).
fn data_persistent() -> bool {
    if paths::root().is_some() {
        return true;
    }
    fs::read_to_string("/proc/mounts")
        .map(|m| m.lines().any(|l| l.split_whitespace().nth(1) == Some("/data")))
        .unwrap_or(false)
}

/// Remove every line whose key blob matches `blob` from both files.
pub fn remove(blob: &str) -> Result<bool, String> {
    let a = remove_file(&paths::ssh_authorized_keys(), blob)?;
    let b = remove_file(&paths::root_authorized_keys(), blob)?;
    Ok(a || b)
}

fn append_file(path: &Path, keys: &[SshKey]) -> Result<usize, String> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    let present: Vec<String> = parse_all(&existing, "x").into_iter().map(|k| k.blob).collect();
    let mut text = existing.clone();
    let mut added = 0;
    for k in keys {
        if present.contains(&k.blob) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&k.line());
        text.push('\n');
        added += 1;
    }
    if text != existing {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            sys::chmod(dir, 0o700);
        }
        fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        sys::chmod(path, 0o600);
    }
    Ok(added)
}

fn remove_file(path: &Path, blob: &str) -> Result<bool, String> {
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(false);
    };
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| SshKey::parse(line, "x").map(|k| k.blob != blob).unwrap_or(true))
        .collect();
    let mut new = kept.join("\n");
    if !new.is_empty() {
        new.push('\n');
    }
    if new == text {
        return Ok(false);
    }
    fs::write(path, new).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

/// Two throwaway public keys for demo/snapshot mode.
pub fn demo() -> Vec<SshKey> {
    parse_all(&demo_keys("you"), "github:you")
}

fn demo_keys(user: &str) -> String {
    format!(
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKDgLaeocpxjyYcVfzkVhNoYwyzzw0TqveMdqGKqXB6 {user}@laptop\n\
         ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBP0K5Ces4KfwCkozfoY+XsKAQdYc0VC/sBUHfHSgtLPLaui97KYpMA0qjCu+H8SlbBE0xWgTnxpF6qwYFMVZoGM= {user}@desktop\n"
    )
}

// ---- base64 (no dependency; matches installer/src/sha256.rs) --------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 without `=` padding (the OpenSSH fingerprint form).
fn base64_nopad(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..chunk.len() + 1 {
            out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// Decode standard base64 (padding optional). `None` on any invalid byte.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = B64.iter().position(|&b| b == c)? as u32;
        acc = (acc << 6 | v) & 0xff_ffff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // throwaway key; fingerprint from `ssh-keygen -lf`
    const ED: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPX2zdvjV01sS6kSNEuFCraBHx+RtIL5q/nnODxc+Dys alice@box";

    fn tmpdir() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "wayang-sshkeys-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_and_fingerprints() {
        let k = SshKey::parse(ED, "t").unwrap();
        assert_eq!(k.kind(), "ED25519");
        assert_eq!(k.comment, "alice@box");
        assert_eq!(k.fingerprint(), "SHA256:J7zZ3uqDctuY6b2lgeWAVv9TlUivub7Ri/4LLDr3TJo");
        assert_eq!(k.line(), ED);
    }

    #[test]
    fn skips_options_and_rejects_junk() {
        let with_opts = format!("no-pty,from=\"10.0.0.0/8\" {ED}");
        assert_eq!(SshKey::parse(&with_opts, "t").unwrap().comment, "alice@box");
        assert!(SshKey::parse("ssh-ed25519 notbase64!!", "t").is_none());
        assert!(SshKey::parse(&ED.replace("ssh-ed25519", "ssh-rsa"), "t").is_none());
        assert!(SshKey::parse("# comment", "t").is_none());
        assert!(SshKey::parse("not a key at all", "t").is_none());
    }

    #[test]
    fn remote_specs() {
        assert_eq!(remote_spec("alice").unwrap(), ("github.com", "alice".into()));
        assert_eq!(remote_spec("gitlab:bob").unwrap(), ("gitlab.com", "bob".into()));
        assert!(remote_spec("github:a b").is_err());
        assert!(remote_spec("bitbucket:x").is_err());
    }

    #[test]
    fn merge_dedupes() {
        let mut v = parse_all(ED, "a");
        assert_eq!(merge(&mut v, parse_all(ED, "b")), 0);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn add_and_remove_roundtrip() {
        let dir = tmpdir();
        let file = dir.join("authorized_keys");
        let k = SshKey::parse(ED, "typed").unwrap();
        assert_eq!(append_file(&file, std::slice::from_ref(&k)).unwrap(), 1);
        assert_eq!(append_file(&file, std::slice::from_ref(&k)).unwrap(), 0, "already present");
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains(&k.blob));
        assert!(remove_file(&file, &k.blob).unwrap());
        assert_eq!(fs::read_to_string(&file).unwrap(), "");
        assert!(!remove_file(&file, &k.blob).unwrap(), "already gone");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_keeps_other_lines() {
        let dir = tmpdir();
        let file = dir.join("authorized_keys");
        fs::write(&file, format!("# keep me\n{ED}\n")).unwrap();
        assert!(remove_file(&file, SshKey::parse(ED, "t").unwrap().blob.as_str()).unwrap());
        assert_eq!(fs::read_to_string(&file).unwrap(), "# keep me\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn demo_keys_parse() {
        let keys = demo();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| !k.fingerprint().is_empty()));
    }

    #[test]
    fn resolve_addkey_accepts_literal_and_file() {
        assert_eq!(resolve_addkey(ED).unwrap().len(), 1);
        let dir = tmpdir();
        let f = dir.join("id_ed25519.pub");
        fs::write(&f, format!("{ED}\n")).unwrap();
        assert_eq!(resolve_addkey(&f.to_string_lossy()).unwrap().len(), 1);
        assert!(resolve_addkey("not a key").unwrap().is_empty());
        assert!(resolve_addkey("").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn addkey_cmd_rejects_junk_before_writing() {
        assert_eq!(addkey_cmd("").unwrap(), 1, "no argument prints usage");
        assert!(addkey_cmd("not a key").is_err());
    }
}
