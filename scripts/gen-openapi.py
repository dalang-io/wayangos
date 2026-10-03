#!/usr/bin/env python3
"""Generates docs/openapi.json for wayang-fw and wayang-router and docs/openapi-wayang.json for wayang (OS).

Usage (from the directory holding the three repos):
  python3 wayangos/scripts/gen-openapi.py fw     wayang-fw/docs/openapi.json
  python3 wayangos/scripts/gen-openapi.py router wayang-router/docs/openapi.json
  python3 wayangos/scripts/gen-openapi.py wayang wayangos/docs/openapi-wayang.json

Each tool has a unit test that fails when its route table and its file disagree
(method, path, required scope); the response schemas of the shared contract
(candidate, check, commit, confirm, rollback, history, audit) are defined once here.
"""
import json, sys

def ref(n): return {"$ref": f"#/components/schemas/{n}"}
def obj(props, required=None, extra=False, desc=None):
    o = {"type": "object", "properties": props, "additionalProperties": extra}
    if required: o["required"] = required
    if desc: o["description"] = desc
    return o
def free(desc): return {"type": "object", "additionalProperties": True, "description": desc}
I, S, B = {"type": "integer"}, {"type": "string"}, {"type": "boolean"}
NI = {"type": ["integer", "null"]}

def ok(data):
    return obj({"ok": {"const": True}, "data": data,
                "rev": {**NI, "description": "the active revision"},
                "pending": {**NI, "description": "the revision awaiting confirmation, if any"}},
               ["ok", "data"])

COMMON_SCHEMAS = {
    "Issue": obj({"level": {"enum": ["error", "warning"]}, "what": S, "msg": S}, ["level", "what", "msg"]),
    "Error": obj({"ok": {"const": False},
                  "error": obj({"code": S, "message": S, "issues": {"type": "array", "items": ref("Issue")}},
                               ["code", "message"])}, ["ok", "error"]),
    "Health": obj({"version": S, "phase": S, "rw": B, "tls": B, "mtls": B}, ["version"], True,
                  "Harmless liveness data; tools add their own fields."),
    "AuditEntry": obj({"ts": I, "method": S, "path": S, "status": I, "ok": B, "remote": S,
                       "actor": {**S, "description": "the token's label"},
                       "scope": {"enum": ["ro", "rw", "admin"]}, "key": {"type": ["string", "null"]},
                       "rev": NI}, ["ts", "method", "path", "status"]),
    "Revision": obj({"rev": I, "time": S, "comment": S, "rolled_back": B, "active": B}, ["rev"]),
}
LIFECYCLE_SCHEMAS = {
    "Candidate": obj({"config": {**S, "description": "the config as TOML"}, "has_candidate": B,
                      "changed_lines": I, "summary": {**S, "description": "what changed, by kind: `+1 zone · ~2 policy`"}},
                     ["config", "has_candidate", "changed_lines"]),
    "PutResult": obj({"stored": {"const": True}, "changed_lines": I, "summary": S,
                      "issues": {"type": "array", "items": ref("Issue"), "description": "warnings"}},
                     ["stored", "changed_lines"]),
    "CheckResult": obj({"valid": B, "errors": I, "warnings": I, "issues": {"type": "array", "items": ref("Issue")}},
                       ["valid", "errors", "warnings", "issues"]),
    "Pending": obj({"rev": I, "deadline": I, "seconds_left": I}, ["rev", "deadline", "seconds_left"]),
    "CommitResult": obj({"committed": I, "confirm_within": I, "comment": S, "pending": ref("Pending"), "next": S},
                        ["committed", "confirm_within", "pending"]),
    "ConfirmResult": obj({"confirmed": I}, ["confirmed"]),
    "RollbackResult": obj({"rolled_back": {"const": True}, "message": S}, ["rolled_back"]),
    "HistoryList": obj({"revisions": {"type": "array", "items": ref("Revision")}, "active_rev": NI}, ["revisions"]),
    "HistoryRev": obj({"rev": I, "time": S, "comment": S, "rolled_back": B, "active": B,
                       "config": {"description": "TOML text, or an object with ?format=json"}}, ["rev", "config"], True),
    "AuditList": obj({"entries": {"type": "array", "items": ref("AuditEntry")}}, ["entries"]),
}

ERR_CODES = {
    "400": "bad request (a malformed body, a missing or forbidden confirm window, a bad parameter)",
    "401": "no or wrong bearer token (also: with mutual TLS the certificate alone is not enough)",
    "403": "`read_only` (the server was not started with --rw) or `forbidden` (the token's scope is too low)",
    "404": "no such route or revision",
    "405": "wrong method for this path (see `Allow`)",
    "409": "the engine refused (no nft, a failed apply, nothing pending to confirm); the reason is in plain words",
    "413": "request body over 1 MiB",
    "422": "validation: `error.issues` lists what is wrong; nothing was stored or applied",
    "423": "locked: a commit is already waiting for confirmation",
    "429": "rate limited, or too many failed tokens from this address (`Retry-After`)",
    "502": "the update channel is unreachable",
    "503": "too many connections or event streams",
}

def responses(codes, ok_schema=None, ctype="application/json"):
    r = {}
    if ok_schema is not None:
        r["200"] = {"description": "OK", "content": {ctype: {"schema": ok_schema}}}
    for c in codes:
        r[c] = {"$ref": f"#/components/responses/E{c}"}
    return r

def op(summary, scope, ok_schema, codes, desc=None, params=None, body=None, rw=False, idem=False, tag="read"):
    o = {"summary": summary, "tags": [tag], "x-wayang-scope": scope,
         "security": [{"bearer": []}], "responses": responses(["401", "429"] + codes, ok_schema)}
    if rw:
        o["x-wayang-requires-rw"] = True
    if desc: o["description"] = desc
    ps = list(params or [])
    if idem:
        ps.append({"name": "Idempotency-Key", "in": "header", "required": False, "schema": {"type": "string", "maxLength": 128},
                   "description": "a retry with the same key replays the first answer (`Idempotent-Replay: true`) instead of acting twice"})
    if ps: o["parameters"] = ps
    if body: o["requestBody"] = body
    return o

def q(name, schema, desc, req=False): return {"name": name, "in": "query", "required": req, "schema": schema, "description": desc}
TOML_BODY = {"required": False, "content": {"text/plain": {"schema": S, "description": "the config as TOML"}}}
FMT = q("format", {"enum": ["toml", "json"]}, "toml (default) or json")

def shared_paths(extra_health=None):
    return {
        "/v1/health": {"get": {"summary": "Liveness", "tags": ["read"], "x-wayang-scope": "none", "security": [],
                               "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": ok(ref("Health"))}}},
                                             "429": {"$ref": "#/components/responses/E429"}}}},
        "/v1/audit": {"get": op("The audit trail, newest first", "ro", ok(ref("AuditList")), ["400"],
                                params=[q("limit", {"type": "integer", "minimum": 1, "maximum": 500, "default": 50}, "how many entries")],
                                desc="One line per mutating request, accepted or refused: method, path, status, peer, token label and scope, Idempotency-Key, revision. Never a body, a query string or a token.")},
        "/v1/events": {"get": {"summary": "Server-sent events: the audit trail as it happens", "tags": ["read"],
                               "x-wayang-scope": "ro", "security": [{"bearer": []}],
                               "parameters": [{"name": "Last-Event-ID", "in": "header", "required": False, "schema": S,
                                               "description": "resume after this event id (the last 256 are kept); without it the stream starts at now"}],
                               "description": "`id:` numbered `event: audit` frames whose `data:` is an AuditEntry, and a `: keepalive` comment every 15 s. At most 8 concurrent streams (`503` beyond).",
                               "responses": {"200": {"description": "text/event-stream", "content": {"text/event-stream": {"schema": S}}},
                                             "401": {"$ref": "#/components/responses/E401"}, "429": {"$ref": "#/components/responses/E429"},
                                             "503": {"$ref": "#/components/responses/E503"}}}},
    }

def lifecycle_paths(plan_schema, plan_desc):
    return {
        "/v1/candidate": {
            "get": op("The stored candidate", "ro", ok(ref("Candidate")), ["409"], tag="config"),
            "put": op("Store a whole config as the candidate", "rw", ok(ref("PutResult")), ["400", "403", "413", "422"],
                      desc="Body = the config as TOML. Validated like the CLI: errors are 422 with `issues[]` and nothing is stored.",
                      body={"required": True, "content": {"text/plain": {"schema": S}}}, rw=True, idem=True, tag="config"),
        },
        "/v1/check": {"post": op("Validate a config (or the candidate)", "ro", ok(ref("CheckResult")), ["400", "413", "422"],
                                 desc="Optional TOML body, else the candidate. An invalid config is a *result* (200, `valid: false`). Applies nothing.",
                                 body=TOML_BODY, tag="config")},
        "/v1/plan": {"post": op("What a commit would change", "ro", ok(plan_schema), ["400", "413"],
                                desc=plan_desc, body=TOML_BODY, tag="config")},
        "/v1/commit": {"post": op("Apply the candidate with the watchdog armed", "rw", ok(ref("CommitResult")), ["400", "403", "409", "422", "423"],
                                  desc="The confirm window is **mandatory** (anti-lockout): `confirm` defaults to 60 and must be 10-3600; `confirm=0`/`none` is 400. Confirm from a *new* connection, or the box undoes the change by itself.",
                                  params=[q("confirm", {"type": "integer", "minimum": 10, "maximum": 3600, "default": 60}, "seconds"),
                                          q("comment", S, "the history comment")],
                                  rw=True, idem=True, tag="config")},
        "/v1/confirm": {"post": op("Keep the pending commit", "rw", ok(ref("ConfirmResult")), ["403", "409"], rw=True, idem=True, tag="config")},
        "/v1/rollback": {"post": op("Undo the pending commit now", "rw", ok(ref("RollbackResult")), ["403", "409"], rw=True, idem=True, tag="config")},
        "/v1/history": {"get": op("Committed revisions, newest first", "ro", ok(ref("HistoryList")), [], tag="config")},
        "/v1/history/{n}": {"get": op("One revision's config", "ro", ok(ref("HistoryRev")), ["400", "404"],
                                      params=[{"name": "n", "in": "path", "required": True, "schema": I}, FMT], tag="config")},
    }

def doc(title, version, port, desc, schemas, paths, tags):
    return {
        "openapi": "3.1.0",
        "info": {"title": title, "version": version, "description": desc, "license": {"name": "MIT"}},
        "servers": [{"url": f"http://127.0.0.1:{port}", "description": "loopback (the default bind); non-loopback needs TLS or --insecure-http"}],
        "tags": tags,
        "security": [{"bearer": []}],
        "paths": paths,
        "components": {
            "securitySchemes": {"bearer": {"type": "http", "scheme": "bearer",
                                           "description": "Tokens have a scope: `ro` reads (and may check/plan), `rw` may also change the configuration on a server started with --rw, `admin` may also do the sensitive actions a tool marks. With mutual TLS the client certificate is required **and** the token is still checked."}},
            "schemas": schemas,
            "responses": {f"E{c}": {"description": d, "content": {"application/json": {"schema": ref("Error")}}} for c, d in ERR_CODES.items()},
        },
        "x-wayang-contract": "1",
    }

TAGS = [{"name": "read", "description": "read-only"}, {"name": "config", "description": "the config lifecycle: candidate → check → plan → commit (watchdog) → confirm"}]

# ------------------------------------------------------------------ wayang-fw
def fw():
    schemas = {**COMMON_SCHEMAS, **LIFECYCLE_SCHEMAS,
               "FwPlan": obj({"valid": B, "changed_lines": I, "summary": S,
                              "diff": {"type": "array", "items": obj({"op": {"enum": ["add", "del"]}, "line": S}, ["op", "line"])},
                              "diff_truncated": B, "issues": {"type": "array", "items": ref("Issue")},
                              "confirm": obj({"default": I, "min": I, "max": I}),
                              "ruleset": {"type": ["string", "object"], "description": "the nftables text (only with ?ruleset=1), or {error}"}},
                             ["valid", "changed_lines", "issues"], True)}
    p = {**shared_paths(),
         **lifecycle_paths(ref("FwPlan"), "Optional TOML body, else the candidate: the line diff against the active config, a summary by kind, and with `?ruleset=1` the nftables ruleset it would load. Applies nothing."),
         "/v1/status": {"get": op("The `status` command plus host, conntrack, interfaces", "ro", ok(free("engine, active/loaded revision, drift, pending, candidate, interfaces…")), [])},
         "/v1/config": {"get": op("The confirmed config", "ro", ok(obj({"format": S, "config": {}}, ["format", "config"], True)), ["400", "404"], params=[FMT])},
         "/v1/stats": {"get": op("Per-policy hits, bytes and last hit (the `wfw:` nft counters)", "ro", ok(free("policies with counters, implicit deny, DoS")), [])},
         "/v1/logs": {"get": op("The traffic log with FortiView-style tops", "ro", ok(free("entries and tops")), ["400"],
                                params=[q("since", S, "e.g. 15m"), q("policy", S, "policy id"), q("limit", {"type": "integer", "maximum": 500, "default": 50}, "lines")])},
         "/v1/caps": {"get": op("What this box can apply (nft, dnsmasq, agent)", "ro", ok(free("capabilities")), [])},
         "/v1/waf": {"get": op("The WAF's runtime view", "ro", ok(free("state, mode, vhosts, counters, the access ring")), [])}}
    return doc("wayang-fw management API", "0.7.0", 8630,
               "The firewall's operations as JSON. Read-only unless the server runs with `--rw`. Transport (HTTP, scopes, rate limit, TLS/mTLS, idempotency, audit, SSE) is the shared crate `wayang-api`.",
               schemas, p, TAGS)

# ------------------------------------------------------------------ wayang-router
def router():
    schemas = {**COMMON_SCHEMAS, **LIFECYCLE_SCHEMAS,
               "RouterPlan": obj({"valid": B, "changed_lines": I, "summary": S, "steps": {"type": "array", "items": S},
                                  "nothing_to_do": B, "missing_tools": {"type": "array", "items": S},
                                  "cannot_apply": {"type": "array", "items": ref("Issue")},
                                  "diff": obj({"changed_lines": I, "added": I, "removed": I}),
                                  "issues": {"type": "array", "items": ref("Issue")}},
                                 ["valid", "changed_lines", "steps", "issues"], True)}
    gen = lambda s, d: {"get": op(s, "ro", ok(free(d)), [])}
    p = {**shared_paths(),
         **lifecycle_paths(ref("RouterPlan"), "Optional TOML body, else the candidate: the steps a commit would run (the CLI's `plan`), a diff summary, missing tools and what this kernel cannot do. An invalid config is 200 with `valid: false` and no steps. Applies nothing."),
         "/v1/status": gen("Engine, backend, features, active revision, drift", "status"),
         "/v1/config": {"get": op("The confirmed config", "ro", ok(obj({"format": S, "config": {}}, ["format", "config"], True)), ["400", "404"], params=[FMT])},
         "/v1/wan": gen("Uplinks with health, the default owner, wan groups", "uplinks"),
         "/v1/routes": gen("Kernel routes and tables", "routes"),
         "/v1/wg": gen("WireGuard interfaces, peers and handshakes (never keys)", "tunnels"),
         "/v1/bgp": gen("Router id, local AS, BIRD state, neighbours and sessions", "BGP/OSPF"),
         "/v1/ospf": {"get": op("OSPF neighbours / interfaces / topology / state (BIRD)", "ro", ok(free("BIRD's answer")), ["400", "409"],
                                params=[q("view", {"enum": ["neighbors", "interfaces", "topology", "state"]}, "which view (default neighbors)")])},
         "/v1/dhcp": gen("Configured pools and the live servers/clients", "dhcp"),
         "/v1/qos": gen("Shaping units with their applied state", "qos")}
    return doc("wayang-router management API", "0.8.0", 8631,
               "The router's operations as JSON. Read-only unless the server runs with `--rw`. Transport is the shared crate `wayang-api`.",
               schemas, p, TAGS)

# ------------------------------------------------------------------ wayang (OS)
def wayang():
    schemas = {**COMMON_SCHEMAS, "AuditList": LIFECYCLE_SCHEMAS["AuditList"]}
    gen = lambda s, d, c=None: {"get": op(s, "ro", ok(free(d)), c or [])}
    adm = lambda s, d, c, **k: op(s, "admin", ok(free(d)), ["403"] + c, rw=True, tag="actions", **k)
    p = {**shared_paths(),
         "/v1/status": gen("Slots, versions, update state, uptime, the tools' versions, a network summary", "status"),
         "/v1/update/check": gen("Installed vs the channel's bundle (downloads nothing)", "available, notes", ["502"]),
         "/v1/net": gen("Interfaces, the uplink choice, the also-lease list", "net"),
         "/v1/ssh-keys": {"get": op("Authorized keys: kind, fingerprint, comment — never the key", "ro", ok(free("keys")), []),
                          "post": adm("Add one public key", "the added key", ["422"], body={"required": True, "content": {"text/plain": {"schema": S}}}, idem=True)},
         "/v1/ssh-keys/remove": {"post": adm("Remove a key by fingerprint (never the last one)", "removed", ["404", "409"],
                                             params=[q("allow_empty", {"enum": ["yes"]}, "remove the last key too (can lock SSH out)")],
                                             body={"required": True, "content": {"text/plain": {"schema": S}}}, idem=True)},
         "/v1/edgerouter/status": gen("The wayangi agent's health and next step", "agent"),
         "/v1/edgerouter/apply": {"post": adm("Install an Edge bundle (applies nothing)", "installed paths", ["400", "409", "413"],
                                              desc="Body = the bundle as .tar.gz, or JSON {router_toml, fw_toml, token?}. Writes the configs and enrols the token; the old config becomes .bak. Commit with confirm afterwards.",
                                              params=[q("force", {"enum": ["1"]}, "replace an existing config")], body={"required": True, "content": {"application/octet-stream": {"schema": {"type": "string", "format": "binary"}}, "application/json": {"schema": obj({"router_toml": S, "fw_toml": S, "token": S}, ["router_toml", "fw_toml"])}}}, idem=True)},
         "/v1/update": {"post": adm("Download, verify and stage an update into the idle slot (never reboots)", "staged version", ["409", "423", "502"], idem=True)},
         "/v1/update/boot-other": {"post": adm("Arm a one-shot boot of the idle slot", "armed", ["409"], idem=True)},
         "/v1/update/confirm": {"post": adm("Mark the running slot good (`mark-ok`)", "confirmed", ["409"], idem=True)},
         "/v1/update/rollback": {"post": adm("The next boot uses the previous slot", "armed", ["409"], idem=True)},
         "/v1/reset": {"post": adm("Reset the config dirs to defaults (`wayang reset --yes`)", "reset", ["400"],
                                   params=[q("confirm", {"enum": ["yes-reset"]}, "the second key: mandatory", True)], idem=True)}}
    t = [{"name": "read", "description": "read-only"}, {"name": "actions", "description": "sensitive actions: scope admin on a --rw server; nothing here reboots the box"}]
    return doc("wayang (WayangOS) management API", "0.1.0", 8632,
               "The OS CLI's operations as JSON. Everything that changes the box needs scope `admin` **and** `--rw`. Transport is the shared crate `wayang-api`.",
               schemas, p, t)

if __name__ == "__main__":
    which, out = sys.argv[1], sys.argv[2]
    d = {"fw": fw, "router": router, "wayang": wayang}[which]()
    open(out, "w").write(json.dumps(d, indent=2, ensure_ascii=False) + "\n")
    n = sum(len(v) for v in d["paths"].values())
    print(f"{which}: {len(d['paths'])} paths, {n} operations -> {out}")
