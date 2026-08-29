#!/usr/bin/env bash
# Runs INSIDE the test VM (as root, via `vm.sh seat`). Gives the managed child
# `mia` a real graphical local seat and accelerates the agent's clock.
#
#   * a systemd service starts Weston as mia on tty1 with PAMName=login, so
#     logind opens a real seat0 session (Active=yes, Remote=no) — which is both
#     what the agent counts as screen time AND where the lockout overlay looks
#     for a Wayland socket (/run/user/1000/wayland-0);
#   * seatd handles the seat/VT (logind's handoff to an autologin compositor is
#     flaky under QEMU), and Weston uses the CPU (pixman) renderer because the
#     GL/GBM path hangs on the emulated GPU;
#   * a drop-in runs the agent with --time-accel so the daily budget is reachable
#     in seconds.
#
# Arg 1: time-accel factor (default 60 → 1 real second = 1 simulated minute).
set -euo pipefail
accel="${1:-60}"

# Deterministic seat management via seatd; mia needs seat + video group access.
systemctl enable --now seatd 2>/dev/null || true
usermod -aG seat,video mia 2>/dev/null || true

# Guarantee mia's XDG_RUNTIME_DIR (/run/user/1000) exists and her user manager
# is running — without it Weston has nowhere to bind its Wayland socket and just
# blocks. Linger creates it independently of the login PAM stack.
loginctl enable-linger mia
for _ in $(seq 1 10); do [ -d /run/user/1000 ] && break; sleep 1; done

# tty1 belongs to Weston now — stop the getty that would fight it for the VT,
# and drop any leftover autologin/​profile hacks from earlier attempts.
systemctl disable --now getty@tty1.service 2>/dev/null || true
rm -f /etc/systemd/system/getty@tty1.service.d/autologin.conf
rm -f /home/mia/.bash_profile /tmp/profile-ran
pkill -9 weston 2>/dev/null || true

# Weston as a login session. PAMName=login makes logind register the seat0
# session (so the agent counts mia's time and the overlay finds her runtime
# dir); seatd owns the VT. Do NOT bind a controlling tty here — TTYPath/
# StandardInput=tty stalls Weston before it can exec under QEMU. seatd handles
# the VT switch itself. pixman = CPU renderer (GL/GBM hangs on the emulated GPU).
cat >/etc/systemd/system/mia-weston.service <<'UNIT'
[Unit]
Description=Weston (managed child mia) — test VM desktop
After=seatd.service systemd-user-sessions.service
Wants=seatd.service

[Service]
User=mia
PAMName=login
StandardInput=null
StandardOutput=journal
StandardError=journal
Environment=XDG_RUNTIME_DIR=/run/user/1000
Environment=XDG_SEAT=seat0
Environment=LIBSEAT_BACKEND=seatd
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

# Give Weston a moment to bind its socket.
for _ in $(seq 1 15); do
    [ -e /run/user/1000/wayland-0 ] && break
    sleep 1
done

echo -n 'mia local seat: '
loginctl list-sessions --no-legend | awk '$3=="mia" && $4=="seat0" {print "session "$1" on "$4}'
if ls /run/user/1000/wayland-* >/dev/null 2>&1; then
    echo "wayland socket: $(ls /run/user/1000/wayland-* 2>/dev/null | tr '\n' ' ')"
    echo "weston: up"
else
    echo "wayland socket: NOT up — recent weston journal:"
    journalctl -u mia-weston.service --no-pager -n 12 | sed 's/^/    /'
fi
