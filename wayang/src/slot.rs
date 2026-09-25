//! A/B slot selection and the anti-brick boot decision.
//!
//! GRUB boots `wayang_slot` unless `wayang_attempts >= 3`, in which case it
//! falls back to `wayang_good`.

use crate::state::EnvView;

pub const ATTEMPT_LIMIT: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn as_str(self) -> &'static str {
        match self {
            Slot::A => "A",
            Slot::B => "B",
        }
    }

    pub fn parse(s: &str) -> Option<Slot> {
        match s.trim() {
            "A" | "a" => Some(Slot::A),
            "B" | "b" => Some(Slot::B),
            _ => None,
        }
    }

    pub fn idle(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
}

/// Slot GRUB will boot next, honouring the attempt budget.
pub fn boot_slot(env: &impl EnvView) -> Slot {
    let attempts = attempts(env);
    let wanted = env.get("wayang_slot").and_then(Slot::parse).unwrap_or(Slot::A);
    if attempts >= ATTEMPT_LIMIT {
        env.get("wayang_good").and_then(Slot::parse).unwrap_or(wanted)
    } else {
        wanted
    }
}

/// The slot currently staged to boot (`wayang_slot`), without the fallback.
pub fn staged_slot(env: &impl EnvView) -> Slot {
    env.get("wayang_slot").and_then(Slot::parse).unwrap_or(Slot::A)
}

pub fn attempts(env: &impl EnvView) -> u32 {
    env.get("wayang_attempts")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grubenv::GrubEnv;

    fn env(kv: &[(&str, &str)]) -> GrubEnv {
        let mut e = GrubEnv::default();
        for (k, v) in kv {
            e.set(k, v);
        }
        e
    }

    #[test]
    fn idle_is_other() {
        assert_eq!(Slot::A.idle(), Slot::B);
        assert_eq!(Slot::B.idle(), Slot::A);
    }

    #[test]
    fn uses_staged_slot_below_limit() {
        let e = env(&[("wayang_slot", "B"), ("wayang_good", "A"), ("wayang_attempts", "2")]);
        assert_eq!(boot_slot(&e), Slot::B);
    }

    #[test]
    fn uses_good_at_limit() {
        let e = env(&[("wayang_slot", "B"), ("wayang_good", "A"), ("wayang_attempts", "3")]);
        assert_eq!(boot_slot(&e), Slot::A);
        let e = env(&[("wayang_slot", "B"), ("wayang_good", "A"), ("wayang_attempts", "9")]);
        assert_eq!(boot_slot(&e), Slot::A);
    }

    #[test]
    fn missing_values_default() {
        let e = env(&[]);
        assert_eq!(boot_slot(&e), Slot::A);
        assert_eq!(staged_slot(&e), Slot::A);
        assert_eq!(attempts(&e), 0);
    }

    #[test]
    fn good_missing_at_limit_keeps_staged() {
        let e = env(&[("wayang_slot", "B"), ("wayang_attempts", "3")]);
        assert_eq!(boot_slot(&e), Slot::B);
    }
}
