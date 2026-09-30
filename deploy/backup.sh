#!/usr/bin/env bash
# OpenScreenTime — back up the database to backups/ in this checkout.
#
#   deploy/backup.sh [nightly|pre-update|<label>]     (default: manual)
#
# Writes backups/ost-<UTC time>-<label>.dump (pg_dump custom format, compressed)
# and keeps a copy of .env next to it as backups/env.backup — the database
# password lives only there, and a dump is useless without it. Keeps the newest
# 7 nightly and 5 of every other label. Each run is recorded in the server's
# ops_log, so a failing backup (or none at all for two days) reaches your
# phone if alerts are set up.
#
# The nightly timer (openscreentime-backup.timer) runs this; setup.sh installs
# it. Restore with deploy/restore.sh. Copy backups/ off this machine too — a
# backup on the same disk does not survive the disk.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
# shellcheck source=deploy/lib.sh
source deploy/lib.sh

label="${1:-manual}"
[[ "$label" =~ ^[a-z0-9-]+$ ]] || ost_die "label must be lowercase letters, digits and dashes: $label"
[[ -f .env ]] || ost_die "no .env here — run deploy/setup.sh first."
ost_detect_engine

mkdir -p backups
chmod 700 backups
umask 077

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out="backups/ost-${stamp}-${label}.dump"
tmp="${out}.partial"
trap 'rm -f "$tmp"' EXIT

fail() {
    ost_record backup false "$1"
    ost_die "backup failed: $1"
}

"$ENGINE" container inspect "$OST_DB" >/dev/null 2>&1 || fail "the ${OST_DB} container does not exist"

# pg_dump runs inside the db container, so its version always matches the
# server's; the archive streams out to this host.
if ! "$ENGINE" exec "$OST_DB" pg_dump -Fc \
    -U "$(ost_env POSTGRES_USER openscreentime)" \
    -d "$(ost_env POSTGRES_DB openscreentime)" >"$tmp" 2>backups/.last-error; then
    fail "pg_dump: $(tail -n1 backups/.last-error 2>/dev/null || echo 'unknown error')"
fi
# A dump that pg_restore can't read is not a backup.
if ! "$ENGINE" exec -i "$OST_DB" pg_restore --list <"$tmp" >/dev/null 2>backups/.last-error; then
    fail "the dump does not read back: $(tail -n1 backups/.last-error 2>/dev/null || true)"
fi
rm -f backups/.last-error
mv "$tmp" "$out"
cp .env backups/env.backup

# Rotation: newest 7 nightly, newest 5 of each other label.
keep_newest() {
    local pattern="$1" keep="$2"
    # Names sort by time (UTC stamp), so a reverse name sort is newest-first.
    find backups -maxdepth 1 -name "$pattern" -printf '%f\n' | sort -r |
        tail -n +"$((keep + 1))" | while read -r f; do rm -f "backups/$f"; done
}
for l in $(find backups -maxdepth 1 -name 'ost-*.dump' -printf '%f\n' |
    sed -E 's/^ost-[0-9TZ]+-(.*)\.dump$/\1/' | sort -u); do
    if [[ "$l" == nightly ]]; then keep_newest "ost-*-nightly.dump" 7; else keep_newest "ost-*-${l}.dump" 5; fi
done

size="$(du -h "$out" | cut -f1)"
ost_record backup true "${label} backup ${out##*/} (${size})"
ost_log "backup written: ${out} (${size})"
printf '%s\n' "$out"
