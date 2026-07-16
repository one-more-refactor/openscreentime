//! The parent companion surface.
//!
//! Two halves:
//!   * **admin token management** (`AuthAdmin`) — a logged-in admin mints,
//!     lists, and revokes `parent_access_tokens` from Settings. The raw token
//!     is returned once at mint and never again (stored sha256-hashed).
//!   * **the parent API** (`ParentAuth`, `/api/parent/*`) — a paired companion
//!     (tray parent-mode or a phone) reads pending earn-requests + recent
//!     alerts and approves/denies requests with its bearer token. Deliberately
//!     narrow: it cannot touch policy, devices, SSH, or admin settings.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::AppResult;
use crate::state::{AppState, AuthAdmin, ParentAuth};

// ---------------------------------------------------------------------------
// Admin: manage parent tokens
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct MintReq {
    #[serde(default)]
    pub label: String,
}

/// POST /api/parent-tokens — mint a new parent access token. Returns the raw
/// token exactly once; only its hash is stored.
pub async fn mint_token(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Json(req): Json<MintReq>,
) -> AppResult<Json<Value>> {
    let raw = crate::auth::gen_token();
    let hash = crate::auth::hash_token(&raw);
    let label = req.label.trim();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO parent_access_tokens (tenant_id, token_hash, label, created_by)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(admin.tenant_id)
    .bind(&hash)
    .bind(label)
    .bind(admin.admin_id)
    .fetch_one(&st.db)
    .await?;
    // `token` is shown once — the client must copy it now.
    Ok(Json(json!({ "id": id, "label": label, "token": raw })))
}

/// (id, label, created_at, last_used_at, revoked_at) — one parent token, minus
/// the hash (never returned).
type TokenRow = (
    Uuid,
    String,
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// GET /api/parent-tokens — list the tenant's parent tokens (never the raw
/// value; it isn't recoverable).
pub async fn list_tokens(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let rows: Vec<TokenRow> = sqlx::query_as(
        "SELECT id, label, created_at, last_used_at, revoked_at
         FROM parent_access_tokens
         WHERE tenant_id = $1 ORDER BY created_at DESC",
    )
    .bind(admin.tenant_id)
    .fetch_all(&st.db)
    .await?;
    let tokens: Vec<Value> = rows
        .into_iter()
        .map(|(id, label, created_at, last_used_at, revoked_at)| {
            json!({
                "id": id,
                "label": label,
                "created_at": created_at,
                "last_used_at": last_used_at,
                "revoked": revoked_at.is_some(),
            })
        })
        .collect();
    Ok(Json(json!({ "tokens": tokens })))
}

/// DELETE /api/parent-tokens/:id — revoke a token (idempotent). Scoped to the
/// admin's tenant so one tenant can't revoke another's.
pub async fn revoke_token(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    sqlx::query(
        "UPDATE parent_access_tokens SET revoked_at = COALESCE(revoked_at, now())
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(admin.tenant_id)
    .execute(&st.db)
    .await?;
    Ok(Json(json!({ "revoked": true, "id": id })))
}

// ---------------------------------------------------------------------------
// Parent API (ParentAuth) — the narrow, token-scoped surface
// ---------------------------------------------------------------------------

/// GET /api/parent/earn-requests — pending time requests for the parent to
/// decide (defaults to pending; the companion only ever needs that view).
pub async fn list_earn_requests(
    State(st): State<AppState>,
    parent: ParentAuth,
) -> AppResult<Json<Value>> {
    Ok(Json(
        crate::earn::list_for_tenant(&st.db, parent.tenant_id, Some("pending".into())).await?,
    ))
}

/// POST /api/parent/earn-requests/:id/approve
pub async fn approve(
    State(st): State<AppState>,
    parent: ParentAuth,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    crate::earn::decide(
        st,
        parent.tenant_id,
        id,
        true,
        json!({ "parent_token": parent.token_id }),
    )
    .await
}

/// POST /api/parent/earn-requests/:id/deny
pub async fn deny(
    State(st): State<AppState>,
    parent: ParentAuth,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    crate::earn::decide(
        st,
        parent.tenant_id,
        id,
        false,
        json!({ "parent_token": parent.token_id }),
    )
    .await
}

/// GET /api/parent/alerts — recent warnings + criticals (tamper, evasion,
/// locks, screen-time) for the parent to glance at.
pub async fn alerts(State(st): State<AppState>, parent: ParentAuth) -> AppResult<Json<Value>> {
    let events = crate::events::recent_alerts(&st.db, parent.tenant_id, 50).await?;
    Ok(Json(json!({ "alerts": events })))
}
