# Product API — reference

Concrete reference for the `api` subcommand. Design/roadmap:
[`PRODUCT-API.md`](PRODUCT-API.md). **Status (2026-10-03):** `wayang-router` and
`wayang-fw` implement **P0 (read-only) and P1 (the config lifecycle, `--rw`)**;
the `wayang` CLI has its own API (§`wayang api`, on `master`, built on the shared
`wayang-api` crate with scopes, rate limit, TLS/mTLS and an event stream). Per-product details:
wayang-router `docs/API.md`, wayang-fw `docs/ARCHITECTURE.md` §Management API.

## Start it

```sh
wayang-router api --gen-token                 # writes /data/etc/router/api-token (0600), prints it
wayang-router api                             # 127.0.0.1:8631, token from that file
wayang-router api --listen 127.0.0.1:8631 --token-file /etc/... --ro
```

* Default bind is **loopback**. Any non-loopback bind is an explicit opt-in; the
  server **refuses to start unauthenticated off-loopback**.
* **Read-only is the default.** `--rw` turns the config lifecycle on (see P1
  below); it **requires a token** and is refused together with
  `--insecure-no-auth`. Without `--rw`, every write answers `403 read_only`.
  A mistyped flag is refused, never ignored.
* Token: `Authorization: Bearer <token>` (constant-time compare). **Only**
  `/v1/health` needs no token. No secret is logged.

## Envelope

```json
{ "ok": true,  "data": { … }, "rev": 7, "pending": null }
{ "ok": false, "error": { "code": "validation", "message": "…" } }
```

HTTP: `200` ok · `400` bad request · `401` no/wrong token (`WWW-Authenticate`;
checked before anything else) · `403` `read_only` (write on a read-only server) ·
`404` · `405` wrong method for that path · `409` conflict (engine/caps refusal,
nothing pending to confirm) · `413` body over 1 MiB · `422` validation (with
`issues[]`) · `423` locked (a commit is already pending) · `500`.

## Endpoints — `wayang-router` (P0, implemented)

| Method | Path | Returns |
|---|---|---|
| GET | `/v1/health` | version, phase, `ro`, auth on/off, feature caps — **no auth** |
| GET | `/v1/status` | engine/backend/caps, active rev, pending + deadline, drift, candidate, forwarding, root |
| GET | `/v1/config?format=toml\|json` | the active config (`404` if none; `400` bad format) |
| GET | `/v1/wan` | uplinks (health, role, `default_owner`), `wan_groups` + table |
| GET | `/v1/routes` | kernel routes, tables with a default, forwarding |
| GET | `/v1/wg` | WireGuard interfaces / peers / handshakes (**PSK omitted**) |
| GET | `/v1/bgp` | router id, local AS, BIRD state, neighbours + sessions |
| GET | `/v1/dhcp` | configured pools, live servers/clients |
| GET | `/v1/qos` | shaping units + applied state, shapers, queues |

### Examples

```sh
TOKEN=$(cat /data/etc/router/api-token)

curl -s localhost:8631/v1/health | jq .
curl -s -H "Authorization: Bearer $TOKEN" localhost:8631/v1/status | jq .
curl -s -H "Authorization: Bearer $TOKEN" localhost:8631/v1/wan | jq '.data.default_owner'
curl -s -H "Authorization: Bearer $TOKEN" 'localhost:8631/v1/config?format=json' | jq '.data.system'
```

### P1 — config lifecycle (`--rw`; router **and** fw, implemented)

| Method | Path | Does |
|---|---|---|
| GET | `/v1/candidate` | the candidate as TOML + `changed_lines` |
| PUT | `/v1/candidate` | body = the whole config as TOML. Validated: errors are `422` with `issues[]` and **nothing is stored** |
| POST | `/v1/check` | validate the posted config (on its own) or the candidate: `{valid, errors, warnings, issues}` |
| POST | `/v1/plan` | what a commit would change (router: the steps; fw: the line diff, `?ruleset=1` adds the nftables text). **Applies nothing** |
| POST | `/v1/commit?confirm=SECS&comment=TEXT` | applies the candidate through the CLI's engine path **with the watchdog armed** |
| POST | `/v1/confirm` · `/v1/rollback` | keep / undo the pending commit |
| GET | `/v1/history/{n}[?format=json]` | one revision's config, time and comment |
| GET | `/v1/audit?limit=N` | the audit trail, newest first (default 50, max 500) |

* **The confirm window is mandatory** (anti-lockout): `confirm` defaults to 60
  and must be 10-3600; `confirm=0` / `none` is `400`. There is no immediate
  commit over HTTP. Confirm from a *new* connection, or the box rolls back by
  itself (the watchdog is spawned by the API server).
* **`Idempotency-Key`** (≤128 chars) on a PUT/POST replays the first answer with
  `Idempotent-Replay: true` instead of acting twice (256 keys, in memory).
* **Audit**: every write, accepted or refused, appends one line to
  `…/api-audit.jsonl` (0600): time, method, path, status, peer, revision. Never a
  body, query string or token.
* Staging ≠ committing, and the same refusals as the CLI apply (`409` for the
  engine / caps, `423` for a commit already pending).

```sh
H="Authorization: Bearer $TOKEN"; U=localhost:8630        # wayang-fw (router: :8631)
curl -s -X PUT  -H "$H" --data-binary @site.toml $U/v1/candidate | jq .data.changed_lines
curl -s -X POST -H "$H" $U/v1/plan | jq .data.summary
curl -s -X POST -H "$H" -H 'Idempotency-Key: deploy-42' "$U/v1/commit?confirm=60&comment=deploy+42"
curl -s -X POST -H "$H" $U/v1/confirm            # from a new connection
```

*Still planned:* `/v1/events` (SSE), HTTPS/mTLS, per-token scopes + rate limit,
and the shared `wayang-api` crate (the transport is currently a copy in each
tool). Both servers are single-threaded: a long commit delays the next request.

## Tool-specific reads — *(planned)*

* **`wayang-fw`** (implemented, P0): `/v1/status`, `/v1/config`, `/v1/history`,
  `/v1/stats`, `/v1/logs`, `/v1/caps`, `/v1/waf` (port 8630 by default; the
  per-object `/v1/policies|nat|drops|objects` are still planned).
* **`wayang` (OS)**: implemented — see the next section (`/v1/wifi` is still
  planned).

## `wayang api` — the OS itself

`wayang api` (default `127.0.0.1:8632`, token file
`/data/etc/wayangi/api-token`, audit `/data/var/wayang/api-audit.jsonl`) serves
the CLI's operations. Every handler calls the code the CLI/HUD calls
(`status::gather`, `update::plan`/`stage_plan`, `staging::*`, `edgerouter::*`,
`sshkeys::*`, `net::list`, `reset_config`), so the API does nothing the CLI
cannot. **Nothing here reboots the box**: the API stages and arms; a person (or
an orchestrator that knows the box can be lost for a minute) reboots.

| Method | Path | Scope | Does |
|---|---|---|---|
| GET | `/v1/status` | any | the CLI `status` (slots, versions, fallback state) plus `update_pending`, `uptime_s`, `tools` (fw/router versions) and a `net` summary |
| GET | `/v1/update/check` | any | installed vs the channel's bundle (`available`, `notes`); downloads nothing. `502` when the channel is unreachable |
| GET | `/v1/net` | any | interfaces (link, driver, mac, ipv4, primary), the persisted uplink choice, the also-lease list |
| GET | `/v1/ssh-keys` | any | the authorized keys: kind, **fingerprint**, comment, source — never the key material |
| GET | `/v1/edgerouter/status` | any | the wayangi agent: health + the next step, token present (never the token), tunnel, handshake |
| POST | `/v1/update` | admin | download + verify + **stage** into the idle slot (`wayang update`); `409` when nothing newer, `423` when an update is already staged |
| POST | `/v1/update/boot-other` | admin | arm a one-shot boot of the idle slot (`--boot-other`) |
| POST | `/v1/update/confirm` | admin | `mark-ok` |
| POST | `/v1/update/rollback` | admin | `--rollback`: the next boot uses the previous slot |
| POST | `/v1/reset?confirm=yes-reset` | admin | `wayang reset --yes` (config dirs removed, pending update cleared; `/data/bin` and SSH keys kept). The second key is mandatory |
| POST | `/v1/edgerouter/apply[?force=1]` | admin | body = the Edge bundle as `.tar.gz`, or JSON `{router_toml, fw_toml, token?}`. Writes `/data/etc/{router,fw}/config.toml` and enrols the token; **applies nothing** (`409` when a config exists and `force` is not set; the old one becomes `.bak`) |
| POST | `/v1/ssh-keys` | admin | body = one public key line (validated; `422` if not a key) |
| POST | `/v1/ssh-keys/remove` | admin | body = a fingerprint. Refuses the **last** authorized key (`409`: it would lock SSH out) unless `?allow_empty=yes` |

Everything that changes the box needs scope `admin` **and** `--rw`: an update, a
slot switch, a reset, a bundle and an SSH key all decide who can reach the box.
`rw` tokens do not get these (403 `forbidden`).

## Enabling it on a WayangOS box

A default image listens on **nothing**. Each tool's API starts at boot only when
its `api.args` file exists (`/etc/init.d/api`, started after SSH; stopped on
shutdown):

| Tool | `api.args` | Binary |
|---|---|---|
| wayang-fw | `/data/etc/fw/api.args` | `wayang-fw api` (port 8630) |
| wayang-router | `/data/etc/router/api.args` | `wayang-router api` (port 8631) |
| wayang | `/data/etc/wayangi/api.args` | `wayang api` (port 8632) |

```sh
# 1. a token for the tool (printed once; the file is 0600). Scopes: ro, rw, admin.
wayang-fw api --gen-token --scope admin --label ops           # /data/etc/fw/api-token
wayang-fw api --gen-token --scope ro --label dashboard --append
# 2. the flags, one line (loopback + token is the default and needs none)
echo '--listen 127.0.0.1:8630 --rw' > /data/etc/fw/api.args
# 3. start it now (or reboot)
/etc/init.d/fw-api restart          # also: router-api, wayang-api, or `api` for all
/etc/init.d/api status
```

To reach it from another machine bind a non-loopback address **with TLS**
(`--tls-cert F --tls-key F`, optionally `--client-ca F` for mutual TLS — the
token is still required); plain HTTP off loopback needs an explicit
`--insecure-http`. Generate the certificate elsewhere (`openssl req -x509 …`);
the box does not make one. Logs: `/var/log/<tool>-api.log`. The flag line is
word-split, so quote nothing and keep paths free of spaces.

## Notes on values

* The envelope `rev`/`pending` mirror the revision engine; `pending` is the
  commit awaiting `confirm`.
* `/v1/qos` exposes configured units and applied state but **not** sampled byte
  counters/rates (that sampling sleeps ~1 s; avoided for endpoint latency).
* The transport/auth/audit layer is the shared **`wayang-api`** crate
  (github.com/dalang-io/wayang-api, v0.1.0): token scopes, per-peer rate limiting
  (60 burst / 20 per s; failed token guesses throttled), TLS/mTLS (rustls),
  `GET /v1/events` (server-sent events of every audited write) and `GET /v1/audit`.
  The `wayang` CLI uses it; fw and router are being migrated to it.
