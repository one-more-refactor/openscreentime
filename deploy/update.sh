#!/usr/bin/env bash
# OpenScreenTime — update the server, safely. The daily timer runs this.
#
#   deploy/update.sh                 pull the published image (or build here)
#   deploy/update.sh --image REF     deploy an image already on this machine
#                                    (deploy/push-image.sh uses this)
#
# 1. Fast-forwards this checkout (compose.yaml, scripts). A dirty or diverged
#    checkout is reported and skipped — it never blocks the image update.
# 2. Gets the new image: pulls OST_IMAGE (default: the image CI publishes),
#    or builds one from the checkout when pulling isn't possible.
# 3. Nothing new → done. Otherwise backs the database up, swaps the server
#    container to the new image and waits for /health (which checks the DB).
# 4. Unhealthy → puts the previous image back AND restores the pre-update
#    backup (a new version may already have migrated the schema, which the old
#    one refuses to start on), remembers the bad image so tomorrow's run skips
#    it, and records the rollback — the server then tells you on your phone.
#
# Only the server is updated here; enrolled devices update themselves from it.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
# shellcheck source=deploy/lib.sh
source deploy/lib.sh

args=("$@") # for re-running this script below (the loop shifts them away)
image_arg=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --image) image_arg="${2:-}"; [[ -n "$image_arg" ]] || ost_die "--image needs a value"; shift 2 ;;
        --image=*) image_arg="${1#--image=}"; shift ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) ost_die "unknown argument: $1" ;;
    esac
done

[[ -f .env ]] || ost_die "no .env found — run deploy/setup.sh first."

# One update at a time (the timer and a manual run must not interleave).
# flock(1) holds the lock and runs this script as its child. Not `exec 9>`:
# every process started below would inherit that fd — including the
# container's conmon, which would then hold the lock for as long as the
# container runs.
if [[ -z "${OST_UPDATE_LOCKED:-}" ]] && command -v flock >/dev/null 2>&1; then
    rc=0
    OST_UPDATE_LOCKED=1 flock -n -E 75 .update.lock deploy/update.sh ${args[@]+"${args[@]}"} || rc=$?
    if [[ "$rc" == 75 ]]; then
        ost_log "another update is running — nothing to do"
        exit 0
    fi
    exit "$rc"
fi

# --- 1. the checkout --------------------------------------------------------
if [[ -z "$image_arg" && -z "${OST_UPDATE_REEXEC:-}" ]]; then
    if [[ -d .git ]] && command -v git >/dev/null 2>&1; then
        before="$(git rev-parse HEAD 2>/dev/null || echo none)"
        if git pull --ff-only -q 2>/dev/null; then
            if [[ "$(git rev-parse HEAD)" != "$before" ]]; then
                ost_log "checkout updated to $(git rev-parse --short HEAD); continuing with the new scripts"
                OST_UPDATE_REEXEC=1 exec deploy/update.sh ${args[@]+"${args[@]}"}
            fi
        else
            ost_warn "could not fast-forward the checkout (local edits or a diverged branch — see 'git status'). Updating the image anyway; compose.yaml and the scripts stay as they are."
        fi
    else
        ost_warn "no git checkout here — only the image is updated."
    fi
fi

ost_detect_engine

record_fail() {
    ost_record update false "$1"
    ost_die "$1"
}

# --- 2. the new image -------------------------------------------------------
if [[ -n "$image_arg" ]]; then
    [[ -n "$(ost_image_id "$image_arg")" ]] || ost_die "no such image here: $image_arg"
    candidate="$image_arg"
else
    candidate="$(ost_fetch_image)" || record_fail "the update could not get a new image (pull and build both failed); nothing was changed. Details: journalctl -u openscreentime-update"
fi
new_id="$(ost_image_id "$candidate")"
new_name="$(ost_image_name "$candidate")"
running_id="$(ost_running_image_id)"

if [[ "$new_id" == "$running_id" ]]; then
    if ! ost_wait_healthy 10; then
        # Not an update problem — but the timer is the one thing looking.
        ost_warn "the server is unhealthy on the current version — restarting it"
        ost_recreate_server
        ost_wait_healthy 180 ||
            record_fail "the server is unhealthy and restarting it did not help (this is not about an update). Check: ${ENGINE} logs ${OST_SERVER}"
    fi
    ost_log "already running ${new_name} — up to date"
    # "Up to date" only because the registry can't be reached (and the old
    # local copy was used) must not look like health for ever.
    if [[ -z "$image_arg" && "$candidate" != "$OST_IMAGE_BUILD" ]] &&
        [[ -z "$(find backups/.last-pull-ok -mtime -3 2>/dev/null)" ]]; then
        ost_record update false "no update could be pulled from ${candidate} for 3 days — the server is not getting updates (registry unreachable, or the image not public?)."
    else
        ost_record update true "up to date (${new_name})"
    fi
    ost_install_units 0
    exit 0
fi
if [[ -f backups/.rejected-image && "$(cat backups/.rejected-image)" == "$new_id" ]]; then
    # Rolled back once already; don't take the server down for it every day.
    ost_log "${new_name} failed before and was rolled back — skipping it until a newer one appears"
    exit 0
fi

# --- 3. backup, swap, health ------------------------------------------------
ost_log "backing up the database before updating"
dump="$(deploy/backup.sh pre-update | tail -n1)" ||
    record_fail "the update was not started: the pre-update backup failed (see journalctl -u openscreentime-update). The current version keeps running."

if [[ -n "$running_id" ]]; then
    "$ENGINE" tag "$running_id" "$OST_IMAGE_PREVIOUS"
    old_name="$(ost_image_name "$OST_IMAGE_PREVIOUS")"
else
    old_name="(none)"
fi
"$ENGINE" tag "$candidate" "$OST_IMAGE_CURRENT"

ost_log "switching the server from ${old_name} to ${new_name}"
ost_recreate_server
if ost_wait_healthy 180; then
    ost_log "updated to ${new_name} — healthy"
    rm -f backups/.rejected-image
    ost_record update true "updated from ${old_name} to ${new_name}"
    # Old images and on-box build layers otherwise fill the disk, update by update.
    "$ENGINE" image prune -f >/dev/null 2>&1 || true
    ost_install_units 0
    exit 0
fi

# --- 4. roll back -----------------------------------------------------------
"$ENGINE" logs --tail 30 "$OST_SERVER" >&2 2>&1 || true
if [[ -z "$running_id" ]]; then
    record_fail "the update to ${new_name} did not become healthy and there is no previous version to go back to. Check: ${ENGINE} logs ${OST_SERVER}"
fi
ost_warn "${new_name} did not become healthy — rolling back to ${old_name} and the pre-update database"
printf '%s' "$new_id" >backups/.rejected-image
"$ENGINE" tag "$OST_IMAGE_PREVIOUS" "$OST_IMAGE_CURRENT"
if deploy/restore.sh --yes "$dump"; then
    record_fail "the update to ${new_name} failed its health check and was rolled back: ${old_name} is running again on the database as it was just before the update. The new version will be skipped until a newer one is published."
else
    record_fail "the update to ${new_name} failed AND the rollback did not come back healthy. Needs a look: ${ENGINE} logs ${OST_SERVER}; the pre-update backup is ${dump}."
fi
