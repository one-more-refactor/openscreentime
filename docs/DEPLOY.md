# Deploying Sentinel (production, rootless Podman)

This is the operator guide for running Sentinel on an internet-exposed VPS
behind your own reverse proxy. It is not a dev setup guide — see
`docs/DEVELOPMENT.md` for that.

## Architecture

- `compose.yaml` (repo root) runs two containers: `db` (Postgres 15) and
  `server` (the Sentinel API + the built web UI, single image, built from
  `Containerfile`).
- The server binds `0.0.0.0:8080` **inside** its container, but the compose
  file only publishes it to `127.0.0.1:${SENTINEL_PORT:-8080}` on the host.
  It is never reachable directly from the internet.
- **You provide the reverse proxy** (Caddy, nginx, Traefik, whatever you
  already run on the VPS) that terminates TLS and forwards to
  `127.0.0.1:8080`. Sentinel does not bundle one.
- The server serves the web UI itself (same origin as the API) — no CORS
  hop, no separate web server needed.

## Prerequisites

- A Linux VPS with rootless Podman set up for your deploy user, plus
  `podman-compose` (or Podman >= 4 with the `compose` plugin, or Docker as a
  fallback).
- A reverse proxy already running on the host and terminating TLS for your
  domain (e.g. Caddy with automatic HTTPS, or nginx + certbot).
- A DNS name pointing at the VPS (e.g. `sentinel.example.com`).
- git access to this repository from the VPS.

## Reverse proxy requirements

Your proxy MUST:

1. Forward all traffic for the domain to `127.0.0.1:${SENTINEL_PORT:-8080}`
   (plain HTTP — TLS is terminated at the proxy).
2. **Upgrade WebSocket connections** for `/agent/ws` and `/api/ssh/*/ws`.
   These are long-lived bidirectional connections (agent command channel and
   the browser SSH terminal) — if your proxy doesn't forward the `Upgrade`
   and `Connection` headers, both features silently break.
3. Set `X-Forwarded-For` with the **real client IP as the last hop**. The
   server's rate limiter (`server/src/rate_limit.rs`) reads the last XFF hop
   to key rate limits per-client; if your proxy doesn't set this (or another
   hop further upstream overwrites it incorrectly), rate limiting will be
   keyed on the proxy's own address instead of real clients.

### Example (Caddy)

```caddyfile
sentinel.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Caddy forwards WebSocket upgrades and sets `X-Forwarded-For` automatically.

### Example (nginx)

```nginx
server {
    listen 443 ssl;
    server_name sentinel.example.com;

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

## First boot

```sh
git clone <this-repo-url> sentinel && cd sentinel
cp .env.example .env
$EDITOR .env        # set POSTGRES_PASSWORD, RP_ID, RP_ORIGIN, SENTINEL_PUBLIC_URL
deploy/build.sh      # builds the server+web image locally on the VPS
podman-compose up -d # or: podman compose up -d / docker compose up -d
podman-compose logs -f server
```

`RP_ID` is the bare domain (e.g. `sentinel.example.com`); `RP_ORIGIN` and
`SENTINEL_PUBLIC_URL` are the full `https://` URL of the reverse proxy —
**not** an internal container address. WebAuthn/passkeys will fail to
register if these don't match what the browser sees.

Database migrations run automatically on every server startup
(`db::migrate` in `server/src/main.rs`) — no manual migration step needed.

### First admin & registration lockdown

While the database has **zero admins**, the login page's FIRST ADMIN tab is open: register
with any email + passkey and the tenant is bootstrapped around you. From the moment the
first admin exists, the register endpoints refuse with `403 registration_closed` — a
public Sentinel URL can't be hijacked by whoever finds it first thereafter.

To deliberately allow another *new* admin account to register, set
`SENTINEL_OPEN_REGISTRATION=1` in the server environment, restart, let them register, then
remove it again. (Logged-in admins can always add more passkeys to their own account via
Settings; OIDC SSO admin matching is unaffected.)

### Enrolling devices

The image bundles the headless agent binary and serves an installer, so enrolling a device
is one command (shown, pre-filled, in the web console's ADD DEVICE modal):

```sh
curl -fsSL https://sentinel.example.com/install.sh | \
  sudo SENTINEL_TOKEN=<ENROLL_TOKEN> sh -s -- --server https://sentinel.example.com
```

It downloads the sha256-verified binary to `/usr/local/bin/sentinel-agent`, enrolls, and
installs the systemd service. Installed agents self-update from the server daily
(`auto_update = true` in `/etc/sentinel/agent.toml`; `SENTINEL_NO_SELF_UPDATE=1` disables;
the previous binary is kept as `/usr/local/bin/sentinel-agent.bak` for manual rollback).
Desktop builds with the gui/tray features are built from source — see docs/DEVELOPMENT.md.

## Updating

```sh
cd sentinel
deploy/build.sh --pull   # git pull --ff-only, then rebuild images
podman-compose up -d     # recreates the server container with the new image
```

`db` data lives in the named volume `sentinel_pgdata` and is untouched by
rebuilds/updates.

## Rootless port note

The compose file publishes the app on `127.0.0.1:${SENTINEL_PORT:-8080}`
(default 8080), which is an unprivileged port — rootless Podman can bind it
with no extra configuration. If you ever want the *container* itself to bind
a port below 1024, rootless Podman needs
`sysctl net.ipv4.ip_unprivileged_port_start=<port>` on the host first; this
does not apply to the default setup here.

## Troubleshooting

- **Passkey registration fails / "invalid origin"**: `RP_ID`/`RP_ORIGIN`
  don't match what the browser actually sees. They must be the public HTTPS
  origin, not `localhost` or an internal address.
- **SSH terminal or agent connections drop immediately**: your proxy isn't
  forwarding WebSocket upgrades — see the reverse-proxy requirements above.
- **Everything 429s**: `X-Forwarded-For` isn't set correctly by the proxy,
  so the rate limiter may be collapsing all clients onto one key (the
  proxy's own IP, or a spoofable client-supplied hop).
- **Server logs `SENTINEL_WEB_DIR (...) not found`**: the web build didn't
  make it into the image — rerun `deploy/build.sh`, and check the
  `web-builder` stage in `Containerfile` succeeded.
