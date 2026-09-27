# Update channel

How the online update channel is laid out, served, consumed by devices, and
published — by hand or from CI. The frozen interface lives in
[`UPDATE-DESIGN.md`](UPDATE-DESIGN.md) ("Channel publishing layout"); the
operator guide is [`UPDATE.md`](UPDATE.md).

## Layout

`wayang update` fetches two files over HTTPS:

```
<base>/<channel>/<arch>/manifest.json
<base>/<channel>/<arch>/wayang-<version>-<arch>.wup
```

- `<base>` defaults to `https://wayang.dalang.io/channel` (override with
  `WAYANG_REPO_URL`). See `wayang/src/fetch.rs` (`DEFAULT_BASE`).
- `<channel>` is `stable` (default) or `edge`, from `/etc/wayang/channel` or
  `--channel`.
- `<arch>` is `x86_64` or `arm64`.
- `manifest.json` has the same schema as the manifest inside a bundle
  (`UPDATE-DESIGN.md`). The ed25519 signature is **inside** the `.wup`
  (`manifest.json.sig`), so only these two files need to be served.

On the deploy host the base maps to a served directory:

```
/root/wayang.dalang.io/public/channel/stable/x86_64/manifest.json
/root/wayang.dalang.io/public/channel/stable/x86_64/wayang-1.0.17-x86_64.wup
```

The tree is **append/versioned**: every published version's `.wup` is kept and
`manifest.json` is overwritten to point at the newest. Publishing therefore
never uses `rsync --delete` — that would wipe older versions and other
channels.

`scripts/release-wayang.sh` emits this tree under `dist/channel/`; the CI job
builds the same tree under `wayangos-build/channel/`.

## How a device consumes it

`wayang update` (or `--check` to only report):

1. reads the channel (`--channel` → `/etc/wayang/channel` → `stable`);
2. GETs `<base>/<channel>/<arch>/manifest.json`;
3. if a newer compatible version exists, GETs the `.wup`;
4. verifies the sha256 of `vmlinuz`/`initramfs.img` and the manifest signature
   against `/etc/wayang/trusted_keys`, then stages it to the idle A/B slot.

Exit codes: `0` ok · `1` error · `2` no update · `3` verify/signature failure ·
`4` incompatible (arch/`min_from`).

## Manual publish

`scripts/publish-channel.sh` rsyncs a local channel tree to the deploy host.

```sh
# build the tree first (scripts/release-wayang.sh writes dist/channel/)
WAYANG_VERSION=1.0.17 WAYANG_KEY=~/wayang-keys/release.key \
    ./scripts/release-wayang.sh --channel stable

# preview exactly what would be transferred — NO change on the remote
./scripts/publish-channel.sh --dry-run dist/channel

# publish (default source: dist/channel)
./scripts/publish-channel.sh
```

`--dry-run` runs rsync with `-n --itemize-changes --stats`, so it contacts the
host read-only to compare but never creates the remote directory and never
writes. Drop `--dry-run` to publish.

Environment (all optional; defaults preserve the historical behaviour). The
old names `HOST`, `REMOTE_DIR`, `CHANNEL_SUBDIR` are still accepted as
fallbacks:

| Variable | Default | Meaning |
|----------|---------|---------|
| `WAYANG_DEPLOY_HOST` | `root@10.0.0.251` | ssh target of the deploy host |
| `WAYANG_DEPLOY_REMOTE_DIR` | `/root/wayang.dalang.io/public` | served directory |
| `WAYANG_DEPLOY_CHANNEL_SUBDIR` | `channel` | subdir under it (the URL base) |
| `WAYANG_DEPLOY_SSH_KEY` | *(unset)* | path to a private key; when set, ssh/rsync use it with `IdentitiesOnly` + `StrictHostKeyChecking=accept-new` |
| `WAYANG_DEPLOY_SSH_PORT` | *(unset)* | non-default ssh port |

Verify after publishing:

```sh
curl -fsSL https://wayang.dalang.io/channel/stable/x86_64/manifest.json
```

## CI path

`.github/workflows/installer-iso.yml` has a `publish-channel` job that runs
**after** the ISO and signed `.wup` are built:

- on a `v*` tag it publishes the tree produced by that run (downloaded from the
  run's artifact);
- on a manual `workflow_dispatch` it can **(re)publish an already-released
  version** without rebuilding: set the `publish_channel_version` input (e.g.
  `1.0.17`). The job re-downloads `manifest.json` and
  `wayang-<version>-x86_64.wup` from that GitHub release, reshapes them into
  the channel layout, and rsyncs them.

The job is `continue-on-error`, and when the secrets are absent it exits 0 with
a `::notice::` telling you to publish by hand — so channel publishing can never
fail or block a release.

`.github/workflows/release.yml` (a tag-triggered dry-run validator) shellchecks
and `bash -n`s `scripts/publish-channel.sh`; it does not publish.

### Required repository secrets

| Secret | Required | Purpose |
|--------|----------|---------|
| `WAYANG_DEPLOY_HOST` | yes | ssh target, e.g. `root@10.0.0.251` |
| `WAYANG_DEPLOY_KEY` | yes | full OpenSSH **private** key whose public half is authorised on the deploy host |
| `WAYANG_DEPLOY_REMOTE_DIR` | no | overrides the served dir |
| `WAYANG_DEPLOY_CHANNEL_SUBDIR` | no | overrides the URL-base subdir |

Publishing stays dormant until `WAYANG_DEPLOY_HOST` and `WAYANG_DEPLOY_KEY`
both exist.

### One-time setup

```sh
# 1. dedicated deploy key (on the deploy host, or locally)
ssh-keygen -t ed25519 -f ~/.ssh/wayangos-ci -N ''

# 2. authorise the public half on the host that serves the channel
cat ~/.ssh/wayangos-ci.pub >> ~/.ssh/authorized_keys   # run on the host

# 3. store the PRIVATE key file's full contents as a repository secret
#    (Settings → Secrets and variables → Actions):
#      WAYANG_DEPLOY_KEY  = contents of ~/.ssh/wayangos-ci
#      WAYANG_DEPLOY_HOST = root@<deploy-host>

# 4. verify: run the workflow manually with
#      publish_channel_version = <current version>
#    and confirm the job succeeds and the manifest URL updates.
```

The script trusts the host key on first connect (`StrictHostKeyChecking=
accept-new`). For a hardened setup, pin the host key in the runner's
`known_hosts` instead.

## Troubleshooting

- **Job says `::notice:: ... skipping channel publish`** — `WAYANG_DEPLOY_HOST`
  and/or `WAYANG_DEPLOY_KEY` are not set, or the job ran on a non-tag,
  non-republish trigger. Publish by hand.
- **`--dry-run` shows `Number of regular files transferred: 0`** — the remote is
  already in sync; nothing would change.
- **Manifest not updating after a publish** — confirm the served directory has
  the new `manifest.json` (the file is overwritten in place; a CDN cache may
  need a purge).
