//! The Telegram companion — pair once, then the bot is a hand in your pocket.
//!
//! One bot per deployment (`OST_TELEGRAM_BOT_TOKEN`). A parent pairs their
//! personal chat from the console's Security room: the console shows a short
//! code, they send `/start <code>` to the bot, done. A paired chat can:
//!
//!   - **get alerts** (tamper, lockdown, time requests — see `alerts.rs`),
//!   - **ok a chore**: the time-request alert carries inline ✅/❌ buttons
//!     that answer the request right from the phone,
//!   - **confirm it's you**: the console's confirm dialog can send one tap
//!     to the phone instead of asking for a typed code.
//!
//! The worker long-polls `getUpdates` — no webhook server, nothing listens,
//! which keeps the self-hosted story ("the agent dials out") intact.
//!
//! Chat ids, not usernames, are the identity: Telegram usernames change,
//! `chat_id` doesn't. Every callback is checked against `telegram_chats`
//! before it does anything — an unpaired chat can watch the bot all day and
//! achieve nothing.

use axum::{extract::State, Json};
use axum_extra::extract::cookie::CookieJar;
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde_json::{json, Value};
use std::sync::OnceLock;
use uuid::Uuid;

use crate::auth::hash_token;
use crate::error::{AppError, AppResult};
use crate::state::{AppState, AuthAdmin, SESSION_COOKIE};

/// How long a pairing code is redeemable.
const PAIR_CODE_MINUTES: i64 = 10;
/// How long a confirm-tap request waits for the phone.
const VERIFY_MINUTES: i64 = 2;
/// The confirm window a tap opens — same as a typed factor (stepup.rs).
const GRANT_MINUTES: i64 = 15;

pub fn bot_token() -> Option<String> {
    std::env::var("OST_TELEGRAM_BOT_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

/// The bot's @username, learned from `getMe` at startup — the pairing UI
/// builds a t.me deep link from it.
static BOT_USERNAME: OnceLock<String> = OnceLock::new();

fn api(token: &str, method: &str) -> String {
    format!("https://api.telegram.org/bot{token}/{method}")
}

async fn call(client: &reqwest::Client, token: &str, method: &str, body: Value) -> Option<Value> {
    match client.post(api(token, method)).json(&body).send().await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => v.get("result").cloned(),
            Ok(v) => {
                tracing::warn!(method, "telegram api said no: {v}");
                None
            }
            Err(_) => {
                // Never interpolate the reqwest error: its Display carries the
                // request URL, which for Telegram embeds the bot token.
                tracing::warn!(method, "telegram api response wasn't valid JSON");
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                method,
                timeout = e.is_timeout(),
                connect = e.is_connect(),
                "telegram api unreachable"
            );
            None
        }
    }
}

/// Send a plain message to one chat; used by alerts.rs too.
pub async fn send_message(
    client: &reqwest::Client,
    token: &str,
    chat_id: i64,
    text: &str,
    reply_markup: Option<Value>,
) {
    let mut body = json!({ "chat_id": chat_id, "text": text });
    if let Some(m) = reply_markup {
        body["reply_markup"] = m;
    }
    let _ = call(client, token, "sendMessage", body).await;
}

/// The ✅/❌ keyboard for a time request. Public so alerts.rs can attach it.
pub fn earn_keyboard(request_id: Uuid) -> Value {
    json!({ "inline_keyboard": [[
        { "text": "✅ Allow", "callback_data": format!("earn:{request_id}:yes") },
        { "text": "❌ No",    "callback_data": format!("earn:{request_id}:no") },
    ]]})
}

/// All paired chat ids for a tenant — the alert fan-out list.
pub async fn chats_for_tenant(db: &sqlx::PgPool, tenant_id: Uuid) -> Vec<i64> {
    sqlx::query_scalar("SELECT chat_id FROM telegram_chats WHERE tenant_id = $1")
        .bind(tenant_id)
        .fetch_all(db)
        .await
        .unwrap_or_default()
}

// ── the worker ──────────────────────────────────────────────────────────────

/// Spawn the long-poll worker if a bot token is configured.
pub fn spawn(st: AppState) {
    let Some(token) = bot_token() else {
        return;
    };
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        if let Some(me) = call(&client, &token, "getMe", json!({})).await {
            if let Some(name) = me.get("username").and_then(Value::as_str) {
                let _ = BOT_USERNAME.set(name.to_string());
                tracing::info!(bot = name, "telegram bot connected");
            }
        }
        let mut offset: i64 = 0;
        loop {
            let updates = call(
                &client,
                &token,
                "getUpdates",
                json!({
                    "offset": offset,
                    "timeout": 50,
                    "allowed_updates": ["message", "callback_query"],
                }),
            )
            .await;
            let Some(Value::Array(updates)) = updates else {
                // API hiccup — breathe, then poll again.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            };
            for u in updates {
                if let Some(id) = u.get("update_id").and_then(Value::as_i64) {
                    offset = offset.max(id + 1);
                }
                if let Some(msg) = u.get("message") {
                    handle_message(&st, &client, &token, msg).await;
                } else if let Some(cq) = u.get("callback_query") {
                    handle_callback(&st, &client, &token, cq).await;
                }
            }
        }
    });
}

/// `/start <code>` pairs the chat; anything else gets a gentle pointer.
async fn handle_message(st: &AppState, client: &reqwest::Client, token: &str, msg: &Value) {
    let Some(chat_id) = msg.pointer("/chat/id").and_then(Value::as_i64) else {
        return;
    };
    let text = msg.get("text").and_then(Value::as_str).unwrap_or("");
    let username = msg
        .pointer("/from/username")
        .and_then(Value::as_str)
        .map(str::to_string);

    let code = text
        .strip_prefix("/start")
        .map(str::trim)
        .filter(|c| !c.is_empty());

    let Some(code) = code else {
        let paired: Option<Uuid> =
            sqlx::query_scalar("SELECT admin_id FROM telegram_chats WHERE chat_id = $1")
                .bind(chat_id)
                .fetch_optional(&st.db)
                .await
                .ok()
                .flatten();
        let reply = if paired.is_some() {
            "This phone is paired. You'll get alerts here, and you can answer them with the buttons."
        } else {
            "To pair this phone: open OpenScreenTime → Settings → Security & access → Phone, and send me the code it shows (/start <code>)."
        };
        send_message(client, token, chat_id, reply, None).await;
        return;
    };

    // Redeem the code: single-use, short-lived, hashed at rest.
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "UPDATE telegram_pair_codes SET consumed_at = now()
          WHERE code_hash = $1 AND consumed_at IS NULL AND expires_at > now()
        RETURNING admin_id, tenant_id",
    )
    .bind(hash_token(&code.to_uppercase()))
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten();

    let Some((admin_id, tenant_id)) = row else {
        send_message(
            client,
            token,
            chat_id,
            "That code didn't match (they expire after 10 minutes). Get a fresh one from Settings → Security & access.",
            None,
        )
        .await;
        return;
    };

    // One row per chat; re-pairing moves the chat to the new account.
    let _ = sqlx::query(
        "INSERT INTO telegram_chats (chat_id, admin_id, tenant_id, username)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (chat_id) DO UPDATE
            SET admin_id = $2, tenant_id = $3, username = $4, created_at = now()",
    )
    .bind(chat_id)
    .bind(admin_id)
    .bind(tenant_id)
    .bind(&username)
    .execute(&st.db)
    .await;

    send_message(
        client,
        token,
        chat_id,
        "Paired ✅ — you'll get alerts here, you can ok a time request with one tap, and approve the console's confirm checks.",
        None,
    )
    .await;
}

/// Inline-button taps: answer a time request, or approve a confirm check.
async fn handle_callback(st: &AppState, client: &reqwest::Client, token: &str, cq: &Value) {
    let Some(cq_id) = cq.get("id").and_then(Value::as_str) else {
        return;
    };
    let chat_id = cq.pointer("/message/chat/id").and_then(Value::as_i64);
    let message_id = cq.pointer("/message/message_id").and_then(Value::as_i64);
    let data = cq.get("data").and_then(Value::as_str).unwrap_or("");

    let answer = |text: &str| {
        let body = json!({ "callback_query_id": cq_id, "text": text });
        let client = client.clone();
        let token = token.to_string();
        async move {
            let _ = call(&client, &token, "answerCallbackQuery", body).await;
        }
    };

    // Only a paired chat may press buttons that do things.
    let Some(chat_id) = chat_id else {
        answer("").await;
        return;
    };
    let paired: Option<(Uuid, Uuid)> =
        sqlx::query_as("SELECT admin_id, tenant_id FROM telegram_chats WHERE chat_id = $1")
            .bind(chat_id)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
    let Some((admin_id, tenant_id)) = paired else {
        answer("This phone isn't paired.").await;
        return;
    };

    let parts: Vec<&str> = data.split(':').collect();
    match parts.as_slice() {
        ["earn", id, verdict] => {
            let Ok(id) = Uuid::parse_str(id) else {
                answer("That button is broken.").await;
                return;
            };
            let approve = *verdict == "yes";
            match crate::earn::decide(
                st.clone(),
                tenant_id,
                id,
                approve,
                json!({ "admin_id": admin_id, "via": "telegram" }),
            )
            .await
            {
                Ok(_) => {
                    answer(if approve { "Allowed ✅" } else { "Said no." }).await;
                    if let Some(mid) = message_id {
                        // Freeze the message so the buttons can't be pressed twice.
                        let old = cq
                            .pointer("/message/text")
                            .and_then(Value::as_str)
                            .unwrap_or("Time request");
                        let _ = call(
                            client,
                            token,
                            "editMessageText",
                            json!({
                                "chat_id": chat_id, "message_id": mid,
                                "text": format!("{old}\n{}", if approve { "→ Allowed ✅" } else { "→ Said no ❌" }),
                            }),
                        )
                        .await;
                    }
                }
                Err(AppError::Conflict(_)) => answer("Already answered.").await,
                Err(_) => answer("That didn't work — answer it in the console.").await,
            }
        }
        ["verify", id, verdict] => {
            let Ok(id) = Uuid::parse_str(id) else {
                answer("That button is broken.").await;
                return;
            };
            let ok = *verdict == "ok";
            // Decide once; the tap must come from the phone of the person who asked.
            let session: Option<Uuid> = sqlx::query_scalar(
                "UPDATE telegram_verifications SET decided_at = now(), approved = $3
                  WHERE id = $1 AND admin_id = $2 AND decided_at IS NULL AND expires_at > now()
                RETURNING session_id",
            )
            .bind(id)
            .bind(admin_id)
            .bind(ok)
            .fetch_optional(&st.db)
            .await
            .ok()
            .flatten();
            match (session, ok) {
                (Some(session_id), true) => {
                    // Open the asking session's confirm window — same grant a
                    // typed factor earns (no token rotation: there is no HTTP
                    // response here to carry a fresh cookie).
                    let until = Utc::now() + Duration::minutes(GRANT_MINUTES);
                    let _ = sqlx::query(
                        "UPDATE admin_sessions
                            SET stepup_until = $2, stepup_extended = false, trusted = true
                          WHERE id = $1",
                    )
                    .bind(session_id)
                    .bind(until)
                    .execute(&st.db)
                    .await;
                    answer("Confirmed ✅").await;
                }
                (Some(_), false) => answer("Blocked. That session stays unconfirmed.").await,
                (None, _) => answer("That check expired — ask again from the console.").await,
            }
            if let Some(mid) = message_id {
                let _ = call(
                    client,
                    token,
                    "editMessageReplyMarkup",
                    json!({ "chat_id": chat_id, "message_id": mid, "reply_markup": { "inline_keyboard": [] } }),
                )
                .await;
            }
        }
        _ => answer("").await,
    }
}

// ── console API ─────────────────────────────────────────────────────────────

/// `GET /api/me/telegram` → pairing state for the Security room.
pub async fn status(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    let row: Option<(i64, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT chat_id, username, created_at FROM telegram_chats WHERE admin_id = $1
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(admin.admin_id)
    .fetch_optional(&st.db)
    .await?;
    Ok(Json(json!({
        "configured": bot_token().is_some(),
        "bot": BOT_USERNAME.get(),
        "paired": row.is_some(),
        "username": row.as_ref().and_then(|r| r.1.clone()),
        "paired_at": row.as_ref().map(|r| r.2),
    })))
}

/// `POST /api/me/telegram/pair` → a short single-use code and a deep link.
pub async fn pair_start(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    if bot_token().is_none() {
        return Err(AppError::BadRequest(
            "no Telegram bot is configured on this server (OST_TELEGRAM_BOT_TOKEN)".into(),
        ));
    }
    // 8 chars from an unambiguous alphabet — easy to type on a phone. The
    // rng lives in its own block: ThreadRng is !Send and must drop before
    // the first await or the handler future stops being one.
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let code: String = {
        let mut rng = rand::thread_rng();
        (0..8)
            .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
            .collect()
    };

    sqlx::query(
        "INSERT INTO telegram_pair_codes (admin_id, tenant_id, code_hash, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(mins => $4))",
    )
    .bind(admin.admin_id)
    .bind(admin.tenant_id)
    .bind(hash_token(&code))
    .bind(PAIR_CODE_MINUTES as i32)
    .execute(&st.db)
    .await?;
    let _ = sqlx::query("DELETE FROM telegram_pair_codes WHERE expires_at < now()")
        .execute(&st.db)
        .await;

    Ok(Json(json!({
        "code": code,
        "bot": BOT_USERNAME.get(),
        "deep_link": BOT_USERNAME.get().map(|b| format!("https://t.me/{b}?start={code}")),
        "expires_in_minutes": PAIR_CODE_MINUTES,
    })))
}

/// `DELETE /api/me/telegram` → unpair every chat of this account.
pub async fn unpair(State(st): State<AppState>, admin: AuthAdmin) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM telegram_chats WHERE admin_id = $1")
        .bind(admin.admin_id)
        .execute(&st.db)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// `POST /api/auth/stepup/telegram/start` — send one tap to the phone. The
/// console then polls `GET /api/auth/stepup` until the window opens.
pub async fn verify_start(
    State(st): State<AppState>,
    admin: AuthAdmin,
    jar: CookieJar,
) -> AppResult<Json<Value>> {
    let Some(token) = bot_token() else {
        return Err(AppError::BadRequest("no Telegram bot configured".into()));
    };
    let chat: Option<i64> = sqlx::query_scalar(
        "SELECT chat_id FROM telegram_chats WHERE admin_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(admin.admin_id)
    .fetch_optional(&st.db)
    .await?;
    let Some(chat_id) = chat else {
        return Err(AppError::BadRequest(
            "no phone is paired — pair Telegram in Settings first".into(),
        ));
    };

    // Bind the check to the session that is asking.
    let cookie = jar
        .get(SESSION_COOKIE)
        .ok_or_else(|| AppError::Unauthorized("no session".into()))?;
    let session_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM admin_sessions
         WHERE (token_hash = $1 OR (prev_token_hash = $1 AND prev_valid_until > now()))
           AND expires_at > now()",
    )
    .bind(hash_token(cookie.value()))
    .fetch_optional(&st.db)
    .await?;
    let session_id = session_id.ok_or_else(|| AppError::Unauthorized("no session".into()))?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO telegram_verifications (admin_id, session_id, expires_at)
         VALUES ($1, $2, now() + make_interval(mins => $3))
        RETURNING id",
    )
    .bind(admin.admin_id)
    .bind(session_id)
    .bind(VERIFY_MINUTES as i32)
    .fetch_one(&st.db)
    .await?;

    let keyboard = json!({ "inline_keyboard": [[
        { "text": "✅ It's me",  "callback_data": format!("verify:{id}:ok") },
        { "text": "❌ Block it", "callback_data": format!("verify:{id}:no") },
    ]]});
    let client = reqwest::Client::new();
    send_message(
        &client,
        &token,
        chat_id,
        "Someone at your console wants to touch the household's keys. Was that you?",
        Some(keyboard),
    )
    .await;

    Ok(Json(json!({ "ok": true, "expires_in_seconds": VERIFY_MINUTES * 60 })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earn_keyboard_carries_the_request_id() {
        let id = Uuid::nil();
        let kb = earn_keyboard(id);
        let row = kb.pointer("/inline_keyboard/0").unwrap();
        assert!(row[0]["callback_data"]
            .as_str()
            .unwrap()
            .ends_with(":yes"));
        assert!(row[1]["callback_data"].as_str().unwrap().contains(&id.to_string()));
    }
}
