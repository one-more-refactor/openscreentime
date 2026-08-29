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
#   deploy/test/vm.sh install <token>    # copy the built agent in + enroll + service
#   deploy/test/vm.sh seat [accel]       # give mia a LOCAL tty1 login + accel the agent (default 60)
#   deploy/test/vm.sh watch              # poll mia's cgroup freeze state until it flips
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
    qemu-system-x86_64 "${accel[@]}" -m 2048 -smp 2 \
        -drive "file=$overlay,if=virtio" \
        -drive "file=$seed,if=virtio,format=raw" \
        -netdev "user,id=n0,hostfwd=tcp::$ssh_port-:22" -device virtio-net,netdev=n0 \
        -display none -serial "file:$work/console.log" -monitor none \
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
    ssh -q -i "$sshkey" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -o ConnectTimeout=4 -p "$ssh_port" "$user@localhost" "$@"
}

cmd_install() {
    local token="${1:-}"
    [ -n "$token" ] || { echo "usage: vm.sh install <enroll-token>"; exit 1; }
    # Ordinary release build — the Arch VM's glibc matches the host's, so it
    # just runs. The desktop (gui/tray) features aren't needed to prove the
    # freeze; the headless build locks via cgroup + a wall broadcast.
    local bin="$root/client/target/release/openscreentime"
    echo "==> building the agent (release)"
    ( cd "$root/client" && cargo build --release )
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

# Give mia a LOCAL seat login (tty1 autologin → loginctl Active=yes, Remote=no,
# Seat=seat0) — the only kind of session the agent counts as screen time — and
# accelerate the agent's clock so the daily budget is reachable in seconds.
cmd_seat() {
    local accel="${1:-60}"
    echo "==> mia: tty1 autologin (local seat) + agent --time-accel $accel"
    ssh_as rescue "set -e
        sudo mkdir -p /etc/systemd/system/getty@tty1.service.d /etc/systemd/system/openscreentime-agent.service.d
        printf '[Service]\nExecStart=\nExecStart=-/sbin/agetty --autologin mia --noclear %%I 38400 linux\n' \
            | sudo tee /etc/systemd/system/getty@tty1.service.d/autologin.conf >/dev/null
        printf '[Service]\nExecStart=\nExecStart=/usr/local/bin/openscreentime --time-accel $accel run\n' \
            | sudo tee /etc/systemd/system/openscreentime-agent.service.d/accel.conf >/dev/null
        sudo systemctl daemon-reload
        sudo systemctl restart getty@tty1
        sudo systemctl restart openscreentime-agent.service
        sleep 2
        echo -n 'mia local seat: '; loginctl list-sessions --no-legend | awk '\$3==\"mia\" && \$4==\"seat0\" {print \"session \"\$1\" on \"\$4}'"
    echo "==> now: deploy/test/vm.sh watch"
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

case "${1:-}" in
    up)      cmd_up ;;
    ssh)     shift; ssh_as mia "$@" ;;
    rescue)  shift; ssh_as rescue "$@" ;;
    install) shift; cmd_install "$@" ;;
    seat)    shift; cmd_seat "$@" ;;
    watch)   cmd_watch ;;
    thaw)    cmd_thaw ;;
    console) exec tail -f "$work/console.log" ;;
    reset)   rm -f "$overlay"; echo "overlay wiped — next 'up' boots a pristine VM." ;;
    down)    [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null && rm -f "$pidfile" && echo "VM stopped." || echo "not running." ;;
    *) grep '^#' "$0" | sed 's/^# \{0,1\}//' | head -52 ;;
esac
