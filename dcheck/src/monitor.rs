//! Monitoring: one-shot health gate (`check`) and a watch loop with alerts.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::json::{self, Json};
use crate::model::Device;
use crate::report;

/// Per-device health snapshot used for diffing between cycles.
#[derive(Debug, Clone, PartialEq)]
pub struct DevState {
    pub device: String,
    pub severity: u8,
    pub verdict: &'static str,
    pub issues: Vec<String>,
    pub notes: Vec<String>,
}

pub fn snapshot(devices: &[Device]) -> Vec<DevState> {
    devices
        .iter()
        .zip(report::metrics_all(devices))
        .map(|(d, m)| match m.map(|(_, h)| h) {
            Some(h) => DevState {
                device: d.path.clone(),
                severity: h.verdict.severity(),
                verdict: h.verdict.label(),
                issues: h.issues.clone(),
                notes: h.notes.clone(),
            },
            None if crate::virt::is_virtual_disk(d) => DevState {
                device: d.path.clone(),
                severity: 0,
                verdict: "VIRTUAL",
                issues: Vec::new(),
                notes: vec![crate::virt::DISK_NOTE.into()],
            },
            None => DevState {
                device: d.path.clone(),
                severity: 1,
                verdict: "UNKNOWN",
                issues: Vec::new(),
                notes: Vec::new(),
            },
        })
        .collect()
}

/// Alerts describing what changed for the worse between two snapshots.
pub fn diff_alert(prev: &[DevState], cur: &[DevState]) -> Vec<String> {
    let mut alerts = Vec::new();
    for c in cur {
        match prev.iter().find(|p| p.device == c.device) {
            None => alerts.push(format!("{} appeared ({})", c.device, c.verdict)),
            Some(p) => {
                if c.severity > p.severity {
                    alerts.push(format!("{} worsened: {} -> {}", c.device, p.verdict, c.verdict));
                }
                for issue in &c.issues {
                    if !p.issues.contains(issue) {
                        alerts.push(format!("{}: {}", c.device, issue));
                    }
                }
            }
        }
    }
    alerts
}

fn worst(states: &[DevState]) -> u8 {
    states.iter().map(|s| s.severity).max().unwrap_or(0)
}

/// Exit code from a worst severity: 0 ok, 1 unknown, 2 monitor, 3 backup/replace.
fn exit_code(sev: u8) -> i32 {
    match sev {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 3,
    }
}

fn states_json(states: &[DevState]) -> Json {
    Json::Arr(
        states
            .iter()
            .map(|s| {
                json::object(vec![
                    ("device", json::string(s.device.clone())),
                    ("verdict", json::string(s.verdict)),
                    ("severity", json::num(s.severity as f64)),
                    (
                        "issues",
                        Json::Arr(
                            s.issues
                                .iter()
                                .map(|i| json::string(i.clone()))
                                .collect(),
                        ),
                    ),
                    (
                        "notes",
                        Json::Arr(
                            s.notes
                                .iter()
                                .map(|n| json::string(n.clone()))
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    )
}

/// One-shot health gate. Prints a summary and returns the exit code.
pub fn check(devices: &[Device], as_json: bool) -> i32 {
    let states = snapshot(devices);
    if as_json {
        println!("{}", states_json(&states));
    } else {
        for s in &states {
            let detail = if s.verdict == "VIRTUAL" { s.notes.join("; ") } else { s.issues.join("; ") };
            println!("{:<14} {:<12} {}", s.device, s.verdict, detail);
        }
    }
    exit_code(worst(&states))
}

/// Watch loop: print each cycle, alert on worsening, POST to a webhook.
pub fn watch(
    devices: &[Device],
    interval_secs: u64,
    as_json: bool,
    quiet: bool,
    webhook: Option<&str>,
) -> i32 {
    let mut prev: Vec<DevState> = Vec::new();
    loop {
        let cur = snapshot(devices);
        let alerts = diff_alert(&prev, &cur);

        if as_json {
            let cycle = json::object(vec![
                ("timestamp", json::num(now_secs() as f64)),
                ("devices", states_json(&cur)),
                (
                    "alerts",
                    Json::Arr(alerts.iter().map(|a| json::string(a.clone())).collect()),
                ),
            ]);
            println!("{}", cycle);
        } else if !quiet {
            let ts = now_secs();
            for s in &cur {
                println!("[{ts}] {:<14} {:<12} {}", s.device, s.verdict, s.issues.join("; "));
            }
        }

        for a in &alerts {
            eprintln!("ALERT: {a}");
        }
        if !alerts.is_empty() {
            if let Some(url) = webhook {
                post_webhook(url, &webhook_body(&cur, &alerts));
            }
        }

        prev = cur;
        std::thread::sleep(Duration::from_secs(interval_secs.max(1)));
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Webhook payload: `{ "event", "alerts": [..], "devices": [..] }`.
fn webhook_body(states: &[DevState], alerts: &[String]) -> Json {
    json::object(vec![
        ("event", json::string("dcheck.alert")),
        ("alerts", Json::Arr(alerts.iter().map(|a| json::string(a.clone())).collect())),
        ("devices", states_json(states)),
    ])
}

/// POST a JSON alert via `curl` (no TLS handling in-tree).
fn post_webhook(url: &str, body: &Json) {
    let body = body.to_string();
    match std::process::Command::new("curl")
        .args([
            "-sS",
            "-m",
            "10",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "--data",
            &body,
            url,
        ])
        .status()
    {
        Ok(s) if s.success() => {}
        _ => eprintln!("dcheck: webhook POST failed (curl missing?)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(device: &str, severity: u8, verdict: &'static str, issues: &[&str]) -> DevState {
        DevState {
            device: device.into(),
            severity,
            verdict,
            issues: issues.iter().map(|s| s.to_string()).collect(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn detects_worsening_and_new_issues() {
        let prev = vec![st("/dev/sda", 0, "OK", &[])];
        let cur = vec![st("/dev/sda", 2, "MONITOR", &["3 reallocated sectors"])];
        let alerts = diff_alert(&prev, &cur);
        assert_eq!(alerts.len(), 2);
        assert!(alerts[0].contains("worsened"));
        assert!(alerts[1].contains("reallocated"));
    }

    #[test]
    fn stable_state_has_no_alerts() {
        let prev = vec![st("/dev/sda", 0, "OK", &[])];
        let cur = vec![st("/dev/sda", 0, "OK", &[])];
        assert!(diff_alert(&prev, &cur).is_empty());
    }

    #[test]
    fn exit_codes_map_from_severity() {
        assert_eq!(exit_code(0), 0);
        assert_eq!(exit_code(1), 1);
        assert_eq!(exit_code(2), 2);
        assert_eq!(exit_code(3), 3);
        assert_eq!(exit_code(4), 3);
    }

    #[test]
    fn webhook_devices_is_an_array() {
        let body = webhook_body(&[st("/dev/sda", 2, "MONITOR", &[])], &["x".into()]);
        let Some(Json::Obj(map)) = Json::parse(&body.to_string()) else {
            panic!("webhook body is not an object");
        };
        assert!(matches!(map.get("devices"), Some(Json::Arr(a)) if a.len() == 1));
    }
}
