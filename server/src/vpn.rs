//! Named VPN profiles per device: several stored, exactly one active, and the
//! agent tests a newly-activated config before enforcing it (rolling back and
//! reporting `failed` if the tunnel doesn't come up).
//!
//! Secrets policy: the config body carries private keys, so admin responses
//! only ever contain a MASKED rendering. Edits round-trip through the mask —
//! a line whose value is the mask token keeps its stored secret, so the admin
//! can change endpoints/DNS/allowed-ips without ever seeing or re-pasting keys.
//! The raw config is served exclusively on the authenticated agent policy pull.

use axum::extract::{Path, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::agent::enqueue_command;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{AppState, AuthAdmin};

/// Hard cap on an uploaded VPN config. Real wg/ovpn client configs are a few
/// KB; anything bigger is a mistake (or an attempt to stuff the DB/policy pull).
const MAX_VPN_CONFIG_BYTES: usize = 64 * 1024;

/// What a masked secret renders as, and what an edited config may contain in
/// place of a secret to mean "keep the stored one".
const MASK: &str = "•••";

/// WireGuard-style `Key = value` lines whose values are secrets.
const SECRET_KEYS: &[&str] = &["PrivateKey", "PresharedKey"];
/// OpenVPN inline blocks whose contents are secrets.
const SECRET_BLOCKS: &[&str] = &["key", "tls-auth", "tls-crypt", "secret", "pkcs12"];

// ---------------------------------------------------------------------------
// Masking / merging
// ---------------------------------------------------------------------------

/// Render a config with every secret replaced by the mask token.
fn mask_config(config: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_secret_block: Option<String> = None;
    for line in config.lines() {
        let trimmed = line.trim();
        if let Some(tag) = &in_secret_block {
            if trimmed == format!("</{tag}>") {
                out.push(MASK.to_string());
                out.push(line.to_string());
                in_secret_block = None;
            }
            // secret block content: dropped (replaced by one MASK line at close)
            continue;
        }
        if let Some(tag) = SECRET_BLOCKS.iter().find(|t| trimmed == format!("<{}>", t)) {
            out.push(line.to_string());
            in_secret_block = Some(tag.to_string());
            continue;
        }
        if let Some((k, _)) = line.split_once('=') {
            if SECRET_KEYS.iter().any(|s| k.trim() == *s) {
                out.push(format!("{} = {MASK}", k.trim_end()));
                continue;
            }
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

/// Merge an admin-edited (possibly masked) config with the stored original:
/// any secret whose value is the mask token is restored from `stored`.
fn merge_config(edited: &str, stored: &str) -> String {
    // Collect stored secrets: key-line values and block bodies.
    let mut stored_kv: std::collections::HashMap<String, String> = Default::default();
    let mut stored_blocks: std::collections::HashMap<String, String> = Default::default();
    let mut block: Option<(String, Vec<String>)> = None;
    for line in stored.lines() {
        let trimmed = line.trim();
        if let Some((tag, body)) = &mut block {
            if trimmed == format!("</{tag}>") {
                stored_blocks.insert(tag.clone(), body.join("\n"));
                block = None;
            } else {
                body.push(line.to_string());
            }
            continue;
        }
        if let Some(tag) = SECRET_BLOCKS.iter().find(|t| trimmed == format!("<{}>", t)) {
            block = Some((tag.to_string(), Vec::new()));
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if SECRET_KEYS.iter().any(|s| k.trim() == *s) {
                stored_kv.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut in_block: Option<String> = None;
    let mut block_buf: Vec<String> = Vec::new();
    for line in edited.lines() {
        let trimmed = line.trim();
        if let Some(tag) = &in_block {
            if trimmed == format!("</{tag}>") {
                let body = block_buf.join("\n");
                let restored = if body.trim() == MASK {
                    stored_blocks.get(tag).cloned().unwrap_or(body)
                } else {
                    body
                };
                out.push(restored);
                out.push(line.to_string());
                in_block = None;
                block_buf = Vec::new();
            } else {
                block_buf.push(line.to_string());
            }
            continue;
        }
        if let Some(tag) = SECRET_BLOCKS.iter().find(|t| trimmed == format!("<{}>", t)) {
            out.push(line.to_string());
            in_block = Some(tag.to_string());
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if SECRET_KEYS.iter().any(|s| k.trim() == *s) && v.trim() == MASK {
                let stored_v = stored_kv.get(k.trim()).cloned().unwrap_or_default();
                out.push(format!("{} = {}", k.trim_end(), stored_v));
                continue;
            }
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

/// Cheap shape check so a wrong-file upload fails here with a clear message
/// instead of on the device with a dead tunnel. Returns the sniffed kind.
fn sniff_kind(config: &str, kind_hint: Option<&str>) -> AppResult<&'static str> {
    let looks_wg = config.contains("[Interface]") && config.contains("PrivateKey");
    let looks_ovpn = config.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("remote ") || l == "client"
    });
    let kind = match kind_hint {
        Some("wireguard") => {
            if !looks_wg {
                return Err(AppError::BadRequest(
                    "this does not look like a wireguard client config".into(),
                ));
            }
            "wireguard"
        }
        Some("openvpn") => {
            if !looks_ovpn {
                return Err(AppError::BadRequest(
                    "this does not look like an openvpn client config".into(),
                ));
            }
            "openvpn"
        }
        Some(_) => {
            return Err(AppError::BadRequest(
                "kind must be 'wireguard' or 'openvpn'".into(),
            ))
        }
        None if looks_wg => "wireguard",
        None if looks_ovpn => "openvpn",
        None => {
            return Err(AppError::BadRequest(
                "couldn't recognize this as a wireguard or openvpn client config".into(),
            ))
        }
    };
    Ok(kind)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

type ProfileRow = (
    Uuid,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<DateTime<Utc>>,
    bool,
    DateTime<Utc>,
);

const PROFILE_COLS: &str =
    "id, name, kind, config, status, last_error, last_tested_at, is_active, updated_at";

fn profile_to_json(r: &ProfileRow) -> Value {
    json!({
        "id": r.0,
        "name": r.1,
        "kind": r.2,
        "config_masked": mask_config(&r.3),
        "status": r.4,
        "last_error": r.5,
        "last_tested_at": r.6,
        "is_active": r.7,
        "updated_at": r.8,
    })
}

async fn ensure_device(st: &AppState, id: Uuid, tenant_id: Uuid) -> AppResult<()> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM devices WHERE id = $1 AND tenant_id = $2")
            .bind(id)
            .bind(tenant_id)
            .fetch_optional(&st.db)
            .await?;
    found.ok_or_else(|| AppError::NotFound("device not found".into()))?;
    Ok(())
}

/// A profile row scoped to the admin's tenant, or 404.
async fn get_profile(st: &AppState, id: Uuid, tenant_id: Uuid) -> AppResult<(Uuid, ProfileRow)> {
    let row: Option<(Uuid, ProfileRow)> = sqlx::query_as::<_, (Uuid, ProfileRow)>(&format!(
        "SELECT p.device_id, {} FROM device_vpn_profiles p
         JOIN devices d ON d.id = p.device_id
         WHERE p.id = $1 AND d.tenant_id = $2",
        PROFILE_COLS
            .split(", ")
            .map(|c| format!("p.{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
    .bind(id)
    .bind(tenant_id)
    .fetch_optional(&st.db)
    .await?;
    row.ok_or_else(|| AppError::NotFound("vpn profile not found".into()))
}

/// Bump the device's VPN change stamp (feeds policy_version) and nudge the agent.
async fn propagate(st: &AppState, device_id: Uuid) -> AppResult<()> {
    sqlx::query("UPDATE devices SET vpn_updated_at = now() WHERE id = $1")
        .bind(device_id)
        .execute(&st.db)
        .await?;
    enqueue_command(st, device_id, "apply_policy", json!({})).await?;
    Ok(())
}

/// GET /api/devices/:id/vpn — all profiles for a device, masked.
pub async fn list(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    ensure_device(&st, id, admin.tenant_id).await?;
    let rows: Vec<ProfileRow> = sqlx::query_as(&format!(
        "SELECT {PROFILE_COLS} FROM device_vpn_profiles
         WHERE device_id = $1 ORDER BY created_at"
    ))
    .bind(id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        json!({ "profiles": rows.iter().map(profile_to_json).collect::<Vec<_>>() }),
    ))
}

#[derive(Deserialize)]
pub struct CreateReq {
    pub name: String,
    pub kind: Option<String>,
    pub config: String,
}

/// POST /api/devices/:id/vpn — store a new named profile (inactive, untested).
pub async fn create(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    Json(req): Json<CreateReq>,
) -> AppResult<Json<Value>> {
    ensure_device(&st, id, admin.tenant_id).await?;
    let name = req.name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(AppError::BadRequest(
            "give the profile a name (1–64 chars)".into(),
        ));
    }
    if req.config.len() > MAX_VPN_CONFIG_BYTES {
        return Err(AppError::BadRequest(format!(
            "VPN config too large (max {} KB)",
            MAX_VPN_CONFIG_BYTES / 1024
        )));
    }
    let kind = sniff_kind(&req.config, req.kind.as_deref())?;

    let pid: Uuid = sqlx::query_scalar(
        "INSERT INTO device_vpn_profiles (device_id, name, kind, config)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (device_id, name) DO NOTHING
         RETURNING id",
    )
    .bind(id)
    .bind(name)
    .bind(kind)
    .bind(&req.config)
    .fetch_optional(&st.db)
    .await?
    .ok_or_else(|| AppError::Conflict(format!("a profile named '{name}' already exists")))?;

    events::insert(
        &st.db,
        admin.tenant_id,
        Some(id),
        None,
        "vpn_profile",
        "info",
        json!({ "action": "created", "profile": name, "kind": kind, "by": admin.admin_id }),
    )
    .await?;
    Ok(Json(json!({ "id": pid })))
}

#[derive(Deserialize)]
pub struct UpdateReq {
    pub name: Option<String>,
    /// Full config text; masked secrets (the ••• token) keep their stored value.
    pub config: Option<String>,
}

/// PUT /api/vpn-profiles/:id — rename and/or edit the config through the mask.
/// Editing an active profile re-runs the test cycle (status → testing).
pub async fn update(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateReq>,
) -> AppResult<Json<Value>> {
    let (device_id, row) = get_profile(&st, id, admin.tenant_id).await?;
    let name = match req.name.as_deref().map(str::trim) {
        Some(n) if !n.is_empty() && n.len() <= 64 => n.to_string(),
        Some(_) => return Err(AppError::BadRequest("bad profile name".into())),
        None => row.1.clone(),
    };
    let config = match &req.config {
        Some(edited) => {
            if edited.len() > MAX_VPN_CONFIG_BYTES {
                return Err(AppError::BadRequest("VPN config too large".into()));
            }
            let merged = merge_config(edited, &row.3);
            sniff_kind(&merged, Some(row.2.as_str()))?;
            merged
        }
        None => row.3.clone(),
    };
    let config_changed = config != row.3;
    let was_active = row.7;

    sqlx::query(
        "UPDATE device_vpn_profiles
         SET name = $1, config = $2, updated_at = now(),
             status = CASE WHEN $3 THEN 'untested' ELSE status END,
             last_error = CASE WHEN $3 THEN NULL ELSE last_error END
         WHERE id = $4",
    )
    .bind(&name)
    .bind(&config)
    .bind(config_changed && !was_active)
    .bind(id)
    .execute(&st.db)
    .await?;

    if config_changed && was_active {
        // The active tunnel's config changed: back through the test cycle.
        sqlx::query("UPDATE device_vpn_profiles SET status = 'testing' WHERE id = $1")
            .bind(id)
            .execute(&st.db)
            .await?;
        propagate(&st, device_id).await?;
    }
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        None,
        "vpn_profile",
        "info",
        json!({ "action": "updated", "profile": name, "by": admin.admin_id }),
    )
    .await?;
    Ok(Json(json!({ "updated": true })))
}

/// POST /api/vpn-profiles/:id/activate — make this the device's one active
/// profile. Status goes to `testing`; the agent applies, verifies, and reports
/// back `active` or `failed` (rolling back on failure).
pub async fn activate(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let (device_id, row) = get_profile(&st, id, admin.tenant_id).await?;
    let mut tx = st.db.begin().await?;
    sqlx::query("UPDATE device_vpn_profiles SET is_active = false WHERE device_id = $1")
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE device_vpn_profiles
         SET is_active = true, status = 'testing', last_error = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    propagate(&st, device_id).await?;
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        None,
        "vpn_profile",
        "info",
        json!({ "action": "activated", "profile": row.1, "by": admin.admin_id }),
    )
    .await?;
    Ok(Json(json!({ "activated": true, "status": "testing" })))
}

/// POST /api/vpn-profiles/:id/deactivate — no active profile: the agent tears
/// the tunnel down on its next apply.
pub async fn deactivate(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let (device_id, row) = get_profile(&st, id, admin.tenant_id).await?;
    sqlx::query(
        "UPDATE device_vpn_profiles
         SET is_active = false, status = 'untested', last_error = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .execute(&st.db)
    .await?;
    propagate(&st, device_id).await?;
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        None,
        "vpn_profile",
        "info",
        json!({ "action": "deactivated", "profile": row.1, "by": admin.admin_id }),
    )
    .await?;
    Ok(Json(json!({ "deactivated": true })))
}

/// DELETE /api/vpn-profiles/:id — remove a profile; if it was active the agent
/// tears the tunnel down.
pub async fn remove(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let (device_id, row) = get_profile(&st, id, admin.tenant_id).await?;
    sqlx::query("DELETE FROM device_vpn_profiles WHERE id = $1")
        .bind(id)
        .execute(&st.db)
        .await?;
    if row.7 {
        propagate(&st, device_id).await?;
    }
    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        None,
        "vpn_profile",
        "info",
        json!({ "action": "removed", "profile": row.1, "by": admin.admin_id }),
    )
    .await?;
    Ok(Json(json!({ "removed": true })))
}

// ---------------------------------------------------------------------------
// Agent-facing
// ---------------------------------------------------------------------------

/// The active (or being-tested) profile served on the agent policy pull —
/// the ONLY place the raw config leaves the database.
pub async fn active_for_agent(db: &sqlx::PgPool, device_id: Uuid) -> AppResult<Option<Value>> {
    let row: Option<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT id, kind, config, status FROM device_vpn_profiles
         WHERE device_id = $1 AND is_active",
    )
    .bind(device_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(id, kind, config, status)| {
        json!({
            "id": id,
            "kind": kind,
            "config": config,
            // "testing" asks the agent for the verify-then-report cycle.
            "status": status,
        })
    }))
}

/// Apply an agent's `vpn_profile` test report (event payload:
/// `{ profile_id, result: "active"|"failed", error? }`). Scoped by device so
/// an agent can only ever report on its own profiles.
pub async fn apply_agent_report(db: &sqlx::PgPool, device_id: Uuid, payload: &Value) {
    let Some(pid) = payload
        .get("profile_id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return;
    };
    let Some(result) = payload.get("result").and_then(|v| v.as_str()) else {
        return;
    };
    let (status, error) = match result {
        "active" => ("active", None),
        "failed" => ("failed", payload.get("error").and_then(|v| v.as_str())),
        _ => return,
    };
    let _ = sqlx::query(
        "UPDATE device_vpn_profiles
         SET status = $1, last_error = $2, last_tested_at = now()
         WHERE id = $3 AND device_id = $4",
    )
    .bind(status)
    .bind(error)
    .bind(pid)
    .bind(device_id)
    .execute(db)
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const WG: &str = "[Interface]\nPrivateKey = SECRETKEY123=\nAddress = 10.0.0.2/32\n\n[Peer]\nPublicKey = PUB=\nPresharedKey = PSK456=\nEndpoint = vpn.example.com:51820\nAllowedIPs = 0.0.0.0/0";

    #[test]
    fn masks_wireguard_secrets() {
        let masked = mask_config(WG);
        assert!(!masked.contains("SECRETKEY123"));
        assert!(!masked.contains("PSK456"));
        assert!(masked.contains("PrivateKey = •••"));
        assert!(masked.contains("PublicKey = PUB=")); // public stays visible
        assert!(masked.contains("Endpoint = vpn.example.com:51820"));
    }

    #[test]
    fn merge_restores_masked_secrets_and_keeps_edits() {
        let mut edited = mask_config(WG);
        edited = edited.replace("vpn.example.com:51820", "new.example.com:443");
        let merged = merge_config(&edited, WG);
        assert!(merged.contains("PrivateKey = SECRETKEY123="));
        assert!(merged.contains("PresharedKey = PSK456="));
        assert!(merged.contains("Endpoint = new.example.com:443"));
    }

    #[test]
    fn merge_accepts_replaced_secret() {
        let edited = WG.replace("SECRETKEY123=", "NEWKEY789=");
        let merged = merge_config(&edited, WG);
        assert!(merged.contains("PrivateKey = NEWKEY789="));
    }

    #[test]
    fn masks_openvpn_blocks() {
        let ovpn = "client\nremote vpn.example.com 1194\n<key>\nSECRET\nMATERIAL\n</key>\n<ca>\nCERT\n</ca>";
        let masked = mask_config(ovpn);
        assert!(!masked.contains("SECRET"));
        assert!(masked.contains("<key>\n•••\n</key>"));
        assert!(masked.contains("CERT")); // ca cert is not a secret
        let merged = merge_config(&masked, ovpn);
        assert!(merged.contains("SECRET\nMATERIAL"));
    }

    #[test]
    fn sniffs_kinds() {
        assert_eq!(sniff_kind(WG, None).unwrap(), "wireguard");
        assert_eq!(
            sniff_kind("client\nremote x 1194", None).unwrap(),
            "openvpn"
        );
        assert!(sniff_kind("hello", None).is_err());
        assert!(sniff_kind(WG, Some("openvpn")).is_err());
    }
}
