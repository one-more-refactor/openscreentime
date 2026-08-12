#!/usr/bin/env bash
# OpenScreenTime — install a systemd timer that runs deploy/update.sh once a day,
# as the (rootless-podman) user that owns this checkout. update.sh already
# polls /health and rolls back to the previous revision on failure, so an
# unattended bad update self-heals.
#
# Run with sudo from the repo checkout of the deploy user:
#   sudo deploy/install-auto-update.sh
#
# Undo:
#   sudo systemctl disable --now openscreentime-update.timer
set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
    echo "error: run with sudo (writes /etc/systemd/system)." >&2
    exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The deploy user = whoever owns the checkout (rootless podman runs as them),
# not root. `sudo deploy/...` makes SUDO_USER a good fallback check.
run_user="$(stat -c '%U' "$repo_root")"
if [[ "$run_user" == "root" && -n "${SUDO_USER:-}" ]]; then
    run_user="$SUDO_USER"
fi
if [[ "$run_user" == "root" ]]; then
    echo "error: could not determine the non-root deploy user owning ${repo_root}." >&2
    exit 1
fi

if [[ ! -f "${repo_root}/.env" ]]; then
    echo "error: no .env in ${repo_root} — run deploy/setup.sh first." >&2
    exit 1
fi

cat > /etc/systemd/system/openscreentime-update.service <<EOF
# Managed by openscreentime deploy/install-auto-update.sh — do not edit.
[Unit]
Description=OpenScreenTime server update (git pull, rebuild, health check, rollback on failure)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
User=${run_user}
WorkingDirectory=${repo_root}
ExecStart=${repo_root}/deploy/update.sh
# A full image rebuild can take a while on a small VPS.
TimeoutStartSec=45min
EOF

cat > /etc/systemd/system/openscreentime-update.timer <<EOF
# Managed by openscreentime deploy/install-auto-update.sh — do not edit.
[Unit]
Description=Daily OpenScreenTime server update

[Timer]
OnCalendar=daily
# Spread the load / avoid a thundering herd against the git remote.
RandomizedDelaySec=1h
# Run at next boot if the machine slept through the scheduled time.
Persistent=true

[Install]
WantedBy=timers.target
EOF

systemctl daemon-reload
systemctl enable --now openscreentime-update.timer

echo "==> installed. Next runs:"
systemctl list-timers openscreentime-update.timer --no-pager | head -3
echo "==> logs after a run: journalctl -u openscreentime-update.service"
