# Deploying OpenScreenTime

This is the operator guide for running OpenScreenTime on your own server
behind your own reverse proxy. For a dev setup see `docs/DEVELOPMENT.md`; for
day-2 work (backups, restores, alerts, recovery) see `docs/OPERATIONS.md`.

The goal: you set it up once, and it keeps itself running.

## Quickstart

```sh
git clone <this-repo-url> openscreentime && cd openscreentime
deploy/setup.sh --domain ost.example.com
```

`setup.sh`:

1. writes `.env` with fresh secrets and **one** setting, `OST_PUBLIC_URL`
   (the passkey domain, origin and secure cookies are derived from it),
2. pulls the server image (or builds it here if it can't pull),
3. starts the stack, waits for `/health`, and takes a first database backup,
4. installs systemd units so it keeps running by itself:
   - `openscreentime.service` — starts the containers at boot (Podman),
   - `openscreentime-backup.timer` — nightly database backup, 7 kept,
   - `openscreentime-update.timer` — daily update with automatic rollback
     (skip with `--no-auto-update`).

Then:

1. Point your reverse proxy at `127.0.0.1:8080` (snippets below — setup.sh
   prints them for your domain).
2. Open the one-time setup link setup.sh prints
   (`https://ost.example.com/#setup=<code>`, the code is `OST_BOOTSTRAP_TOKEN`
   in `.env`): your name, then a passkey.
3. **Computers → Add a computer** in the console, and paste the one-liner on
   each computer.

Re-running `deploy/setup.sh` is safe: it never touches an existing `.env`, and
on an existing stack it runs `deploy/update.sh` (the safe update path).

## Prerequisites

- A Linux server (VPS, LXC, spare PC), x86_64, 1 GB RAM is plenty when the
  image is pulled (building it here needs ~2 GB and 10–60 minutes).
- Podman with `podman-compose` — rootful or rootless — or Docker with the
  compose plugin. Plus `git` and `curl`.
  - **Rootful** (simplest for a dedicated box): run setup.sh as root.
  - **Rootless**: run setup.sh as the user that owns the stack, not with sudo.
    It enables lingering for that user (`loginctl enable-linger`) so the stack
    runs at boot without anyone logged in; if that needs root it tells you the
    one command to run.
- A DNS name and a reverse proxy that terminates TLS. Passkeys only work over
  https on a real domain — a plain-http LAN address will not do.

## Architecture

- `compose.yaml` runs two containers: `openscreentime-db` (Postgres 15) and
  `openscreentime-server` (API + web console + the agent binaries devices
  install and update from). The server runs the image tagged
  `localhost/openscreentime-server:current`.
- The server is published on `127.0.0.1:${OST_PORT:-8080}` only. Your proxy
  forwards to it. Set `OST_BIND_ADDR` in `.env` (or `--bind`) to a LAN address
  only when the proxy runs on another machine — never `0.0.0.0`.
- Both containers have `restart: always`; the server has a healthcheck
  (`/health`, which also checks the database).

## Reverse proxy requirements

Your proxy MUST:

1. Forward all traffic for the domain to `127.0.0.1:${OST_PORT:-8080}`
   (plain HTTP — TLS ends at the proxy).
2. **Upgrade WebSocket connections** (`/agent/ws`, the device channel). Without
   it devices fall back to slower polling.
3. Append the real client IP to `X-Forwarded-For` (the rate limiter keys on the
   last hop).

### Caddy

```caddyfile
ost.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Caddy gets the certificate, forwards WebSocket upgrades and sets
`X-Forwarded-For` by itself.

### nginx

```nginx
server {
    listen 443 ssl;
    server_name ost.example.com;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

## Configuration

Everything lives in `.env` next to `compose.yaml` (see `.env.example`). The
only value you choose is `OST_PUBLIC_URL`; the rest is generated or optional:

| Variable | What |
|---|---|
| `OST_PUBLIC_URL` | The public https address. Derives `RP_ID`, `RP_ORIGIN`, secure cookies. |
| `POSTGRES_PASSWORD` | Generated. Exists only here (and in `backups/env.backup`). |
| `OST_BOOTSTRAP_TOKEN` | Generated one-time code for the first account. |
| `OST_PORT`, `OST_BIND_ADDR` | Where the server is published on the host. |
| `OST_IMAGE` | The image updates pull (default: the one CI publishes from `main`); `build` = always build here. |
| `OST_ALERT_WEBHOOK`, `OST_TELEGRAM_BOT_TOKEN` | Phone alerts — see OPERATIONS.md. |
| `OST_OIDC_*` | Optional SSO. If the provider is down, the SSO button just hides. |
| `RP_ID`, `RP_ORIGIN`, `OST_INSECURE_COOKIES` | Overrides; normally unset. |

After editing `.env`, apply it with `podman-compose up -d` (or
`deploy/update.sh`). Migrations run automatically when the server starts.

## First run

`deploy/setup.sh` prints a one-time setup link, `https://<domain>/#setup=<code>`
(the code is `OST_BOOTSTRAP_TOKEN` in `.env`). Open it: **Create your household**
asks only for your name, then a passkey. From the moment that first account
exists, first run refuses with `403 registration_closed` — a public
OpenScreenTime URL can't be hijacked by whoever finds it first. (Without the
link, the page asks for the setup code.) After that, people sign in with their
name and a code on their own computer, or a passkey — see docs/AUTH.md.

## Adding computers

**Computers → Add a computer** (or **Add a person**, which sets up their
computer next) gives a one-liner like:

```sh
curl -fsSL https://ost.example.com/install.sh | \
  sudo OST_TOKEN=<ENROLL_TOKEN> sh -s -- --server https://ost.example.com
```

It installs the agent build **this server** bundles — the desktop build (the
app window, the graphical lock, the companion) on a machine with a graphical
session, headless otherwise — sha256-verified, enrolls, and installs the
systemd service. On a desktop build it also installs `cage` for the lock where
apt, pacman or dnf has it; without cage the lock is a text screen on its own
console. If the one-liner dies halfway, just run it again within 15 minutes —
the token is not used up until the computer has actually connected.

Devices then update themselves from this server (see OPERATIONS.md →
"Devices update themselves"): the server is their release channel.

## Updating

It updates itself daily (`openscreentime-update.timer`). By hand:

```sh
deploy/update.sh
```

What it does:

1. Fast-forwards the checkout (compose.yaml, scripts). Local edits or a
   diverged branch are reported and skipped — they never block the update.
2. Pulls `OST_IMAGE`; if that fails, builds the image from the checkout
   (only when the checkout changed since the last build).
3. If it's the image already running: done. Otherwise it **backs up the
   database**, swaps the server to the new image and waits for `/health`.
4. If the new version isn't healthy within 3 minutes, it **rolls back**: the
   previous image *and* the pre-update database (a new version may already
   have migrated the schema, which the old one would refuse). That image is
   then skipped until a newer one is published, and the rollback is reported
   to your phone if alerts are set up.

To deploy an image built elsewhere, `deploy/update.sh --image <ref>` does the
same backup/health/rollback dance; `deploy/push-image.sh` builds on your dev
box and does exactly that on the server over SSH.

Installs made before these units existed: run `deploy/install-auto-update.sh`
once (as root for rootful Podman, as the owning user for rootless). Plain
`deploy/update.sh` also installs the start-at-boot and backup units.

## Troubleshooting

- **Passkey registration fails / "invalid origin"**: `OST_PUBLIC_URL` does not
  match what the browser shows (scheme, host and port must match exactly).
- **Devices connect but drop to polling**: the proxy isn't forwarding
  WebSocket upgrades.
- **Everything 429s**: the proxy doesn't set `X-Forwarded-For` (all clients
  share the proxy's bucket).
- **`/health` says `degraded` (503)**: the server runs but can't reach
  Postgres — `podman logs openscreentime-db`.
- **Nothing after a reboot** (Podman): `systemctl status openscreentime`
  (rootless: `systemctl --user status openscreentime` and check
  `loginctl show-user $USER -p Linger`).
