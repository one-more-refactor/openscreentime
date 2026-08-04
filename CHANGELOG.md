# Changelog

All notable changes to Sentinel, newest first. Each version's section becomes
the GitHub Release notes verbatim (see `.github/workflows/build.yml`), so it
is written to be read by a person: the first paragraph says what changed in
plain language, the bullets carry the detail. Unreleased work accumulates
under `[Unreleased]` and moves into a version section when a release is cut.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning:
[SemVer](https://semver.org/) — pre-1.0, a minor bump means new features, a
patch bump means fixes only. The agent self-updates by comparing this version
(`x.y.z`, from the crate metadata) against its server's bundled build.

## [Unreleased]

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
