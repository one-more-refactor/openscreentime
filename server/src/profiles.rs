//! Admin profile CRUD. Presets are editable in place; custom profiles are
//! freely created/deleted. Policy is validated by round-tripping through the
//! shared `Policy` type.

use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHasher,
};
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::enqueue_command;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, AuthAdmin};
use openscreentime_policy::{AgeBracket, Policy};

/// Minimum parent-PIN length. Short PINs are still hashed, but we reject them
/// up front so a fat-fingered "1" doesn't become the household's lockout key.
const MIN_PIN_LEN: usize = 4;

/// Hash a parent PIN with Argon2 for storage as `policy.parent_pin_hash`. The
/// plaintext PIN is never stored or returned; the agent verifies entered PINs
/// against this hash locally.
pub(crate) async fn hash_pin(pin: String) -> AppResult<String> {
    // Argon2 is deliberately CPU/memory-hard; run it off the async worker so a
    // burst of PIN saves can't stall heartbeats/WS on this internet-exposed box.
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(pin.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| AppError::Internal(anyhow::anyhow!("failed to hash parent pin: {e}")))
    })
    .await
    .map_err(|e| AppError::Internal(anyhow::anyhow!("pin hash task failed: {e}")))?
}

/// Interpretation of the request's `parent_pin` field against the previously
/// stored policy JSON. Applies the set/clear/preserve semantics documented on
/// `CreateProfileReq`/`UpdateProfileReq`, mutating `policy` (a normalized
/// policy `Value`, always a JSON object) in place.
async fn apply_parent_pin(
    policy: &mut Value,
    parent_pin: Option<String>,
    previous: Option<&Value>,
) -> AppResult<()> {
    // Compute the hash (if any) up front so no mutable borrow of `policy` is
    // held across the `.await`.
    let action = match parent_pin {
        // Explicit non-empty PIN: hash and set it.
        Some(pin) if !pin.is_empty() => {
            if pin.len() < MIN_PIN_LEN {
                return Err(AppError::BadRequest(format!(
                    "parent pin must be at least {MIN_PIN_LEN} characters"
                )));
            }
            Some(json!(hash_pin(pin).await?))
        }
        // Explicit empty string: clear the pin.
        Some(_) => None,
        // Absent: preserve whatever hash the stored policy already had.
        None => previous.and_then(|p| p.get("parent_pin_hash")).cloned(),
    };

    let obj = policy
        .as_object_mut()
        .expect("normalize_policy always yields a JSON object");
    match action {
        Some(hash) => {
            obj.insert("parent_pin_hash".into(), hash);
        }
        None => {
            obj.remove("parent_pin_hash");
        }
    }
    Ok(())
}

type ProfileRow = (
    Uuid,
    Uuid,
    String,
    String,
    bool,
    Value,
    DateTime<Utc>,
    DateTime<Utc>,
);

const PROFILE_COLS: &str = "id, tenant_id, name, kind, is_preset, policy, created_at, updated_at";

fn profile_to_json(r: ProfileRow) -> Value {
    json!({
        "id": r.0,
        "tenant_id": r.1,
        "name": r.2,
        "kind": r.3,
        "is_preset": r.4,
        "policy": r.5,
        "created_at": r.6,
        "updated_at": r.7,
    })
}

/// Validate a raw policy value by deserializing into the shared `Policy` type
/// (forward-compat: unknown fields are tolerated), then re-serialize so what we
/// store is canonical.
pub(crate) fn normalize_policy(v: Value) -> AppResult<Value> {
    normalize(v, true)
}

/// `check_rules`: reject screen-time rules that can't mean anything (an empty
/// window, a whole-day bedtime, unreadable times — `rules::validate_screen_time`)
/// with a plain message. Off only when re-saving an already-stored policy for
/// an unrelated change (a PIN), so an old profile never blocks that.
fn normalize(v: Value, check_rules: bool) -> AppResult<Value> {
    let p: Policy = serde_json::from_value(v)
        .map_err(|e| AppError::BadRequest(format!("invalid policy: {e}")))?;
    if check_rules {
        openscreentime_policy::rules::validate_screen_time(&p.screen_time)
            .map_err(AppError::BadRequest)?;
        openscreentime_policy::rules::validate_focus(&p.focus).map_err(AppError::BadRequest)?;
    }
    // The DNS upstream is interpolated verbatim into the agent's nftables
    // ruleset (`ip daddr <upstream> ...`). Require a literal IP so a hostname,
    // typo, or injected nft syntax can't ever reach the agent — a malformed
    // rule would otherwise abort the whole ruleset load on the device.
    if !p.dns.upstream.is_empty() && p.dns.upstream.parse::<std::net::IpAddr>().is_err() {
        return Err(AppError::BadRequest(format!(
            "dns.upstream must be an IP address, got {:?}",
            p.dns.upstream
        )));
    }
    let mut p = p;
    sanitize_blocks(&mut p.blocks)?;
    sanitize_domains(&mut p.focus.sites, "sites")?;
    Ok(serde_json::to_value(p).unwrap())
}

/// `blocks` hygiene: ids de-duplicated (unknown ones tolerated — a newer
/// console may know apps this server's catalog does not yet), custom domains
/// cleaned by [`sanitize_domains`].
fn sanitize_blocks(b: &mut openscreentime_policy::AppBlocks) -> AppResult<()> {
    fn dedupe(v: &mut Vec<String>) {
        let mut seen = std::collections::BTreeSet::new();
        v.retain(|s| !s.trim().is_empty() && seen.insert(s.trim().to_string()));
        for s in v.iter_mut() {
            *s = s.trim().to_string();
        }
    }
    dedupe(&mut b.apps);
    dedupe(&mut b.categories);
    sanitize_domains(&mut b.custom_domains, "blocks.custom_domains")
}

/// Domains typed by a person (a parent's custom blocks, someone's own focus
/// sites): lower-cased, trimmed of spaces and dots, de-duplicated — and
/// **rejected** if they carry anything but hostname characters: like
/// `dns.upstream`, they end up verbatim in the device's resolver config.
pub(crate) fn sanitize_domains(list: &mut Vec<String>, field: &str) -> AppResult<()> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for raw in list.iter() {
        let d = raw
            .trim()
            .trim_start_matches('.')
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if d.is_empty() {
            continue;
        }
        let ok = d.len() <= 253
            && d.contains('.')
            && d.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_');
        if !ok {
            return Err(AppError::BadRequest(format!(
                "{field}: {raw:?} is not a domain name"
            )));
        }
        if seen.insert(d.clone()) {
            out.push(d);
        }
    }
    *list = out;
    Ok(())
}

/// Write an already-normalized policy to a profile inside `tx`, bumping
/// `updated_at` (the agents' policy version), and return the devices that must
/// re-pull it. Tell them with [`notify_devices`] once `tx` has committed.
pub(crate) async fn write_policy(
    tx: &mut sqlx::PgConnection,
    id: Uuid,
    policy: &Value,
) -> AppResult<Vec<Uuid>> {
    sqlx::query("UPDATE profiles SET policy = $1, updated_at = now() WHERE id = $2")
        .bind(policy)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let devices: Vec<(Uuid,)> =
        sqlx::query_as("SELECT DISTINCT device_id FROM device_users WHERE profile_id = $1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await?;
    Ok(devices.into_iter().map(|(d,)| d).collect())
}

/// Tell devices to re-pull their policy (WS agents get it pushed; poll agents
/// catch up via the heartbeat's policy version). Only after the write is
/// durably committed, and outside the transaction so a hub push can't hold
/// the row lock.
pub(crate) async fn notify_devices(st: &AppState, devices: Vec<Uuid>) -> AppResult<()> {
    for device_id in devices {
        enqueue_command(st, device_id, "apply_policy", json!({})).await?;
    }
    Ok(())
}

/// The profile is someone else's own — the rules a parent keeps for
/// themselves, or an adult's or self-managed person's — which only they see
/// and change (`/api/me/rules`). Presets are never private.
pub(crate) async fn private_to_other(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    profile_id: Uuid,
    viewer: Uuid,
) -> AppResult<bool> {
    Ok(private_profile_ids(db, tenant_id, viewer)
        .await?
        .contains(&profile_id))
}

/// Every profile in the tenant that is someone's own rules and not the
/// viewer's — whoever `members::sets_own_rules` says keeps their own: every
/// parent (the hub included) for themselves, adults, self-managed people. No
/// one else sees or edits those, not even another parent.
pub(crate) async fn private_profile_ids(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    viewer: Uuid,
) -> AppResult<std::collections::HashSet<Uuid>> {
    let rows: Vec<(Uuid, String, String, bool)> = sqlx::query_as(
        "SELECT a.profile_id, a.role, a.age_bracket, a.self_managed
           FROM admins a JOIN profiles p ON p.id = a.profile_id
          WHERE a.tenant_id = $1 AND a.id <> $2 AND NOT p.is_preset",
    )
    .bind(tenant_id)
    .bind(viewer)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|(_, role, bracket, self_managed)| {
            let bracket = AgeBracket::parse(bracket).unwrap_or(AgeBracket::Adult);
            crate::members::sets_own_rules(role, bracket, *self_managed)
        })
        .map(|(id, ..)| id)
        .collect())
}

fn their_rules_are_their_own() -> AppError {
    AppError::ForbiddenForMember("their rules are their own".into())
}

/// Startup backfill: open every pre-0.6 closed-network profile. A profile
/// still on `default_deny` DNS or firewall ("only approved sites work") becomes
/// a normal allow-by-default one — `allow_all` with the `*` wildcard — and
/// keeps every block it had (catalog blocks, the blocklist, safe search, the
/// filtered upstream, lockdown, screen time). Nobody is asked: the old posture
/// broke apt, game launchers and school software, and the console no longer
/// has a control for it. Idempotent; `updated_at` moves so agents re-pull.
pub async fn open_legacy_networks(db: &sqlx::PgPool) -> AppResult<u64> {
    let res = sqlx::query(
        "UPDATE profiles
            SET policy = jsonb_set(
                           jsonb_set(
                             jsonb_set(
                               jsonb_set(policy, '{dns,mode}', '\"allow_all\"', true),
                               '{dns,allowlist}', '[\"*\"]', true),
                             '{firewall,mode}', '\"allow_all\"', true),
                           '{firewall,allow_outbound_ports}', '[]', true),
                updated_at = now()
          WHERE policy->'dns'->>'mode' = 'default_deny'
             OR policy->'firewall'->>'mode' = 'default_deny'",
    )
    .execute(db)
    .await?;
    Ok(res.rows_affected())
}

/// Every profile in a tenant the viewer may see, as JSON — not the rules a
/// self-managed person set for themselves ([`private_profile_ids`]). Shared
/// with the family view so both return the identical shape from the
/// identical query.
pub async fn list_for_tenant(db: &sqlx::PgPool, tenant_id: Uuid, viewer: Uuid) -> AppResult<Value> {
    let hidden = private_profile_ids(db, tenant_id, viewer).await?;
    let rows: Vec<ProfileRow> = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE tenant_id = $1 \
         ORDER BY is_preset DESC, name"
    ))
    .bind(tenant_id)
    .fetch_all(db)
    .await?;
    Ok(json!(rows
        .into_iter()
        .filter(|r| !hidden.contains(&r.0))
        .map(profile_to_json)
        .collect::<Vec<_>>()))
}

pub async fn list_profiles(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    Ok(Json(json!({
        "profiles": list_for_tenant(&st.db, admin.tenant_id, admin.admin_id).await?
    })))
}

pub async fn get_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    if private_to_other(&st.db, admin.tenant_id, id, admin.admin_id).await? {
        return Err(their_rules_are_their_own());
    }
    let row: Option<ProfileRow> = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE id = $1 AND tenant_id = $2"
    ))
    .bind(id)
    .bind(admin.tenant_id)
    .fetch_optional(&st.db)
    .await?;
    let row = row.ok_or_else(|| AppError::NotFound("profile not found".into()))?;
    Ok(Json(json!({ "profile": profile_to_json(row) })))
}

#[derive(Deserialize)]
pub struct CreateProfileReq {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    pub policy: Value,
    /// Optional parent PIN to set on creation. Absent = no PIN; empty string is
    /// treated the same (there is no existing hash to clear). Hashed with
    /// Argon2 before storage; the plaintext is never persisted.
    #[serde(default)]
    pub parent_pin: Option<String>,
}

pub async fn create_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Json(req): Json<CreateProfileReq>,
) -> AppResult<Json<Value>> {
    if req.name.trim().is_empty() {
        return Err(AppError::BadRequest("name required".into()));
    }
    // New profiles are always `custom` (presets are seeded only at tenant
    // creation). We accept the field but pin it.
    let kind = req.kind.unwrap_or_else(|| "custom".into());
    if kind != "custom" {
        return Err(AppError::BadRequest(
            "only custom profiles may be created".into(),
        ));
    }
    let mut policy = normalize_policy(req.policy)?;
    apply_parent_pin(&mut policy, req.parent_pin, None).await?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO profiles (tenant_id, name, kind, is_preset, policy)
         VALUES ($1, $2, 'custom', false, $3) RETURNING id",
    )
    .bind(admin.tenant_id)
    .bind(&req.name)
    .bind(&policy)
    .fetch_one(&st.db)
    .await?;

    let row: ProfileRow = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&st.db)
    .await?;
    Ok(Json(json!({ "profile": profile_to_json(row) })))
}

#[derive(Deserialize)]
pub struct UpdateProfileReq {
    pub name: Option<String>,
    pub policy: Option<Value>,
    /// Parent PIN change: `None` (field absent) preserves the existing hash,
    /// `Some("")` clears it, `Some(non-empty)` sets a new hash. See
    /// `apply_parent_pin`.
    #[serde(default)]
    pub parent_pin: Option<String>,
}

pub async fn update_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateProfileReq>,
) -> AppResult<Json<Value>> {
    // A self-managed person's own rules are theirs to change (/api/me/rules).
    if private_to_other(&st.db, admin.tenant_id, id, admin.admin_id).await? {
        return Err(their_rules_are_their_own());
    }
    // All reads + writes happen in one transaction with the row locked
    // (`FOR UPDATE`), so a concurrent update can't interleave between reading the
    // stored policy and writing the merged one (which would resurrect a
    // just-cleared PIN or drop a just-set one). Presets are editable in place.
    let mut tx = st.db.begin().await?;

    // Fetch + lock the row. The existing policy lets a PIN change (or an update
    // that doesn't resend `parent_pin`) preserve/clear the stored hash.
    let existing: Option<(Uuid, Value)> = sqlx::query_as(
        "SELECT id, policy FROM profiles WHERE id = $1 AND tenant_id = $2 FOR UPDATE",
    )
    .bind(id)
    .bind(admin.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (_, existing_policy) =
        existing.ok_or_else(|| AppError::NotFound("profile not found".into()))?;

    // A policy update always carries the pin decision; if there's no policy
    // update but the admin is only changing the PIN, we still normalize+save the
    // (otherwise unchanged) stored policy with the new hash. Validate/normalize
    // BEFORE any write so a bad policy body can't leave a half-applied name.
    let policy_change = req.policy.is_some() || req.parent_pin.is_some();
    let new_policy = if policy_change {
        let mut policy = match req.policy {
            Some(policy) => normalize_policy(policy)?,
            None => normalize(existing_policy.clone(), false)?,
        };
        apply_parent_pin(&mut policy, req.parent_pin, Some(&existing_policy)).await?;
        Some(policy)
    } else {
        None
    };

    if let Some(name) = &req.name {
        sqlx::query("UPDATE profiles SET name = $1, updated_at = now() WHERE id = $2")
            .bind(name)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }

    let affected_devices = match &new_policy {
        Some(policy) => write_policy(&mut tx, id, policy).await?,
        None => Vec::new(),
    };

    let row: ProfileRow = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    notify_devices(&st, affected_devices).await?;
    Ok(Json(json!({ "profile": profile_to_json(row) })))
}

pub async fn delete_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    if private_to_other(&st.db, admin.tenant_id, id, admin.admin_id).await? {
        return Err(their_rules_are_their_own());
    }
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT is_preset FROM profiles WHERE id = $1 AND tenant_id = $2")
            .bind(id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    let is_preset = row
        .ok_or_else(|| AppError::NotFound("profile not found".into()))?
        .0;
    if is_preset {
        return Err(AppError::BadRequest(
            "preset profiles cannot be deleted".into(),
        ));
    }

    // Guard against deleting a profile still in use.
    let in_use: i64 = sqlx::query_scalar("SELECT count(*) FROM device_users WHERE profile_id = $1")
        .bind(id)
        .fetch_one(&st.db)
        .await?;
    if in_use > 0 {
        return Err(AppError::Conflict(
            "profile is assigned to device users".into(),
        ));
    }

    sqlx::query("DELETE FROM profiles WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(admin.tenant_id)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::password_hash::PasswordVerifier;
    use argon2::PasswordHash;

    #[test]
    fn blocks_are_cleaned_and_bad_domains_rejected() {
        let v = normalize_policy(json!({
            "blocks": { "apps": ["youtube", "youtube", " tiktok "],
                        "categories": ["adult"],
                        "custom_domains": [" .Example.ORG. ", "example.org", "foo.bar"] }
        }))
        .unwrap();
        assert_eq!(v["blocks"]["apps"], json!(["youtube", "tiktok"]));
        assert_eq!(
            v["blocks"]["custom_domains"],
            json!(["example.org", "foo.bar"])
        );

        let err = normalize_policy(json!({ "blocks": { "custom_domains": ["evil.com/x y"] } }))
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
        let err =
            normalize_policy(json!({ "blocks": { "custom_domains": ["nodot"] } })).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
        // Empty blocks vanish from the stored document.
        let v = normalize_policy(json!({ "blocks": { "apps": [] } })).unwrap();
        assert!(v.get("blocks").is_none());
    }

    #[test]
    fn screen_time_rules_are_validated_on_save() {
        // An empty window is rejected with a message a parent can act on.
        let err = normalize_policy(json!({ "screen_time": { "enabled": true,
            "schedule": [{ "days": [1,2,3,4,5], "start": "15:00", "end": "15:00" }] } }))
        .unwrap_err();
        match err {
            AppError::BadRequest(m) => assert!(m.contains("empty"), "{m}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
        // A whole-day bedtime too.
        assert!(normalize_policy(json!({ "screen_time": { "enabled": true,
            "bedtime": { "start": "00:00", "end": "00:00" } } }))
        .is_err());
        // What the console sends: to midnight, across midnight, "any time"
        // (no window for the weekend) — all fine.
        assert!(normalize_policy(json!({ "screen_time": { "enabled": true,
            "daily_limit_minutes": 90,
            "schedule": [{ "days": [1,2,3,4,5], "start": "15:00", "end": "00:00" },
                         { "days": [5], "start": "20:00", "end": "01:00" }],
            "bedtime": { "start": "22:00", "end": "07:00" } } }))
        .is_ok());
        // A stored legacy policy never blocks an unrelated re-save (a PIN).
        assert!(normalize(
            json!({ "screen_time": { "schedule": [{ "days": [1], "start": "9:00", "end": "9:00" }] } }),
            false
        )
        .is_ok());
    }

    #[tokio::test]
    async fn hash_pin_roundtrips_through_argon2_verify() {
        let hash = hash_pin("1234".into()).await.unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(Argon2::default().verify_password(b"1234", &parsed).is_ok());
        assert!(Argon2::default()
            .verify_password(b"wrong", &parsed)
            .is_err());
    }

    #[tokio::test]
    async fn apply_parent_pin_sets_clears_and_preserves() {
        // Set: non-empty pin hashes into parent_pin_hash.
        let mut policy = json!({});
        apply_parent_pin(&mut policy, Some("5678".into()), None)
            .await
            .unwrap();
        assert!(policy.get("parent_pin_hash").is_some());

        // Preserve: absent pin keeps the previously stored hash.
        let previous = policy.clone();
        let mut policy2 = json!({});
        apply_parent_pin(&mut policy2, None, Some(&previous))
            .await
            .unwrap();
        assert_eq!(policy2["parent_pin_hash"], previous["parent_pin_hash"]);

        // Clear: explicit empty string removes the hash.
        let mut policy3 = previous.clone();
        apply_parent_pin(&mut policy3, Some(String::new()), Some(&previous))
            .await
            .unwrap();
        assert!(policy3.get("parent_pin_hash").is_none());
    }

    #[tokio::test]
    async fn apply_parent_pin_rejects_short_pin() {
        let mut policy = json!({});
        let err = apply_parent_pin(&mut policy, Some("12".into()), None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }
}
