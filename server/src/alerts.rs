//! One-way phone alerts via a chat bot.
//!
//! Deliberately NOT web push. The operator points Sentinel at a chat channel
//! they already have — a Discord/Slack incoming webhook, or a Telegram bot — and
//! Sentinel *sends* short messages when something needs attention (a confirmed
//! tamper attempt, a device locking down, a new time request). It never reads
//! anything back: no webhook server, no bot polling, nobody writes to the bot.
//! Setup is a URL (or a token + chat id) in `.env`, and that's it.
//!
//! Config is global to the deployment (env vars). For the common single-family
//! install that's exactly right; a multi-tenant host gets one operator channel
//! for all tenants (documented — revisit with a per-tenant table if needed).

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
    /// Telegram bot token + chat id.
    telegram: Option<(String, String)>,
}

impl AlertConfig {
    pub fn from_env() -> Self {
        Self::build(
            std::env::var("SENTINEL_ALERT_WEBHOOK").ok(),
            std::env::var("SENTINEL_TELEGRAM_BOT_TOKEN").ok(),
            std::env::var("SENTINEL_TELEGRAM_CHAT_ID").ok(),
        )
    }

    /// Build from raw values, treating empty/whitespace as unset and requiring
    /// BOTH halves of the Telegram pair. Pure, so it's unit-testable.
    fn build(webhook: Option<String>, tg_token: Option<String>, tg_chat: Option<String>) -> Self {
        let clean = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
        let telegram = match (clean(tg_token), clean(tg_chat)) {
            (Some(t), Some(c)) => Some((t, c)),
            _ => None,
        };
        AlertConfig {
            webhook: clean(webhook),
            telegram,
        }
    }

    pub fn enabled(&self) -> bool {
        self.webhook.is_some() || self.telegram.is_some()
    }

    /// Send `text` to every configured channel. Best-effort: a failure on one
    /// channel is logged and never propagated (alerts must not break anything).
    async fn send(&self, client: &reqwest::Client, text: &str) {
        if let Some(url) = &self.webhook {
            // Discord uses `content`, Slack uses `text` — send both; each ignores
            // the key it doesn't know.
            let body = json!({ "content": text, "text": text });
            if let Err(e) = client.post(url).json(&body).send().await {
                tracing::warn!("alert webhook failed: {e}");
            }
        }
        if let Some((token, chat_id)) = &self.telegram {
            let url = format!("https://api.telegram.org/bot{token}/sendMessage");
            let body = json!({ "chat_id": chat_id, "text": text });
            if let Err(e) = client.post(url).json(&body).send().await {
                tracing::warn!("alert telegram failed: {e}");
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
        tracing::info!("phone alerts: no channel configured (set SENTINEL_ALERT_WEBHOOK or SENTINEL_TELEGRAM_*)");
        return;
    }
    tracing::info!(
        webhook = cfg.webhook.is_some(),
        telegram = cfg.telegram.is_some(),
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

/// (id, type, payload, created_at, device name) for a critical event.
type EventRow = (Uuid, String, serde_json::Value, DateTime<Utc>, Option<String>);
/// (os_username, display_name, task_label, minutes, created_at) for a request.
type EarnRow = (String, Option<String>, String, i32, DateTime<Utc>);

/// New critical events since `since` → one message each. Returns the advanced
/// high-water mark.
async fn drain_events(
    db: &sqlx::PgPool,
    client: &reqwest::Client,
    cfg: &AlertConfig,
    since: DateTime<Utc>,
) -> DateTime<Utc> {
    let rows: Vec<EventRow> = match sqlx::query_as(
            "SELECT e.id, e.type, e.payload, e.created_at, d.name
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
    for (_, etype, payload, created_at, device) in rows {
        high = high.max(created_at);
        let msg = payload
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or(&etype);
        let dev = device.as_deref().unwrap_or("a device");
        cfg.send(
            client,
            &format!("⚠ Sentinel — {dev}: {msg}"),
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
        "SELECT du.os_username, du.display_name, er.task_label, er.minutes, er.created_at
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
    for (os_username, display_name, task_label, minutes, created_at) in rows {
        high = high.max(created_at);
        let who = display_name.filter(|s| !s.is_empty()).unwrap_or(os_username);
        cfg.send(
            client,
            &format!("⏳ {who} is asking for +{minutes} min ({task_label}). Approve it in Sentinel."),
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
        assert!(cfg.telegram.is_none());
    }

    #[test]
    fn telegram_needs_both_halves() {
        // Only a token, no chat id → not configured.
        let cfg = AlertConfig::build(None, Some("tok".into()), None);
        assert!(!cfg.enabled());
        // Both present → configured.
        let cfg = AlertConfig::build(None, Some("tok".into()), Some("123".into()));
        assert!(cfg.enabled());
        assert_eq!(cfg.telegram, Some(("tok".into(), "123".into())));
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
        cfg.send(&reqwest::Client::new(), "hello-alert").await;

        let req = server.join().unwrap();
        assert!(req.contains("POST"), "should be a POST: {req}");
        assert!(req.contains("hello-alert"), "body should carry the message: {req}");
    }
}
