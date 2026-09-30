# Product API — reference

Concrete reference for the `api` subcommand. Design/roadmap:
[`PRODUCT-API.md`](PRODUCT-API.md). **Status:** only **`wayang-router` P0
(read-only)** is implemented today; `wayang-fw` and the `wayang` CLI are planned
(their endpoints below are marked *(planned)*).

## Start it

```sh
wayang-router api --gen-token                 # writes /data/etc/router/api-token (0600), prints it
wayang-router api                             # 127.0.0.1:8631, token from that file
wayang-router api --listen 127.0.0.1:8631 --token-file /etc/... --ro
```

* Default bind is **loopback**. Any non-loopback bind is an explicit opt-in; the
  server **refuses to start unauthenticated off-loopback**.
* `--ro` advertises read-only (P0 has no mutating route regardless; non-`GET`
  returns `405`).
* Token: `Authorization: Bearer <token>` (constant-time compare). **Only**
  `/v1/health` needs no token. No secret is logged.

## Envelope

```json
{ "ok": true,  "data": { … }, "rev": 7, "pending": null }
{ "ok": false, "error": { "code": "validation", "message": "…" } }
```

HTTP: `200` ok · `400` bad request · `401`/`403` auth · `404` · `405`
non-GET · `409` conflict (caps refusal / drift) · `422` validation · `423`
locked (commit pending) · `500`. `401` carries `WWW-Authenticate`.

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

### Planned (P1) — config lifecycle (router, then fw & CLI)

| Method | Path |
|---|---|
| GET | `/v1/candidate` · `PUT /v1/candidate` |
| POST | `/v1/check` · `/v1/plan` · `/v1/commit?confirm=60` · `/v1/confirm` · `/v1/rollback` |
| GET | `/v1/history` · `/v1/history/{n}` · `/v1/events` (SSE) |

Semantics mirror the CLI: staging ≠ committing, commit-confirm preserved, the
same `caps` refusals (returned as `409`).

## Tool-specific reads — *(planned)*

* **`wayang-fw`**: `/v1/policies`, `/v1/nat`, `/v1/drops`, `/v1/logs`,
  `/v1/stats`, `/v1/objects`.
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
