//! Passkey (WebAuthn) auth via `webauthn-rs`, plus token hashing, session
//! cookies, and tenant bootstrap (creates tenant + admin + seeds presets).

use axum::{extract::State, Json};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use webauthn_rs::prelude::{Passkey, PublicKeyCredential, RegisterPublicKeyCredential};

use crate::error::{AppError, AppResult};
use crate::presets;
use crate::state::{
    AppState, AuthAdmin, AuthChallenge, RegChallenge, SessionData, AUTH_COOKIE, REG_COOKIE,
    SESSION_COOKIE,
};

// ---------------------------------------------------------------------------
// Token helpers
// ---------------------------------------------------------------------------

/// Sha256-hex of a token. Device tokens are stored hashed at rest.
pub fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

/// A fresh random 256-bit token, hex-encoded.
pub fn gen_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn session_cookie(value: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, value))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build()
}

fn temp_cookie(name: &'static str, value: String) -> Cookie<'static> {
    // Short-lived challenge cookie. It is a session cookie (cleared on browser
    // close) and is explicitly removed once the challenge is consumed.
    Cookie::build((name, value))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build()
}

// ---------------------------------------------------------------------------
// Tenant bootstrap
// ---------------------------------------------------------------------------

/// Creates a tenant, seeds the three preset profiles verbatim, and creates the
/// first admin. Returns (tenant_id, admin_id).
pub async fn create_tenant_with_admin(
    db: &sqlx::PgPool,
    email: &str,
    display_name: &str,
) -> AppResult<(Uuid, Uuid)> {
    let mut tx = db.begin().await.map_err(AppError::from)?;

    let tenant_id: Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(format!("{display_name}'s org"))
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::from)?;

    for p in presets::all_presets() {
        sqlx::query(
            "INSERT INTO profiles (tenant_id, name, kind, is_preset, policy)
             VALUES ($1, $2, $3, true, $4)",
        )
        .bind(tenant_id)
        .bind(p.name)
        .bind(p.kind)
        .bind(&p.policy)
        .execute(&mut *tx)
        .await
        .map_err(AppError::from)?;
    }

    let admin_id: Uuid = sqlx::query_scalar(
        "INSERT INTO admins (tenant_id, email, display_name) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(tenant_id)
    .bind(email)
    .bind(display_name)
    .fetch_one(&mut *tx)
    .await
    .map_err(AppError::from)?;

    tx.commit().await.map_err(AppError::from)?;
    Ok((tenant_id, admin_id))
}

/// Look up an admin id + tenant id by email.
async fn find_admin(db: &sqlx::PgPool, email: &str) -> AppResult<Option<(Uuid, Uuid, String)>> {
    let row: Option<(Uuid, Uuid, String)> =
        sqlx::query_as("SELECT id, tenant_id, display_name FROM admins WHERE email = $1")
            .bind(email)
            .fetch_optional(db)
            .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RegisterStartReq {
    pub email: String,
    pub display_name: String,
}

pub async fn register_start(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterStartReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    if req.email.trim().is_empty() {
        return Err(AppError::BadRequest("email required".into()));
    }

    // A new user id for a brand-new admin; if the email already exists we reuse
    // its admin id so a second passkey attaches to the same account.
    let existing = find_admin(&st.db, &req.email).await?;
    let user_id = existing
        .as_ref()
        .map(|(id, _, _)| *id)
        .unwrap_or_else(Uuid::new_v4);

    let (ccr, reg) = st
        .webauthn
        .start_passkey_registration(user_id, &req.email, &req.display_name, None)
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;

    let key = gen_token();
    st.reg_states.write().await.insert(
        key.clone(),
        RegChallenge {
            user_id,
            email: req.email,
            display_name: req.display_name,
            reg,
        },
    );

    let jar = jar.add(temp_cookie(REG_COOKIE, key));
    Ok((jar, Json(serde_json::to_value(ccr).unwrap())))
}

#[derive(Deserialize)]
pub struct RegisterFinishReq {
    pub email: String,
    pub credential: RegisterPublicKeyCredential,
}

pub async fn register_finish(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterFinishReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let key = jar
        .get(REG_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::BadRequest("no registration in progress".into()))?;
    let challenge = st
        .reg_states
        .write()
        .await
        .remove(&key)
        .ok_or_else(|| AppError::BadRequest("registration expired".into()))?;

    if challenge.email != req.email {
        return Err(AppError::BadRequest("email mismatch".into()));
    }

    let passkey: Passkey = st
        .webauthn
        .finish_passkey_registration(&req.credential, &challenge.reg)
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;

    // Ensure the admin (and tenant + presets) exist.
    let (tenant_id, admin_id) = match find_admin(&st.db, &req.email).await? {
        Some((admin_id, tenant_id, _)) => (tenant_id, admin_id),
        None => create_tenant_with_admin(&st.db, &req.email, &challenge.display_name).await?,
    };

    let cred_id = passkey.cred_id().as_ref().to_vec();
    let passkey_json = serde_json::to_value(&passkey).unwrap();
    sqlx::query(
        "INSERT INTO webauthn_credentials (admin_id, credential_id, passkey, nickname)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(admin_id)
    .bind(&cred_id)
    .bind(&passkey_json)
    .bind("passkey")
    .execute(&st.db)
    .await?;

    // Start a session.
    let sid = gen_token();
    st.sessions.write().await.insert(
        sid.clone(),
        SessionData {
            admin_id,
            tenant_id,
        },
    );

    let jar = jar
        .remove(Cookie::from(REG_COOKIE))
        .add(session_cookie(sid));

    let admin = admin_json(&st.db, admin_id).await?;
    Ok((jar, Json(json!({ "admin": admin }))))
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LoginStartReq {
    pub email: String,
}

pub async fn login_start(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginStartReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let (admin_id, tenant_id, _) = find_admin(&st.db, &req.email)
        .await?
        .ok_or_else(|| AppError::Unauthorized("unknown account".into()))?;

    let passkeys = load_passkeys(&st.db, admin_id).await?;
    if passkeys.is_empty() {
        return Err(AppError::Unauthorized("no passkeys registered".into()));
    }

    let (rcr, auth) = st
        .webauthn
        .start_passkey_authentication(&passkeys)
        .map_err(|e| AppError::BadRequest(format!("webauthn: {e}")))?;

    let key = gen_token();
    st.auth_states.write().await.insert(
        key.clone(),
        AuthChallenge {
            admin_id,
            tenant_id,
            auth,
        },
    );

    let jar = jar.add(temp_cookie(AUTH_COOKIE, key));
    Ok((jar, Json(serde_json::to_value(rcr).unwrap())))
}

#[derive(Deserialize)]
pub struct LoginFinishReq {
    #[allow(dead_code)]
    pub email: String,
    pub credential: PublicKeyCredential,
}

pub async fn login_finish(
    State(st): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginFinishReq>,
) -> AppResult<(CookieJar, Json<Value>)> {
    let key = jar
        .get(AUTH_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::BadRequest("no login in progress".into()))?;
    let challenge = st
        .auth_states
        .write()
        .await
        .remove(&key)
        .ok_or_else(|| AppError::BadRequest("login expired".into()))?;

    let result = st
        .webauthn
        .finish_passkey_authentication(&req.credential, &challenge.auth)
        .map_err(|e| AppError::Unauthorized(format!("webauthn: {e}")))?;

    // Update the matching stored passkey's counter + last_used_at.
    if result.needs_update() {
        update_passkey_counter(&st.db, challenge.admin_id, &result).await?;
    }
    sqlx::query(
        "UPDATE webauthn_credentials SET last_used_at = now()
         WHERE admin_id = $1 AND credential_id = $2",
    )
    .bind(challenge.admin_id)
    .bind(result.cred_id().as_ref())
    .execute(&st.db)
    .await?;

    let sid = gen_token();
    st.sessions.write().await.insert(
        sid.clone(),
        SessionData {
            admin_id: challenge.admin_id,
            tenant_id: challenge.tenant_id,
        },
    );

    let jar = jar
        .remove(Cookie::from(AUTH_COOKIE))
        .add(session_cookie(sid));

    let admin = admin_json(&st.db, challenge.admin_id).await?;
    Ok((jar, Json(json!({ "admin": admin }))))
}

pub async fn logout(
    State(st): State<AppState>,
    jar: CookieJar,
) -> AppResult<(CookieJar, Json<Value>)> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        st.sessions.write().await.remove(c.value());
    }
    let jar = jar.remove(Cookie::from(SESSION_COOKIE));
    Ok((jar, Json(json!({ "ok": true }))))
}

pub async fn me(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let a = admin_json(&st.db, admin.admin_id).await?;
    let tenant: (Uuid, String) = sqlx::query_as("SELECT id, name FROM tenants WHERE id = $1")
        .bind(admin.tenant_id)
        .fetch_one(&st.db)
        .await?;
    Ok(Json(json!({
        "admin": a,
        "tenant": { "id": tenant.0, "name": tenant.1 }
    })))
}

/// The admin's registered passkeys (metadata only — never the credential itself).
pub async fn list_passkeys(
    State(st): State<AppState>,
    admin: AuthAdmin,
) -> AppResult<Json<Value>> {
    let rows: Vec<(Uuid, String, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT id, nickname, created_at, last_used_at FROM webauthn_credentials
         WHERE admin_id = $1 ORDER BY created_at",
    )
    .bind(admin.admin_id)
    .fetch_all(&st.db)
    .await?;
    let passkeys: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.0,
                "nickname": r.1,
                "created_at": r.2,
                "last_used_at": r.3,
            })
        })
        .collect();
    Ok(Json(json!({ "passkeys": passkeys })))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn admin_json(db: &sqlx::PgPool, admin_id: Uuid) -> AppResult<Value> {
    let row: (Uuid, Uuid, String, String) =
        sqlx::query_as("SELECT id, tenant_id, email, display_name FROM admins WHERE id = $1")
            .bind(admin_id)
            .fetch_one(db)
            .await?;
    Ok(json!({
        "id": row.0,
        "tenant_id": row.1,
        "email": row.2,
        "display_name": row.3,
    }))
}

async fn load_passkeys(db: &sqlx::PgPool, admin_id: Uuid) -> AppResult<Vec<Passkey>> {
    let rows: Vec<(Value,)> =
        sqlx::query_as("SELECT passkey FROM webauthn_credentials WHERE admin_id = $1")
            .bind(admin_id)
            .fetch_all(db)
            .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (v,) in rows {
        if let Ok(pk) = serde_json::from_value::<Passkey>(v) {
            out.push(pk);
        }
    }
    Ok(out)
}

async fn update_passkey_counter(
    db: &sqlx::PgPool,
    admin_id: Uuid,
    result: &webauthn_rs::prelude::AuthenticationResult,
) -> AppResult<()> {
    let rows: Vec<(Vec<u8>, Value)> = sqlx::query_as(
        "SELECT credential_id, passkey FROM webauthn_credentials WHERE admin_id = $1",
    )
    .bind(admin_id)
    .fetch_all(db)
    .await?;
    for (cred_id, v) in rows {
        if let Ok(mut pk) = serde_json::from_value::<Passkey>(v) {
            if pk.update_credential(result) == Some(true) {
                let updated = serde_json::to_value(&pk).unwrap();
                sqlx::query(
                    "UPDATE webauthn_credentials SET passkey = $1
                     WHERE admin_id = $2 AND credential_id = $3",
                )
                .bind(&updated)
                .bind(admin_id)
                .bind(&cred_id)
                .execute(db)
                .await?;
            }
        }
    }
    Ok(())
}
