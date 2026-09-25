//! Console output gating for the HUD.
//!
//! The CLI prints its results and the TUI must not scribble over the alternate
//! screen, so user-facing lines go through [`outln!`], which is silenced while
//! the HUD runs the same code paths.

use std::sync::atomic::{AtomicBool, Ordering};

static QUIET: AtomicBool = AtomicBool::new(false);

pub fn quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

pub fn set_quiet(value: bool) {
    QUIET.store(value, Ordering::Relaxed);
}

/// `println!` unless the HUD has silenced user output.
#[macro_export]
macro_rules! outln {
    ($($arg:tt)*) => {
        if !$crate::ui::quiet() {
            println!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_toggles() {
        assert!(!quiet());
        set_quiet(true);
        assert!(quiet());
        set_quiet(false);
        assert!(!quiet());
    }
}
