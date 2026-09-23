//! Filesystem usage for mounted partitions (`statvfs` on Linux).

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

#[derive(Debug, Clone, Copy)]
pub struct Usage {
    pub total: u64,
    pub used: u64,
    pub avail: u64,
    pub percent: f64,
}

/// Compute usage from raw `statvfs` counters (used% as `df` does).
pub fn from_blocks(frsize: u64, blocks: u64, bfree: u64, bavail: u64) -> Usage {
    let frsize = frsize.max(1);
    let total = blocks.saturating_mul(frsize);
    let free = bfree.saturating_mul(frsize);
    let avail = bavail.saturating_mul(frsize);
    let used = total.saturating_sub(free);
    let denom = used.saturating_add(avail);
    let percent = if denom > 0 {
        used as f64 * 100.0 / denom as f64
    } else {
        0.0
    };
    Usage {
        total,
        used,
        avail,
        percent,
    }
}

#[cfg(target_os = "linux")]
pub fn usage(mount: &str) -> Option<Usage> {
    use std::ffi::CString;

    #[repr(C)]
    struct Statvfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
        __f_spare: [u32; 6],
    }

    extern "C" {
        fn statvfs(path: *const i8, buf: *mut Statvfs) -> i32;
    }

    let c = CString::new(mount).ok()?;
    let mut st = Statvfs {
        f_bsize: 0,
        f_frsize: 0,
        f_blocks: 0,
        f_bfree: 0,
        f_bavail: 0,
        f_files: 0,
        f_ffree: 0,
        f_favail: 0,
        f_fsid: 0,
        f_flag: 0,
        f_namemax: 0,
        __f_spare: [0; 6],
    };
    let rc = unsafe { statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    Some(from_blocks(st.f_frsize, st.f_blocks, st.f_bfree, st.f_bavail))
}

#[cfg(not(target_os = "linux"))]
pub fn usage(_mount: &str) -> Option<Usage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_percent_like_df() {
        // 100 blocks, 40 free (10 reserved), 4096 bytes: used=60, avail=30.
        let u = from_blocks(4096, 100, 40, 30);
        assert_eq!(u.total, 100 * 4096);
        assert_eq!(u.used, 60 * 4096);
        assert_eq!(u.avail, 30 * 4096);
        assert!((u.percent - 66.67).abs() < 0.5); // 60 / (60+30)
    }
}
