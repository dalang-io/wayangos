//! Hand-rolled argument parsing (no external CLI dependency).

use std::path::PathBuf;

#[derive(Debug, Default, Clone)]
pub struct UpdateArgs {
    pub check: bool,
    pub from: Option<PathBuf>,
    pub channel: Option<String>,
    pub esp: Option<String>,
    pub reboot: bool,
    pub rollback: bool,
    /// Stage the idle slot for the next boot (a one-shot slot switch).
    pub boot_other: bool,
    /// Point the next boot at the last known-good slot (`wayang_good`); used
    /// by the unattended self-test (`wayang-selftest`) after a failed boot.
    pub fallback: bool,
}

/// `wayang edgerouter` actions (docs/EDGEROUTER.md §C).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeRouterAction {
    Status,
    Enroll(String),
    Clear,
    /// Bring the wayangi agent up (background; the tunnel comes up after).
    Start,
    /// Stop the running wayangi agent.
    Stop,
    /// Stop then start the wayangi agent.
    Restart,
    /// Install a wayangi Edge bundle (dir or `.tar.gz`) onto this box.
    Apply { path: PathBuf, force: bool },
    Help,
}

#[derive(Debug)]
pub enum Command {
    Version,
    Status { json: bool },
    Update(UpdateArgs),
    Upgrade(UpdateArgs),
    Net,
    Wifi,
    WifiDetect { json: bool },
    EdgeRouter(EdgeRouterAction),
    Keygen { out: PathBuf, keyid: String },
    Sign { key: PathBuf, keyid: Option<String>, manifest: PathBuf },
    Verify { bundle: PathBuf, esp: Option<String> },
    MarkOk { esp: Option<String> },
    /// `wayang addkey`: the single SSH-key implementation behind
    /// `wayang-addkey` and the SSH screen.
    AddKey { spec: String },
    PrintVersion,
    Help,
}

fn value_after(args: &[String], i: &mut usize, name: &str) -> Result<String, String> {
    // supports both `--flag value` and `--flag=value`
    let cur = &args[*i];
    if let Some(v) = cur.split_once('=').map(|(_, v)| v) {
        if v.is_empty() {
            return Err(format!("{name} needs a value"));
        }
        return Ok(v.to_string());
    }
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{name} needs a value"))
}

fn update_args(args: &[String], sub: &str) -> Result<UpdateArgs, String> {
    let mut a = UpdateArgs::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.split_once('=').map(|(k, _)| k).unwrap_or(arg.as_str()) {
            "--check" => a.check = true,
            "--reboot" => a.reboot = true,
            "--rollback" => a.rollback = true,
            "--boot-other" => a.boot_other = true,
            "--fallback" => a.fallback = true,
            "--from" => a.from = Some(PathBuf::from(value_after(args, &mut i, "--from")?)),
            "--channel" => a.channel = Some(value_after(args, &mut i, "--channel")?),
            "--esp" => a.esp = Some(value_after(args, &mut i, "--esp")?),
            other => return Err(format!("unknown option for `{sub}`: {other}")),
        }
        i += 1;
    }
    Ok(a)
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let sub = match args.first() {
        Some(s) => s.as_str(),
        None => return Ok(Command::Help),
    };
    let rest = &args[1..];
    match sub {
        "version" => Ok(Command::Version),
        "status" => {
            let json = rest.iter().any(|a| a == "--json");
            for a in rest {
                if a != "--json" {
                    return Err(format!("unknown option for `status`: {a}"));
                }
            }
            Ok(Command::Status { json })
        }
        "update" => Ok(Command::Update(update_args(rest, "update")?)),
        "upgrade" => Ok(Command::Upgrade(update_args(rest, "upgrade")?)),
        "net" | "wifi" => {
            let help = rest.iter().any(|a| a == "--help" || a == "-h");
            if sub == "wifi" && !help && rest.first().map(String::as_str) == Some("detect") {
                let json = rest.iter().any(|a| a == "--json");
                for a in &rest[1..] {
                    if a != "--json" {
                        return Err(format!("unknown option for `wifi detect`: {a}"));
                    }
                }
                return Ok(Command::WifiDetect { json });
            }
            for a in rest {
                if a != "--help" && a != "-h" {
                    return Err(format!("unknown option for `{sub}`: {a}"));
                }
            }
            if sub == "net" {
                Ok(Command::Net)
            } else {
                Ok(Command::Wifi)
            }
        }
        "edgerouter" => {
            if rest.iter().any(|a| a == "--help" || a == "-h") {
                return Ok(Command::EdgeRouter(EdgeRouterAction::Help));
            }
            match rest.first().map(String::as_str) {
                None | Some("status") => {
                    if let Some(extra) = rest.get(1) {
                        return Err(format!("unknown option for `edgerouter status`: {extra}"));
                    }
                    Ok(Command::EdgeRouter(EdgeRouterAction::Status))
                }
                Some("enroll") => {
                    let token = rest
                        .get(1)
                        .cloned()
                        .ok_or_else(|| "edgerouter enroll needs the device token".to_string())?;
                    if let Some(extra) = rest.get(2) {
                        return Err(format!("unknown option for `edgerouter enroll`: {extra}"));
                    }
                    Ok(Command::EdgeRouter(EdgeRouterAction::Enroll(token)))
                }
                Some("clear") => {
                    if let Some(extra) = rest.get(1) {
                        return Err(format!("unknown option for `edgerouter clear`: {extra}"));
                    }
                    Ok(Command::EdgeRouter(EdgeRouterAction::Clear))
                }
                Some(action @ ("start" | "stop" | "restart")) => {
                    if let Some(extra) = rest.get(1) {
                        return Err(format!("unknown option for `edgerouter {action}`: {extra}"));
                    }
                    Ok(Command::EdgeRouter(match action {
                        "start" => EdgeRouterAction::Start,
                        "stop" => EdgeRouterAction::Stop,
                        _ => EdgeRouterAction::Restart,
                    }))
                }
                Some("apply") => {
                    let mut path: Option<PathBuf> = None;
                    let mut force = false;
                    let mut i = 1;
                    while i < rest.len() {
                        match rest[i].as_str() {
                            "--force" => force = true,
                            other if other.starts_with("--") => {
                                return Err(format!("unknown option for `edgerouter apply`: {other}"))
                            }
                            other => path = Some(PathBuf::from(other)),
                        }
                        i += 1;
                    }
                    let path = path
                        .ok_or_else(|| "edgerouter apply needs a bundle (a directory or FILE.tar.gz)".to_string())?;
                    Ok(Command::EdgeRouter(EdgeRouterAction::Apply { path, force }))
                }
                Some(other) => Err(format!(
                    "unknown edgerouter action '{other}' (status|start|stop|restart|enroll|clear|apply)"
                )),
            }
        }
        "keygen" => {
            let mut out: Option<PathBuf> = None;
            let mut keyid = "release".to_string();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].split_once('=').map(|(k, _)| k).unwrap_or(rest[i].as_str()) {
                    "--out" => out = Some(PathBuf::from(value_after(rest, &mut i, "--out")?)),
                    "--keyid" => keyid = value_after(rest, &mut i, "--keyid")?,
                    other => return Err(format!("unknown option for `keygen`: {other}")),
                }
                i += 1;
            }
            let out = out.ok_or_else(|| "keygen needs --out DIR".to_string())?;
            Ok(Command::Keygen { out, keyid })
        }
        "sign" => {
            let mut key: Option<PathBuf> = None;
            let mut keyid: Option<String> = None;
            let mut manifest: Option<PathBuf> = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].split_once('=').map(|(k, _)| k).unwrap_or(rest[i].as_str()) {
                    "--key" => key = Some(PathBuf::from(value_after(rest, &mut i, "--key")?)),
                    "--keyid" => keyid = Some(value_after(rest, &mut i, "--keyid")?),
                    other if other.starts_with("--") => {
                        return Err(format!("unknown option for `sign`: {other}"))
                    }
                    other => manifest = Some(PathBuf::from(other)),
                }
                i += 1;
            }
            Ok(Command::Sign {
                key: key.ok_or_else(|| "sign needs --key FILE".to_string())?,
                keyid,
                manifest: manifest.ok_or_else(|| "sign needs MANIFEST.json".to_string())?,
            })
        }
        "verify" => {
            let mut bundle: Option<PathBuf> = None;
            let mut esp: Option<String> = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].split_once('=').map(|(k, _)| k).unwrap_or(rest[i].as_str()) {
                    "--esp" => esp = Some(value_after(rest, &mut i, "--esp")?),
                    other if other.starts_with("--") => {
                        return Err(format!("unknown option for `verify`: {other}"))
                    }
                    other => bundle = Some(PathBuf::from(other)),
                }
                i += 1;
            }
            Ok(Command::Verify {
                bundle: bundle.ok_or_else(|| "verify needs FILE.wup".to_string())?,
                esp,
            })
        }
        "mark-ok" => {
            let mut esp = None;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--esp" => esp = Some(value_after(rest, &mut i, "--esp")?),
                    other => return Err(format!("unknown option for `mark-ok`: {other}")),
                }
                i += 1;
            }
            Ok(Command::MarkOk { esp })
        }
        "addkey" => Ok(Command::AddKey { spec: rest.join(" ") }),
        "-h" | "--help" | "help" => Ok(Command::Help),
        "-V" | "--version" => Ok(Command::PrintVersion),
        other => Err(format!("unknown command '{other}' (try `wayang --help`)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_update_flags() {
        let c = parse(&v(&["update", "--check", "--channel", "edge", "--esp=/dev/sda2", "--reboot"])).unwrap();
        match c {
            Command::Update(a) => {
                assert!(a.check);
                assert_eq!(a.channel.as_deref(), Some("edge"));
                assert_eq!(a.esp.as_deref(), Some("/dev/sda2"));
                assert!(a.reboot);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn parses_rollback_and_from() {
        match parse(&v(&["update", "--rollback", "--from", "/x/y.wup"])).unwrap() {
            Command::Update(a) => {
                assert!(a.rollback);
                assert_eq!(a.from, Some(PathBuf::from("/x/y.wup")));
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn parses_boot_other() {
        match parse(&v(&["update", "--boot-other"])).unwrap() {
            Command::Update(a) => assert!(a.boot_other),
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn parses_fallback() {
        match parse(&v(&["update", "--fallback"])).unwrap() {
            Command::Update(a) => assert!(a.fallback),
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn parses_addkey_spec() {
        match parse(&v(&["addkey", "github:alice"])).unwrap() {
            Command::AddKey { spec } => assert_eq!(spec, "github:alice"),
            _ => panic!("wrong command"),
        }
        match parse(&v(&["addkey", "ssh-ed25519", "AAAA", "me@box"])).unwrap() {
            Command::AddKey { spec } => assert_eq!(spec, "ssh-ed25519 AAAA me@box"),
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn parses_keygen_sign_verify() {
        assert!(matches!(parse(&v(&["keygen", "--out", "/k"])).unwrap(), Command::Keygen { .. }));
        match parse(&v(&["sign", "--key", "/k/release.key", "/m/manifest.json"])).unwrap() {
            Command::Sign { key, manifest, .. } => {
                assert_eq!(key, PathBuf::from("/k/release.key"));
                assert_eq!(manifest, PathBuf::from("/m/manifest.json"));
            }
            _ => panic!("wrong command"),
        }
        assert!(matches!(parse(&v(&["verify", "/b.wup"])).unwrap(), Command::Verify { .. }));
    }

    #[test]
    fn rejects_bad() {
        assert!(parse(&v(&["bogus"])).is_err());
        assert!(parse(&v(&["update", "--wat"])).is_err());
        assert!(parse(&v(&["sign", "/m.json"])).is_err());
        assert!(parse(&v(&["keygen"])).is_err());
        assert!(parse(&v(&["update", "--channel"])).is_err());
    }

    #[test]
    fn help_and_version() {
        assert!(matches!(parse(&v(&["--help"])).unwrap(), Command::Help));
        assert!(matches!(parse(&v(&[])).unwrap(), Command::Help));
        assert!(matches!(parse(&v(&["version"])).unwrap(), Command::Version));
    }

    #[test]
    fn parses_net_and_wifi() {
        assert!(matches!(parse(&v(&["net"])).unwrap(), Command::Net));
        assert!(matches!(parse(&v(&["wifi"])).unwrap(), Command::Wifi));
        assert!(matches!(parse(&v(&["net", "--help"])).unwrap(), Command::Net));
        assert!(parse(&v(&["net", "--bogus"])).is_err());
        assert!(matches!(
            parse(&v(&["wifi", "detect"])).unwrap(),
            Command::WifiDetect { json: false }
        ));
        assert!(matches!(
            parse(&v(&["wifi", "detect", "--json"])).unwrap(),
            Command::WifiDetect { json: true }
        ));
        assert!(parse(&v(&["wifi", "detect", "--bogus"])).is_err());
    }

    #[test]
    fn parses_edgerouter() {
        assert!(matches!(
            parse(&v(&["edgerouter"])).unwrap(),
            Command::EdgeRouter(EdgeRouterAction::Status)
        ));
        assert!(matches!(
            parse(&v(&["edgerouter", "status"])).unwrap(),
            Command::EdgeRouter(EdgeRouterAction::Status)
        ));
        match parse(&v(&["edgerouter", "enroll", "abc123def456"])).unwrap() {
            Command::EdgeRouter(EdgeRouterAction::Enroll(t)) => assert_eq!(t, "abc123def456"),
            _ => panic!("wrong command"),
        }
        assert!(matches!(
            parse(&v(&["edgerouter", "clear"])).unwrap(),
            Command::EdgeRouter(EdgeRouterAction::Clear)
        ));
        assert!(matches!(
            parse(&v(&["edgerouter", "start"])).unwrap(),
            Command::EdgeRouter(EdgeRouterAction::Start)
        ));
        assert!(matches!(
            parse(&v(&["edgerouter", "stop"])).unwrap(),
            Command::EdgeRouter(EdgeRouterAction::Stop)
        ));
        assert!(matches!(
            parse(&v(&["edgerouter", "restart"])).unwrap(),
            Command::EdgeRouter(EdgeRouterAction::Restart)
        ));
        match parse(&v(&["edgerouter", "apply", "/tmp/bundle.tar.gz"])).unwrap() {
            Command::EdgeRouter(EdgeRouterAction::Apply { path, force }) => {
                assert_eq!(path, PathBuf::from("/tmp/bundle.tar.gz"));
                assert!(!force);
            }
            _ => panic!("wrong command"),
        }
        match parse(&v(&["edgerouter", "apply", "/tmp/b", "--force"])).unwrap() {
            Command::EdgeRouter(EdgeRouterAction::Apply { path, force }) => {
                assert_eq!(path, PathBuf::from("/tmp/b"));
                assert!(force);
            }
            _ => panic!("wrong command"),
        }
        assert!(matches!(
            parse(&v(&["edgerouter", "--help"])).unwrap(),
            Command::EdgeRouter(EdgeRouterAction::Help)
        ));
        assert!(parse(&v(&["edgerouter", "enroll"])).is_err(), "token required");
        assert!(parse(&v(&["edgerouter", "bogus"])).is_err());
        assert!(parse(&v(&["edgerouter", "start", "now"])).is_err());
        assert!(parse(&v(&["edgerouter", "status", "extra"])).is_err());
        assert!(parse(&v(&["edgerouter", "apply"])).is_err(), "bundle required");
        assert!(parse(&v(&["edgerouter", "apply", "/x", "--wat"])).is_err());
    }
}
