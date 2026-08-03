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
