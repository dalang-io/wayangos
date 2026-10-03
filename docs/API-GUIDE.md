# API guide — turning it on and exposing it safely

How an operator enables the product API on a WayangOS box, protects it, and drives it.
Route-by-route reference: [PRODUCT-API-REFERENCE.md](PRODUCT-API-REFERENCE.md), and per tool
wayang-fw `docs/API.md`, wayang-router `docs/API.md`. Design: [PRODUCT-API.md](PRODUCT-API.md).

| Tool | Port | Binary | What it drives |
|---|---|---|---|
| wayang-fw | 8630 | `wayang-fw api` | firewall config lifecycle, stats, logs, WAF |
| wayang-router | 8631 | `wayang-router api` | router config lifecycle, WAN, routes, WireGuard, BGP/OSPF, DHCP, QoS |
| wayang (OS) | 8632 | `wayang api` | slots/updates, reset, Edge bundle, SSH keys, network (changes need `admin`) |

A default image listens on **nothing**. Nothing is reachable until you opt in.

## The model: three locks, all required

1. **Network** — the firewall. The API port is just another port: `wayang-fw` only lets
   tcp/22 reach the box until you add a local-in rule.
2. **Client certificate (mTLS)** — with `--client-ca`, a peer without a certificate from your CA
   gets no HTTP at all, not even a 401.
3. **Bearer token** — still checked on top of the certificate (a stolen certificate alone is not
   enough). Scopes: `ro` (read, `check`, `plan`) < `rw` (also store/commit/confirm/rollback) <
   `admin` (update, reset, SSH keys, bundles; only `wayang api` has these).

And two switches that are independent of the token: the server is **read-only unless started with
`--rw`** (a write is `403 read_only` whatever the token), and every commit needs a **confirm
window** (10–3600 s, default 60): an unconfirmed commit is undone by the watchdog.

The server refuses to start unsafe: off loopback it needs tokens **and** TLS (or an explicit
`--insecure-http`), and `--rw` needs a token.

## 1. Tokens

```sh
# on the box (printed once; the file is 0600; without --append the file is replaced)
wayang-fw api --gen-token --token-file /data/etc/fw/api.tokens --scope rw --label ops
wayang-fw api --gen-token --token-file /data/etc/fw/api.tokens --scope ro --label dashboard --append
```

A token file has one `scope token [label]` line each (a bare token is `rw`). The label is what the
audit trail records as the actor. Do not paste the file's content into logs or tickets: **when you
`cat` or `awk` the file, the tokens print** — print the scope/label columns only. To rotate: generate
a fresh file, restart the API, hand the new token to the clients. There is no expiry and no
rotation protocol yet.

## 2. Certificates (made elsewhere — the box has no `openssl`)

```sh
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout ca.key -out ca.pem -days 3650 -subj "/CN=my API CA"
# server: the SANs must list every address a client uses
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout server.key -out server.csr -subj "/CN=box"
printf 'subjectAltName=IP:163.128.55.4,IP:10.99.130.5,IP:127.0.0.1\nextendedKeyUsage=serverAuth\n' > srv.ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -days 825 -extfile srv.ext
# client
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout client.key -out client.csr -subj "/CN=orchestrator"
printf 'extendedKeyUsage=clientAuth\n' > cli.ext
openssl x509 -req -in client.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out client.pem -days 825 -extfile cli.ext
```

Copy `ca.pem`, `server.pem` and `server.key` (0600) to `/data/etc/api-tls/` on the box
(`ssh host 'cat > FILE' < FILE`; there is no sftp). Keep `ca.key` offline. The server certificate
expires after 825 days and nothing renews it: put the date in your calendar.

## 3. `api.args` — the flags the init hook starts

`/etc/init.d/api` (OS 1.0.33 and later) starts a tool's API at boot only if its `api.args` exists:
`/data/etc/fw/api.args`, `/data/etc/router/api.args`, `/data/etc/wayangi/api.args` (for `wayang`).
One line of flags, split on spaces (no quotes, no spaces in paths):

```
--listen 0.0.0.0:8630 --token-file /data/etc/fw/api.tokens --tls-cert /data/etc/api-tls/server.pem --tls-key /data/etc/api-tls/server.key --client-ca /data/etc/api-tls/ca.pem
```

Add `--rw` to allow changes. Bind `0.0.0.0` (not a tunnel address) if the interface may come up after
the API at boot, and let the firewall decide who gets in.

```sh
/etc/init.d/fw-api restart        # also router-api, wayang-api, or `api` for all; /etc/init.d/api status
# logs: /var/log/<tool>-api.log   audit: $WAYANG_FW_VAR/api-audit.jsonl (and the router/wayang equivalents)
```

A binary in `/data/bin` wins over the one in `/usr/bin`: a stale `/data/bin/wayang` hides the OS's
`wayang api` — remove it.

## 4. The firewall rule (a commit: use a confirm window)

A local-in policy in `wayang-fw`'s config (`to = "self"`), limited to where your orchestrator really
comes from. Example for a box whose management traffic arrives through the WireGuard zone:

```toml
[[service]]
name = "wayang-api"
tcp = ["8630", "8631", "8632"]
udp = []
icmp = false

[[policy]]
name = "api-from-orchestrator"
enabled = true
from = "tunnel"
to = "self"
source = ["orchestrator-net"]      # an [[address]] object: never "any" on a WAN zone
destination = ["any"]
service = ["wayang-api"]
action = "accept"
log = true
description = "product API (mTLS + token)"
```

Commit it with `wayang-fw commit --confirm 120` (or through the API itself, in this order: PUT,
plan, commit with a window, confirm from a new connection). Never open the API to `any` on a WAN
zone: even with mTLS, every exposed port is attack surface and a lockout of one NAT'd client after 5
wrong tokens affects everyone behind it.

## 5. Use it

```sh
# curl
curl --cacert ca.pem --cert client.pem --key client.key -H "Authorization: Bearer $(cat fw.ro.token)" \
     https://163.128.55.4:8630/v1/status

# wayangi's client: check → store → commit → reach the box on a NEW connection → confirm
wayangi-boxapi --url https://163.128.55.4:8630 --token-file fw.rw.token \
    --ca ca.pem --cert client.pem --key client.key apply fw.toml --confirm 60
```

`wayangi-boxapi` exit codes: 0 ok · 1 usage/transport · 2 refused (422 validation or 403) · 3 not
confirmed (the watchdog will undo it). Other commands: `health status config candidate check plan
confirm rollback history audit events`. The Go package is `internal/productapi` (wayangi);
the OpenAPI files (`wayang-fw/docs/openapi.json`, `wayang-router/docs/openapi.json`,
`docs/openapi-wayang.json`) are for any other client.

Watch changes live: `GET /v1/events` (server-sent events of the audit trail, resumable with
`Last-Event-ID`).

## Status codes you will meet

`401` no/wrong token · `403 forbidden` token scope too low · `403 read_only` server started without
`--rw` · `409` engine refusal / nothing pending · `413` body > 1 MiB · `422` validation (see
`issues[]`) · `423` a commit is already pending · `429` rate limit or throttled tokens (wait
`Retry-After`) · `503` too many connections (32).

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| connection hangs / times out | the firewall (step 4) or the API is not running (`/etc/init.d/api status`) |
| TLS handshake fails immediately | no/other-CA client certificate, or the address is not in the server certificate's SANs |
| `401` | wrong token, or the scope line is missing from the file |
| `403 read_only` | start the server with `--rw` (and a token) |
| `429` for everyone | five wrong tokens from one address lock that address out for a while (documented) |
| `wayang api` unknown | an old `/data/bin/wayang` shadows the OS one |
| the server will not start | the message says why (non-loopback without TLS, `--rw` without a token, an unknown flag) |

## Current state (2026-10-03)

Verified in QEMU/netns labs against the real binaries (all three locks, the whole lifecycle, the
watchdog, SSE). **Not yet run with `--rw` on a physical device.** On the test box `163.128.55.4` the
files for fw and router exist (`api.args`, `api.tokens`, `/data/etc/api-tls/`) but nothing is
started and the firewall does not allow the ports; the box needs a reboot into OS 1.0.33 for the
init hook. Missing: token expiry/rotation, certificate → scope mapping, certificate renewal, the
wayangi dashboard calling the API.
