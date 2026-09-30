//! Operator-level health: what a family never sees in the console but what
//! decides whether the server keeps working unattended — the database, the
//! nightly backup, server updates, and devices that have gone quiet.
//!
//! Every problem is an *incident* with a stable key. It is announced once when
//! it opens (and, for the server's own problems, once when it clears), never
//! once per check — and because open incidents live in `ops_incidents`, not
//! again after a restart either. The database itself is the exception: while
//! it is down there is nowhere to write, so that one is tracked in memory.
//!
//! Backups and updates are done by the host scripts (`deploy/backup.sh`,
//! `deploy/update.sh`), which record each run in `ops_log`; this module only
//! reads that log.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::alerts::{sanitize, AlertConfig};

/// How often the system checks run.
const CHECK_EVERY: Duration = Duration::from_secs(60);
/// Consecutive failed DB checks before it counts as an outage (~3 minutes):
/// a restarting Postgres or a blip is not worth a message.
const DB_DOWN_CHECKS: u32 = 3;
/// No successful backup for this long is an incident.
const BACKUP_MAX_AGE_HOURS: i32 = 48;
/// A device silent for this long is announced once…
const DARK_AFTER_HOURS: i32 = 24;
/// …unless it has been gone for so long that it is plainly retired (the
/// console flags those as gone dark); announcing them would only be a burst of
/// noise the first time this runs.
const DARK_FORGET_DAYS: i32 = 14;

/// Is the database answering? One cheap round trip, bounded so a wedged pool
/// can't hang the caller.
pub async fn db_ok(db: &PgPool) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(2), sqlx::query("SELECT 1").execute(db)).await,
        Ok(Ok(_))
    )
}

/// [`db_ok`] answered from a two-second cache: `/health` is public and
/// unauthenticated, and must not become a way to hammer Postgres.
pub async fn db_ok_cached(db: &PgPool) -> bool {
    static LAST: tokio::sync::Mutex<Option<(Instant, bool)>> = tokio::sync::Mutex::const_new(None);
    let mut last = LAST.lock().await;
    if let Some((at, ok)) = *last {
        if at.elapsed() < Duration::from_secs(2) {
            return ok;
        }
    }
    let ok = db_ok(db).await;
    *last = Some((Instant::now(), ok));
    ok
}

/// Start the system health loop. It always runs (its findings are logged);
/// messages go out only when an alert channel is configured.
pub fn spawn(db: PgPool, cfg: AlertConfig) {
    crate::supervise::spawn("system-health", move || {
        let (db, cfg) = (db.clone(), cfg.clone());
        async move {
            let client = crate::alerts::http_client();
            let mut watch = Watch::default();
            let mut tick = tokio::time::interval(CHECK_EVERY);
            loop {
                tick.tick().await;
                for notice in watch.check(&db).await {
                    tracing::warn!(text = %notice.text, "system notice");
                    if !cfg.enabled() {
                        continue;
                    }
                    match notice.tenant {
                        Some(t) => cfg.send(&client, &db, Some(t), &notice.text, None).await,
                        None => {
                            cfg.send_operator(&client, &watch.owner_chats, &notice.text)
                                .await
                        }
                    }
                }
            }
        }
    });
}

/// A message to send: operator-level (`tenant: None`) or to one household.
struct Notice {
    tenant: Option<Uuid>,
    text: String,
}

/// Something wrong right now.
#[derive(Debug, Clone, PartialEq)]
struct Incident {
    key: String,
    tenant: Option<Uuid>,
    message: String,
}

impl Incident {
    fn server(key: &str, message: String) -> Self {
        Incident {
            key: key.into(),
            tenant: None,
            message,
        }
    }
}

#[derive(Default)]
struct Watch {
    db_failures: u32,
    db_alerted: bool,
    /// Paired Telegram chats of household owners, refreshed while the DB is up
    /// so a "database is down" message still reaches them.
    owner_chats: Vec<i64>,
}

impl Watch {
    async fn check(&mut self, db: &PgPool) -> Vec<Notice> {
        if !db_ok(db).await {
            self.db_failures += 1;
            if self.db_failures >= DB_DOWN_CHECKS && !self.db_alerted {
                self.db_alerted = true;
                return vec![Notice {
                    tenant: None,
                    text: "⚠ OpenScreenTime server: the database has not answered for a few \
                           minutes. Devices keep enforcing their last rules, but the console \
                           and time requests are down."
                        .into(),
                }];
            }
            return Vec::new();
        }
        let mut out = Vec::new();
        self.db_failures = 0;
        if std::mem::take(&mut self.db_alerted) {
            out.push(Notice {
                tenant: None,
                text: "✓ OpenScreenTime server: the database is answering again.".into(),
            });
        }
        if let Ok(chats) = sqlx::query_scalar(
            "SELECT tc.chat_id FROM telegram_chats tc
               JOIN admins a ON a.id = tc.admin_id
              WHERE a.role = 'owner'",
        )
        .fetch_all(db)
        .await
        {
            self.owner_chats = chats;
        }
        match reconcile(db).await {
            Ok(mut notices) => out.append(&mut notices),
            Err(e) => tracing::warn!(error = %e, "system health check failed"),
        }
        out
    }
}

/// Everything wrong right now.
async fn current_incidents(db: &PgPool) -> sqlx::Result<Vec<Incident>> {
    let mut out = Vec::new();

    // Backups: the latest attempt failed, or none has succeeded for two days
    // (counting from when this log began, so a fresh install gets its grace).
    let last_backup: Option<(bool, String)> = sqlx::query_as(
        "SELECT ok, detail FROM ops_log WHERE kind = 'backup'
          ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    let backup_fresh: bool = sqlx::query_scalar(
        "SELECT coalesce(max(created_at) > now() - make_interval(hours => $1), false)
           FROM ops_log WHERE (kind = 'backup' AND ok) OR kind = 'install'",
    )
    .bind(BACKUP_MAX_AGE_HOURS)
    .fetch_one(db)
    .await?;
    if let Some((false, detail)) = &last_backup {
        out.push(Incident::server(
            "backup",
            format!(
                "⚠ OpenScreenTime server: the last database backup failed ({}). \
                 See `journalctl -u openscreentime-backup` on the server.",
                sanitize(detail)
            ),
        ));
    } else if !backup_fresh {
        out.push(Incident::server(
            "backup",
            "⚠ OpenScreenTime server: no database backup in the last two days. Is the \
             backup timer running? (deploy/backup.sh, docs/OPERATIONS.md)"
                .into(),
        ));
    }

    // Updates: the latest run failed (and was rolled back, or never started).
    let last_update: Option<(bool, String)> = sqlx::query_as(
        "SELECT ok, detail FROM ops_log WHERE kind = 'update'
          ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    if let Some((false, detail)) = &last_update {
        out.push(Incident::server(
            "update",
            format!("⚠ OpenScreenTime server: {}", sanitize(detail)),
        ));
    }

    // Devices that have gone quiet — told to their own household.
    let dark: Vec<(Uuid, Uuid, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, tenant_id, name, last_seen FROM devices
          WHERE status <> 'pending'
            AND last_seen < now() - make_interval(hours => $1)
            AND last_seen > now() - make_interval(days => $2)",
    )
    .bind(DARK_AFTER_HOURS)
    .bind(DARK_FORGET_DAYS)
    .fetch_all(db)
    .await?;
    for (id, tenant, name, last_seen) in dark {
        out.push(Incident {
            key: format!("dark:{id}"),
            tenant: Some(tenant),
            message: format!(
                "📵 {} has not been in touch since {}. If it is switched off or away, \
                 nothing to do — otherwise the OpenScreenTime service on it may have stopped.",
                sanitize(&name),
                last_seen.format("%a %-d %b, %H:%M UTC")
            ),
        });
    }
    Ok(out)
}

/// Open what's newly wrong, close what's fixed, and say so — once each.
async fn reconcile(db: &PgPool) -> sqlx::Result<Vec<Notice>> {
    let now = current_incidents(db).await?;
    let open: Vec<String> = sqlx::query_scalar("SELECT key FROM ops_incidents")
        .fetch_all(db)
        .await?;
    let (opened, closed) = diff(&open, &now);

    let mut out = Vec::new();
    for inc in opened {
        let inserted = sqlx::query(
            "INSERT INTO ops_incidents (key, tenant_id, message) VALUES ($1, $2, $3)
             ON CONFLICT (key) DO NOTHING",
        )
        .bind(&inc.key)
        .bind(inc.tenant)
        .bind(&inc.message)
        .execute(db)
        .await?
        .rows_affected();
        if inserted == 1 {
            out.push(Notice {
                tenant: inc.tenant,
                text: inc.message.clone(),
            });
        }
    }
    for key in closed {
        let deleted = sqlx::query("DELETE FROM ops_incidents WHERE key = $1")
            .bind(key)
            .execute(db)
            .await?
            .rows_affected();
        if deleted == 1 {
            if let Some(text) = resolved_text(key) {
                out.push(Notice {
                    tenant: None,
                    text: text.into(),
                });
            }
        }
    }
    Ok(out)
}

/// Incidents in `now` that aren't open yet, and open keys no longer in `now`.
fn diff<'a>(open: &'a [String], now: &'a [Incident]) -> (Vec<&'a Incident>, Vec<&'a String>) {
    let open_set: HashSet<&str> = open.iter().map(String::as_str).collect();
    let now_set: HashSet<&str> = now.iter().map(|i| i.key.as_str()).collect();
    (
        now.iter()
            .filter(|i| !open_set.contains(i.key.as_str()))
            .collect(),
        open.iter()
            .filter(|k| !now_set.contains(k.as_str()))
            .collect(),
    )
}

/// The all-clear for the server's own problems. A device coming back is its
/// own announcement (the console shows it online); no message for that.
fn resolved_text(key: &str) -> Option<&'static str> {
    match key {
        "backup" => Some("✓ OpenScreenTime server: backups are working again."),
        "update" => Some("✓ OpenScreenTime server: updates are working again."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inc(key: &str) -> Incident {
        Incident::server(key, format!("msg {key}"))
    }

    #[test]
    fn each_incident_opens_and_closes_once() {
        let open = vec!["backup".to_string(), "dark:x".to_string()];
        let now = vec![inc("backup"), inc("update")];
        let (opened, closed) = diff(&open, &now);
        // Still-open incidents are not announced again.
        assert_eq!(
            opened.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(),
            ["update"]
        );
        assert_eq!(closed, [&"dark:x".to_string()]);
    }

    #[test]
    fn only_server_problems_get_an_all_clear() {
        assert!(resolved_text("backup").is_some());
        assert!(resolved_text("update").is_some());
        assert!(resolved_text("dark:3f1e").is_none());
    }
}
