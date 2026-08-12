#!/usr/bin/env bash
# OpenScreenTime — build the compose stack's images.
#
# Works both on the VPS (typical: rootless Podman) and locally (Podman or
# Docker). Safe to re-run; each run rebuilds from the current source tree.
#
# Usage:
#   deploy/build.sh            # build using the checked-out working tree
#   deploy/build.sh --pull     # also git pull first (fast-forward only)
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ "${1:-}" == "--pull" ]]; then
    echo "==> git pull --ff-only"
    git pull --ff-only
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

if [[ ! -f .env ]]; then
    echo "note: no .env found yet — build will still work, but 'up' will need one."
fi

echo "==> building images (server + web, see Containerfile)"
"${compose_bin}" "${compose_args[@]}" -f compose.yaml build

cat <<'EOF'

==> Build complete.

Next steps (first-time setup):
  1. cp .env.example .env
  2. edit .env — set POSTGRES_PASSWORD, RP_ID, RP_ORIGIN, OST_PUBLIC_URL
  3. podman-compose up -d      # (or: podman compose up -d / docker compose up -d)
  4. check logs:  podman-compose logs -f server

To update later: git pull, then re-run this script, then 'up -d' again.
See docs/DEPLOY.md for the full operator guide (reverse-proxy config, etc).
EOF
