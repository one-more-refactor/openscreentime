#!/usr/bin/env bash
# ============================================================================
# Test-drive the managed-laptop agent in a DISPOSABLE Ubuntu VM (QEMU/KVM).
#
# A container can't prove the real lock — the cgroup-v2 freezer is usually
# absent or read-only inside one (the agent now reports `screen_time_no_freezer`
# there). Only a real systemd + cgroup-v2 machine actually freezes a session,
# so this boots a throwaway Ubuntu cloud image where you can watch it happen
# and never risk your own desktop.
#
# THE SAFETY MODEL — you cannot brick anything permanent:
#   * The VM runs on an OVERLAY disk backed by the pristine cloud image. Reset
#     is `vm.sh reset` (deletes the overlay) — an instant, total rollback.
#   * Two users: `mia` is the MANAGED child; `rescue` is NEVER enrolled and has
#     sudo. If a lock freezes mia's session, `vm.sh rescue` SSHes in as rescue
#     and you run `sudo openscreentime unlock` or `sudo systemctl stop`.
#   * Keep tamper at Level 1 (the default) while testing — never pass
#     --tamper-max, which disables TTY switching and the systemctl-stop escape.
#   * Nothing here ever touches the HOST's cgroups, nft, or DNS.
#
# USAGE:
#   deploy/test/vm.sh up                 # fetch image + boot the VM (background)
#   deploy/test/vm.sh ssh                # shell in as the managed user `mia`
#   deploy/test/vm.sh rescue             # shell in as `rescue` (your way back)
#   deploy/test/vm.sh install <token>    # copy the built agent in + enroll + service
#   deploy/test/vm.sh console            # attach to the serial console (Ctrl-a x to quit)
#   deploy/test/vm.sh reset              # wipe the overlay disk (rollback)
#   deploy/test/vm.sh down               # power off the VM
#
# The VM reaches your local test server at http://10.0.2.2:8080 (host loopback).
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
ssh_port=2222
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
    [ -f "$img" ] || { echo "==> fetching Ubuntu 24.04 cloud image (~600 MB, once)"; wget -qO "$img" "$IMG_URL"; }
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
        -nographic -serial "file:$work/console.log" \
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
    echo "==> copying agent into the VM and enrolling against http://10.0.2.2:8080"
    scp -q -i "$sshkey" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -P "$ssh_port" "$bin" rescue@localhost:/tmp/openscreentime
    # dnsmasq + nftables so DNS/firewall enforcement works too (the freeze test
    # needs neither — cgroup2 + logind are already there — but this makes the
    # full loop testable). Best-effort; a missing resolver just degrades.
    ssh_as rescue "sudo pacman -Sy --noconfirm --needed nftables dnsmasq >/dev/null 2>&1 || true; \
        sudo install -m0755 /tmp/openscreentime /usr/local/bin/openscreentime \
        && sudo openscreentime enroll --server http://10.0.2.2:8080 --token '$token' \
        && sudo openscreentime install-service \
        && sudo openscreentime status"
    cat <<EOF

==> enrolled. To watch the lock actually bite:
    1. In the console, assign the child a Kid profile with a tiny daily limit.
    2. deploy/test/vm.sh ssh          # log in AS mia (creates a real seat)
       mia\$ yes > /dev/null &        # something to freeze
    3. Speed it up: the agent honours --time-accel, so re-run the service with
       'sudo systemctl edit openscreentime' adding
       ExecStart= …/openscreentime --time-accel 60 run   (1 real sec = 1 sim min),
       or just wait out the real limit.
    4. When the limit hits, mia's session freezes. Prove it from rescue:
       deploy/test/vm.sh rescue
       rescue\$ cat /sys/fs/cgroup/user.slice/user-\$(id -u mia).slice/cgroup.freeze  # -> 1
    5. Recover: rescue\$ sudo openscreentime unlock   (enter the code from the
       console → Settings → Unlock codes), or sudo systemctl stop openscreentime.
EOF
}

case "${1:-}" in
    up)      cmd_up ;;
    ssh)     ssh_as mia ;;
    rescue)  ssh_as rescue ;;
    install) shift; cmd_install "$@" ;;
    console) exec tail -f "$work/console.log" ;;
    reset)   rm -f "$overlay"; echo "overlay wiped — next 'up' boots a pristine VM." ;;
    down)    [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null && rm -f "$pidfile" && echo "VM stopped." || echo "not running." ;;
    *) grep '^#' "$0" | sed 's/^# \{0,1\}//' | head -40 ;;
esac
