# Product API — reference

Concrete reference for the `api` subcommand. Design/roadmap:
[`PRODUCT-API.md`](PRODUCT-API.md). **Status (2026-10-03):** `wayang-router` and
`wayang-fw` implement **P0 (read-only) and P1 (the config lifecycle, `--rw`)**;
the `wayang` CLI is planned (marked *(planned)*). Per-product details:
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
* **`wayang` (OS)**: `/v1/update/check`, `POST /v1/update` (+`/confirm`),
  `POST /v1/reset`, `/v1/edgerouter/status`, `POST /v1/edgerouter/apply`,
  `/v1/net`, `/v1/wifi`, `/v1/ssh-keys`.

## Notes on values

* The envelope `rev`/`pending` mirror the revision engine; `pending` is the
  commit awaiting `confirm`.
* `/v1/qos` exposes configured units and applied state but **not** sampled byte
  counters/rates (that sampling sleeps ~1 s; avoided for endpoint latency).
* The transport/auth/audit layer is intended to move into a shared
  **`wayang-api`** crate once fw/CLI adopt it (design §7).
