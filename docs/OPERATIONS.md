# Operating OpenScreenTime (day 2)

The day-2 guide: what runs by itself, backups and restores, monitoring and
alerts, recovering access, cleaning up. For the first install see
[`docs/DEPLOY.md`](DEPLOY.md).

Commands run from the checkout on the server. `podman` works as `docker` too;
rootless installs use `systemctl --user` where this says `systemctl`.

## What runs by itself

`deploy/setup.sh` installs these (re-install on an older install with
`deploy/install-auto-update.sh`):

| Unit | Does |
|---|---|
| `openscreentime.service` | starts both containers at boot (Podman; Docker does it itself) |
| `openscreentime-backup.timer` | nightly `deploy/backup.sh nightly` (~03:15) |
| `openscreentime-update.timer` | daily `deploy/update.sh` (~04:30), with rollback |

Check them: `systemctl list-timers 'openscreentime*'`, and what a run did:
`journalctl -u openscreentime-update` / `-u openscreentime-backup`.

## Updating

```sh
deploy/update.sh
```

In order: fast-forward the checkout (a dirty or diverged checkout is reported
and skipped, never fatal); pull `OST_IMAGE` (or build from the checkout if
that fails); stop if that image is already running; back up the database;
swap the server container; wait up to 3 minutes for `/health` (which checks
the database). If it doesn't come up healthy, it puts the previous image back
**and restores the pre-update backup**, remembers the bad image so the next
runs skip it, and records the rollback (your phone hears about it if alerts
are configured). Every run is recorded in the server's `ops_log`.

Pin a version with `OST_IMAGE=ghcr.io/one-more-refactor/openscreentime:<x.y.z>`
in `.env`; `OST_IMAGE=build` always builds on this machine (slow on small
boxes — see `deploy/push-image.sh` below).

**Devices update themselves — from this server.** Each agent checks
`GET /api/agent/latest` about 2 minutes after it starts and then daily, and
installs the build this server bundles when it differs from its own (by a
hash of the agent source, so a fix without a version bump still arrives; it
never downgrades). Before swapping, the new binary must run on the device
(`--version`), which refuses e.g. a build needing a newer glibc. After the
swap, if the new build crash-loops or stops ticking within its first minute,
the device's watchdog puts the previous binary back by itself and skips that
build; the console's event feed shows `agent_updated`,
`agent_update_rolled_back` or `agent_update_refused`. A newly started agent
also refreshes its systemd units if they changed.

To keep one device on its current version:

```sh
# on the device, as root
mkdir -p /etc/systemd/system/openscreentime-agent.service.d
printf '[Service]\nEnvironment=OST_NO_SELF_UPDATE=1\n' \
  > /etc/systemd/system/openscreentime-agent.service.d/no-self-update.conf
systemctl daemon-reload && systemctl restart openscreentime-agent.service
```

## Backup & restore

The durable state is the Postgres volume and `.env` (its `POSTGRES_PASSWORD`
exists nowhere else). Passkeys and device identities live only in the
database.

### Backup

Nightly, automatically: `backups/ost-<UTC time>-nightly.dump` in the checkout
(pg_dump custom format, compressed; 7 kept), plus `backups/env.backup` (the
`.env` it belongs to). Updates add `-pre-update` dumps (5 kept). By hand:

```sh
deploy/backup.sh            # → backups/ost-<time>-manual.dump
```

**Copy `backups/` off the machine** now and then (`scp`, `rsync`, your backup
tool) — a backup on the same disk doesn't survive the disk. If a backup fails,
or none succeeds for two days, the server tells you (see Monitoring).

### Restore

```sh
deploy/restore.sh backups/ost-<time>-<label>.dump
```

It asks for confirmation, stops the server, drops and recreates the database,
restores the dump, starts the server and waits for `/health`.

On a fresh machine: clone, put the old `.env` back (`cp env.backup .env`),
`podman-compose up -d db`, then `deploy/restore.sh <dump>` and
`deploy/setup.sh` (it keeps the `.env` and installs the units).

## Monitoring

**`/health`** (unauthenticated): `200 {"status":"ok","db":"ok","version":…}`
when the server and its database work, `503 {"status":"degraded",
"db":"unreachable"}` when Postgres doesn't answer. The server container's
healthcheck uses it (`podman ps` shows `healthy`), and so do the deploy
scripts. Point an external uptime monitor at `https://<domain>/health` if you
want to hear about the one thing the server can't report itself: being down.

**Phone alerts.** Point OpenScreenTime at a chat channel — a Discord/Slack
incoming webhook (`OST_ALERT_WEBHOOK`) or a Telegram bot
(`OST_TELEGRAM_BOT_TOKEN`, then pair your phone in Settings) — and it sends
short one-way messages for:

- confirmed tamper / device lockdown and new time requests (the household),
- a device that hasn't been in touch for 24 hours (the household),
- server problems: a failed nightly backup or none for 2 days, a failed or
  rolled-back update (or no image pullable for 3 days), the database not
  answering for a few minutes (webhook, plus the paired Telegram chats of
  household owners).

Each problem is sent **once** when it starts (and once more when a server
problem clears), not on every check, and not again after a restart. The
server logs the same notices (`system notice`) even with no channel set up.

**Logs:**

```sh
podman logs -f openscreentime-server
podman logs -f openscreentime-db
```

`RUST_LOG` in `.env` (default `openscreentime_server=info,tower_http=info,info`).

**Device presence.** A device on the WebSocket answers the server's ping
every 6 s; one that goes quiet for 60 s is dropped and shows offline. Polling
devices heartbeat about every 15 s; a sweep every 30 s marks any device
unheard for 90 s offline.

**Events feed is the audit trail.** `GET /api/events` (console: Events page):
enrollment, policy changes, tamper, lock/unlock, self-updates. Pruned after
90 days; usage slices after 21 days.
## Recovering access

**Lost all admin passkeys.** As root inside the server container, run
`podman exec openscreentime-server /app/openscreentime-server recover <name>`
(your name or login name). It prints a one-time sign-in link for your existing
account — single use, 30 minutes — that signs you in with "confirm it's you"
already done; add a new passkey under **Settings → Security & access** right
away. (If one of your computers is set up, typing your name on the sign-in page
and the code it shows works too.)

**Lost the parent PIN.** It's stored per-profile
(`policy.parent_pin_hash`, Argon2-hashed, never returned as plaintext), not
one global secret. Reset it as a logged-in admin: open the profile on the
**Profiles** page and set a new PIN (clearing the field removes the PIN
requirement — `server/src/profiles.rs`). The admin session already proves
access; the PIN itself only gates local, on-device unlock.

## Common failures & fixes

**An update was rolled back.** `deploy/update.sh` already put the previous
version and the pre-update database back; the bad image is skipped until a
newer one appears. See why: `journalctl -u openscreentime-update` (the last
server log lines are in there). To go back further by hand:
`deploy/restore.sh backups/<dump>` restores any backup, and
`podman tag localhost/openscreentime-server:previous localhost/openscreentime-server:current`
followed by `podman-compose up -d` runs the previous image.

**WebAuthn errors (invalid origin, registration/login silently fails).**
`OST_PUBLIC_URL` in `.env` doesn't match what the browser sees — it must be the
exact `https://` origin (with the port if non-standard). Breaks if you changed
the domain, open the console by IP, or sit behind a proxy that rewrites Host.
If you set `RP_ID`/`RP_ORIGIN` explicitly, those win — usually just remove
them. Fix `.env`, then `podman-compose -f compose.yaml up -d`.

**Port conflict on startup.** Something else has `OST_PORT` (default
8080). Change it in `.env`, `up -d`, and repoint your reverse proxy.

**`registration_closed` adding a second admin.** Expected once an account
exists — first run happens once. If you're signed in, add another passkey to
your account under **Settings** instead.

**Rate limiting collapses everyone onto one bucket (mass 429s).** The
limiter keys on the last `X-Forwarded-For` hop only when
`OST_TRUST_PROXY=1` (`server/src/rate_limit.rs`); otherwise it uses the
raw peer address, which behind a reverse proxy is the proxy itself — one
shared bucket for every visitor. `compose.yaml` defaults it to `1` because
this stack only ever sits behind your reverse proxy. If you see mass 429s
anyway, check that `OST_TRUST_PROXY` wasn't overridden to `0` in `.env`
and that your proxy actually appends `X-Forwarded-For` (see DEPLOY.md's
reverse-proxy requirements).

**Stale container/pod name conflicts on Podman.** `podman-compose` names
the pod after the project directory (`pod_openscreentime` for a checkout named
`openscreentime`). A leftover pod from a previous crash/`down` can make `up -d`
refuse with "name already in use":
```sh
podman pod rm -f pod_openscreentime
podman-compose -f compose.yaml up -d
```
Destroys running containers in that pod, not the `ost_pgdata` volume —
data survives.

**Disk filling up.** Each successful update prunes dangling images (old
versions, on-box build layers). To reclaim more: `podman image prune -a`
(keep `localhost/openscreentime-server:current` and `:previous`), and trim
`backups/` if you keep copies elsewhere.

## Uninstalling a device

There's no `ost uninstall` subcommand (`client/src/main.rs` has
only `enroll`, `run`, `install-service`, `status`, `unlock`) — this is the
honest manual path.

**Release enforcement first, if you can.** The agent applies state directly
to the host outside the systemd unit: an `nft` table (`inet openscreentime`) and a
pinned/immutable `/etc/resolv.conf`. Stopping the service does not tear
these down. If you know the parent PIN, run as root on the device:
```sh
ost unlock --pin <PARENT_PIN> --minutes 0
```
`--minutes 0` suspends enforcement with no scheduled re-apply — tears down
the nft table, un-pins `resolv.conf`, un-freezes any frozen users
(`client/src/unlock.rs`). Without the PIN, do the same by hand:
```sh
nft delete table inet openscreentime   # ignore "No such file" if already gone
nft delete table inet sentinel         # only on a box upgraded from the old name
chattr -i /etc/resolv.conf
# then repoint /etc/resolv.conf at whatever resolver the host should use
```

Then remove the agent:
```sh
systemctl disable --now openscreentime-agent.service openscreentime-watchdog.timer
rm -f /etc/systemd/system/openscreentime-agent.service \
      /etc/systemd/system/openscreentime-watchdog.service \
      /etc/systemd/system/openscreentime-watchdog.timer \
      /etc/systemd/user/openscreentime-tray.service \
      /etc/polkit-1/rules.d/49-openscreentime.rules
systemctl daemon-reload
rm -f /usr/local/bin/ost /usr/local/bin/openscreentime.bak
rm -rf /etc/openscreentime /var/lib/openscreentime
```

On a machine that ran the product under its previous name, the installer
retires the old units for you, but a manual uninstall should sweep them too —
leaving `sentinel-agent.service` enabled means a second agent still enforcing:
```sh
systemctl disable --now sentinel-agent.service sentinel-watchdog.timer 2>/dev/null
rm -f /etc/systemd/system/sentinel-agent.service \
      /etc/systemd/system/sentinel-watchdog.service \
      /etc/systemd/system/sentinel-watchdog.timer \
      /etc/systemd/user/sentinel-tray.service \
      /etc/polkit-1/rules.d/49-sentinel.rules \
      /usr/local/bin/sentinel-agent
systemctl daemon-reload
rm -rf /etc/sentinel /var/lib/sentinel
```

Finally, **delete the device in the console** (device detail page, or
`DELETE /api/devices/:id`). The server has no way to know the agent is gone
until you tell it — until then it sits there, eventually swept `offline`
and, after 7 days, flagged gone-dark. Deleting it removes it from the fleet
outright; nothing server-side keeps pointing at that device afterward.

## Port 8080 answers nothing although the container is "Up" (netavark stale rules)

Seen 2026-08-22 on the production LXC: `podman ps` showed the server healthy
and listening, `/health` from the host timed out, and Caddy served 502 for days.
Cause: every `podman-compose down`/`up` cycle created a new compose network
(`podman1`…`podman10`) and, because netavark could not find the previous
network namespace (`failed to Statfs /run/user/0/netns/…`), it never removed
the old port-forward rules. The first matching DNAT rule in
`inet netavark NETAVARK-HOSTPORT-DNAT` still pointed at a container IP that no
longer existed → "No route to host".

Diagnose:

```bash
nft list ruleset | grep -c 'dport 8080'     # healthy = a handful, not 16+
curl -v http://<OST_BIND_ADDR>:8080/health  # "No route to host" = stale DNAT
```

Fix (takes ~20 s, no data touched):

```bash
cd /opt/openscreentime
podman-compose down
nft delete table inet netavark    # netavark rebuilds it for live containers only
podman-compose up -d
```

Avoid: prefer `podman stop/rm openscreentime-server && podman-compose up -d`
over `down` for routine restarts (`deploy/push-image.sh` does this), so the
network — and its rules — stay put.

## Building elsewhere: `deploy/push-image.sh`

When pulling the published image isn't an option and the server is too small
to build (a one-core LXC takes 40+ minutes), `deploy/push-image.sh` builds the
same Containerfile on your dev box, streams the image over SSH (optionally
through `pct exec` for an LXC without sshd), fast-forwards the server's
checkout, and deploys with `deploy/update.sh --image` — the same backup,
health check and rollback as the daily update.

## Login options on the hosted instance

Passkey registration is open only while the instance has zero admins; after
that, sign in with a passkey or with the configured OIDC provider
(`OST_OIDC_*`). On cr3do the provider is Authentik (`login.cr3do.net`,
application `ost`, bound to the `authentik Admins` group) — the first OIDC
login on an empty instance creates the household and its owner.
