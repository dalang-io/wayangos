//! Read-only disk benchmark (sequential + 4K random).
//!
//! Linux only. Never writes. Uses `O_DIRECT` when available (falls back to
//! buffered reads). Requires permission to open the raw device.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

/// Run a read-only benchmark and return a human summary, or `None` if it cannot
/// run (wrong OS, no permission, open failed).
#[cfg(target_os = "linux")]
pub fn run(path: &str, device_bytes: u64) -> Option<String> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{FileExt, OpenOptionsExt};

    const O_DIRECT: i32 = 0o40000;

    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_DIRECT)
        .open(path)
        .ok()?;

    let seq_len: u64 = device_bytes.min(256 << 20).max(8 << 20);
    let block: usize = 1 << 20;

    // Aligned buffer for O_DIRECT.
    let mut raw = vec![0u8; block + 4096];
    let addr = raw.as_ptr() as usize;
    let off = ((addr + 4095) & !4095) - addr;
    let buf = &mut raw[off..off + block];

    // Sequential read.
    let start = std::time::Instant::now();
    let mut pos: u64 = 0;
    let mut bytes: u64 = 0;
    while pos < seq_len {
        match file.read_at(buf, pos) {
            Ok(0) => break,
            Ok(n) => {
                bytes += n as u64;
                pos += n as u64;
            }
            Err(_) => break,
        }
    }
    let seq_elapsed = start.elapsed().as_secs_f64();
    let seq_mbps = if seq_elapsed > 0.0 {
        bytes as f64 / seq_elapsed / 1.0e6
    } else {
        0.0
    };

    // 4K QD1 random read.
    let mut r4 = vec![0u8; 4096 + 4096];
    let a4 = r4.as_ptr() as usize;
    let o4 = ((a4 + 4095) & !4095) - a4;
    let buf4 = &mut r4[o4..o4 + 4096];
    let span = seq_len.saturating_sub(4096).max(4096);
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let start = std::time::Instant::now();
    let iterations = 2000u32;
    for _ in 0..iterations {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let off = (seed % span) & !4095u64;
        let _ = file.read_at(buf4, off);
    }
    let rnd_elapsed = start.elapsed().as_secs_f64();
    let iops = if rnd_elapsed > 0.0 {
        iterations as f64 / rnd_elapsed
    } else {
        0.0
    };

    Some(format!(
        "sequential read: {:.0} MB/s over {} MB | 4K QD1 random: {:.0} IOPS ({:.1} MB/s)",
        seq_mbps,
        bytes >> 20,
        iops,
        iops * 4096.0 / 1.0e6
    ))
}

#[cfg(not(target_os = "linux"))]
pub fn run(_path: &str, _device_bytes: u64) -> Option<String> {
    None
}
