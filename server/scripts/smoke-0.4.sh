#!/usr/bin/env bash
# End-to-end smoke test for the 0.4 server against a real Postgres.
#
# Spins up a throwaway Postgres (rootless podman), builds + runs the server on
# 127.0.0.1:18080, seeds an owner session straight into the DB (passkeys can't
# be driven from bash), then walks the 0.4 contract: member → device (parent
# code) → enroll (linking) → voucher bound to the OS user → member session is
# confined → lock is pending until the agent says so → WS state frame → offline
# on hangup. Prints PASS/FAIL per step; exits non-zero on any FAIL.
#
#   server/scripts/smoke-0.4.sh            # from the repo root or server/
#
# Needs: podman, curl, jq, bun (for the stand-in WS agent), cargo.
set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
PG_NAME=ost-test-pg-srv
PG_PORT=55433
PORT=18080
BASE="http://127.0.0.1:${PORT}"
export DATABASE_URL="postgres://ost:pw@127.0.0.1:${PG_PORT}/ost"
export RP_ID=localhost RP_ORIGIN="http://localhost:${PORT}" OST_INSECURE_COOKIES=1 OST_OPEN_REGISTRATION=1
export BIND_ADDR="127.0.0.1:${PORT}" RUST_LOG=openscreentime_server=info,warn
fails=0
pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; fails=$((fails + 1)); }
check() { # check <desc> <actual> <expected>
    if [[ "$2" == "$3" ]]; then pass "$1 ($2)"; else fail "$1: got '$2', want '$3'"; fi
}
psql_q() { podman exec -i "$PG_NAME" psql -U ost -d ost -tAq -c "$1"; }

cleanup() {
    [[ -n "${SERVER_PID:-}" ]] && kill "$SERVER_PID" 2>/dev/null
    podman rm -f "$PG_NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> postgres"
podman rm -f "$PG_NAME" >/dev/null 2>&1 || true
podman run -d --name "$PG_NAME" -e POSTGRES_PASSWORD=pw -e POSTGRES_USER=ost -e POSTGRES_DB=ost \
    -p "127.0.0.1:${PG_PORT}:5432" docker.io/library/postgres:15 >/dev/null
for _ in $(seq 1 60); do
    podman exec "$PG_NAME" pg_isready -U ost -d ost >/dev/null 2>&1 && break
    sleep 1
done

echo "==> build"
cargo build -q 2>&1 | tail -5
BIN=target/debug/openscreentime-server

start_server() {
    "$BIN" >/tmp/ost-smoke-server.log 2>&1 &
    SERVER_PID=$!
    for _ in $(seq 1 60); do
        curl -fsS "$BASE/health" >/dev/null 2>&1 && return 0
        sleep 0.5
    done
    echo "server did not come up:"; tail -20 /tmp/ost-smoke-server.log; exit 1
}
start_server
pass "server up (migrations applied)"

# ---- seed an owner + a stepped-up session directly ----------------------------
TOKEN=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
HASH=$(printf '%s' "$TOKEN" | sha256sum | cut -d' ' -f1)
TOKEN2=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
HASH2=$(printf '%s' "$TOKEN2" | sha256sum | cut -d' ' -f1)
TENANT=$(psql_q "INSERT INTO tenants (name) VALUES ('Smoke family') RETURNING id")
OWNER=$(psql_q "INSERT INTO admins (tenant_id, email, display_name) VALUES ('$TENANT', 'parent@example.com', 'Parent') RETURNING id")
psql_q "INSERT INTO admin_sessions (token_hash, admin_id, tenant_id, expires_at, stepup_until) VALUES ('$HASH', '$OWNER', '$TENANT', now() + interval '1 day', now() + interval '1 hour')" >/dev/null
psql_q "INSERT INTO admin_sessions (token_hash, admin_id, tenant_id, expires_at) VALUES ('$HASH2', '$OWNER', '$TENANT', now() + interval '1 day')" >/dev/null
# a legacy, unlinked OS login on a pre-0.4 device, to exercise the backfill
LEGACY_DEV=$(psql_q "INSERT INTO devices (tenant_id, name, status) VALUES ('$TENANT', 'Old laptop', 'offline') RETURNING id")
psql_q "INSERT INTO profiles (tenant_id, name, kind, is_preset, policy) VALUES ('$TENANT','Default','default',true,'{}') RETURNING id" >/dev/null
LEGACY_PROFILE=$(psql_q "SELECT id FROM profiles WHERE tenant_id='$TENANT' AND kind='default'")
psql_q "INSERT INTO device_users (device_id, os_username, display_name, profile_id) VALUES ('$LEGACY_DEV', 'leo', 'Leo', '$LEGACY_PROFILE')" >/dev/null

# restart so the startup backfills see the seeded tenant
kill "$SERVER_PID"; wait "$SERVER_PID" 2>/dev/null; start_server
check "bracket presets backfilled for an existing tenant" \
    "$(psql_q "SELECT count(*) FROM profiles WHERE tenant_id='$TENANT' AND is_preset AND kind IN ('little','kid','younger_teen','older_teen','adult')")" "5"
check "legacy OS login got a member" \
    "$(psql_q "SELECT a.role||':'||a.display_name FROM device_users du JOIN admins a ON a.id=du.account_id WHERE du.os_username='leo'")" "member:Leo"

P() { curl -sS -b "ost_session=$TOKEN" -H 'content-type: application/json' "$@"; }          # parent, stepped up
P2() { curl -sS -b "ost_session=$TOKEN2" -H 'content-type: application/json' "$@"; }        # parent, no grant
code() { curl -sS -o /dev/null -w '%{http_code}' "$@"; }

# ---- /api/me as owner ----------------------------------------------------------
ME=$(P "$BASE/api/me")
check "owner /api/me role" "$(jq -r .account.role <<<"$ME")" "owner"
check "owner /api/me has household" "$(jq -r .household.name <<<"$ME")" "Smoke family"

# ---- member -------------------------------------------------------------------------
M=$(P -X POST "$BASE/api/members" -d '{"display_name":"Mia","birthdate":"2018-03-04"}')
MEMBER=$(jq -r .member.id <<<"$M")
check "create member → kid from birthdate" "$(jq -r .member.age_bracket <<<"$M")" "kid"
check "create member → playful theme" "$(jq -r .member.effective_theme <<<"$M")" "playful"
check "create member → has rules" "$(jq -r '.member.profile_id != null' <<<"$M")" "true"
check "members need step-up" "$(code -b "ost_session=$TOKEN2" -H 'content-type: application/json' -X POST "$BASE/api/members" -d '{"display_name":"X"}')" "428"
PM=$(P -X PATCH "$BASE/api/members/$MEMBER" -d '{"theme":"calm"}')
check "patch member theme" "$(jq -r .member.effective_theme <<<"$PM")" "calm"

# ---- device + parent code -------------------------------------------------------------
D=$(P -X POST "$BASE/api/devices" -d "{\"name\":\"Mia laptop\",\"account_id\":\"$MEMBER\"}")
DEVICE=$(jq -r .device.id <<<"$D"); ENROLL=$(jq -r .enroll_token <<<"$D"); SECRET=$(jq -r .parent_code.secret <<<"$D")
check "device created pending" "$(jq -r .device.status <<<"$D")" "pending"
check "device not locked" "$(jq -r .device.locked <<<"$D")" "false"
check "parent code secret is base32 (32 chars)" "${#SECRET}" "32"
check "parent code otpauth uri" "$(jq -r .parent_code.otpauth_uri <<<"$D" | grep -c '^otpauth://totp/OpenScreenTime:Mia%20laptop?secret=')" "1"
check "parent-code read is a sensitive read (428 without grant)" "$(code -b "ost_session=$TOKEN2" "$BASE/api/devices/$DEVICE/parent-code")" "428"
check "parent-code read with grant matches" "$(P "$BASE/api/devices/$DEVICE/parent-code" | jq -r .parent_code.secret)" "$SECRET"

# ---- enroll ---------------------------------------------------------------------------------
E=$(curl -sS -H 'content-type: application/json' -X POST "$BASE/agent/enroll" \
    -d "{\"enroll_token\":\"$ENROLL\",\"hostname\":\"mia-laptop\",\"os\":\"linux\",\"agent_version\":\"0.4.0-smoke\",\"os_users\":[{\"username\":\"mia\",\"display_name\":\"Mia\"},{\"username\":\"guest\",\"display_name\":\"Guest\"}]}")
DTOKEN=$(jq -r .device_token <<<"$E")
check "enroll returned a device token" "$(jq -r '.device_token|length>10' <<<"$E")" "true"
check "enroll: mia linked by name" "$(psql_q "SELECT account_id='$MEMBER' FROM device_users WHERE device_id='$DEVICE' AND os_username='mia'")" "t"
check "enroll: guest linked to the device owner" "$(psql_q "SELECT account_id='$MEMBER' FROM device_users WHERE device_id='$DEVICE' AND os_username='guest'")" "t"
check "enroll: device_users point at the member's rules" "$(psql_q "SELECT count(DISTINCT profile_id) FROM device_users WHERE device_id='$DEVICE'")" "1"
A() { curl -sS -H "Authorization: Bearer $DTOKEN" -H 'content-type: application/json' "$@"; }
POL=$(A "$BASE/agent/policy")
check "agent policy carries parent_code.totp_secret" "$(jq -r .parent_code.totp_secret <<<"$POL")" "$SECRET"
check "agent policy carries blocks (kid preset)" "$(jq -r '.users[0].policy.blocks.categories|index("adult")!=null' <<<"$POL")" "true"
check "agent policy keeps the PIN backup" "$(jq -r '.users[0].policy.parent_pin_hash|startswith("$argon2")' <<<"$POL")" "true"

# ---- voucher → member session -------------------------------------------------------------
check "voucher for an unknown OS user → 404 no_account" \
    "$(A -X POST "$BASE/agent/voucher" -d '{"os_username":"nobody"}' | jq -r .error.code)" "no_account"
V=$(A -X POST "$BASE/agent/voucher" -d '{"os_username":"mia"}' | jq -r .voucher)
R=$(curl -sS -c /tmp/ost-smoke-jar -H 'content-type: application/json' -X POST "$BASE/api/auth/voucher" -d "{\"voucher\":\"$V\"}")
check "voucher redeemed for the member" "$(jq -r .role <<<"$R")" "member"
C() { curl -sS -b /tmp/ost-smoke-jar -H 'content-type: application/json' "$@"; }
check "member /api/me role" "$(C "$BASE/api/me" | jq -r .account.role)" "member"
check "member /api/me name" "$(C "$BASE/api/me" | jq -r .account.display_name)" "Mia"
T=$(C "$BASE/api/me/today")
check "member /api/me/today limit (kid = 60)" "$(jq -r .limit_minutes <<<"$T")" "60"
check "member /api/me/today theme" "$(jq -r .theme <<<"$T")" "calm"
check "member /api/me/today lists the device" "$(jq -r '.devices[0].name' <<<"$T")" "Mia laptop"
check "member /api/me/today blocked apps include tiktok" "$(jq -r '.blocked_apps|index("tiktok")!=null' <<<"$T")" "true"
check "member cannot read /api/family" "$(C -o /dev/null -w '%{http_code}' "$BASE/api/family")" "403"
check "member 403 code" "$(C "$BASE/api/family" | jq -r .error.code)" "forbidden_for_member"
check "member cannot list devices" "$(C -o /dev/null -w '%{http_code}' "$BASE/api/devices")" "403"
check "member cannot reach a hub route not on the allow-list" "$(C -o /dev/null -w '%{http_code}' "$BASE/api/profiles")" "403"
check "member cannot mutate profiles" "$(C -o /dev/null -w '%{http_code}' -X POST "$BASE/api/profiles" -d '{}')" "403"
ASK=$(C -X POST "$BASE/api/me/ask" -d '{"minutes":15,"reason":"homework done"}')
check "member can ask for time (no step-up)" "$(jq -r .request.status <<<"$ASK")" "pending"
check "ask is deduped per day" "$(C -X POST "$BASE/api/me/ask" -d '{"minutes":30}' | jq -r .request.id)" "$(jq -r .request.id <<<"$ASK")"
check "catalog is readable by a member" "$(C "$BASE/api/catalog" | jq -r '.apps|length>20')" "true"
check "today shows the pending ask" "$(C "$BASE/api/me/today" | jq -r .pending_request)" "true"

# ---- family ------------------------------------------------------------------------------------
F=$(P "$BASE/api/family")
check "family children are members (Leo, Mia)" "$(jq -r '[.children[].name]|join(",")' <<<"$F")" "Leo,Mia"
check "family child key = account id" "$(jq -r --arg m "$MEMBER" '.children[]|select(.name=="Mia")|.key==$m' <<<"$F")" "true"
check "family child has the device" "$(jq -r '.children[]|select(.name=="Mia")|.devices|length' <<<"$F")" "2"
check "family child pending request" "$(jq -r '.children[]|select(.name=="Mia")|.pending_requests' <<<"$F")" "1"
check "family device status online after enroll" "$(jq -r --arg d "$DEVICE" '.devices[]|select(.id==$d)|.status' <<<"$F")" "online"

# ---- lock: pending until the agent says so ------------------------------------------------
L=$(P -X POST "$BASE/api/devices/$DEVICE/lock")
check "lock enqueued (agent not connected → not delivered)" "$(jq -r .delivered <<<"$L")" "false"
G=$(P "$BASE/api/devices/$DEVICE")
check "lock_pending true" "$(jq -r .device.lock_pending <<<"$G")" "true"
check "locked still false" "$(jq -r .device.locked <<<"$G")" "false"
check "status is not 'locked' any more" "$(jq -r '.device.status' <<<"$G")" "online"

# ---- WS: the stand-in agent acks + reports state -----------------------------------------------------
bun scripts/ws-agent.js "ws://127.0.0.1:${PORT}/agent/ws" "$DTOKEN" 3000 >/tmp/ost-smoke-ws.log 2>&1 &
WS_PID=$!
sleep 1.8
G=$(P "$BASE/api/devices/$DEVICE")
check "while connected: status online" "$(jq -r .device.status <<<"$G")" "online"
check "while connected: hub says online" "$(jq -r .device.online <<<"$G")" "true"
check "while connected: locked true (agent said so)" "$(jq -r .device.locked <<<"$G")" "true"
check "while connected: lock_pending cleared by ack" "$(jq -r .device.lock_pending <<<"$G")" "false"
check "state frame persisted" "$(jq -r '.device.last_state.frozen_users[0]' <<<"$G")" "mia"
check "agent_version from state frame" "$(jq -r .device.agent_version <<<"$G")" "0.4.0-smoke"
wait "$WS_PID"; WS_RC=$?
check "stand-in agent exited clean" "$WS_RC" "0"
sleep 0.5
G=$(P "$BASE/api/devices/$DEVICE")
check "after hangup: status offline immediately" "$(jq -r .device.status <<<"$G")" "offline"
check "after hangup: locked survives" "$(jq -r .device.locked <<<"$G")" "true"
check "WS heartbeat usage landed in the ledger" "$(P "$BASE/api/family" | jq -r '.children[]|select(.name=="Mia")|.used_minutes')" "12"
check "family child locked" "$(P "$BASE/api/family" | jq -r '.children[]|select(.name=="Mia")|.locked')" "true"

# ---- unlock + rotate --------------------------------------------------------------------------------
U=$(P -X POST "$BASE/api/devices/$DEVICE/unlock")
check "unlock enqueued" "$(jq -r .queued <<<"$U")" "true"
check "unlock pending, still locked" "$(P "$BASE/api/devices/$DEVICE" | jq -r '[.device.lock_pending,.device.locked]|join(",")')" "true,true"
ROT=$(P -X POST "$BASE/api/devices/$DEVICE/parent-code/rotate")
NEWSECRET=$(jq -r .parent_code.secret <<<"$ROT")
check "rotate yields a new secret" "$([[ "$NEWSECRET" != "$SECRET" && ${#NEWSECRET} == 32 ]] && echo yes)" "yes"
check "agent sees the rotated secret" "$(A "$BASE/agent/policy" | jq -r .parent_code.totp_secret)" "$NEWSECRET"

# ---- delete member --------------------------------------------------------------------------------------
check "cannot delete a parent via /api/members" "$(P -o /dev/null -w '%{http_code}' -X DELETE "$BASE/api/members/$OWNER")" "400"
check "delete member" "$(P -X DELETE "$BASE/api/members/$MEMBER" | jq -r .ok)" "true"
check "member session is gone with the member" "$(C -o /dev/null -w '%{http_code}' "$BASE/api/me")" "401"

echo
if [[ $fails -eq 0 ]]; then echo "ALL PASS"; else echo "$fails FAILED"; tail -30 /tmp/ost-smoke-server.log; exit 1; fi
