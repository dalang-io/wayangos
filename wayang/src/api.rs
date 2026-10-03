//! `wayang api` — the WayangOS CLI as a local HTTP/JSON API.
//!
//! The transport (HTTP, token scopes, rate limit, TLS/mTLS, audit, events) is
//! the shared `wayang-api` crate; the routes are in [`crate::api_routes`]. The
//! flags are the same as `wayang-fw api` and `wayang-router api` — one contract
//! for the three tools:
//!
//! ```text
//! wayang api [--listen ADDR] [--token-file F] [--rw] [--insecure-no-auth]
//!            [--insecure-http] [--tls-cert F --tls-key F [--client-ca F]]
//!            [--rate-burst N] [--rate-per-sec X] [--no-rate-limit]
//! wayang api --gen-token [--token-file F] [--scope ro|rw|admin] [--label WORD] [--append]
//! ```
//!
//! Read-only unless `--rw`; `--rw` needs tokens (never `--insecure-no-auth`); a
//! non-loopback bind needs tokens **and** TLS (or an explicit `--insecure-http`).

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;

use wayang_api::audit::AuditLog;
use wayang_api::auth::{self, Scope, TokenStore};
use wayang_api::limit::RateConfig;
use wayang_api::{Options, Server, tls};

use crate::api_routes::WayangApp;
use crate::paths;

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8632";

/// Where the API's token lives: next to the other wayangi state, so one
/// `/data/etc/wayangi` holds what is enrolled on this box.
pub fn default_token_file() -> PathBuf {
    paths::wayangi_dir().join("api-token")
}

pub fn audit_file() -> PathBuf {
    paths::data_var_dir().join("wayang/api-audit.jsonl")
}

#[derive(Debug, Default, PartialEq)]
pub struct ApiArgs {
    pub listen: Option<String>,
    pub token_file: Option<PathBuf>,
    pub rw: bool,
    pub insecure_no_auth: bool,
    pub insecure_http: bool,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub client_ca: Option<PathBuf>,
    pub rate_burst: Option<f64>,
    pub rate_per_sec: Option<f64>,
    pub no_rate_limit: bool,
    pub gen_token: bool,
    pub scope: Option<String>,
    pub label: Option<String>,
    pub append: bool,
}

const FLAGS: &[&str] = &[
    "--listen",
    "--token-file",
    "--rw",
    "--ro",
    "--insecure-no-auth",
    "--insecure-http",
    "--tls-cert",
    "--tls-key",
    "--client-ca",
    "--rate-burst",
    "--rate-per-sec",
    "--no-rate-limit",
    "--gen-token",
    "--scope",
    "--label",
    "--append",
];

pub fn parse_args(args: &[String]) -> Result<ApiArgs, String> {
    let mut a = ApiArgs::default();
    let mut ro = false;
    let mut i = 0;
    while i < args.len() {
        let raw = &args[i];
        let (flag, inline) = match raw.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (raw.as_str(), None),
        };
        if !FLAGS.contains(&flag) {
            return Err(format!(
                "unknown option for `api`: {raw} (known: {}); the server is read-only unless started with --rw",
                FLAGS.join(" ")
            ));
        }
        let mut value = |name: &str| -> Result<String, String> {
            if let Some(v) = &inline {
                return Ok(v.clone());
            }
            i += 1;
            args.get(i)
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag {
            "--listen" => a.listen = Some(value("--listen")?),
            "--token-file" => a.token_file = Some(PathBuf::from(value("--token-file")?)),
            "--rw" => a.rw = true,
            "--ro" => ro = true,
            "--insecure-no-auth" => a.insecure_no_auth = true,
            "--insecure-http" => a.insecure_http = true,
            "--tls-cert" => a.tls_cert = Some(PathBuf::from(value("--tls-cert")?)),
            "--tls-key" => a.tls_key = Some(PathBuf::from(value("--tls-key")?)),
            "--client-ca" => a.client_ca = Some(PathBuf::from(value("--client-ca")?)),
            "--rate-burst" => {
                let v = value("--rate-burst")?;
                a.rate_burst = Some(
                    v.parse::<f64>()
                        .ok()
                        .filter(|n| *n >= 1.0)
                        .ok_or_else(|| format!("--rate-burst: '{v}' is not a number >= 1"))?,
                );
            }
            "--rate-per-sec" => {
                let v = value("--rate-per-sec")?;
                a.rate_per_sec = Some(
                    v.parse::<f64>()
                        .ok()
                        .filter(|n| *n > 0.0)
                        .ok_or_else(|| format!("--rate-per-sec: '{v}' is not a number > 0"))?,
                );
            }
            "--no-rate-limit" => a.no_rate_limit = true,
            "--gen-token" => a.gen_token = true,
            "--scope" => a.scope = Some(value("--scope")?),
            "--label" => a.label = Some(value("--label")?),
            "--append" => a.append = true,
            _ => unreachable!(),
        }
        i += 1;
    }
    if a.rw && ro {
        return Err("--rw and --ro contradict each other".into());
    }
    Ok(a)
}

/// What the flags amount to, once checked against each other.
pub struct Plan {
    pub addr: SocketAddr,
    /// The server options (the audit trail is added by [`build`]).
    pub opts: Options,
    pub tls: bool,
    pub mtls: bool,
}

pub fn plan(a: &ApiArgs) -> Result<Plan, String> {
    let listen = a
        .listen
        .clone()
        .unwrap_or_else(|| DEFAULT_LISTEN.to_string());
    let addr: SocketAddr = listen
        .parse()
        .map_err(|_| format!("--listen expects HOST:PORT (got '{listen}')"))?;
    let loopback = addr.ip().is_loopback();

    if a.tls_cert.is_some() != a.tls_key.is_some() {
        return Err("--tls-cert and --tls-key go together".into());
    }
    if a.client_ca.is_some() && a.tls_cert.is_none() {
        return Err("--client-ca (mutual TLS) needs --tls-cert and --tls-key".into());
    }
    if a.rw && a.insecure_no_auth {
        return Err(
            "--rw needs a bearer token: it will not run without authentication, not even with \
             --insecure-no-auth"
                .into(),
        );
    }

    // `--insecure-no-auth` only ever means anything on loopback.
    let no_auth = a.insecure_no_auth && loopback;
    let token_file = a.token_file.clone().unwrap_or_else(default_token_file);
    let tokens = if no_auth {
        None
    } else {
        TokenStore::load(&token_file)
    };
    if tokens.is_none() && !no_auth {
        return Err(format!(
            "no API token in {} — run `wayang api --gen-token`, or for loopback dev pass \
             --insecure-no-auth",
            token_file.display()
        ));
    }
    if a.rw && tokens.is_none() {
        return Err("--rw needs a bearer token".into());
    }

    let tls = match (&a.tls_cert, &a.tls_key) {
        (Some(c), Some(k)) => Some(tls::server_config(c, k, a.client_ca.as_deref())?),
        _ => None,
    };
    if !loopback && tls.is_none() && !a.insecure_http {
        return Err(
            "a non-loopback bind needs TLS (--tls-cert/--tls-key; add --client-ca for mutual TLS) \
             or an explicit --insecure-http"
                .into(),
        );
    }

    let rate = if a.no_rate_limit {
        None
    } else {
        let d = RateConfig::DEFAULT;
        Some(RateConfig {
            burst: a.rate_burst.unwrap_or(d.burst),
            per_sec: a.rate_per_sec.unwrap_or(d.per_sec),
        })
    };
    Ok(Plan {
        addr,
        tls: tls.is_some(),
        mtls: a.client_ca.is_some(),
        opts: Options {
            tokens,
            rw: a.rw,
            tls,
            rate,
            ..Options::default()
        },
    })
}

fn gen_token(a: &ApiArgs) -> Result<String, String> {
    let scope = match a.scope.as_deref() {
        None => Scope::Rw,
        Some(s) => {
            Scope::parse(s).ok_or_else(|| format!("--scope: '{s}' is not ro, rw or admin"))?
        }
    };
    let file = a.token_file.clone().unwrap_or_else(default_token_file);
    auth::gen_token(
        &file,
        scope,
        a.label.as_deref().unwrap_or("default"),
        a.append,
    )
}

/// Builds the server for `plan` (the audit trail lives under `/data/var`).
pub fn build(mut p: Plan) -> Result<Arc<Server>, String> {
    let audit = audit_file();
    if let Some(dir) = audit.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        crate::sys::chmod(dir, 0o700);
    }
    p.opts.audit = Some(AuditLog::new(audit));
    let app = Arc::new(WayangApp::new(p.tls, p.mtls, p.opts.rw));
    Ok(Arc::new(Server::new(app, p.opts)?))
}

/// `wayang api …` (the arguments after `api`).
pub fn run(args: &[String]) -> Result<i32, String> {
    let a = parse_args(args)?;
    if a.gen_token {
        // the token goes to stdout so it can be captured; nothing else does
        println!("{}", gen_token(&a)?);
        return Ok(0);
    }
    let p = plan(&a)?;
    // `outln!` is for the CLI's own results; a server has none to print
    crate::ui::set_quiet(true);
    let listener = TcpListener::bind(p.addr).map_err(|e| format!("listen {}: {e}", p.addr))?;
    let banner = format!(
        "wayang api: listening on {} ({}, {}, {})",
        listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default(),
        if p.opts.tokens.is_some() {
            "bearer tokens"
        } else {
            "NO AUTH (loopback dev)"
        },
        match (p.tls, p.mtls) {
            (false, _) => "plain HTTP",
            (true, false) => "TLS",
            (true, true) => "mutual TLS",
        },
        if p.opts.rw {
            "READ-WRITE (admin tokens may update/reset/apply)"
        } else {
            "read-only"
        }
    );
    let server = build(p)?;
    eprintln!("{banner}");
    server.serve(listener);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wayang-api-cmd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn with_token(d: &std::path::Path, scope: Scope) -> PathBuf {
        let f = d.join("tokens");
        auth::gen_token(&f, scope, "t", false).unwrap();
        f
    }

    #[test]
    fn flags_parse_in_both_forms_and_unknown_ones_are_refused() {
        let a = parse_args(&args(
            "--listen=127.0.0.1:9 --rw --rate-burst 5 --token-file /x/t",
        ))
        .unwrap();
        assert_eq!(a.listen.as_deref(), Some("127.0.0.1:9"));
        assert!(a.rw);
        assert_eq!(a.rate_burst, Some(5.0));
        assert_eq!(a.token_file, Some(PathBuf::from("/x/t")));
        let e = parse_args(&args("--commit")).unwrap_err();
        assert!(e.contains("unknown option") && e.contains("--rw"), "{e}");
        assert!(
            parse_args(&args("--listen"))
                .unwrap_err()
                .contains("needs a value")
        );
        assert!(
            parse_args(&args("--rate-burst 0"))
                .unwrap_err()
                .contains(">= 1")
        );
        assert!(
            parse_args(&args("--rate-per-sec x"))
                .unwrap_err()
                .contains("> 0")
        );
        assert!(
            parse_args(&args("--rw --ro"))
                .unwrap_err()
                .contains("contradict")
        );
    }

    #[test]
    fn a_server_without_tokens_does_not_start_and_dev_mode_is_loopback_only() {
        let d = tmp("tok");
        let none = ApiArgs {
            token_file: Some(d.join("missing")),
            ..ApiArgs::default()
        };
        assert!(plan(&none).err().unwrap().contains("no API token"));
        let dev = ApiArgs {
            insecure_no_auth: true,
            token_file: Some(d.join("missing")),
            ..ApiArgs::default()
        };
        let p = plan(&dev).unwrap();
        assert!(p.opts.tokens.is_none() && !p.opts.rw);
        // --insecure-no-auth does not count off loopback
        let off = ApiArgs {
            insecure_no_auth: true,
            insecure_http: true,
            listen: Some("0.0.0.0:8632".into()),
            token_file: Some(d.join("missing")),
            ..ApiArgs::default()
        };
        assert!(plan(&off).err().unwrap().contains("no API token"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rw_needs_a_token_never_the_dev_mode() {
        let d = tmp("rw");
        let dev_rw = ApiArgs {
            rw: true,
            insecure_no_auth: true,
            ..ApiArgs::default()
        };
        assert!(
            plan(&dev_rw)
                .err()
                .unwrap()
                .contains("--rw needs a bearer token")
        );
        let ok = ApiArgs {
            rw: true,
            token_file: Some(with_token(&d, Scope::Admin)),
            ..ApiArgs::default()
        };
        let p = plan(&ok).unwrap();
        assert!(p.opts.rw && p.opts.tokens.is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_non_loopback_bind_needs_tls_or_an_explicit_opt_out() {
        let d = tmp("bind");
        let f = with_token(&d, Scope::Ro);
        let base = ApiArgs {
            listen: Some("0.0.0.0:8632".into()),
            token_file: Some(f),
            ..ApiArgs::default()
        };
        let e = plan(&base).err().unwrap();
        assert!(
            e.contains("needs TLS") && e.contains("--insecure-http"),
            "{e}"
        );
        let plain = ApiArgs {
            insecure_http: true,
            listen: base.listen.clone(),
            token_file: base.token_file.clone(),
            ..ApiArgs::default()
        };
        assert!(plan(&plain).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn tls_flags_are_checked_against_each_other_and_the_files() {
        let d = tmp("tls");
        let f = with_token(&d, Scope::Ro);
        let mut a = ApiArgs {
            token_file: Some(f),
            tls_cert: Some(d.join("c.pem")),
            ..ApiArgs::default()
        };
        assert!(plan(&a).err().unwrap().contains("go together"));
        a.tls_cert = None;
        a.client_ca = Some(d.join("ca.pem"));
        assert!(plan(&a).err().unwrap().contains("needs --tls-cert"));
        a.client_ca = None;
        a.tls_cert = Some(d.join("nope.pem"));
        a.tls_key = Some(d.join("nope.key"));
        assert!(plan(&a).err().unwrap().contains("nope.pem"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rate_flags_set_or_disable_the_limiter() {
        let d = tmp("rate");
        let f = with_token(&d, Scope::Ro);
        let mk = |extra: &str| {
            let mut a = parse_args(&args(extra)).unwrap();
            a.token_file = Some(f.clone());
            plan(&a).unwrap()
        };
        let p = mk("");
        assert_eq!(p.opts.rate.unwrap().burst, RateConfig::DEFAULT.burst);
        let p = mk("--rate-burst 7 --rate-per-sec 2");
        assert_eq!(
            (p.opts.rate.unwrap().burst, p.opts.rate.unwrap().per_sec),
            (7.0, 2.0)
        );
        assert!(mk("--no-rate-limit").opts.rate.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn gen_token_writes_the_requested_scope_and_appends() {
        let d = tmp("gen");
        let f = d.join("api-token");
        let a = parse_args(&args(&format!(
            "--gen-token --token-file {} --scope admin --label ops",
            f.display()
        )))
        .unwrap();
        let t1 = gen_token(&a).unwrap();
        let b = parse_args(&args(&format!(
            "--gen-token --token-file {} --scope ro --label dash --append",
            f.display()
        )))
        .unwrap();
        let t2 = gen_token(&b).unwrap();
        let s = TokenStore::load(&f).unwrap();
        assert_eq!(s.authenticate(&t1).unwrap().scope, Scope::Admin);
        assert_eq!(s.authenticate(&t2).unwrap().label, "dash");
        let bad = parse_args(&args(&format!(
            "--gen-token --token-file {} --scope root",
            f.display()
        )))
        .unwrap();
        assert!(gen_token(&bad).unwrap_err().contains("--scope"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_default_token_file_is_under_wayangi() {
        assert!(default_token_file().ends_with("etc/wayangi/api-token"));
        assert!(audit_file().ends_with("var/wayang/api-audit.jsonl"));
    }
}
