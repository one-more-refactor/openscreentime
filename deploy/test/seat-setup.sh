#!/usr/bin/env bash
# Runs INSIDE the test VM (as root, via `vm.sh seat`). Gives the managed child
# `mia` a real graphical local seat and accelerates the agent's clock.
#
#   * a systemd service starts Weston as mia on tty1 with PAMName=login and
#     TTYPath=/dev/tty1, so logind registers a real seat0 session on VT 1
#     (Class=user, Active=yes) — what the agent counts as screen time, and
#     what the lock needs to know who is on screen and which VT to return to;
#   * Weston talks to logind for the seat (as on a real desktop), and uses the
#     CPU (pixman) renderer because the GL/GBM path hangs on the emulated GPU;
#   * cage is installed for the lock screen;
#   * a drop-in runs the agent with --time-accel so the daily budget is
#     reachable in seconds.
#
# Arg 1: time-accel factor (default 60 → 1 real second = 1 simulated minute).
set -euo pipefail
accel="${1:-60}"

# What a fresh overlay lacks: the desktop (Weston, software GL) and cage, the
# lock screen's compositor. Best-effort; an image that already has them just
# moves on.
pacman -Sy --noconfirm --needed weston mesa cage >/dev/null 2>&1 || true
usermod -aG video mia 2>/dev/null || true
# An earlier harness drove the seat with seatd; logind owns it now.
systemctl disable --now seatd 2>/dev/null || true

# Guarantee mia's user manager is running (the lock test drives her companion
# through it).
loginctl enable-linger mia
for _ in $(seq 1 10); do [ -d /run/user/1000 ] && break; sleep 1; done

# tty1 belongs to Weston now — stop the getty that would fight it for the VT,
# and drop any leftover autologin/profile hacks from earlier attempts.
systemctl disable --now getty@tty1.service 2>/dev/null || true
rm -f /etc/systemd/system/getty@tty1.service.d/autologin.conf
rm -f /home/mia/.bash_profile /tmp/profile-ran
pkill -9 weston 2>/dev/null || true

cat >/etc/systemd/system/mia-weston.service <<'UNIT'
[Unit]
Description=Weston (managed child mia) — test VM desktop, a real logind seat session
After=systemd-user-sessions.service systemd-logind.service getty@tty1.service
Conflicts=getty@tty1.service

[Service]
User=mia
PAMName=login
TTYPath=/dev/tty1
TTYReset=yes
TTYVHangup=yes
StandardInput=tty
StandardOutput=journal
StandardError=journal
UtmpIdentifier=tty1
UtmpMode=user
Environment=XDG_SESSION_TYPE=wayland
Environment=LIBSEAT_BACKEND=logind
ExecStart=/usr/bin/weston --renderer=pixman --idle-time=0
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT

# Accelerate the agent's clock.
mkdir -p /etc/systemd/system/openscreentime-agent.service.d
cat >/etc/systemd/system/openscreentime-agent.service.d/accel.conf <<UNIT
[Service]
ExecStart=
ExecStart=/usr/local/bin/openscreentime --time-accel ${accel} run
UNIT

systemctl daemon-reload
systemctl restart openscreentime-agent.service
systemctl enable mia-weston.service >/dev/null 2>&1 || true
systemctl restart mia-weston.service

# Give Weston a moment to come up.
for _ in $(seq 1 15); do
    loginctl list-sessions --no-legend | awk '$3=="mia" && $4=="seat0"' | grep -q . && break
    sleep 1
done

echo -n 'mia local seat: '
loginctl list-sessions --no-legend | awk '$3=="mia" && $4=="seat0" {print "session "$1" on "$4" "$7}'
if systemctl is-active --quiet mia-weston.service; then
    echo "weston: up"
else
    echo "weston: NOT up — recent journal:"
    journalctl -u mia-weston.service --no-pager -n 12 | sed 's/^/    /'
fi
