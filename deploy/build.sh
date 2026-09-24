#!/usr/bin/env bash
# OpenScreenTime — build the server image from this checkout.
#
# You rarely need this: deploy/setup.sh and deploy/update.sh pull the
# published image and only build here when pulling isn't possible. Use it to
# deploy local changes:
#
#   deploy/build.sh [--pull]                          # --pull: git pull first
#   deploy/update.sh --image localhost/openscreentime-server:build
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
# shellcheck source=deploy/lib.sh
source deploy/lib.sh

if [[ "${1:-}" == "--pull" ]]; then
    ost_log "git pull --ff-only"
    git pull --ff-only
fi

ost_detect_engine
rev="$(git rev-parse HEAD 2>/dev/null || true)"
ost_log "building ${OST_IMAGE_BUILD} (server + web + agents, see Containerfile)"
"$ENGINE" build -t "$OST_IMAGE_BUILD" ${rev:+--label "org.opencontainers.image.revision=${rev}"} \
    -f Containerfile .

cat <<EOF

==> Built ${OST_IMAGE_BUILD}.

Deploy it (backup, swap, health check, automatic rollback):
  deploy/update.sh --image ${OST_IMAGE_BUILD}

First install instead? Run deploy/setup.sh --domain <your domain>.
EOF
