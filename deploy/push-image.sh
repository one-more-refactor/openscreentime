#!/usr/bin/env bash
# OpenScreenTime — build the server image HERE (a fast dev box) and push it to
# the host that runs the compose stack, instead of compiling Rust on the server.
#
# The production host is often a one-core LXC; `deploy/update.sh` (git pull +
# in-place image build) takes 40+ minutes there and starves everything else on
# the node. This script builds the exact same Containerfile locally, streams the
# image over SSH, loads it, and recreates the stack. The host's checkout is also
# fast-forwarded so `docs/`, `compose.yaml` and `.env.example` match the image.
#
# Usage:
#   deploy/push-image.sh <ssh-target> [--repo /opt/openscreentime] [--via "<prefix>"]
#
#   <ssh-target>   e.g. root@192.168.8.131, or a Proxmox node when combined
#                  with --via "pct exec 141 --" (the LXC has no reachable sshd).
#   --repo         checkout path on the host (default /opt/openscreentime)
#   --via          command prefix run ON the ssh target to reach the real host
#                  (default: none). Example: --via "pct exec 141 --"
#   --no-build     skip the local image build (reuse localhost/openscreentime_server:latest)
#
# Requires podman (or docker) locally; curl on the host.
set -euo pipefail

target="${1:?usage: deploy/push-image.sh <ssh-target> [--repo PATH] [--via PREFIX] [--no-build]}"
shift
repo="/opt/openscreentime"
via=""
build=1
while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo) repo="$2"; shift 2 ;;
        --via) via="$2"; shift 2 ;;
        --no-build) build=0; shift ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"

engine=""
if command -v podman >/dev/null 2>&1; then engine=podman
elif command -v docker >/dev/null 2>&1; then engine=docker
else echo "error: need podman or docker locally" >&2; exit 1; fi

image="localhost/openscreentime_server:latest"
rev="$(git rev-parse --short HEAD)"

if [[ "$build" == 1 ]]; then
    echo "==> building ${image} from Containerfile (rev ${rev})"
    "$engine" build -t "$image" -f Containerfile .
fi

# Remote helper: everything on the host runs through $via (e.g. pct exec).
remote() { ssh -o BatchMode=yes "$target" "${via} bash -c $(printf '%q' "$*")"; }

echo "==> streaming image to ${target} ${via:+(via: $via)}"
# The tar goes to a file on the ssh target first so `pct exec` (which cannot
# take stdin reliably) can load it from disk.
tmp="/tmp/openscreentime-image-${rev}.tar"
"$engine" save "$image" | ssh -o BatchMode=yes "$target" "cat > ${tmp}"
if [[ -n "$via" ]]; then
    # pct push moves the file into the container, then we load from there.
    ctid="$(echo "$via" | grep -oE '[0-9]+' | head -1)"
    ssh -o BatchMode=yes "$target" "pct push ${ctid} ${tmp} ${tmp} && rm -f ${tmp}"
fi
remote "podman load -i ${tmp} && rm -f ${tmp}"

echo "==> fast-forwarding the host checkout (docs/compose only; the image is already built)"
remote "cd ${repo} && git fetch -q origin && git reset -q --hard origin/main && git log --oneline -1" || \
    echo "   (checkout not updated — fine as long as compose.yaml did not change)"

echo "==> recreating the server container with the new image"
# stop+rm instead of `down`: `down` removes the compose network, and netavark
# has been seen leaving stale port-forward rules behind when it cannot find the
# old netns (docs/OPERATIONS.md → 'port 8080 answers nothing').
remote "cd ${repo} && (podman stop -t 20 openscreentime-server >/dev/null 2>&1 || true) && (podman rm openscreentime-server >/dev/null 2>&1 || true) && podman-compose up -d 2>&1 | tail -2"

echo "==> health"
port="$(remote "grep -E '^OST_PORT=' ${repo}/.env | tail -n1 | cut -d= -f2-" || true)"
port="${port:-8080}"
bind="$(remote "grep -E '^OST_BIND_ADDR=' ${repo}/.env | tail -n1 | cut -d= -f2-" || true)"
bind="${bind:-127.0.0.1}"
for _ in $(seq 1 60); do
    if remote "curl -fsS -m 5 http://${bind}:${port}/health" 2>/dev/null; then
        echo; echo "==> deployed rev ${rev}"; exit 0
    fi
    sleep 2
done
echo "error: server did not become healthy; check: podman logs openscreentime-server" >&2
exit 1
