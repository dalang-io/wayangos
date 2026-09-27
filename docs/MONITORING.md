# Persistent firewall / router monitoring

`wayang-fw` and `wayang-router` each ship a `monitor --daemon` subcommand that
appends one JSON object per line (JSON Lines) to a history file on `/data`, so
the metrics of a boot survive a reboot, a freeze and an OS update. This document
covers how WayangOS starts the collectors and reads the history back; the record
schema is owned by each app (see
`dalang-io/wayang-fw` / `dalang-io/wayang-router`).

## Files

```
/data/var/wayang-fw/history.jsonl       # wayang-fw monitor --daemon
/data/var/wayang-router/history.jsonl   # wayang-router monitor --daemon
```

- The directories are created (0755) at every boot, right after `/data` is
  mounted, in `/etc/init.d/rcS`. Because they live on `/data`, the history
  survives A/B updates — an update never touches `/data`.
- The files are append-only JSON Lines: one self-contained JSON object per line,
  so they can be read with plain text tools and never need a database.
- If `/data` is not mounted (no `wayang.data=` or the partition is missing) the
  collectors do not run; nothing is written.

## Starting and stopping

The collectors are started at boot by the same init scripts that apply the
config, and gated on both the binary and a **confirmed** config:

| Init script | Binary | Config gate | Pidfile | Log |
|-------------|--------|-------------|---------|-----|
| `/etc/init.d/fw` | `/data/bin/wayang-fw`, else `/usr/bin/wayang-fw` | `/data/etc/fw/config.toml` | `/var/run/wayang-fw-monitor.pid` | `/var/log/wayang-fw-monitor.log` |
| `/etc/init.d/router` | `/data/bin/wayang-router`, else `/usr/bin/wayang-router` | `/data/etc/router/config.toml` | `/var/run/wayang-router-monitor.pid` | `/var/log/wayang-router-monitor.log` |

- **No-op without the binary or config.** An image that never installed
  `wayang-fw` / `wayang-router` (or has no confirmed config yet) boots unchanged.
- **Never blocks boot.** Each collector is backgrounded; boot and SSH do not wait
  on it. The router collector starts inside the router's existing background
  job, after `wayang-router boot` and `bird`, so interfaces already exist.
- **Idempotent.** `start` checks the pidfile and the live process (`kill -0`)
  before spawning, so a second `start` does not launch a duplicate.
- `stop` only stops the collector — the loaded firewall / applied routing stay
  in effect until power-off. `status` prints an extra `monitor: running (pid N)`
  or `monitor: stopped` line.

## Retention

Each history is capped at its **newest 20 000 records**. At boot, before the
collectors start, `rcS` counts the lines and, if over the cap, rewrites the file
keeping the tail (`tail -n 20000`). That bounds `/data` growth on a long-lived
box while keeping recent history through a reboot. Readers must not assume the
history is unlimited.

## Reading it

There is no `jq` in the image; use BusyBox text tools, or copy the file off the
box (e.g. `scp`) and use whatever JSON tooling you have.

```sh
# newest first, last 20 records
tail -n 20 /data/var/wayang-fw/history.jsonl

# follow live
tail -f /data/var/wayang-router/history.jsonl

# count records / first and last timestamps
wc -l /data/var/wayang-router/history.jsonl
head -n 1 /data/var/wayang-router/history.jsonl
tail -n 1 /data/var/wayang-router/history.jsonl

# grep a field (JSONL is line-oriented, so grep works record by record)
grep '"action":"drop"' /data/var/wayang-fw/history.jsonl | tail -n 50
```

The apps' own HUDs also read this history (the same in-process reader they use
for live views), so the files are the durable backing store behind those views.
