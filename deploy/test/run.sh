#!/usr/bin/env bash
# Drive the throwaway managed-laptop container (see Containerfile next to this).
#
#   deploy/test/run.sh build [agent-binary]     # image + picks the agent binary
#   deploy/test/run.sh up <server-url> <enroll-token> [name]
#   deploy/test/run.sh sh                       # root shell inside
#   deploy/test/run.sh exec <cmd...>
#   deploy/test/run.sh status                   # agent status + service + nft + dnsmasq
#   deploy/test/run.sh dns <domain>             # resolve from inside (is it sinkholed?)
#   deploy/test/run.sh offline | online         # cut / restore the network (presence test)
#   deploy/test/run.sh logs                     # journal of the agent unit
#   deploy/test/run.sh down
#
# A local dev server can be reached from inside as http://ost.local:<port>
# (the agent only accepts plain http for loopback/.local hosts).
#
# Agent binary: pass a path to `build`, or it is taken from
# client/target/x86_64-unknown-linux-musl/release/openscreentime, or extracted
# from the locally built server image (localhost/openscreentime_server:latest,
# /app/agent/openscreentime-<ver>-x86_64-musl). The glibc desktop build from an
# Arch host will NOT run on the Debian image — use the musl one.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
name="${OST_TEST_NAME:-ost-kid-laptop}"
img="localhost/ost-test-laptop:latest"
bin_dir="$root/deploy/test/.bin"

pick_binary() {
    local given="${1:-}"
    mkdir -p "$bin_dir"
    if [[ -n "$given" ]]; then cp -f "$given" "$bin_dir/openscreentime"; return; fi
    local musl="$root/client/target/x86_64-unknown-linux-musl/release/openscreentime"
    if [[ -x "$musl" ]]; then cp -f "$musl" "$bin_dir/openscreentime"; return; fi
    echo "==> extracting the musl agent from localhost/openscreentime_server:latest"
    local cid; cid="$(podman create localhost/openscreentime_server:latest)"
    local f; f="$(podman run --rm --entrypoint sh localhost/openscreentime_server:latest -c 'ls /app/agent | grep musl | head -1')"
    podman cp "$cid:/app/agent/$f" "$bin_dir/openscreentime"
    podman rm "$cid" >/dev/null
    chmod +x "$bin_dir/openscreentime"
}

case "${1:-}" in
    build)
        pick_binary "${2:-}"
        podman build -t "$img" -f "$here/Containerfile" "$here"
        echo "==> built $img; agent: $("$bin_dir/openscreentime" --version 2>/dev/null || echo '?')"
        ;;
    up)
        server="${2:?server url}"; token="${3:?enroll token}"; devname="${4:-$name}"
        podman rm -f "$name" >/dev/null 2>&1 || true
        # --systemd=always boots /sbin/init; NET_ADMIN/NET_RAW for nft inside the
        # container's own netns; SYS_ADMIN is needed for the cgroup freezer and
        # chattr +i on resolv.conf (SYS_ADMIN inside a rootless userns is not
        # host root — it is scoped to the namespace).
        podman run -d --name "$name" --hostname "$devname" \
            --systemd=always --dns 1.1.1.1 --add-host ost.local:host-gateway \
            --cap-add NET_ADMIN,NET_RAW,SYS_ADMIN,LINUX_IMMUTABLE,AUDIT_WRITE \
            --security-opt unmask=/sys/fs/cgroup \
            -v "$bin_dir/openscreentime:/usr/local/bin/openscreentime:ro" \
            -e OST_SERVER="$server" -e OST_TOKEN="$token" \
            "$img" >/dev/null
        echo "==> booting systemd"; sleep 4
        podman exec "$name" systemctl is-system-running --wait >/dev/null 2>&1 || true
        podman exec "$name" systemctl is-active dnsmasq
        echo "==> enrolling against $server"
        podman exec "$name" openscreentime enroll --server "$server" --token "$token"
        echo "==> installing the hardened unit (+ PAM/sudoers parent code)"
        podman exec "$name" openscreentime install-service || true
        sleep 3
        podman exec "$name" systemctl is-active openscreentime-agent || podman exec "$name" systemctl status openscreentime-agent --no-pager | tail -20
        ;;
    sh) exec podman exec -it "$name" bash ;;
    exec) shift; exec podman exec "$name" "$@" ;;
    status)
        podman exec "$name" openscreentime status || true
        podman exec "$name" systemctl is-active openscreentime-agent dnsmasq
        podman exec "$name" sh -c 'nft list table inet openscreentime 2>/dev/null | head -40; echo; ls -la /etc/openscreentime /etc/openscreentime/dnsmasq.d 2>/dev/null; grep -c address= /etc/openscreentime/dnsmasq.d/*.conf 2>/dev/null'
        ;;
    dns) podman exec "$name" sh -c "getent hosts ${2:?domain} || echo 'NXDOMAIN / blocked'" ;;
    offline) podman exec "$name" sh -c 'nft add table inet osttest; nft add chain inet osttest out "{ type filter hook output priority -300; }"; nft add rule inet osttest out ip daddr != 127.0.0.0/8 drop'; echo "network cut" ;;
    online) podman exec "$name" sh -c 'nft delete table inet osttest' ; echo "network restored" ;;
    logs) podman exec "$name" journalctl -u openscreentime-agent --no-pager -n "${2:-80}" ;;
    down) podman rm -f "$name" ;;
    *) sed -n 2,14p "$0"; exit 2 ;;
esac
