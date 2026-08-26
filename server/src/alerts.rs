//! Phone alerts via a chat bot.
//!
//! Deliberately NOT web push. The operator points OpenScreenTime at a chat
//! channel they already have — a Discord/Slack incoming webhook, or a Telegram
//! bot — and OpenScreenTime *sends* short messages when something needs
//! attention (a confirmed tamper attempt, a device locking down, a new time
//! request). No webhook server: nothing listens, the bot dials out.
//!
//! Telegram grew hands (`telegram.rs`): alongside the optional legacy
//! broadcast chat (`OST_TELEGRAM_CHAT_ID`), every chat a parent has **paired**
//! gets the alerts too — and a time request arrives with inline ✅/❌ buttons
//! so "ok to a chore" is one tap on the phone. Webhook channels stay
//! send-only.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

/// Which chat channels are configured. Empty env vars count as unset (the
/// `${VAR:-}`-through-compose empty-string trap).
#[derive(Clone, Default)]
pub struct AlertConfig {
    /// A Discord/Slack-style incoming webhook URL (POST JSON, no auth).
    webhook: Option<String>,
    /// Telegram bot token. Alone it serves the *paired* chats; with
    /// `tg_env_chat` it additionally broadcasts to that fixed chat (legacy).
    tg_token: Option<String>,
    /// Legacy fixed broadcast chat id (`OST_TELEGRAM_CHAT_ID`).
    tg_env_chat: Option<String>,
}

impl AlertConfig {
    pub fn from_env() -> Self {
        Self::build(
            std::env::var("OST_ALERT_WEBHOOK").ok(),
            std::env::var("OST_TELEGRAM_BOT_TOKEN").ok(),
            std::env::var("OST_TELEGRAM_CHAT_ID").ok(),
        )
    }

    /// Build from raw values, treating empty/whitespace as unset. A bot token
    /// alone is a live channel now — paired chats give it somewhere to send.
    fn build(webhook: Option<String>, tg_token: Option<String>, tg_chat: Option<String>) -> Self {
        let clean = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
        // Only accept an http(s) webhook URL: a stray value with some other
        // scheme (or garbage) must disable the channel, not be POSTed to.
        let webhook = clean(webhook).filter(|u| match url::Url::parse(u) {
            Ok(p) => matches!(p.scheme(), "http" | "https"),
            Err(_) => {
                tracing::warn!("ignoring OST_ALERT_WEBHOOK: not a valid http(s) URL");
                false
            }
        });
        AlertConfig {
            webhook,
            tg_token: clean(tg_token),
            tg_env_chat: clean(tg_chat),
        }
    }

    pub fn enabled(&self) -> bool {
        self.webhook.is_some() || self.tg_token.is_some()
    }

    /// Send `text` to every configured channel: the webhook, the legacy env
    /// chat, and every chat paired to the event's tenant. Best-effort: a
    /// failure on one channel is logged and never propagated.
    ///
    /// Callers must pass text whose user/device-controlled parts have been run
    /// through [`sanitize`]; `allowed_mentions` additionally disables Discord
    /// `@everyone`/`@here`/role pings so a crafted name/message can't ping.
    /// `tg_keyboard` rides only on Telegram sends — webhooks stay send-only.
    async fn send(
        &self,
        client: &reqwest::Client,
        db: &sqlx::PgPool,
        tenant_id: Option<Uuid>,
        text: &str,
        tg_keyboard: Option<serde_json::Value>,
    ) {
        if let Some(url) = &self.webhook {
            // Discord uses `content`, Slack uses `text` — send both; each ignores
            // the key it doesn't know. `allowed_mentions: {parse: []}` tells
            // Discord to resolve no mentions from the content.
            let body = json!({
                "content": text,
                "text": text,
                "allowed_mentions": { "parse": [] },
            });
            if let Err(e) = client.post(url).json(&body).send().await {
                tracing::warn!("alert webhook failed: {e}");
            }
        }
        if let Some(token) = &self.tg_token {
            if let Some(chat) = &self.tg_env_chat {
                let url = format!("https://api.telegram.org/bot{token}/sendMessage");
                let mut body = json!({ "chat_id": chat, "text": text });
                if let Some(kb) = &tg_keyboard {
                    body["reply_markup"] = kb.clone();
                }
                if let Err(e) = client.post(url).json(&body).send().await {
                    tracing::warn!("alert telegram failed: {e}");
                }
            }
            if let Some(tenant) = tenant_id {
                for chat_id in crate::telegram::chats_for_tenant(db, tenant).await {
                    // Skip a paired chat that duplicates the env broadcast.
                    if self.tg_env_chat.as_deref() == Some(chat_id.to_string().as_str()) {
                        continue;
                    }
                    crate::telegram::send_message(client, token, chat_id, text, tg_keyboard.clone())
                        .await;
                }
            }
        }
    }
}

/// Spawn the alert fan-out worker if any channel is configured. It polls for new
/// critical events and new pending time requests and sends a short message for
/// each. High-water marks are in-memory and primed to "now" at startup, so a
/// restart won't replay history (best-effort — a message missed during downtime
/// is not resent; the console/tray remain the durable record).
pub fn spawn(db: sqlx::PgPool, cfg: AlertConfig) {
    if !cfg.enabled() {
        tracing::info!(
            "phone alerts: no channel configured (set OST_ALERT_WEBHOOK or OST_TELEGRAM_*)"
        );
        return;
    }
    tracing::info!(
        webhook = cfg.webhook.is_some(),
        telegram = cfg.tg_token.is_some(),
        "phone alerts enabled"
    );
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        // Prime to now so startup doesn't blast the backlog.
        let mut since_events: DateTime<Utc> = Utc::now();
        let mut since_earn: DateTime<Utc> = Utc::now();
        let mut tick = tokio::time::interval(Duration::from_secs(20));
        loop {
            tick.tick().await;
            since_events = drain_events(&db, &client, &cfg, since_events).await;
            since_earn = drain_earn(&db, &client, &cfg, since_earn).await;
        }
    });
}

/// Neutralize user/device-controlled text before it goes into an outbound chat
/// message: replace control characters (newline/CR structure-spoofing) with
/// spaces and bound the length. Device names, usernames, task labels, and event
/// `payload.message` are all attacker-influenceable (a rooted managed device can
/// push a `critical` event with an arbitrary message), so every such field is
/// sanitized before interpolation. Mass-mention pings are separately disabled
/// via `allowed_mentions` on the webhook body.
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() > 200 {
        let mut out: String = trimmed.chars().take(199).collect();
        out.push('…');
        out
    } else {
        trimmed.to_string()
    }
}

/// (id, type, payload, created_at, device name, tenant) for a critical event.
type EventRow = (
    Uuid,
    String,
    serde_json::Value,
    DateTime<Utc>,
    Option<String>,
    Uuid,
);
/// (request id, tenant, os_username, display_name, task_label, minutes,
/// created_at) for a request.
type EarnRow = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    String,
    i32,
    DateTime<Utc>,
);

/// New critical events since `since` → one message each. Returns the advanced
/// high-water mark.
async fn drain_events(
    db: &sqlx::PgPool,
    client: &reqwest::Client,
    cfg: &AlertConfig,
    since: DateTime<Utc>,
) -> DateTime<Utc> {
    let rows: Vec<EventRow> = match sqlx::query_as(
        "SELECT e.id, e.type, e.payload, e.created_at, d.name, e.tenant_id
             FROM events e LEFT JOIN devices d ON d.id = e.device_id
             WHERE e.severity = 'critical' AND e.created_at > $1
             ORDER BY e.created_at LIMIT 50",
    )
    .bind(since)
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("alert event query failed: {e}");
            return since;
        }
    };

    let mut high = since;
    for (_, etype, payload, created_at, device, tenant_id) in rows {
        high = high.max(created_at);
        let raw_msg = payload
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or(etype.as_str());
        let msg = sanitize(raw_msg);
        let dev = sanitize(device.as_deref().unwrap_or("a device"));
        cfg.send(
            client,
            db,
            Some(tenant_id),
            &format!("⚠ OpenScreenTime — {dev}: {msg}"),
            None,
        )
        .await;
    }
    high
}

/// New pending time requests since `since` → one message each.
async fn drain_earn(
    db: &sqlx::PgPool,
    client: &reqwest::Client,
    cfg: &AlertConfig,
    since: DateTime<Utc>,
) -> DateTime<Utc> {
    let rows: Vec<EarnRow> = match sqlx::query_as(
        "SELECT er.id, er.tenant_id, du.os_username, du.display_name, er.task_label,
                er.minutes, er.created_at
         FROM earn_requests er JOIN device_users du ON du.id = er.device_user_id
         WHERE er.status = 'pending' AND er.created_at > $1
         ORDER BY er.created_at LIMIT 50",
    )
    .bind(since)
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("alert earn query failed: {e}");
            return since;
        }
    };

    let mut high = since;
    for (id, tenant_id, os_username, display_name, task_label, minutes, created_at) in rows {
        high = high.max(created_at);
        let who = sanitize(
            &display_name
                .filter(|s| !s.is_empty())
                .unwrap_or(os_username),
        );
        let label = sanitize(&task_label);
        // Paired Telegram chats can answer right here — one tap, done.
        cfg.send(
            client,
            db,
            Some(tenant_id),
            &format!("⏳ {who} is asking for +{minutes} min ({label})."),
            Some(crate::telegram::earn_keyboard(id)),
        )
        .await;
    }
    high
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_env_counts_as_unset() {
        // The ${VAR:-}-through-compose empty-string trap: an empty value must
        // not enable a channel.
        let cfg = AlertConfig::build(Some("".into()), Some("  ".into()), Some("".into()));
        assert!(!cfg.enabled());
    }

    #[test]
    fn webhook_alone_enables() {
        let cfg = AlertConfig::build(Some("https://hooks.example/x".into()), None, None);
        assert!(cfg.enabled());
        assert!(cfg.tg_token.is_none());
    }

    #[test]
    fn sanitize_strips_control_chars_and_bounds_length() {
        // Newlines/CR (structure-spoofing) collapse to spaces.
        assert_eq!(
            super::sanitize("hi\nthere\r\n@everyone"),
            "hi there  @everyone"
        );
        // Length is bounded so a device can't blast a huge message.
        let long = "a".repeat(500);
        assert!(super::sanitize(&long).chars().count() <= 200);
    }

    #[test]
    fn non_http_webhook_is_rejected() {
        // A non-http(s) scheme disables the channel rather than being POSTed to.
        assert!(!AlertConfig::build(Some("file:///etc/passwd".into()), None, None).enabled());
        assert!(!AlertConfig::build(Some("not a url".into()), None, None).enabled());
        // A normal https webhook still enables.
        assert!(AlertConfig::build(Some("https://hooks.example/x".into()), None, None).enabled());
    }

    #[test]
    fn a_bot_token_alone_is_a_live_channel_now() {
        // Paired chats give a bare token somewhere to send — it enables.
        let cfg = AlertConfig::build(None, Some("tok".into()), None);
        assert!(cfg.enabled());
        assert!(cfg.tg_env_chat.is_none());
        // The legacy fixed chat still works alongside.
        let cfg = AlertConfig::build(None, Some("tok".into()), Some("123".into()));
        assert!(cfg.enabled());
        assert_eq!(cfg.tg_env_chat.as_deref(), Some("123"));
        // A chat id without a token sends nothing.
        let cfg = AlertConfig::build(None, None, Some("123".into()));
        assert!(!cfg.enabled());
    }

    #[tokio::test]
    async fn webhook_actually_posts_the_message() {
        use std::io::{Read, Write};
        use std::time::Duration;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            // Read until we've seen the body (or time out) — headers and body may
            // arrive in separate segments.
            let mut got = String::new();
            let mut buf = [0u8; 2048];
            for _ in 0..8 {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        got.push_str(&String::from_utf8_lossy(&buf[..n]));
                        if got.contains("hello-alert") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            got
        });

        let cfg = AlertConfig::build(Some(format!("http://127.0.0.1:{port}/hook")), None, None);
        // A lazy pool never connects unless used — and with no tenant and no
        // bot token, send() never touches it.
        let db = sqlx::PgPool::connect_lazy("postgres://unused@localhost/unused").unwrap();
        cfg.send(&reqwest::Client::new(), &db, None, "hello-alert", None)
            .await;

        let req = server.join().unwrap();
        assert!(req.contains("POST"), "should be a POST: {req}");
        assert!(
            req.contains("hello-alert"),
            "body should carry the message: {req}"
        );
    }
}
