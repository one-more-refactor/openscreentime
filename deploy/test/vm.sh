#!/usr/bin/env bash
# ============================================================================
# Test-drive the managed-laptop agent in a DISPOSABLE Arch VM (QEMU/KVM).
#
# A container can't prove the real lock — the cgroup-v2 freezer is usually
# absent or read-only inside one (the agent now reports `screen_time_no_freezer`
# there). Only a real systemd + cgroup-v2 machine actually freezes a session,
# so this boots a throwaway Arch cloud image where you can watch it happen
# and never risk your own desktop. (Arch, not Ubuntu: the agent is built against
# the host's rolling glibc, newer than any Ubuntu LTS ships — see IMG_URL below.)
#
# THE SAFETY MODEL — you cannot brick anything permanent:
#   * The VM runs on an OVERLAY disk backed by the pristine cloud image. Reset
#     is `vm.sh reset` (deletes the overlay) — an instant, total rollback.
#   * Two users: `mia` is the MANAGED child; `rescue` is NEVER enrolled and has
#     sudo. If a lock freezes mia's session, `vm.sh rescue` SSHes in as rescue
#     and you run `vm.sh thaw` (stops the agent + unfreezes) or an unlock code.
#   * Keep tamper at Level 1 (the default) while testing — never pass
#     --tamper-max, which disables TTY switching and the systemctl-stop escape.
#   * Nothing here ever touches the HOST's cgroups, nft, or DNS.
#
# WHY A LOCAL SEAT, NOT SSH: the agent only counts screen time for LOCAL seat
# sessions (loginctl Active=yes AND Remote=no). An SSH login is Remote=yes and
# never accrues usage — so `vm.sh ssh` will NOT drive mia toward a lock. Use
# `vm.sh seat` to give mia a real tty1 autologin (a seat0 session) and to
# accelerate the agent's clock, which is what actually makes the lock bite.
#
# USAGE:
#   deploy/test/vm.sh up                 # fetch image + boot the VM (background)
#   deploy/test/vm.sh ssh                # shell in as the managed user `mia` (Remote — no usage)
#   deploy/test/vm.sh rescue             # shell in as `rescue` (your way back)
#   deploy/test/vm.sh install <token>    # build (--features gui) + copy + enroll + service
#   deploy/test/vm.sh seat [accel]       # give mia a GRAPHICAL Weston login + accel the agent (default 60)
#   deploy/test/vm.sh view               # watch mia's SCREEN in your browser (noVNC)
#   deploy/test/vm.sh unview             # stop the browser viewer server
#   deploy/test/vm.sh watch              # poll mia's cgroup freeze state until it flips (text)
#   deploy/test/vm.sh thaw               # rescue path: stop the agent + unfreeze mia
#   deploy/test/vm.sh console            # attach to the serial console (Ctrl-a x to quit)
#   deploy/test/vm.sh reset              # wipe the overlay disk (rollback)
#   deploy/test/vm.sh down               # power off the VM
#
# The VM reaches your local test server at http://10.0.2.2:8080 (host loopback,
# aliased to ost-host.local inside the VM so the agent's https-or-.local guard
# accepts the plain-http dev server).
# ============================================================================
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
work="${OST_VM_DIR:-$here/.vm}"
img="$work/arch-cloudimg.qcow2"
overlay="$work/overlay.qcow2"
seed="$work/seed.iso"
pidfile="$work/qemu.pid"
sshkey="$work/id_ed25519"
ssh_port=28022   # a high, uncontended host port (2222 is often taken by tunnels/bastions)
vnc_display=0    # QEMU VNC on 127.0.0.1:5900 (= 5900 + display), localhost-only
ws_port=5700     # QEMU's BUILT-IN VNC-over-websocket — noVNC connects straight here
novnc_port=6080  # local static server for the in-browser noVNC client
mem=3072         # a Wayland compositor + software GL wants more than the headless 2G
novnc_dir="$work/novnc"
# Arch, not Ubuntu, on purpose: the agent is built against the host's (rolling)
# glibc, which is newer than any Ubuntu LTS ships — an Ubuntu VM can't run the
# host binary and the musl route needs an extra cross-compiler. An Arch cloud
# image matches the host glibc exactly (the ordinary release build just runs)
# and is the representative target for the kids' Linux laptops anyway.
IMG_URL="https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2"

need() { command -v "$1" >/dev/null 2>&1 || { echo "missing: $1"; MISSING=1; }; }
preflight() {
    MISSING=0
    need qemu-system-x86_64
    need qemu-img
    need wget
    need ssh
    need ssh-keygen
    # A seed-ISO builder — any one of these works.
    if ! command -v cloud-localds >/dev/null 2>&1 \
        && ! command -v xorriso >/dev/null 2>&1 \
        && ! command -v genisoimage >/dev/null 2>&1; then
        echo "missing: a seed-ISO builder (cloud-localds OR xorriso OR genisoimage)"
        MISSING=1
    fi
    [ -e /dev/kvm ] || echo "note: no /dev/kvm — the VM will run (slowly) without KVM acceleration"
    if [ "$MISSING" = 1 ]; then
        cat <<EOF

Install the tooling first, e.g. on Arch:
    sudo pacman -S qemu-base wget xorriso cdrtools
(cloud-image-utils provides cloud-localds if you prefer; xorriso/cdrtools also work.)
EOF
        exit 1
    fi
}

build_seed() {
    mkdir -p "$work"
    [ -f "$sshkey" ] || ssh-keygen -q -t ed25519 -N "" -f "$sshkey"
    local pub; pub="$(cat "$sshkey.pub")"
    cat >"$work/meta-data" <<EOF
instance-id: ost-testdrive
local-hostname: kid-laptop
EOF
    # mia = managed child (a real seat session for the freezer to grab — an SSH
    # login under pam_systemd creates user-<uid>.slice, which is what gets
    # frozen). rescue = your unmanaged way back in. Throwaway-VM passwords let
    # you log in at the serial console too; both users also carry the SSH key.
    cat >"$work/user-data" <<EOF
#cloud-config
users:
  - name: mia
    groups: [wheel]
    shell: /bin/bash
    lock_passwd: false
    plain_text_passwd: mia
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys: ["$pub"]
  - name: rescue
    groups: [wheel]
    shell: /bin/bash
    lock_passwd: false
    plain_text_passwd: rescue
    sudo: ALL=(ALL) NOPASSWD:ALL
    ssh_authorized_keys: ["$pub"]
ssh_pwauth: true
package_update: false
runcmd:
  - [ bash, -c, "systemctl enable --now sshd 2>/dev/null || systemctl enable --now ssh 2>/dev/null || true" ]
EOF
    if command -v cloud-localds >/dev/null 2>&1; then
        cloud-localds "$seed" "$work/user-data" "$work/meta-data"
    elif command -v xorriso >/dev/null 2>&1; then
        xorriso -as mkisofs -output "$seed" -volid cidata -joliet -rock \
            "$work/user-data" "$work/meta-data" >/dev/null 2>&1
    else
        genisoimage -output "$seed" -volid cidata -joliet -rock \
            "$work/user-data" "$work/meta-data" >/dev/null 2>&1
    fi
}

cmd_up() {
    preflight
    mkdir -p "$work"
    if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
        echo "VM already running (pid $(cat "$pidfile"))."; return
    fi
    [ -f "$img" ] || { echo "==> fetching Arch Linux cloud image (~530 MB, once)"; wget -qO "$img" "$IMG_URL"; }
    if [ ! -f "$overlay" ]; then
        echo "==> creating disposable overlay disk (base image stays pristine)"
        qemu-img create -q -f qcow2 -F qcow2 -b "$img" "$overlay" 20G
        qemu-img resize -q "$overlay" 20G 2>/dev/null || true
    fi
    build_seed
    local accel=(); [ -e /dev/kvm ] && accel=(-enable-kvm -cpu host)
    echo "==> booting VM (serial log: $work/console.log)"
    qemu-system-x86_64 "${accel[@]}" -m "$mem" -smp 2 \
        -drive "file=$overlay,if=virtio" \
        -drive "file=$seed,if=virtio,format=raw" \
        -netdev "user,id=n0,hostfwd=tcp::$ssh_port-:22" -device virtio-net,netdev=n0 \
        -vga std -vnc "127.0.0.1:$vnc_display,websocket=$ws_port" \
        -serial "file:$work/console.log" -monitor none \
        -qmp "unix:$work/qmp.sock,server,nowait" \
        -pidfile "$pidfile" -daemonize
    echo "==> waiting for SSH (cloud-init runs on first boot, ~40-90s)…"
    for _ in $(seq 1 60); do
        if ssh_as rescue true 2>/dev/null; then
            echo "==> VM is up. The host test server is reachable inside at http://10.0.2.2:8080"
            echo "    next: register a parent + add a child device on the console, then:"
            echo "          deploy/test/vm.sh install <enroll-token>"
            return
        fi
        sleep 3
    done
    echo "!! SSH didn't come up — check $work/console.log"
    exit 1
}

ssh_as() {
    local user="$1"; shift
    # No ServerAliveInterval: under the software-rendered desktop's load a brief
    # stall would trip it and return 255 mid-command. A generous connect timeout
    # is enough; callers that must not miss retry.
    ssh -q -i "$sshkey" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -o ConnectTimeout=20 -p "$ssh_port" "$user@localhost" "$@"
}

cmd_install() {
    local token="${1:-}"
    [ -n "$token" ] || { echo "usage: vm.sh install <enroll-token>"; exit 1; }
    # Built with --features gui: the Arch VM's glibc matches the host's, so it
    # just runs. gui adds the real fullscreen egui lockout OVERLAY (in place of
    # the headless `wall` text broadcast) — which is the whole point of watching
    # this over VNC. It still locks via the cgroup freezer underneath.
    local bin="$root/client/target/release/openscreentime"
    echo "==> building the agent (release, --features gui)"
    ( cd "$root/client" && cargo build --release --features gui )
    # The host server (10.0.2.2 from inside QEMU user-net) is plain http, which
    # the agent refuses UNLESS the host is loopback or `.local` — a deliberate
    # anti-downgrade guard. Give it a `.local` alias so the dev URL is honoured
    # without weakening the check.
    local server="http://ost-host.local:8080"
    echo "==> copying agent into the VM and enrolling against $server (→ 10.0.2.2)"
    scp -q -i "$sshkey" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -P "$ssh_port" "$bin" rescue@localhost:/tmp/openscreentime
    # dnsmasq + nftables so DNS/firewall enforcement works too (the freeze test
    # needs neither — cgroup2 + logind are already there — but this makes the
    # full loop testable). Best-effort; a missing resolver just degrades.
    ssh_as rescue "grep -q ost-host.local /etc/hosts || echo '10.0.2.2 ost-host.local' | sudo tee -a /etc/hosts >/dev/null; \
        sudo pacman -Sy --noconfirm --needed nftables dnsmasq >/dev/null 2>&1 || true; \
        sudo install -m0755 /tmp/openscreentime /usr/local/bin/openscreentime \
        && sudo openscreentime enroll --server $server --token '$token' \
        && sudo openscreentime install-service \
        && sudo openscreentime status"
    cat <<EOF

==> enrolled. To watch the lock actually bite:
    1. In the console, give mia's Kid profile a tiny daily limit (e.g. 1 min).
       (SSH does NOT count as screen time — the agent ignores Remote sessions —
        so mia needs a real LOCAL seat, which the next step sets up.)
    2. deploy/test/vm.sh seat          # mia autologin on tty1 (a seat0 session)
                                       # + accelerate the agent (1 real sec = 1 sim min)
    3. deploy/test/vm.sh watch         # poll mia's freezer; it flips to 1 when the
                                       # limit is hit (after a short on-screen countdown).
    4. Recover — the lock is STICKY (hitting the daily limit locks mia for the
       day; being back "under budget" does NOT auto-thaw — that needs an unlock
       grant). The guaranteed way back, since rescue is unmanaged:
         deploy/test/vm.sh thaw        # stop the agent + write 0 to the freezer
       Or the real UX: an unlock code / earn-time grant from the console.
EOF
}

# Give mia a real GRAPHICAL local seat: autologin on tty1 → a Weston (Wayland)
# session. That does three things at once — it is a LOCAL seat (Active=yes,
# Remote=no) so the agent counts it as screen time; it puts a /run/user/1000/
# wayland-0 socket where the lockout overlay looks for it; and it renders a
# desktop QEMU's VGA scans out, so VNC/noVNC shows it. Also accelerates the
# agent's clock so the daily budget is reachable in seconds.
cmd_seat() {
    local accel="${1:-60}"
    echo "==> mia: graphical (Weston) autologin on tty1 + agent --time-accel $accel"
    # Robust over SSH: stage a real script and run it, rather than fight nested
    # shell quoting for the heredocs it writes.
    scp -q -i "$sshkey" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -P "$ssh_port" "$here/seat-setup.sh" rescue@localhost:/tmp/seat-setup.sh
    ssh_as rescue "sudo bash /tmp/seat-setup.sh $accel"
    echo "==> watch it in the browser:  deploy/test/vm.sh view"
}

# Poll mia's cgroup-v2 freezer until it flips (or ~2 min elapse).
cmd_watch() {
    ssh_as rescue 'uid=$(id -u mia); f=/sys/fs/cgroup/user.slice/user-$uid.slice/cgroup.freeze
        echo "watching $f (Ctrl-C to stop)"
        for i in $(seq 1 60); do
            v=$(cat "$f" 2>/dev/null || echo "?")
            printf "t=%3ds freeze=%s\n" "$((i*2))" "$v"
            [ "$v" = "1" ] && { echo ">>> FROZEN — mia'"'"'s whole seat is suspended. Recover: vm.sh thaw"; exit 0; }
            sleep 2
        done
        echo "(still 0 — is mia on a LOCAL seat? run vm.sh seat; is the limit tiny?)"'
}

# The guaranteed rescue. The agent is Restart=always AND has a watchdog timer
# that re-starts it, so a plain `stop` doesn't hold — it re-freezes mia within
# seconds. The durable move is to stop the watchdog and MASK the agent (so no
# restart can bring it back), then thaw. rescue is never enrolled, so this works
# even when mia is fully locked out.
cmd_thaw() {
    ssh_as rescue 'uid=$(id -u mia); f=/sys/fs/cgroup/user.slice/user-$uid.slice/cgroup.freeze
        sudo systemctl stop openscreentime-watchdog.timer 2>/dev/null || true
        sudo systemctl mask --now openscreentime-agent.service >/dev/null 2>&1 || sudo systemctl stop openscreentime-agent.service
        sleep 1
        echo 0 | sudo tee "$f" >/dev/null 2>&1 || true
        sleep 3   # prove it stays down (the watchdog would have re-frozen by now)
        echo "agent=$(systemctl is-active openscreentime-agent.service) freeze=$(cat "$f" 2>/dev/null || echo n/a)"
        echo "mia is thawed and the agent is masked. Re-arm with:"
        echo "  sudo systemctl unmask openscreentime-agent.service && sudo systemctl start openscreentime-agent.service openscreentime-watchdog.timer"'
}

# Watch the VM's SCREEN in a browser. QEMU already serves the framebuffer as a
# VNC-over-websocket on 127.0.0.1:$ws_port (see cmd_up); noVNC is the static
# HTML/JS client. We just serve the noVNC directory over http and hand you a URL
# that points its websocket at QEMU. Nothing is exposed off localhost.
cmd_view() {
    [ -f "$novnc_dir/vnc.html" ] || { echo "noVNC assets missing at $novnc_dir (git clone https://github.com/novnc/noVNC.git \"$novnc_dir\")"; exit 1; }
    if [ -f "$work/novnc.pid" ] && kill -0 "$(cat "$work/novnc.pid")" 2>/dev/null; then
        echo "noVNC server already running (pid $(cat "$work/novnc.pid"))."
    else
        ( cd "$novnc_dir" && python3 -m http.server "$novnc_port" --bind 127.0.0.1 >"$work/novnc.log" 2>&1 & echo $! >"$work/novnc.pid" )
        sleep 1
    fi
    local url="http://localhost:$novnc_port/vnc.html?host=localhost&port=$ws_port&resize=scale&autoconnect=1"
    echo "==> open this in your browser:"
    echo "      $url"
    echo "    (mia's Weston desktop; the lockout overlay appears fullscreen when the limit hits.)"
    echo "    stop the viewer server later with: vm.sh unview"
    command -v xdg-open >/dev/null 2>&1 && xdg-open "$url" >/dev/null 2>&1 &
    true
}

cmd_unview() {
    [ -f "$work/novnc.pid" ] && kill "$(cat "$work/novnc.pid")" 2>/dev/null && rm -f "$work/novnc.pid" && echo "noVNC server stopped." || echo "noVNC server not running."
}

# Grab a still of the VM's screen via QMP screendump (a headless screenshot —
# handy for eyeballing/CI without a browser). QEMU writes PPM; we convert to PNG
# if a converter is around, else leave the .ppm. Absolute path — QEMU resolves
# the filename against its own cwd, not yours.
cmd_shot() {
    local out; out="$(readlink -f "${1:-$work/screen.png}")"
    [ -S "$work/qmp.sock" ] || { echo "no QMP socket — is the VM up (with the current vm.sh)?"; exit 1; }
    local ppm="$work/.shot.ppm"; rm -f "$ppm"
    python3 - "$work/qmp.sock" "$ppm" <<'PY'
import socket, json, sys
sock_path, out = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX); s.connect(sock_path); f = s.makefile("rwb")
def cmd(o): f.write((json.dumps(o)+"\n").encode()); f.flush()
f.readline()                                   # greeting
cmd({"execute":"qmp_capabilities"}); f.readline()
cmd({"execute":"screendump","arguments":{"filename":out}})
for _ in range(80):
    line = f.readline()
    if not line: break
    try: msg = json.loads(line)
    except: continue
    if "error" in msg: sys.exit("QMP error: %s" % msg["error"]);
    if "return" in msg: break
PY
    if command -v magick >/dev/null 2>&1; then magick "$ppm" "$out" && rm -f "$ppm"
    elif command -v convert >/dev/null 2>&1; then convert "$ppm" "$out" && rm -f "$ppm"
    elif command -v ffmpeg >/dev/null 2>&1; then ffmpeg -y -loglevel error -i "$ppm" "$out" && rm -f "$ppm"
    else out="${out%.png}.ppm"; mv "$ppm" "$out"; fi
    echo "wrote $out"
}

# Reset for a fresh, watchable lock: clear the persisted usage ledger + freeze
# state, thaw mia (which un-suspends her Weston too), kill any leftover overlay,
# and re-arm the agent at $accel. Then you can watch the desktop → "Time's up"
# overlay transition again from a clean slate. mia's daily budget comes from the
# console/DB — set her Kid limit small first (e.g. 1 min), and note the agent
# only re-reads policy on (re)start, which this does.
cmd_relock() {
    local accel="${1:-10}"
    # Discrete, tolerant steps. Every ssh_as gets `|| true` at the LOCAL level:
    # the script runs under `set -e`, and SSH itself can return 255 under the
    # software-rendered desktop's load even when the remote command succeeded.
    ssh_as rescue "sudo systemctl unmask openscreentime-agent.service 2>/dev/null; sudo systemctl stop openscreentime-agent.service openscreentime-watchdog.timer 2>/dev/null; true" || true
    ssh_as rescue "sudo pkill -f __lockout 2>/dev/null; echo 0 | sudo tee /sys/fs/cgroup/user.slice/user-\$(id -u mia).slice/cgroup.freeze >/dev/null 2>&1; true" || true
    ssh_as rescue "sudo rm -f /var/lib/openscreentime/usage_ledger.json /var/lib/openscreentime/freeze_state.json; true" || true
    ssh_as rescue "printf '[Service]\nExecStart=\nExecStart=/usr/local/bin/openscreentime --time-accel $accel run\n' | sudo tee /etc/systemd/system/openscreentime-agent.service.d/accel.conf >/dev/null; sudo systemctl daemon-reload" || true
    # Start + verify with a couple retries — SSH can 255 under the VM's load.
    local ok=""
    for _ in 1 2 3; do
        ssh_as rescue "sudo systemctl start openscreentime-agent.service openscreentime-watchdog.timer" >/dev/null 2>&1 || true
        if [ "$(ssh_as rescue 'systemctl is-active openscreentime-agent.service' 2>/dev/null)" = active ]; then ok=1; break; fi
        sleep 2
    done
    if [ -n "$ok" ]; then echo "==> clean slate — agent re-armed, accel=$accel."
    else echo "==> agent did NOT come active (SSH flaked under load) — just run: vm.sh relock $accel"; fi
    echo "    Watch in the browser (vm.sh view); the 'Time's up' overlay lands once mia's"
    echo "    accelerated screen time passes her daily limit."
}

case "${1:-}" in
    up)      cmd_up ;;
    ssh)     shift; ssh_as mia "$@" ;;
    rescue)  shift; ssh_as rescue "$@" ;;
    install) shift; cmd_install "$@" ;;
    seat)    shift; cmd_seat "$@" ;;
    watch)   cmd_watch ;;
    thaw)    cmd_thaw ;;
    view)    cmd_view ;;
    unview)  cmd_unview ;;
    shot)    shift; cmd_shot "$@" ;;
    relock)  shift; cmd_relock "$@" ;;
    console) exec tail -f "$work/console.log" ;;
    reset)   rm -f "$overlay"; echo "overlay wiped — next 'up' boots a pristine VM." ;;
    down)    [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null && rm -f "$pidfile" && echo "VM stopped." || echo "not running." ;;
    *) grep '^#' "$0" | sed 's/^# \{0,1\}//' | head -54 ;;
esac
