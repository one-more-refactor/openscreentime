#!/usr/bin/env bash
# ============================================================================
# A fresh GNOME "child's device" in QEMU, watchable over VNC in the browser.
#
# This is the realistic target: a plain Debian 12 + GNOME (Wayland) install that
# looks like a new kid's laptop out of the box — NOT enrolled, no agent, no
# OpenScreenTime anything. Pair it with the panel running on the host
# (server :8080 + web console :5173) and enrol it from there when you want.
#
# Unlike the throwaway freeze-test VM, this box is PERSISTENT (a normal install
# on its own disk — no disposable overlay). GNOME first-boot installs the
# desktop over apt, so the very first `up` takes a while (watch: gnome-vm.sh
# console). After that it boots straight to the GNOME desktop, autologin as the
# child user `emma`.
#
# USAGE:
#   deploy/test/gnome-vm.sh up          # boot (first time: installs GNOME, ~10-20 min)
#   deploy/test/gnome-vm.sh view        # watch the GNOME screen in your browser (noVNC)
#   deploy/test/gnome-vm.sh unview      # stop the browser viewer server
#   deploy/test/gnome-vm.sh shot [file] # headless screenshot (QMP screendump → PNG)
#   deploy/test/gnome-vm.sh ssh [cmd]   # shell in as emma (sudo; for setup only)
#   deploy/test/gnome-vm.sh console     # tail the serial/cloud-init log
#   deploy/test/gnome-vm.sh status      # is GNOME up yet?
#   deploy/test/gnome-vm.sh down        # power off
#   deploy/test/gnome-vm.sh nuke        # power off + delete the disk (start over)
#
# The VM reaches the host panel at http://10.0.2.2:8080 (agent) / :5173 (web).
# ============================================================================
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
work="$here/.gnome"
base="$work/debian12.qcow2"
disk="$work/gnome-disk.qcow2"
seed="$work/seed.iso"
pidfile="$work/qemu.pid"
sshkey="$work/id_ed25519"
novnc_dir="$here/.vm/novnc"          # reuse the noVNC client cloned for the other harness
ssh_port=28122
ws_port=5702
vnc_display=1                        # 127.0.0.1:5901
novnc_port=6081
mem=4096

need(){ command -v "$1" >/dev/null 2>&1 || { echo "missing: $1"; MISS=1; }; }
preflight(){
    MISS=0; need qemu-system-x86_64; need qemu-img; need ssh; need ssh-keygen; need python3
    command -v cloud-localds >/dev/null 2>&1 || command -v xorriso >/dev/null 2>&1 || command -v genisoimage >/dev/null 2>&1 \
        || { echo "missing: a seed-ISO builder (cloud-localds / xorriso / genisoimage)"; MISS=1; }
    [ -f "$base" ] || { echo "missing base image $base — re-download the Debian cloud qcow2"; MISS=1; }
    [ "${MISS:-0}" = 1 ] && exit 1 || true
}

build_seed(){
    [ -f "$sshkey" ] || ssh-keygen -q -t ed25519 -N "" -f "$sshkey"
    local pub; pub="$(cat "$sshkey.pub")"
    cat >"$work/meta-data" <<EOF
instance-id: gnome-childbox
local-hostname: emma-laptop
EOF
    # emma = the kid. Autologin to GNOME; a throwaway password + the ssh key for
    # setup only. gnome-initial-setup is pre-dismissed so autologin lands on a
    # clean desktop (a fresh device, not a setup wizard).
    cat >"$work/user-data" <<EOF
#cloud-config
hostname: emma-laptop
users:
  - name: emma
    groups: [sudo, video, audio]
    shell: /bin/bash
    lock_passwd: false
    plain_text_passwd: emma
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys: ["$pub"]
ssh_pwauth: true
package_update: true
packages:
  - gnome-core
  - gdm3
  - gnome-terminal
  - firefox-esr
  - mesa-utils
runcmd:
  - [ bash, -c, "install -d -m0755 /etc/gdm3 && printf '[daemon]\\nAutomaticLoginEnable=true\\nAutomaticLogin=emma\\nWaylandEnable=true\\n' > /etc/gdm3/daemon.conf" ]
  - [ bash, -c, "install -d -o emma -g emma -m0700 /home/emma/.config && : > /home/emma/.config/gnome-initial-setup-done && chown emma:emma /home/emma/.config/gnome-initial-setup-done" ]
  - [ systemctl, set-default, graphical.target ]
  - [ systemctl, enable, gdm3 ]
  - [ bash, -c, "touch /var/lib/gnome-ready && systemctl isolate graphical.target || true" ]
EOF
    if command -v cloud-localds >/dev/null 2>&1; then
        cloud-localds "$seed" "$work/user-data" "$work/meta-data"
    elif command -v xorriso >/dev/null 2>&1; then
        xorriso -as mkisofs -output "$seed" -volid cidata -joliet -rock "$work/user-data" "$work/meta-data" >/dev/null 2>&1
    else
        genisoimage -output "$seed" -volid cidata -joliet -rock "$work/user-data" "$work/meta-data" >/dev/null 2>&1
    fi
}

ssh_as(){ ssh -q -i "$sshkey" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=20 -p "$ssh_port" "emma@localhost" "$@"; }

cmd_up(){
    preflight
    if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then echo "already running (pid $(cat "$pidfile"))."; return; fi
    [ -f "$disk" ] || { echo "creating working disk from base"; cp "$base" "$disk"; qemu-img resize -q "$disk" 14G; }
    build_seed
    local accel=(); [ -e /dev/kvm ] && accel=(-enable-kvm -cpu host)
    echo "==> booting GNOME child box (serial log: $work/console.log)"
    # virtio-vga: a KMS framebuffer GNOME/mutter scans out (software GL via
    # llvmpipe — no host GPU needed); QEMU shows it over VNC + a websocket noVNC
    # connects to. QMP socket for headless screenshots.
    qemu-system-x86_64 "${accel[@]}" -m "$mem" -smp 4 \
        -drive "file=$disk,if=virtio" \
        -drive "file=$seed,if=virtio,format=raw" \
        -netdev "user,id=n0,hostfwd=tcp::$ssh_port-:22" -device virtio-net,netdev=n0 \
        -vga std -vnc "127.0.0.1:$vnc_display,websocket=$ws_port" \
        -serial "file:$work/console.log" -monitor none \
        -qmp "unix:$work/qmp.sock,server,nowait" \
        -pidfile "$pidfile" -daemonize
    echo "==> waiting for SSH…"
    local i; for i in $(seq 1 60); do ssh_as true 2>/dev/null && { echo "==> VM up."; break; }; sleep 3; done
    echo "==> First boot installs GNOME over apt (~10-20 min). Track it with:"
    echo "      deploy/test/gnome-vm.sh status      # says READY when the desktop is up"
    echo "      deploy/test/gnome-vm.sh view        # watch it in the browser"
}

cmd_status(){
    if ! ssh_as true 2>/dev/null; then echo "SSH not up yet (VM still booting) — deploy/test/gnome-vm.sh console"; return; fi
    if ssh_as 'test -f /var/lib/gnome-ready' 2>/dev/null && ssh_as 'systemctl is-active gdm3 >/dev/null 2>&1' 2>/dev/null; then
        echo "READY — GNOME is installed and gdm3 is up. Watch: deploy/test/gnome-vm.sh view"
        ssh_as 'echo "  gnome-shell: $(pgrep -c gnome-shell 2>/dev/null || echo 0) proc(s); session: $(loginctl list-sessions --no-legend 2>/dev/null | awk "{print \$3}" | tr "\n" " ")"' 2>/dev/null || true
    else
        echo "still installing GNOME (apt) — deploy/test/gnome-vm.sh console  to watch cloud-init"
        ssh_as 'echo "  cloud-init: $(cloud-init status 2>/dev/null | tr -d "\n"); dpkg gnome-core: $(dpkg -s gnome-core 2>/dev/null | grep -m1 Status | cut -d: -f2-)"' 2>/dev/null || true
    fi
}

cmd_view(){
    [ -f "$novnc_dir/vnc.html" ] || { echo "noVNC assets missing at $novnc_dir"; exit 1; }
    if [ -f "$work/novnc.pid" ] && kill -0 "$(cat "$work/novnc.pid")" 2>/dev/null; then echo "viewer already running."; else
        ( cd "$novnc_dir" && python3 -m http.server "$novnc_port" --bind 127.0.0.1 >"$work/novnc.log" 2>&1 & echo $! >"$work/novnc.pid" ); sleep 1
    fi
    # path= empty: QEMU serves the raw VNC websocket at the root (noVNC would
    # otherwise default the path to `websockify`, which QEMU 404s).
    echo "==> open in your browser:"
    echo "      http://localhost:$novnc_port/vnc.html?host=localhost&port=$ws_port&path=&resize=scale&autoconnect=1"
    echo "    (emma's fresh GNOME desktop — unenrolled. Stop the viewer: gnome-vm.sh unview)"
}
cmd_unview(){ [ -f "$work/novnc.pid" ] && kill "$(cat "$work/novnc.pid")" 2>/dev/null && rm -f "$work/novnc.pid" && echo "viewer stopped." || echo "viewer not running."; }

cmd_shot(){
    local out; out="$(readlink -f "${1:-$work/screen.png}")"
    [ -S "$work/qmp.sock" ] || { echo "no QMP socket — is the VM up?"; exit 1; }
    local ppm="$work/.shot.ppm"; rm -f "$ppm"
    python3 - "$work/qmp.sock" "$ppm" <<'PY'
import socket, json, sys
p,out=sys.argv[1],sys.argv[2]
s=socket.socket(socket.AF_UNIX); s.connect(p); f=s.makefile("rwb")
def c(o): f.write((json.dumps(o)+"\n").encode()); f.flush()
f.readline(); c({"execute":"qmp_capabilities"}); f.readline()
c({"execute":"screendump","arguments":{"filename":out}})
for _ in range(80):
    l=f.readline()
    if not l: break
    try: m=json.loads(l)
    except: continue
    if "error" in m: sys.exit("QMP error: %s"%m["error"])
    if "return" in m: break
PY
    if command -v magick >/dev/null 2>&1; then magick "$ppm" "$out" && rm -f "$ppm"
    elif command -v convert >/dev/null 2>&1; then convert "$ppm" "$out" && rm -f "$ppm"
    else out="${out%.png}.ppm"; mv "$ppm" "$out"; fi
    echo "wrote $out"
}

case "${1:-}" in
    up)      cmd_up ;;
    view)    cmd_view ;;
    unview)  cmd_unview ;;
    shot)    shift; cmd_shot "$@" ;;
    status)  cmd_status ;;
    ssh)     shift; ssh_as "$@" ;;
    console) exec tail -f "$work/console.log" ;;
    down)    [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null && rm -f "$pidfile" && echo "stopped." || echo "not running." ;;
    nuke)    [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null; rm -f "$pidfile" "$disk"; echo "powered off + disk deleted." ;;
    *) grep '^#' "$0" | sed 's/^# \{0,1\}//' | head -34 ;;
esac
