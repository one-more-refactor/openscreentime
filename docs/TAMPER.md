# Tamper Resistance & Remote Access

## Threat model & honesty

The managed user may have physical access. If they also have root, **no software can make
shutdown or network disconnection truly impossible.** Sentinel's goal is therefore:

1. **Raise the cost** of tampering (make casual bypass hard).
2. **Detect and report** every tamper attempt in real time.
3. **Recover automatically** (auto-restart, re-apply policy on boot).

We do NOT claim unbypassable enforcement. Anti-tamper marketing that claims otherwise is lying.

## Levels

Configured per device via `devices.tamper_level`. Default **1**. **3** is opt-in per device.

### Level 1 — Strong deterrence + alerting (DEFAULT)
- Agent runs as a **root-owned systemd service**, hardened unit:
  `Restart=always`, `RestartSec=1`, `StartLimitIntervalSec=0` (never give up restarting),
  `ProtectSystem=strict`, `ProtectHome` off (needs to watch users), `NoNewPrivileges` off
  (needs nft/dns control), `OOMScoreAdjust=-1000`.
- **Watchdog:** a second lightweight unit (or systemd `WatchdogSec`) that restarts the agent if
  its heartbeat file goes stale.
- **Mask user-level power controls:** polkit rule denying non-root
  `org.freedesktop.login1.power-off` / `reboot` / `suspend` for managed users; a root escape
  (`sentinel-admin unlock`) always exists.
- **Config & binary integrity:** agent binary + config are root-owned `0700`; agent verifies its
  own config signature on load.
- **NetworkManager guard:** watch for connection edits / device disconnect via NM D-Bus signals;
  re-assert managed connection and fire a `tamper` event if a managed user tries to disconnect.
- **Boot persistence:** systemd unit is `WantedBy=multi-user.target`; policy re-applied on every
  boot before the graphical target (network-online.target ordering).
- **Every tamper attempt** (service stop attempt, nft flush, NM disconnect, clock skew) →
  immediate `tamper` event (severity `warn`/`critical`) over WS, buffered to disk if offline.

### Level 3 — Maximum lockdown (OPT-IN)
Everything in level 1 **plus**:
- Disable extra TTYs / `Ctrl+Alt+F*` switching for managed sessions.
- Lock the systemd unit against user `systemctl stop` via polkit (only `sentinel-admin` token).
- Bootloader/firmware **guidance** surfaced in the UI (set GRUB password, BIOS admin password,
  disable USB boot) — these are physical mitigations we can only advise, not enforce.
- Kill known escape hatches (recovery shells for managed users, `init=/bin/bash` guidance).
- **Danger:** level 3 can lock the admin out too. The UI must require an explicit confirm and
  show the recovery procedure before enabling.

> The `--tamper-max` flag on the agent and the `set_tamper_level` command toggle this. Always
> keep the `sentinel-admin` recovery path working at every level.

## Zero-trust enforcement primitives (Linux)

- **DNS:** agent runs a local resolver (or configures `systemd-resolved` / `dnsmasq`) that under
  `default_deny` answers only allowlisted names (with wildcard support) and forwards them to the
  filtered `upstream`; everything else → NXDOMAIN. `/etc/resolv.conf` is pinned & guarded.
- **Firewall:** `nftables` ruleset, default-deny both directions, allow only policy ports +
  established/related + loopback + the server + DNS upstream. Ruleset is re-applied on any change.
- **Screen time / app limits:** per-user session accounting (who is active on seat), enforced by
  freezing user processes (cgroup freezer) or ending the session when the balance hits zero,
  after showing the lockout overlay.

## Remote SSH (server-brokered reverse tunnel)

Devices are behind NAT, so the **agent dials out** to the server; the server brokers a shell.

Flow:
1. Admin clicks **SSH** on a device → `POST /api/devices/:id/ssh`.
2. Server allocates a `broker_port`, creates an `ssh_session` (`opening`), and enqueues an
   `ssh_open` command `{ session_id, broker_port }`.
3. Agent receives it over WS and opens a reverse channel back to the server (either a real
   `ssh -R` to the broker's embedded SSH server, or a multiplexed data stream over the existing
   WS that the server bridges to a local listener). Skeleton: multiplex a PTY over WS and expose
   it via a server-side `ws->pty` bridge + a small `sentinel ssh <device>` CLI; document the
   `ssh -R` production path.
4. Server marks session `open` and returns a `connect_cmd` to the admin
   (e.g. `ssh -p <broker_port> device@broker.sentinel.example`) or opens an in-browser terminal.
5. `POST /api/devices/:id/ssh` again with `{ close: true }` or session idle timeout → `ssh_close`.

All remote-shell sessions are **audited** (`event` rows) and only initiated by an authenticated
admin. The agent only ever *dials out*; it never opens an inbound listener, preserving the
firewall's default-deny inbound stance.

## Device discovery

`discover` command → agent scans its local subnet (ARP + a light TCP connect sweep on common
ports) and returns found hosts as a `discovery_result` event: `{ ip, mac, hostname?, open_ports,
vendor? }`. The control center lists them so an admin can push an enrollment token / QR to
onboard the next device. No unsolicited scanning — only when an admin triggers it.
