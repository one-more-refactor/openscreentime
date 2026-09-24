#!/usr/bin/env bash
# OpenScreenTime — install the systemd units that keep the server running by
# itself: start at boot, nightly backup, and a daily deploy/update.sh (which
# backs up, swaps the image, checks health and rolls back on failure).
# deploy/setup.sh already does this; run it on installs made before that.
#
#   Rootful Podman (containers belong to root):   sudo deploy/install-auto-update.sh
#   Rootless Podman (containers belong to you):   deploy/install-auto-update.sh
#
# Undo (add --user and drop sudo for rootless):
#   sudo systemctl disable --now openscreentime-update.timer
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
# shellcheck source=deploy/lib.sh
source deploy/lib.sh

[[ -f .env ]] || ost_die "no .env in $(pwd) — run deploy/setup.sh first."
ost_detect_engine

# The units must run as whoever owns the containers. Root with no containers
# of its own is the old "sudo for a rootless install" habit — that would
# install units that update nothing.
if [[ "$(id -u)" == 0 && "$ENGINE" == podman ]] &&
    ! "$ENGINE" container inspect "$OST_SERVER" >/dev/null 2>&1; then
    owner="$(stat -c '%U' .)"
    if [[ "$owner" != root ]]; then
        ost_die "root has no ${OST_SERVER} container — this looks like a rootless install. Run this as ${owner}, without sudo."
    fi
    ost_warn "no ${OST_SERVER} container yet; installing the units anyway."
fi

ost_install_units 1
