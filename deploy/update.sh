#!/usr/bin/env bash
# Sentinel — one-command update: pull latest, rebuild, restart, wait for
# health.
#
# Note: this only updates the SERVER. Already-enrolled agents self-update
# from the new image's bundled binary automatically (daily, via
# `auto_update = true` in /etc/sentinel/agent.toml) — no separate rollout
# step needed for devices.
#
# Usage:
#   deploy/update.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ ! -f .env ]]; then
    echo "error: no .env found — run deploy/setup.sh first to do initial setup." >&2
    exit 1
fi

# Remember where we are so a failed update can roll back (used below when the
# health poll fails — important once this runs unattended from the timer).
prev_rev="$(git rev-parse HEAD)"

echo "==> git pull --ff-only"
git pull --ff-only

if [[ "$(git rev-parse HEAD)" == "$prev_rev" ]]; then
    echo "==> already up to date ($(git rev-parse --short HEAD)) — rebuilding anyway (idempotent)"
fi

compose_bin=""
compose_args=()

if command -v podman-compose >/dev/null 2>&1; then
    compose_bin="podman-compose"
elif command -v podman >/dev/null 2>&1 && podman compose version >/dev/null 2>&1; then
    compose_bin="podman"
    compose_args=("compose")
elif command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1; then
    compose_bin="docker"
    compose_args=("compose")
else
    echo "error: none of podman-compose, 'podman compose', or 'docker compose' found." >&2
    echo "       install podman-compose (rootless Podman recommended for the VPS)." >&2
    exit 1
fi

echo "==> using: ${compose_bin} ${compose_args[*]}"

echo "==> building images (server + web, see Containerfile)"
"${compose_bin}" "${compose_args[@]}" -f compose.yaml build

echo "==> recreating the server container with the new image"
"${compose_bin}" "${compose_args[@]}" -f compose.yaml up -d

# SENTINEL_PORT lives in .env; default to 8080 if it's unset there.
port="$(grep -E '^SENTINEL_PORT=' .env | tail -n1 | cut -d= -f2-)"
port="${port:-8080}"

echo "==> waiting for the server to report healthy on 127.0.0.1:${port}"

health_url="http://127.0.0.1:${port}/health"
healthy=""
for _ in $(seq 1 90); do
    if command -v curl >/dev/null 2>&1; then
        if curl -fsS "$health_url" >/dev/null 2>&1; then
            healthy="1"
            break
        fi
    elif command -v wget >/dev/null 2>&1; then
        if wget -q -O /dev/null "$health_url" >/dev/null 2>&1; then
            healthy="1"
            break
        fi
    else
        echo "error: neither curl nor wget found — cannot poll /health." >&2
        exit 1
    fi
    printf '.'
    sleep 1
done
echo

if [[ -z "$healthy" ]]; then
    echo "error: server did not become healthy within 90s after update." >&2
    echo "       check logs: ${compose_bin} ${compose_args[*]} -f compose.yaml logs server" >&2
    if [[ "$(git rev-parse HEAD)" != "$prev_rev" ]]; then
        echo "==> ROLLING BACK to ${prev_rev} (the previously running revision)" >&2
        git reset --hard "$prev_rev"
        "${compose_bin}" "${compose_args[@]}" -f compose.yaml build
        "${compose_bin}" "${compose_args[@]}" -f compose.yaml up -d
        echo "==> rollback deployed — verify with: curl ${health_url}" >&2
    fi
    exit 1
fi

rev="$(git rev-parse --short HEAD)"
echo "==> updated + healthy (${rev})"
