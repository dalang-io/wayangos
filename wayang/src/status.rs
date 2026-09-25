//! `wayang status` — installed version, A/B slots, fallback state, channel, /data.

use std::path::Path;

use serde_json::json;

use crate::error::Result;
use crate::grubenv::GrubEnv;
use crate::manifest::SlotMeta;
use crate::mount;
use crate::paths;
use crate::slot::{self, Slot};
use crate::version;

#[derive(Debug, Clone)]
pub struct SlotInfo {
    pub slot: Slot,
    pub meta: Option<SlotMeta>,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub version: Option<String>,
    pub channel: String,
    pub active: Slot,
    pub boot_next: Slot,
    pub good: Option<Slot>,
    pub attempts: u32,
    pub slots: [SlotInfo; 2],
    pub data: bool,
}

pub fn gather(boot_dir: &Path, version: Option<String>, channel: String, data: bool) -> Status {
    let env = GrubEnv::read(&boot_dir.join("grub/grubenv")).unwrap_or_default();
    let slot_info = |s: Slot| SlotInfo {
        slot: s,
        meta: SlotMeta::read(&boot_dir.join("var").join(format!("meta-{}.json", s.as_str()))),
    };
    Status {
        version,
        channel,
        active: slot::staged_slot(&env),
        boot_next: slot::boot_slot(&env),
        good: env.get("wayang_good").and_then(Slot::parse),
        attempts: slot::attempts(&env),
        slots: [slot_info(Slot::A), slot_info(Slot::B)],
        data,
    }
}

impl Status {
    pub fn to_json(&self) -> String {
        let slot_json = |si: &SlotInfo| {
            json!({
                "slot": si.slot.as_str(),
                "version": si.meta.as_ref().map(|m| m.version.clone()),
                "channel": si.meta.as_ref().map(|m| m.channel.clone()),
                "arch": si.meta.as_ref().map(|m| m.arch.clone()),
            })
        };
        json!({
            "version": self.version,
            "channel": self.channel,
            "active_slot": self.active.as_str(),
            "boot_next_slot": self.boot_next.as_str(),
            "good_slot": self.good.map(|s| s.as_str()),
            "attempts": self.attempts,
            "slots": [slot_json(&self.slots[0]), slot_json(&self.slots[1])],
            "data": self.data,
        })
        .to_string()
    }

    pub fn print_human(&self) {
        println!("version:   {}", self.version.clone().unwrap_or_else(|| "unknown".into()));
        println!("channel:   {}", self.channel);
        println!("active:    {}", self.active.as_str());
        println!("boot next: {} (good: {}, attempts: {})", self.boot_next.as_str(), self.good.map(|s| s.as_str()).unwrap_or("-"), self.attempts);
        for si in &self.slots {
            let v = si.meta.as_ref().map(|m| m.version.as_str()).unwrap_or("-");
            println!("slot {}:    {}", si.slot.as_str(), v);
        }
        println!("data:      {}", if self.data { "present" } else { "missing" });
    }
}

pub fn run(json: bool, esp: Option<&str>) -> Result<i32> {
    let boot = mount::open(esp)?;
    let st = gather(
        &boot.path,
        version::read().ok(),
        version::channel_or("stable"),
        paths::data_dir().exists(),
    );
    if json {
        println!("{}", st.to_json());
    } else {
        st.print_human();
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn tmp() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "wayang-status-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("grub")).unwrap();
        std::fs::create_dir_all(d.join("var")).unwrap();
        d
    }

    #[test]
    fn gathers_slots_and_fallback() {
        let d = tmp();
        let mut env = GrubEnv::default();
        env.set("wayang_slot", "B");
        env.set("wayang_good", "A");
        env.set("wayang_attempts", "3");
        env.write(&d.join("grub/grubenv")).unwrap();
        std::fs::write(d.join("var/meta-A.json"), r#"{"version":"1.2.0","channel":"stable","arch":"x86_64"}"#).unwrap();
        std::fs::write(d.join("var/meta-B.json"), r#"{"version":"1.4.1","channel":"stable","arch":"x86_64"}"#).unwrap();

        let st = gather(&d, Some("1.4.1".into()), "stable".into(), true);
        assert_eq!(st.active, Slot::B);
        assert_eq!(st.boot_next, Slot::A);
        assert_eq!(st.attempts, 3);
        assert_eq!(st.slots[0].meta.as_ref().unwrap().version, "1.2.0");
        assert_eq!(st.slots[1].meta.as_ref().unwrap().version, "1.4.1");
        let j = st.to_json();
        assert!(j.contains("\"boot_next_slot\":\"A\""));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_files_default() {
        let d = tmp();
        let st = gather(&d, None, "stable".into(), false);
        assert_eq!(st.active, Slot::A);
        assert!(st.good.is_none());
        assert!(st.slots[0].meta.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
