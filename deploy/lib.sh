# shellcheck shell=bash
# OpenScreenTime — helpers shared by the deploy scripts (sourced, not run).
#
# Everything here works the same for rootful Podman, rootless Podman and
# Docker. The two containers have fixed names (compose.yaml `container_name`),
# so the scripts talk to them directly instead of through compose.

OST_DB=openscreentime-db
OST_SERVER=openscreentime-server
# The tag compose.yaml runs, and the one kept for rolling back.
# shellcheck disable=SC2034 # used by the scripts that source this
OST_IMAGE_CURRENT=localhost/openscreentime-server:current
# shellcheck disable=SC2034
OST_IMAGE_PREVIOUS=localhost/openscreentime-server:previous
OST_IMAGE_BUILD=localhost/openscreentime-server:build
# The prebuilt image CI publishes on every push to main. OST_IMAGE in .env
# picks another one (a version tag, a mirror), or `build` to always compile here.
OST_DEFAULT_IMAGE=ghcr.io/one-more-refactor/openscreentime:latest

ost_repo_root() { cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd; }

ost_log() { printf '==> %s\n' "$*"; }
ost_warn() { printf 'warning: %s\n' "$*" >&2; }
ost_die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# ost_env KEY [DEFAULT] — a value from .env (last one wins, quotes stripped),
# else from the environment, else DEFAULT.
ost_env() {
    local v=""
    if [[ -f .env ]]; then
        v="$(grep -E "^${1}=" .env | tail -n1 | cut -d= -f2- || true)"
        v="${v%\"}"; v="${v#\"}"; v="${v%\'}"; v="${v#\'}"
    fi
    printf '%s' "${v:-${!1:-${2:-}}}"
}

# Sets ENGINE (podman|docker) and COMPOSE (array: the compose command).
ost_detect_engine() {
    if command -v podman >/dev/null 2>&1; then
        ENGINE=podman
    elif command -v docker >/dev/null 2>&1; then
        ENGINE=docker
    else
        ost_die "neither podman nor docker found — install podman and podman-compose."
    fi
    if [[ "$ENGINE" == podman ]] && command -v podman-compose >/dev/null 2>&1; then
        COMPOSE=(podman-compose)
    elif "$ENGINE" compose version >/dev/null 2>&1; then
        COMPOSE=("$ENGINE" compose)
    elif command -v docker-compose >/dev/null 2>&1; then
        COMPOSE=(docker-compose)
    else
        ost_die "no compose tool found — install podman-compose (or the docker compose plugin)."
    fi
}

ost_compose() { "${COMPOSE[@]}" -f compose.yaml "$@"; }

ost_image_id() { "$ENGINE" image inspect --format '{{.Id}}' "$1" 2>/dev/null || true; }
ost_running_image_id() { "$ENGINE" container inspect --format '{{.Image}}' "$OST_SERVER" 2>/dev/null || true; }

# A short human name for an image: its git revision label when CI set one.
ost_image_name() {
    local rev
    rev="$("$ENGINE" image inspect --format '{{index .Labels "org.opencontainers.image.revision"}}' "$1" 2>/dev/null || true)"
    if [[ -n "$rev" && "$rev" != "<no value>" ]]; then
        printf '%s' "${rev:0:12}"
    else
        local id; id="$(ost_image_id "$1")"; id="${id#sha256:}"
        printf 'image %s' "${id:0:12}"
    fi
}

# ost_fetch_image — pull the published image, or build one from this
# checkout if that isn't possible. Prints the image reference to use.
ost_fetch_image() {
    local image
    image="$(ost_env OST_IMAGE "$OST_DEFAULT_IMAGE")"
    if [[ "$image" != build ]]; then
        ost_log "pulling ${image}" >&2
        if "$ENGINE" pull -q "$image" >/dev/null 2>&1; then
            # update.sh notices when this goes stale (updates silently stopping).
            mkdir -p backups && touch backups/.last-pull-ok
            printf '%s' "$image"
            return 0
        fi
        # Registry unreachable, but we have that image already (pulled
        # earlier, or loaded by hand): use it rather than compile for an hour.
        if [[ -n "$(ost_image_id "$image")" ]]; then
            ost_warn "could not pull ${image} — using the copy already on this machine."
            printf '%s' "$image"
            return 0
        fi
        ost_warn "could not pull ${image} — building on this machine instead (slow: 10–60 min)."
    fi
    # Build only when the source changed: a rebuild of the same commit would
    # still yield a new image (fresh timestamps) — and with it a pointless
    # backup-and-restart every day.
    local rev="" built=""
    if [[ -d .git ]] && command -v git >/dev/null 2>&1 &&
        [[ -z "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]]; then
        rev="$(git rev-parse HEAD 2>/dev/null || true)"
    fi
    if [[ -n "$rev" ]]; then
        built="$("$ENGINE" image inspect --format '{{index .Labels "org.opencontainers.image.revision"}}' "$OST_IMAGE_BUILD" 2>/dev/null || true)"
        if [[ "$built" == "$rev" ]]; then
            printf '%s' "$OST_IMAGE_BUILD"
            return 0
        fi
    fi
    ost_log "building the image from this checkout (Containerfile)" >&2
    "$ENGINE" build -t "$OST_IMAGE_BUILD" ${rev:+--label "org.opencontainers.image.revision=${rev}"} \
        -f Containerfile . >&2 || return 1
    printf '%s' "$OST_IMAGE_BUILD"
}

ost_health_url() {
    printf 'http://%s:%s/health' "$(ost_env OST_BIND_ADDR 127.0.0.1)" "$(ost_env OST_PORT 8080)"
}

# ost_wait_healthy SECONDS — until /health answers 200 (server up AND its
# database answering).
ost_wait_healthy() {
    local url i
    url="$(ost_health_url)"
    for ((i = 0; i < $1; i += 2)); do
        if command -v curl >/dev/null 2>&1; then
            curl -fsS -m 3 "$url" >/dev/null 2>&1 && return 0
        elif command -v wget >/dev/null 2>&1; then
            wget -q -T 3 -O /dev/null "$url" >/dev/null 2>&1 && return 0
        else
            ost_die "neither curl nor wget found — cannot poll ${url}."
        fi
        sleep 2
    done
    return 1
}

# (Re)create the server container from $OST_IMAGE_CURRENT. stop+rm rather
# than `down`: `down` drops the compose network, and netavark has been seen
# leaving stale port-forward rules behind (docs/OPERATIONS.md).
ost_recreate_server() {
    "$ENGINE" stop -t 20 "$OST_SERVER" >/dev/null 2>&1 || true
    "$ENGINE" rm -f "$OST_SERVER" >/dev/null 2>&1 || true
    ost_compose up -d >/dev/null
}

# psql inside the db container, reading SQL from stdin.
ost_psql() {
    "$ENGINE" exec -i "$OST_DB" psql -X -q -tA -v ON_ERROR_STOP=1 \
        -U "$(ost_env POSTGRES_USER openscreentime)" "$@"
}

# ost_record KIND OK DETAIL — note a backup/update run in the server's
# ops_log, which is how the server finds out (and tells the operator) that
# something went wrong. Best-effort: an older server has no ops_log yet.
ost_record() {
    printf "INSERT INTO ops_log (kind, ok, detail) VALUES (:'kind', :'ok', :'detail');\n" |
        ost_psql -d "$(ost_env POSTGRES_DB openscreentime)" \
            -v kind="$1" -v ok="$2" -v detail="$3" >/dev/null 2>&1 ||
        ost_warn "could not record this $1 run in the database (older server, or db down)."
}

# ---------------------------------------------------------------------------
# systemd: start at boot, nightly backup, (optional) daily update
# ---------------------------------------------------------------------------

# ost_install_units WITH_UPDATE_TIMER(0|1)
#
# Rootful (run as root): system units. Rootless: user units + lingering, so
# they run without anyone logged in. The update timer is only (re)written when
# asked for or already installed. OST_NO_SYSTEMD=1 skips all of it;
# OST_UNIT_DIR=<dir> only writes the files there (for testing).
ost_install_units() {
    local with_update="$1" repo mode dir scope=() want podman_bin changed=0
    repo="$(pwd)"
    if [[ "${OST_NO_SYSTEMD:-0}" == 1 ]]; then
        ost_log "not touching systemd (OST_NO_SYSTEMD=1)"
        return 0
    fi
    if [[ -n "${OST_UNIT_DIR:-}" ]]; then
        mode=test dir="$OST_UNIT_DIR" want=default.target
        mkdir -p "$dir"
    elif ! command -v systemctl >/dev/null 2>&1; then
        ost_warn "no systemd here — make sure the containers start at boot and deploy/backup.sh runs nightly."
        return 0
    elif [[ "$(id -u)" == 0 ]]; then
        mode=system dir=/etc/systemd/system want=multi-user.target
    else
        mode=user dir="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user" want=default.target scope=(--user)
        mkdir -p "$dir"
        if ! systemctl --user show-environment >/dev/null 2>&1; then
            ost_warn "no systemd user session here (su/sudo shell?). Log in as $(id -un) directly and re-run this, or the stack will not start at boot."
            return 0
        fi
    fi

    # Write a unit only when its content changed.
    _unit() {
        local path="$dir/$1"
        if [[ ! -f "$path" ]] || [[ "$(cat "$path")" != "$2" ]]; then
            printf '%s\n' "$2" >"$path"
            changed=1
        fi
    }
    local header="# Managed by openscreentime deploy/lib.sh — rewritten on setup/update."

    # 1. Start the containers at boot. Docker restarts `restart: always`
    #    containers itself when its daemon starts; Podman has no daemon.
    if [[ "$ENGINE" == podman ]]; then
        podman_bin="$(command -v podman)"
        _unit openscreentime.service "$header
[Unit]
Description=OpenScreenTime (start the server containers at boot)
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=${podman_bin} start ${OST_DB} ${OST_SERVER}
ExecStop=${podman_bin} stop -t 30 ${OST_SERVER} ${OST_DB}

[Install]
WantedBy=${want}"
    elif [[ "$mode" != test ]] && ! systemctl is-enabled docker >/dev/null 2>&1; then
        ost_warn "docker.service is not enabled — the containers will not come back after a reboot (systemctl enable docker)."
    fi

    # 2. Nightly database backup (kept 7 nights, see deploy/backup.sh).
    _unit openscreentime-backup.service "$header
[Unit]
Description=OpenScreenTime nightly database backup

[Service]
Type=oneshot
WorkingDirectory=${repo}
ExecStart=${repo}/deploy/backup.sh nightly"
    _unit openscreentime-backup.timer "$header
[Unit]
Description=Nightly OpenScreenTime database backup

[Timer]
OnCalendar=*-*-* 03:15:00
RandomizedDelaySec=30min
Persistent=true

[Install]
WantedBy=timers.target"

    # 3. Daily update (pull, back up, swap, health check, roll back on failure).
    if [[ "$with_update" == 1 || -f "$dir/openscreentime-update.timer" ]]; then
        with_update=1
        _unit openscreentime-update.service "$header
[Unit]
Description=OpenScreenTime server update (pull, backup, swap, health check, roll back on failure)
Wants=network-online.target
After=network-online.target

[Service]
Type=oneshot
WorkingDirectory=${repo}
ExecStart=${repo}/deploy/update.sh
# Only a fallback build on the box takes long; a pulled image takes minutes.
TimeoutStartSec=3h"
        _unit openscreentime-update.timer "$header
[Unit]
Description=Daily OpenScreenTime server update

[Timer]
OnCalendar=*-*-* 04:30:00
RandomizedDelaySec=1h
Persistent=true

[Install]
WantedBy=timers.target"
    fi

    local units=(openscreentime-backup.timer)
    [[ "$ENGINE" == podman ]] && units=(openscreentime.service "${units[@]}")
    [[ "$with_update" == 1 ]] && units+=(openscreentime-update.timer)

    if [[ "$mode" == test ]]; then
        ost_log "wrote units to ${dir} (test mode; would enable: ${units[*]})"
        return 0
    fi
    if [[ "$changed" == 1 ]]; then
        systemctl "${scope[@]}" daemon-reload
    fi
    systemctl "${scope[@]}" enable --now "${units[@]}" >/dev/null 2>&1 ||
        ost_warn "could not enable ${units[*]} — check: systemctl ${scope[*]} status ${units[*]}"
    ost_log "enabled (${mode}): ${units[*]}"

    if [[ "$mode" == user ]]; then
        # User units only run at boot (and keep running after logout) with
        # lingering on. Without it the stack dies with the last SSH session.
        if [[ "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null)" != yes ]]; then
            if loginctl enable-linger "$(id -un)" >/dev/null 2>&1; then
                ost_log "enabled lingering for $(id -un) (the stack runs without a login)"
            else
                ost_warn "run once:  sudo loginctl enable-linger $(id -un)   — without it nothing starts at boot."
            fi
        fi
        if [[ -f /etc/systemd/system/openscreentime-update.timer ]]; then
            ost_warn "an older system-wide openscreentime-update.timer exists; remove it:
    sudo systemctl disable --now openscreentime-update.timer
    sudo rm /etc/systemd/system/openscreentime-update.service /etc/systemd/system/openscreentime-update.timer"
        fi
    fi
}
