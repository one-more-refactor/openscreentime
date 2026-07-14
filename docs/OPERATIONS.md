# Operating Sentinel (day 2)

This is the day-2 operator guide: updating, backup/restore, monitoring,
recovering access, and cleaning up. For first-time install (reverse proxy,
`.env`, first admin, enrolling devices), see
[`docs/DEPLOY.md`](DEPLOY.md) — this assumes that's done and doesn't repeat it.

Commands below assume rootless Podman with `podman-compose`, run from the
repo root on the VPS. Substitute `podman compose` or `docker compose` if
that's what you run instead.

## Updating

```sh
cd sentinel
deploy/update.sh
```

This does, in order: `git pull --ff-only` (fails on a dirty or diverged
checkout — resolve that first), rebuilds the `server` image, recreates the
`server` container (`db` is untouched, no downtime there), then polls
`GET /health` for up to 90s and exits non-zero with a log hint if it never
comes back healthy. Migrations run automatically on server startup
(`db::migrate` in `server/src/main.rs`) — no separate step.

**Devices update themselves.** `deploy/update.sh` only touches the server.
Enrolled agents check `GET /api/agent/latest` ~2 minutes after startup and
then daily, self-updating if the server has a newer version
(`client/src/update.rs`). Expect the fleet to catch up over the next day,
not instantly.

To pin one device to its current version (skip a bad release):

```sh
# on the device, as root
mkdir -p /etc/systemd/system/sentinel-agent.service.d
printf '[Service]\nEnvironment=SENTINEL_NO_SELF_UPDATE=1\n' \
  > /etc/systemd/system/sentinel-agent.service.d/no-self-update.conf
systemctl daemon-reload && systemctl restart sentinel-agent.service
```

Remove the drop-in and restart to resume. This only stops future
self-updates — it doesn't roll back a bad one already installed. The agent
keeps the previous binary as a backup for exactly that case:

```sh
systemctl stop sentinel-agent.service
mv /usr/local/bin/sentinel-agent.bak /usr/local/bin/sentinel-agent
systemctl start sentinel-agent.service
```

## Backup & restore

Two things are all the durable state: the Postgres volume
(`sentinel_pgdata`) and `.env`. Everything else rebuilds from git +
`Containerfile`.

**`.env` holds `POSTGRES_PASSWORD` and is not recoverable if lost.** It's
not stored in the database. Losing it means `db` still runs (Postgres
already has the password baked into its data dir) but `server` can't
authenticate until you restore the correct password into `.env`. Back it up
alongside the DB dump, not instead of it.

**Passkeys live in the database only** — no separate credential store. Lose
`sentinel_pgdata` with no backup and every admin passkey and every device
identity is gone: admins re-register (see below), devices get re-enrolled
with fresh tokens. Back up the database like you mean it.

### Backup

Names are from `compose.yaml`: the `db` container is `sentinel-db`, running
`POSTGRES_USER=sentinel` / `POSTGRES_DB=sentinel` by default (check `.env`
if you changed them).

```sh
cd sentinel
podman exec sentinel-db pg_dump -U sentinel sentinel > backup-$(date +%F).sql
cp .env env-backup-$(date +%F)
```

Store both off the VPS — the dump is plain-text SQL, pipe it through
`gzip`/`age`/your backup pipeline. Run this on a schedule; nothing in
Sentinel does it for you.

### Restore

Onto a fresh stack (new VPS, or recovering a wiped volume):

```sh
cd sentinel
cp env-backup-<date> .env
podman-compose -f compose.yaml up -d db
podman-compose -f compose.yaml ps db     # wait for healthy
cat backup-<date>.sql | podman exec -i sentinel-db psql -U sentinel sentinel
podman-compose -f compose.yaml up -d server
```

If the `db` volume already has data (e.g. retrying after a bad migration),
wipe it first — replaying a dump into an already-populated database errors
on duplicate keys instead of merging:

```sh
podman-compose -f compose.yaml down
podman volume rm sentinel_pgdata
podman-compose -f compose.yaml up -d db
# then replay the dump as above
```

Verify with `podman exec sentinel-db psql -U sentinel sentinel -c '\dt'`,
then check `/health` and log in.

## Monitoring

**`/health`** (unauthenticated) returns `{"status":"ok","service":
"sentinel-server"}` once the server is accepting connections. It's a
**liveness** check only — it never touches the DB pool (`server/src/main.rs`),
so 200 doesn't prove Postgres is reachable. `deploy/*.sh` poll it after
`up -d`. For a real DB check, log in or hit any `/api/*` route.

**Logs:**
```sh
podman-compose -f compose.yaml logs -f server
podman-compose -f compose.yaml logs -f db
```
`RUST_LOG` in `.env` (default `sentinel_server=info,tower_http=info,info`)
controls verbosity — `debug` is noisy, use it temporarily.

**Heartbeat cadence.** WS-connected devices (`/agent/ws`) flip `offline`
immediately on disconnect. Polling agents fall back to
`POST /agent/heartbeat` roughly every 15s (`server/src/agent.rs`). A
background sweep every 60s flips any device still marked `online` with
`last_seen` older than **3 minutes** to `offline`. A healthy device's
`last_seen` should never be more than a few minutes old.

**Events feed is the audit trail.** `GET /api/events` (console: Events page)
logs enrollment, policy changes, tamper detections, lock/unlock,
self-updates, etc. (`server/src/events.rs`). There's no separate audit log —
this table is the record, and it isn't auto-pruned.

**Gone-dark detection is UI-computed, not server-alerted.** A device that's
`offline` with `last_seen` 7+ days in the past is flagged gone-dark in the
console (`goneDarkDays`, `web/src/lib/format.ts`). Nothing emails or pages
you about it — check the console, or poll `GET /api/devices` and compute
the same threshold yourself if you want proactive alerting.

## Recovering access

**Lost all admin passkeys.** Registration locks the moment the first admin
exists (`403 registration_closed`, `server/src/auth.rs`). To get back in:

1. Add `SENTINEL_OPEN_REGISTRATION=1` to `.env`.
2. `podman-compose -f compose.yaml up -d server` to pick it up.
3. Register a new admin (email + passkey) from the login page.
4. **Remove it from `.env` and recreate again immediately.**

While that variable is set, registration is open to anyone who can reach
your public URL, not just you — treat steps 2–4 as one uninterrupted
operation. If you can still log in and just want a second passkey on your
own account, don't use this path — add it from **Settings** instead.

**Lost the parent PIN.** It's stored per-profile
(`policy.parent_pin_hash`, Argon2-hashed, never returned as plaintext), not
one global secret. Reset it as a logged-in admin: open the profile on the
**Profiles** page and set a new PIN (clearing the field removes the PIN
requirement — `server/src/profiles.rs`). The admin session already proves
access; the PIN itself only gates local, on-device unlock.

## Common failures & fixes

**Health check times out after an update.** Check `podman-compose -f
compose.yaml logs server` — usually a failed migration or a missing/bad env
var. To get back to known-good (there are no release tags, so "rollback"
means a commit SHA):
```sh
git log --oneline -10
git checkout <previous-sha>   # detached HEAD
deploy/build.sh               # rebuild WITHOUT --pull, stays on that commit
podman-compose -f compose.yaml up -d
```
`deploy/update.sh` won't run from detached HEAD (`git pull --ff-only` needs
a branch) — use `deploy/build.sh` + `up -d` by hand, then `git checkout
main` + `deploy/update.sh` once ready to move forward again.

**WebAuthn errors (invalid origin, registration/login silently fails).**
`RP_ID`/`RP_ORIGIN` in `.env` don't match what the browser sees. `RP_ID` is
the bare domain, no scheme; `RP_ORIGIN` is the exact `https://` origin
including port if non-standard. Breaks if you changed the domain, hit the
console by IP, or sit behind a proxy that rewrites Host. Fix `.env`, then
`podman-compose -f compose.yaml up -d server`.

**Port conflict on startup.** Something else has `SENTINEL_PORT` (default
8080). Change it in `.env`, `up -d`, and repoint your reverse proxy.

**`registration_closed` adding a second admin.** Expected once an admin
exists — it's the register endpoint, not a bug. If you're logged in, use
**Settings** to add a passkey to your account instead; only use
`SENTINEL_OPEN_REGISTRATION=1` (above) for a genuinely new, separate admin.

**Rate limiting collapses everyone onto one bucket (mass 429s).** The
limiter keys on the last `X-Forwarded-For` hop only when
`SENTINEL_TRUST_PROXY=1` (`server/src/rate_limit.rs`); otherwise it uses the
raw peer address, which behind a reverse proxy is the proxy itself — one
shared bucket for every visitor. `compose.yaml` defaults it to `1` because
this stack only ever sits behind your reverse proxy. If you see mass 429s
anyway, check that `SENTINEL_TRUST_PROXY` wasn't overridden to `0` in `.env`
and that your proxy actually appends `X-Forwarded-For` (see DEPLOY.md's
reverse-proxy requirements).

**Stale container/pod name conflicts on Podman.** `podman-compose` names
the pod after the project directory (`pod_sentinel` for a checkout named
`sentinel`). A leftover pod from a previous crash/`down` can make `up -d`
refuse with "name already in use":
```sh
podman pod rm -f pod_sentinel
podman-compose -f compose.yaml up -d
```
Destroys running containers in that pod, not the `sentinel_pgdata` volume —
data survives.

**Disk filling up from old images.** Every rebuild leaves old layers
behind:
```sh
podman image prune       # add -a to also drop unused-but-tagged images
```

## Uninstalling a device

There's no `sentinel-agent uninstall` subcommand (`client/src/main.rs` has
only `enroll`, `run`, `install-service`, `status`, `unlock`) — this is the
honest manual path.

**Release enforcement first, if you can.** The agent applies state directly
to the host outside the systemd unit: an `nft` table (`inet sentinel`) and a
pinned/immutable `/etc/resolv.conf`. Stopping the service does not tear
these down. If you know the parent PIN, run as root on the device:
```sh
sentinel-agent unlock --pin <PARENT_PIN> --minutes 0
```
`--minutes 0` suspends enforcement with no scheduled re-apply — tears down
the nft table, un-pins `resolv.conf`, un-freezes any frozen users
(`client/src/unlock.rs`). Without the PIN, do the same by hand:
```sh
nft delete table inet sentinel     # ignore "No such file" if already gone
chattr -i /etc/resolv.conf
# then repoint /etc/resolv.conf at whatever resolver the host should use
```

Then remove the agent:
```sh
systemctl disable --now sentinel-agent.service sentinel-watchdog.timer
rm -f /etc/systemd/system/sentinel-agent.service \
      /etc/systemd/system/sentinel-watchdog.service \
      /etc/systemd/system/sentinel-watchdog.timer \
      /etc/systemd/user/sentinel-tray.service \
      /etc/polkit-1/rules.d/49-sentinel.rules
systemctl daemon-reload
rm -f /usr/local/bin/sentinel-agent /usr/local/bin/sentinel-agent.bak
rm -rf /etc/sentinel /var/lib/sentinel
```

Finally, **delete the device in the console** (device detail page, or
`DELETE /api/devices/:id`). The server has no way to know the agent is gone
until you tell it — until then it sits there, eventually swept `offline`
and, after 7 days, flagged gone-dark. Deleting it removes it from the fleet
outright; nothing server-side keeps pointing at that device afterward.
