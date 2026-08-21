# Changelog

All notable changes to OpenScreenTime, newest first. Sections below `0.4.0`
were written when the product was called Sentinel and keep that name — they
describe what actually shipped. Each version's section becomes
the GitHub Release notes verbatim (see `.github/workflows/build.yml`), so it
is written to be read by a person: the first paragraph says what changed in
plain language, the bullets carry the detail. Unreleased work accumulates
under `[Unreleased]` and moves into a version section when a release is cut.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning:
[SemVer](https://semver.org/) — pre-1.0, a minor bump means new features, a
patch bump means fixes only. The agent self-updates by comparing this version
(`x.y.z`, from the crate metadata) against its server's bundled build.

The project is **alpha**: every release is published as a pre-release and
there is no stable version. See the notice at the top of `README.md`.

## [Unreleased]

**Breaking: the product is now OpenScreenTime.** The agent was renamed in the
previous release; this one finishes the job across the server, the deployment
and the docs. There are no compatibility shims for the server side — an
existing deployment needs its `.env` rewritten and its stack recreated.

What changed, and what you have to do about it:

- **Every `SENTINEL_*` environment variable is now `OST_*`** — same names
  otherwise (`OST_PUBLIC_URL`, `OST_TRUST_PROXY`, `OST_OIDC_*`,
  `OST_ALERT_WEBHOOK`, `OST_TOKEN`, …). The old names are not read. Rename
  them in `.env` before redeploying or the server starts with defaults.
- **Container, volume and database names changed**: `sentinel-db` →
  `openscreentime-db`, `sentinel-server` → `openscreentime-server`,
  `sentinel_pgdata` → `ost_pgdata`, and the default Postgres user/database are
  now `openscreentime`. Compose will not adopt the old volume — dump the
  database first (`docs/OPERATIONS.md`) and restore into the new stack, or
  point `POSTGRES_USER`/`POSTGRES_DB` at the old values in `.env`.
- **The admin session cookie is now `ost_session`**, so every logged-in admin
  is signed out once on upgrade. Passkeys themselves are unaffected.
- **New `OST_BIND_ADDR`** (default `127.0.0.1`) publishes the app port on a
  specific host address, for deployments whose reverse proxy runs on a
  different machine. Do not set it to `0.0.0.0`: the server trusts
  `X-Forwarded-For`, so a directly reachable port is a rate-limiter bypass.
- **Crates renamed** to `openscreentime-server` and `openscreentime-policy`.
- **Agent-side identifiers** follow the binary: the nftables table is now
  `inet openscreentime`, the polkit rule `49-openscreentime.rules`, the VPN
  tunnel `wg-quick@openscreentime` / `openvpn-client@openscreentime`, and the
  recovery account is **`ost-admin`** (create it before raising the tamper
  level — the old `sentinel-admin` is no longer exempt from anything).
- The agent tears down a leftover `inet sentinel` table in the same atomic
  `nft` transaction as every apply, so an upgraded device can't be left
  enforcing a stale ruleset that nothing writes to any more.
- Migration files keep their original wording: sqlx checksums them, and
  editing one would fail validation on every existing database.

Headline: **the remote shell is gone**. Sentinel no longer contains a remote
shell at all — everything a parent can do now goes through the UI. The
transparency promise gets simpler and stronger: instead of "a shell is never
open without the device being told", there is no shell to open.

### Removed

- **Breaking**: the remote shell feature, end to end. The
  `POST /api/devices/:id/ssh`, `GET /api/ssh/:id/ws`, and
  `POST /api/ssh/:id/close` routes are gone, the `ssh_open`/`ssh_close`
  command types are gone, and the `ssh_sessions` table is dropped
  (`0008_remove_ssh.sql`). The web terminal and the agent's PTY endpoint are
  removed with them.
- Historical `ssh` **events remain readable** in the event log — the record
  that past sessions happened survives; only the capability is removed.
- A possible future replacement — a secure reverse tunnel carrying native
  SSH+RDP — was considered and deferred; nothing of it ships today.

### Security

A red-team pass on the enforcement and transparency surface. This round lands
the fixes that were surgical and low-risk; the deeper enforcement-logic items
(edge-triggered freeze reconciliation, timezone-anchored day rolls, brick-safe
lockdown gating) are tracked separately.

- **The console now shows the event feed again.** After the Family/Child
  redesign, no reachable page rendered events at all — every tamper the server
  recorded was invisible, quietly breaking the "tampering is never silent"
  promise. The child page now carries a **Recent activity** audit trail, and
  the feed renders the previously-unmapped `evasion`, `enforcement_degraded`,
  and `vpn_profile` types (they used to draw as blank rows) and falls back to
  the raw type name for any future type.
- **Devices no longer force inbound SSH (port 22) open.** The protection slider
  had pinned `allow_inbound_ports: [22]` at every level — a leftover from the
  removed remote shell — exposing the box's own `sshd` (and the polkit-exempt
  `sentinel-admin` account) on every network it joined. The agent opens no
  inbound listener, so this is now `[]`.
- **A device can no longer forge its own lock state.** Command acks were not
  status-guarded, so a device could re-ack any command id it had seen —
  replaying an old `unlock` to appear online, or rewriting the audit timestamp
  on historical rows. Acks now only apply to still-open (`queued`/`sent`)
  commands.
- **Confirmed usage-ledger resets now reach the parent.** The server-side
  anti-cheat `evasion` event was `warn`, but the alert fan-out only pushes
  `critical` — so the one signal the server derives independently of the
  device's honesty never left the console. It is now `critical`.
- **Power masking covers halt and hibernate.** The polkit rule denied
  power-off/reboot/suspend but not `halt`, `hibernate`, or
  `suspend-then-hibernate` — any desktop power menu's Hibernate froze the whole
  machine (agent included) undetected. All power paths are denied now.
- **Level 3 now protects the watchdog units too.** The stop/disable/mask block
  covered `sentinel-agent.service` but not `sentinel-watchdog.{service,timer}` —
  masking the watchdog alone silently disarmed the recovery net. All three
  units are guarded.
- **Enrolling against a plaintext `http://` server is refused** (loopback and
  `.local` excepted for dev/LAN), since plaintext transport makes the
  self-update sha256 check decorative against an on-path attacker.
- Internal server errors no longer leak raw Postgres detail (table/constraint
  names, cast errors) to API clients; the full error is still logged
  server-side.

## [0.3.0] - 2026-08-04

Headline: **VPN profiles**. Drop a WireGuard `.conf` or OpenVPN `.ovpn`
client config on a device's page and that machine routes through your VPN —
the agent keeps the tunnel up, across reboots and config changes, and the
device firewall automatically lets your own tunnel through even with
VPN-blocking lockdown enabled. Plus: server deployments can now update
themselves daily, rolling back automatically if the new version fails its
health check.

### Added

- Per-device VPN profiles: upload in the console (drag & drop), enforced on
  the device as `wg-quick@sentinel` / `openvpn-client@sentinel`. The config
  (it contains private keys) is stored write-only — the console only ever
  shows kind + upload time — and is written on the device root-only. A
  tunnel that isn't actually running (e.g. WireGuard tools not installed)
  raises a critical `enforcement_degraded` alert instead of pretending.
- Removing a profile propagates like setting one: the agent tears the
  tunnel down on its next sync, even in polling mode.
- Optional automatic server updates: `sudo deploy/install-auto-update.sh`
  installs a daily systemd timer around `deploy/update.sh`, which now rolls
  back to the previously running revision if the updated server fails its
  health check — an unattended bad update self-heals.

### Fixed

- The `enforcement_degraded` alerts introduced in 0.2.0 were rejected by the
  database (missing from the events type constraint) — agents buffered and
  retried them forever and the console never saw them. They now land.
- The SSH session reaper's cleanup query was rejected by PostgreSQL on every
  sweep (`make_interval` type mismatch), so sessions stuck in `opening`
  were never cleaned up. Stale sessions now reap after 15 minutes as
  intended.
- Release builds: the Build workflow pinned a Rust older than the project's
  minimum supported version and failed on every run since the MSRV bump;
  release notes extraction produced empty notes on every tag. Both fixed —
  v0.2.0 was the first release to actually ship because of it.

## [0.2.0] - 2026-08-04

The first tagged Sentinel release, and the first one devices can auto-update
to. Headline: **a device that can't actually enforce its DNS rules now says
so loudly in the console** instead of showing a reassuring green light —
previously, on common setups (systemd-resolved distros, missing dnsmasq),
filtering could silently not be in force while everything looked fine.

### Added

- Release pipeline: tagged builds publish the server + static agent binaries,
  checksums, and a container image; enrolled devices pick up new agent
  versions automatically within a day (`auto_update`, on by default).
- `enforcement_degraded` critical events: every gap between "policy accepted"
  and "policy actually enforced" is reported per-cause (no local resolver,
  unpinnable `/etc/resolv.conf`, symlinked resolv.conf) with a plain-language
  explanation of what to do about it.
- `policy_applied` events now carry a `dns_gaps` count, so "applied" and
  "fully enforced" are distinguishable at a glance.

### Fixed

- DNS enforcement no longer hides failures at debug log level: a failed
  `chattr +i` pin, a dnsmasq that failed to start, and re-pin errors in the
  tamper loop all surface as critical events now.
- A symlinked `/etc/resolv.conf` (systemd-resolved's stub) is replaced with a
  real pinned file instead of being written through — the old behavior failed
  open and quiet; the new one fails closed and loud.
- CI: pinned `sqlx-cli` to 0.8.x and raised MSRV to 1.89 so main builds
  reproducibly again.

### Known limitations

- Only the headless x86_64 agent self-updates; desktop builds with the
  `gui`/`tray` features are built locally and must be updated the same way.
- Update trust is sha256-over-TLS from the enrolled server; independent
  binary signing (minisign) is planned.
