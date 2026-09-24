#!/usr/bin/env bash
# OpenScreenTime — restore the database from a backup made by deploy/backup.sh.
#
#   deploy/restore.sh [--yes] backups/ost-<time>-<label>.dump
#
# REPLACES the current database with the dump: stops the server, drops and
# recreates the database, restores into it, starts the server again and waits
# for it to report healthy. deploy/update.sh uses this to roll back a failed
# update together with the previous version.
#
# On a new machine: put the old .env in place first (backups/env.backup — the
# database password must match), run `podman-compose up -d db`, then this.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
# shellcheck source=deploy/lib.sh
source deploy/lib.sh

yes=0
if [[ "${1:-}" == --yes ]]; then yes=1; shift; fi
dump="${1:-}"
[[ -n "$dump" && -f "$dump" ]] || ost_die "usage: deploy/restore.sh [--yes] <backup .dump file>"
[[ -f .env ]] || ost_die "no .env here — restore it first (backups/env.backup)."
ost_detect_engine

user="$(ost_env POSTGRES_USER openscreentime)"
db="$(ost_env POSTGRES_DB openscreentime)"

if [[ "$yes" != 1 ]]; then
    read -r -p "This REPLACES database '${db}' with ${dump}. Type 'restore' to go on: " answer
    [[ "$answer" == restore ]] || ost_die "not restoring."
fi

# The database container must be up (the server need not be).
if [[ "$("$ENGINE" container inspect --format '{{.State.Running}}' "$OST_DB" 2>/dev/null)" != true ]]; then
    ost_log "starting the database container"
    ost_compose up -d db >/dev/null
fi
for _ in $(seq 1 30); do
    "$ENGINE" exec "$OST_DB" pg_isready -q -U "$user" && break
    sleep 1
done

ost_log "stopping the server"
"$ENGINE" stop -t 20 "$OST_SERVER" >/dev/null 2>&1 || true

ost_log "recreating database '${db}'"
# Identifiers come from .env; quote them as SQL identifiers via psql.
printf 'DROP DATABASE IF EXISTS :"db" WITH (FORCE);\nCREATE DATABASE :"db" OWNER :"user";\n' |
    ost_psql -d postgres -v db="$db" -v user="$user"

ost_log "restoring ${dump}"
"$ENGINE" exec -i "$OST_DB" pg_restore --exit-on-error --no-owner -U "$user" -d "$db" <"$dump"

ost_log "starting the server"
ost_recreate_server
if ost_wait_healthy 180; then
    ost_log "restored from ${dump} — server healthy"
else
    ost_die "restored, but the server did not become healthy — check: ${ENGINE} logs ${OST_SERVER}"
fi
