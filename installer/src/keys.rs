//! SSH public keys for root: parsing, fingerprints, and the places the
//! installer can get them from (GitHub/GitLab, a USB drive, typed in).

use std::fs;
use std::path::Path;

use crate::sha256::{base64_decode, base64_nopad, sha256};
use crate::sys;

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

#[derive(Debug, Clone, PartialEq)]
pub struct SshKey {
    pub algo: String,
    pub blob: String,
    pub comment: String,
    /// Where it came from, e.g. `github:alice`, `usb:sdb1/id_ed25519.pub`.
    pub source: String,
}

impl SshKey {
    /// Parse one `authorized_keys` / `.pub` line. Leading options
    /// (`no-pty,from=...`) are skipped; the blob must decode and name the
    /// same algorithm.
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
        format!("SHA256:{}", base64_nopad(&sha256(&raw)))
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
    text.lines()
        .filter_map(|l| SshKey::parse(l, source))
        .collect()
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
        Some((h, _)) => {
            return Err(format!(
                "unknown site '{h}' - use github:USER or gitlab:USER"
            ))
        }
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
            format!("cannot reach {host} - is the network cable plugged in?")
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

/// Look for `*.pub` / `authorized_keys` files (top two directory levels) on
/// every USB or removable partition except the installer's own stick.
pub fn scan_usb(disks: &[crate::disks::Disk], demo: bool) -> Result<Vec<SshKey>, String> {
    if demo {
        std::thread::sleep(std::time::Duration::from_millis(900));
        return Ok(parse_all(&demo_keys("usb-user"), "usb:sdc1/id_ed25519.pub"));
    }
    let mut keys = Vec::new();
    let mut looked = 0;
    for d in disks
        .iter()
        .filter(|d| (d.removable || d.bus == crate::disks::Bus::Usb) && !d.is_installer())
    {
        let parts: Vec<String> = if d.parts.is_empty() {
            vec![d.name.clone()]
        } else {
            d.parts.iter().map(|p| p.name.clone()).collect()
        };
        for part in parts {
            let mnt = format!("/tmp/wayang-keys-{part}");
            let _ = fs::create_dir_all(&mnt);
            let dev = format!("/dev/{part}");
            if sys::run("mount", &["-o", "ro", &dev, &mnt]).is_err() {
                continue;
            }
            looked += 1;
            collect_keys(Path::new(&mnt), &mnt, &part, 0, &mut keys);
            let _ = sys::run("umount", &[&mnt]);
            let _ = fs::remove_dir(&mnt);
        }
    }
    if looked == 0 {
        return Err("no USB drive found - plug in a stick with your .pub file".into());
    }
    if keys.is_empty() {
        return Err("no .pub or authorized_keys file on the USB drive".into());
    }
    Ok(keys)
}

fn collect_keys(dir: &Path, root: &str, part: &str, depth: u8, out: &mut Vec<SshKey>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if depth < 2 && (!name.starts_with('.') || name == ".ssh") {
                collect_keys(&path, root, part, depth + 1, out);
            }
        } else if (name.ends_with(".pub") || name == "authorized_keys")
            && e.metadata().map(|m| m.len() < 256 * 1024).unwrap_or(false)
        {
            if let Ok(text) = fs::read_to_string(&path) {
                let rel = path
                    .to_string_lossy()
                    .trim_start_matches(root)
                    .trim_start_matches('/')
                    .to_string();
                merge(out, parse_all(&text, &format!("usb:{part}/{rel}")));
            }
        }
    }
}

/// Authorize the keys for the running (live) system too, so the rest of the
/// install can be driven over SSH.
pub fn authorize_live(keys: &[SshKey]) {
    let path = "/root/.ssh/authorized_keys";
    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut text = existing.clone();
    for k in keys {
        if !existing.contains(&k.blob) {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&k.line());
            text.push('\n');
        }
    }
    if text != existing {
        let _ = fs::create_dir_all("/root/.ssh");
        let _ = fs::write(path, text);
        sys::chmod(path, 0o600);
    }
}

fn demo_keys(user: &str) -> String {
    format!(
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKDgLaeocpxjyYcVfzkVhNoYwyzzw0TqveMdqGKqXB6 {user}@laptop\n\
         ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBP0K5Ces4KfwCkozfoY+XsKAQdYc0VC/sBUHfHSgtLPLaui97KYpMA0qjCu+H8SlbBE0xWgTnxpF6qwYFMVZoGM= {user}@desktop\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // throwaway key; fingerprint from `ssh-keygen -lf`
    const ED: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPX2zdvjV01sS6kSNEuFCraBHx+RtIL5q/nnODxc+Dys alice@box";

    #[test]
    fn parses_and_fingerprints() {
        let k = SshKey::parse(ED, "t").unwrap();
        assert_eq!(k.kind(), "ED25519");
        assert_eq!(k.comment, "alice@box");
        assert_eq!(
            k.fingerprint(),
            "SHA256:J7zZ3uqDctuY6b2lgeWAVv9TlUivub7Ri/4LLDr3TJo"
        );
        assert_eq!(k.line(), ED);
    }

    #[test]
    fn skips_options_and_rejects_junk() {
        let with_opts = format!("no-pty,from=\"10.0.0.0/8\" {ED}");
        assert_eq!(SshKey::parse(&with_opts, "t").unwrap().comment, "alice@box");
        assert!(SshKey::parse("ssh-ed25519 notbase64!!", "t").is_none());
        // blob of a different algorithm than the prefix claims
        assert!(SshKey::parse(&ED.replace("ssh-ed25519", "ssh-rsa"), "t").is_none());
        assert!(SshKey::parse("# comment", "t").is_none());
    }

    #[test]
    fn remote_specs() {
        assert_eq!(
            remote_spec("alice").unwrap(),
            ("github.com", "alice".into())
        );
        assert_eq!(
            remote_spec("gitlab:bob").unwrap(),
            ("gitlab.com", "bob".into())
        );
        assert!(remote_spec("github:a b").is_err());
        assert!(remote_spec("bitbucket:x").is_err());
    }

    #[test]
    fn merge_dedupes() {
        let mut v = parse_all(ED, "a");
        assert_eq!(merge(&mut v, parse_all(ED, "b")), 0);
        assert_eq!(v.len(), 1);
    }
}
