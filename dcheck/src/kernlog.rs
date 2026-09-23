//! Kernel log: SATA/ATA link failures that never become a block device.
//!
//! A drive whose controller is dead (or a counterfeit/broken SSD) can still
//! raise the SATA link but never answer IDENTIFY; libata then gives up and no
//! `/dev/sdX` appears, so `/sys/block` alone never shows it. The kernel log
//! records it, e.g.:
//!
//! ```text
//! ata1: link is slow to respond, please be patient (ready=0)
//! ata1: limiting SATA link speed to 3.0 Gbps
//! ata1: hardreset failed
//! ata1: reset failed, giving up
//! ```
//!
//! Read from `/dev/kmsg` (root; one record per read) or, for tests and
//! offline analysis, from a text file in `$DCHECK_KMSG`.

use std::collections::BTreeMap;

/// What the kernel log says about one ATA port.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PortState {
    /// Why the device on this port is considered failed (latest verdict).
    pub failed: Option<String>,
    /// "link is slow to respond" messages.
    pub slow: u32,
    /// Error-handling events on a working link (exceptions, resets).
    pub errors: u32,
    /// Link speed was reduced by error handling.
    pub downgraded: bool,
}

/// `"ata3.00: foo"` / `"ata3: foo"` -> `(3, "foo")`.
fn split_port(msg: &str) -> Option<(u32, &str)> {
    let rest = msg.trim_start().strip_prefix("ata")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let after = &rest[digits.len()..];
    // Optional ".00" device suffix, then ':'.
    let after = after.strip_prefix('.').map_or(after, |a| a.trim_start_matches(|c: char| c.is_ascii_digit()));
    let text = after.strip_prefix(':')?;
    Some((digits.parse().ok()?, text.trim()))
}

/// Fold kernel messages (in order) into per-port states.
pub fn ata_port_states<'a>(messages: impl IntoIterator<Item = &'a str>) -> BTreeMap<u32, PortState> {
    let mut ports: BTreeMap<u32, PortState> = BTreeMap::new();
    for msg in messages {
        let Some((port, text)) = split_port(msg) else { continue };
        let st = ports.entry(port).or_default();
        if text.contains("configured for") || text.contains("SATA link down") {
            // Working again (or nothing attached): clear any failure.
            st.failed = None;
        } else if text.contains("reset failed, giving up") {
            let mut why = "a device is attached but never became ready (link reset failed, the kernel gave up)".to_string();
            if st.slow > 0 {
                why.push_str(&format!("; {} × \"link is slow to respond\"", st.slow));
            }
            if st.downgraded {
                why.push_str("; link speed was reduced");
            }
            st.failed = Some(why);
        } else if text.contains("failed to IDENTIFY") || text.contains("IDENTIFY failed") {
            st.failed = Some("the device does not answer IDENTIFY".into());
        } else if text.contains("link is slow to respond") {
            st.slow += 1;
        } else if text.contains("limiting SATA link speed") {
            st.downgraded = true;
        } else if text.contains("exception Emask") || text.contains("hard resetting link") {
            st.errors += 1;
        }
    }
    ports
}

/// Kernel messages from `$DCHECK_KMSG` (plain text) or `/dev/kmsg`.
pub fn read_messages() -> Vec<String> {
    if let Some(path) = std::env::var_os("DCHECK_KMSG") {
        return std::fs::read_to_string(path)
            .map(|t| t.lines().map(str::to_string).collect())
            .unwrap_or_default();
    }
    read_kmsg()
}

#[cfg(target_os = "linux")]
fn read_kmsg() -> Vec<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    const O_NONBLOCK: i32 = 0o4000;
    let Ok(mut f) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open("/dev/kmsg")
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            // "prio,seq,usec,flags;message\n[ continuation lines ]"
            Ok(n) => {
                let rec = String::from_utf8_lossy(&buf[..n]);
                if let Some((_, msg)) = rec.split_once(';') {
                    out.push(msg.lines().next().unwrap_or("").to_string());
                }
            }
            // EPIPE: records were overwritten while reading; keep going.
            Err(e) if e.raw_os_error() == Some(32) => continue,
            Err(_) => break, // EAGAIN at the end, or no permission
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
fn read_kmsg() -> Vec<String> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAB_243: &str = "\
ata1: SATA max UDMA/133 abar m2048@0xfb616000 port 0xfb616100 irq 38 lpm-pol 4
ata3: SATA max UDMA/133 abar m2048@0xfb616000 port 0xfb616200 irq 38 lpm-pol 4
ata3: SATA link up 6.0 Gbps (SStatus 133 SControl 300)
ata3.00: ATA-10: SSD 1TB, VE0R6304, max UDMA/133
ata3.00: configured for UDMA/133
ata1: link is slow to respond, please be patient (ready=0)
ata1: link is slow to respond, please be patient (ready=0)
ata1: link is slow to respond, please be patient (ready=0)
ata1: limiting SATA link speed to 3.0 Gbps
ata1: hardreset failed
ata1: reset failed, giving up";

    #[test]
    fn detects_the_dead_ssd_on_lab_243() {
        let st = ata_port_states(LAB_243.lines());
        let a1 = &st[&1];
        assert!(a1.failed.as_deref().unwrap().contains("never became ready"));
        assert!(a1.failed.as_deref().unwrap().contains("3 × \"link is slow to respond\""));
        assert!(a1.failed.as_deref().unwrap().contains("speed was reduced"));
        assert_eq!(st[&3].failed, None);
    }

    #[test]
    fn recovery_or_unplug_clears_failure() {
        let msgs = ["ata2: reset failed, giving up", "ata2: SATA link down (SStatus 0 SControl 300)"];
        assert_eq!(ata_port_states(msgs).get(&2).unwrap().failed, None);
        let msgs = ["ata2: reset failed, giving up", "ata2.00: configured for UDMA/133"];
        assert_eq!(ata_port_states(msgs).get(&2).unwrap().failed, None);
    }

    #[test]
    fn counts_errors_on_working_links() {
        let msgs = [
            "ata4.00: configured for UDMA/133",
            "ata4.00: exception Emask 0x10 SAct 0x0 SErr 0x4050000 action 0xe frozen",
            "ata4: hard resetting link",
        ];
        let st = ata_port_states(msgs);
        assert_eq!(st[&4].errors, 2);
        assert_eq!(st[&4].failed, None);
    }

    #[test]
    fn ignores_unrelated_lines() {
        assert!(ata_port_states(["usb 1-1: new device", "sd 2:0:0:0: [sda] Attached"]).is_empty());
        assert_eq!(split_port("ata12.01: x"), Some((12, "x")));
        assert_eq!(split_port("data: nope"), None);
        assert_eq!(split_port("sd 2:0:0:0: metadata3: x"), None);
    }
}
