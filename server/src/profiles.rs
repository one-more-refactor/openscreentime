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
use openscreentime_policy::Policy;

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
fn normalize_policy(v: Value) -> AppResult<Value> {
    let p: Policy = serde_json::from_value(v)
        .map_err(|e| AppError::BadRequest(format!("invalid policy: {e}")))?;
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
    Ok(serde_json::to_value(p).unwrap())
}

/// `blocks` hygiene: ids de-duplicated (unknown ones tolerated — a newer
/// console may know apps this server's catalog does not yet), custom domains
/// lower-cased, trimmed, de-duplicated, and **rejected** if they carry anything
/// but hostname characters — like `dns.upstream`, they end up verbatim in the
/// device's resolver config.
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
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for raw in &b.custom_domains {
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
                "blocks.custom_domains: {raw:?} is not a domain name"
            )));
        }
        if seen.insert(d.clone()) {
            out.push(d);
        }
    }
    b.custom_domains = out;
    Ok(())
}

/// Every profile in a tenant, as JSON. Shared with the family view so both
/// return the identical shape from the identical query.
pub async fn list_for_tenant(db: &sqlx::PgPool, tenant_id: Uuid) -> AppResult<Value> {
    let rows: Vec<ProfileRow> = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE tenant_id = $1 \
         ORDER BY is_preset DESC, name"
    ))
    .bind(tenant_id)
    .fetch_all(db)
    .await?;
    Ok(json!(rows
        .into_iter()
        .map(profile_to_json)
        .collect::<Vec<_>>()))
}

pub async fn list_profiles(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    Ok(Json(json!({
        "profiles": list_for_tenant(&st.db, admin.tenant_id).await?
    })))
}

pub async fn get_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
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
            None => normalize_policy(existing_policy.clone())?,
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

    let mut affected_devices: Vec<(Uuid,)> = Vec::new();
    if let Some(policy) = &new_policy {
        sqlx::query("UPDATE profiles SET policy = $1, updated_at = now() WHERE id = $2")
            .bind(policy)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        affected_devices =
            sqlx::query_as("SELECT DISTINCT device_id FROM device_users WHERE profile_id = $1")
                .bind(id)
                .fetch_all(&mut *tx)
                .await?;
    }

    let row: ProfileRow = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM profiles WHERE id = $1"
    ))
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    // Only after the row is durably committed do we tell affected devices to
    // re-pull (WS agents get it pushed; poll agents catch up via the heartbeat
    // policy_version). Done outside the tx so a hub push can't hold the lock.
    for (device_id,) in affected_devices {
        enqueue_command(&st, device_id, "apply_policy", json!({})).await?;
    }
    Ok(Json(json!({ "profile": profile_to_json(row) })))
}

pub async fn delete_profile(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
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
