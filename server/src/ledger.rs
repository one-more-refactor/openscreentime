//! The screen-time ledger: one row per (device user, **device-local** day).
//!
//! The agent enforces a person's day on the device's local calendar; the
//! ledger used to file it under Postgres' `CURRENT_DATE` (UTC), so the console
//! lagged local midnight by an hour or two and — worse — every legitimate
//! local-midnight reset looked like a wiped ledger, firing a critical "clock
//! games" alert about every 30 s per child until UTC midnight.
//!
//! Now:
//! * usage is filed under the day the agent reports (sanity-checked against
//!   the device's UTC offset);
//! * the regression ("evasion") check compares within that same day only, is
//!   skipped for agents too old to say which day they mean, and fires once per
//!   device user per day;
//! * **a daily limit is one budget per person across all their computers**:
//!   each report is answered with what the same person used (and was granted)
//!   on their other devices that day, and the agent enforces its own use plus
//!   that;
//! * "today" on the console is each device's local today
//!   ([`DEVICE_TODAY_SQL`]).

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use openscreentime_policy::{rules, Policy};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::events;

/// SQL for "today" on a device's own calendar. Needs the `devices` row
/// aliased `d`. Devices that never reported an offset fall back to UTC.
pub const DEVICE_TODAY_SQL: &str = "((now() AT TIME ZONE 'UTC') \
     + make_interval(secs => COALESCE(d.utc_offset_secs, 0)))::date";

/// A reported daily total may dip this far below the recorded total without
/// being flagged — absorbs the minute-granularity of older agents. A larger
/// drop within the same device day is a real regression (a wiped or
/// rolled-back client ledger) worth an `evasion` event.
const USAGE_REGRESSION_SECS: i64 = 300;

/// Real UTC offsets span −12 h … +14 h.
const MAX_OFFSET_SECS: i32 = 14 * 3600;

/// Per-user usage reported with each heartbeat (HTTP body or WS frame).
#[derive(Debug, Clone, Deserialize)]
pub struct UsageEntry {
    pub os_username: String,
    /// Whole minutes — all an agent before 0.7 sends.
    #[serde(default)]
    pub used_minutes_today: i64,
    /// Seconds (0.7+).
    #[serde(default)]
    pub used_seconds_today: Option<i64>,
    /// The device-local day these numbers belong to (0.7+).
    #[serde(default)]
    pub day: Option<NaiveDate>,
    /// The device's UTC offset in seconds (0.7+).
    #[serde(default)]
    pub utc_offset_secs: Option<i32>,
}

impl UsageEntry {
    fn seconds(&self) -> i64 {
        self.used_seconds_today
            .unwrap_or(self.used_minutes_today.saturating_mul(60))
            .clamp(0, 24 * 3600)
    }
}

/// The device's local date at `now`, from its UTC offset.
pub fn local_today(offset_secs: Option<i32>, now: DateTime<Utc>) -> NaiveDate {
    let off = offset_secs
        .unwrap_or(0)
        .clamp(-MAX_OFFSET_SECS, MAX_OFFSET_SECS);
    (now + Duration::seconds(i64::from(off))).date_naive()
}

/// The day a report is filed under: the agent's own day when it is a
/// plausible calendar date for *some* time zone right now (within a day of
/// UTC's), else the device-local today.
pub fn filing_day(
    reported: Option<NaiveDate>,
    offset_secs: Option<i32>,
    now: DateTime<Utc>,
) -> NaiveDate {
    match reported {
        Some(d) if (d - now.date_naive()).num_days().abs() <= 1 => d,
        _ => local_today(offset_secs, now),
    }
}

/// Should this report raise a usage-regression event? Only when the agent
/// says which day it means (an old agent can't be told apart from a
/// legitimate local-midnight reset) and the total dropped by more than the
/// slack *within that day*.
pub fn is_regression(reported_day_known: bool, new_secs: i64, prev_secs: i64) -> bool {
    reported_day_known && new_secs + USAGE_REGRESSION_SECS < prev_secs
}

/// What the agent is told back for one of its users: the person's use and
/// grants on their other logins, plus the grants the server has on record
/// for *this* login (so a device that lost its ledger still knows them; the
/// agent takes the larger of its own count and this, never the sum).
pub fn person_day_json(
    os_username: &str,
    day: NaiveDate,
    used_else: i64,
    earned_else: i64,
    earned_here: i64,
) -> Value {
    json!({
        "os_username": os_username,
        "day": day,
        "used_elsewhere_secs": used_else.max(0),
        "earned_elsewhere_secs": earned_else.max(0),
        "earned_here_secs": earned_here.max(0),
    })
}

/// File a device's usage report and answer with each user's day elsewhere.
///
/// Shared by the HTTP heartbeat and the WS `heartbeat` frame so both report
/// identically. The server-side anti-cheat hook stays: within a day the
/// client ledger only moves forward, so a report of *less* than recorded
/// means the counter was reset behind our back. `GREATEST` keeps the total
/// from going down; the event makes it visible — once per user per day.
pub async fn upsert_usage(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    usage: &[UsageEntry],
) -> Result<Vec<Value>, sqlx::Error> {
    let now = Utc::now();
    if let Some(off) = usage.iter().find_map(|u| u.utc_offset_secs) {
        sqlx::query("UPDATE devices SET utc_offset_secs = $2 WHERE id = $1")
            .bind(device_id)
            .bind(off.clamp(-MAX_OFFSET_SECS, MAX_OFFSET_SECS))
            .execute(db)
            .await?;
    }
    let mut answer = Vec::with_capacity(usage.len());
    for u in usage {
        let new_seconds = u.seconds();
        let day = filing_day(u.day, u.utc_offset_secs, now);

        let du: Option<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT id, account_id FROM device_users WHERE device_id = $1 AND os_username = $2",
        )
        .bind(device_id)
        .bind(&u.os_username)
        .fetch_optional(db)
        .await?;
        let Some((device_user_id, account_id)) = du else {
            continue;
        };

        // Read the recorded total for that day BEFORE the GREATEST clamp hides a drop.
        let row: Option<(i32, i32)> = sqlx::query_as(
            "SELECT used_seconds, earned_seconds FROM screen_time_ledger
              WHERE device_user_id = $1 AND day = $2",
        )
        .bind(device_user_id)
        .bind(day)
        .fetch_optional(db)
        .await?;
        let earned_here = row.map(|r| i64::from(r.1)).unwrap_or(0);
        if let Some((prev, _)) = row {
            if is_regression(u.day.is_some(), new_seconds, i64::from(prev)) {
                report_regression(
                    db,
                    tenant_id,
                    device_id,
                    device_user_id,
                    u,
                    day,
                    new_seconds,
                    prev,
                )
                .await;
            }
        }

        sqlx::query(
            "INSERT INTO screen_time_ledger (device_user_id, day, used_seconds)
             VALUES ($1, $2, $3)
             ON CONFLICT (device_user_id, day)
             DO UPDATE SET used_seconds = GREATEST(screen_time_ledger.used_seconds, EXCLUDED.used_seconds)",
        )
        .bind(device_user_id)
        .bind(day)
        .bind(new_seconds as i32)
        .execute(db)
        .await?;

        let (used_else, earned_else) = match account_id {
            Some(account) => elsewhere(db, tenant_id, account, device_user_id, day).await?,
            None => (0, 0),
        };
        answer.push(person_day_json(
            &u.os_username,
            day,
            used_else,
            earned_else,
            earned_here,
        ));
    }
    Ok(answer)
}

/// The person's use and grants on `day` on every login except `except`.
pub async fn elsewhere(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    account_id: Uuid,
    except: Uuid,
    day: NaiveDate,
) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT COALESCE(SUM(l.used_seconds), 0)::bigint, COALESCE(SUM(l.earned_seconds), 0)::bigint
           FROM device_users du
           JOIN devices d ON d.id = du.device_id AND d.tenant_id = $1
           JOIN screen_time_ledger l ON l.device_user_id = du.id AND l.day = $4
          WHERE du.account_id = $2 AND du.id <> $3",
    )
    .bind(tenant_id)
    .bind(account_id)
    .bind(except)
    .bind(day)
    .fetch_one(db)
    .await
}

/// One usage-regression event per device user per day, not one per heartbeat.
#[allow(clippy::too_many_arguments)]
async fn report_regression(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    device_user_id: Uuid,
    u: &UsageEntry,
    day: NaiveDate,
    new_seconds: i64,
    prev: i32,
) {
    let already: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM events
          WHERE device_user_id = $1 AND type = 'evasion'
            AND payload->>'kind' = 'usage_regression' AND payload->>'day' = $2
          LIMIT 1",
    )
    .bind(device_user_id)
    .bind(day.to_string())
    .fetch_optional(db)
    .await
    .unwrap_or(None);
    if already.is_some() {
        return;
    }
    // Best-effort audit; a failed insert must not drop the heartbeat.
    let _ = events::insert(
        db,
        tenant_id,
        Some(device_id),
        Some(device_user_id),
        "evasion",
        // Critical: this is the one evasion signal the server derives
        // independently of the device's honesty. It can no longer fire at a
        // legitimate midnight reset (same-day comparison), and fires once.
        "critical",
        json!({
            "kind": "usage_regression",
            "os_username": u.os_username,
            "day": day,
            "reported_seconds": new_seconds,
            "ledger_seconds": prev,
            "message": "reported usage dropped below the recorded total for the same day; \
                        counter clamped (possible client-ledger reset)",
        }),
    )
    .await;
}

/// The rules' view of a person's day for the console, on a device's clock
/// (the same function the agent enforces with). A local override at the
/// device (a parent code) isn't known here — this is "by the rules".
pub fn rules_json(
    policy: &Policy,
    used_secs: i64,
    earned_secs: i64,
    offset_secs: Option<i32>,
    now: DateTime<Utc>,
) -> Value {
    let tz = FixedOffset::east_opt(
        offset_secs
            .unwrap_or(0)
            .clamp(-MAX_OFFSET_SECS, MAX_OFFSET_SECS),
    )
    .unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    let v = rules::evaluate(
        &policy.screen_time,
        &now.with_timezone(&tz),
        rules::Day {
            used_secs: used_secs.max(0) as u64,
            earned_secs: earned_secs.max(0) as u64,
        },
        None,
        false,
    );
    json!({
        "allowed": v.allowed,
        "reason": v.reason.map(|r| r.id()),
        "minutes_left": v.minutes_left,
        "stop_at": v.stop_at,
        "resume_at": v.resume_at,
    })
}

/// The device-local date for a given device, at `now` (for writes that the
/// server originates, like a parent's grant).
pub async fn device_local_day(
    db: &sqlx::PgPool,
    device_id: Uuid,
) -> Result<NaiveDate, sqlx::Error> {
    let off: Option<i32> = sqlx::query_scalar("SELECT utc_offset_secs FROM devices WHERE id = $1")
        .bind(device_id)
        .fetch_optional(db)
        .await?
        .flatten();
    Ok(local_today(off, Utc::now()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(d: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, d, h, m, 0).unwrap()
    }
    fn date(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, d).unwrap()
    }

    #[test]
    fn a_device_in_berlin_files_after_midnight_under_its_own_new_day() {
        // 00:30 CEST on the 25th is 22:30 UTC on the 24th.
        let now = utc(24, 22, 30);
        assert_eq!(local_today(Some(2 * 3600), now), date(25));
        assert_eq!(filing_day(Some(date(25)), Some(2 * 3600), now), date(25));
        // UTC− zones: 20:00 in New York on the 24th is 00:00 UTC on the 25th.
        assert_eq!(
            filing_day(Some(date(24)), Some(-4 * 3600), utc(25, 0, 0)),
            date(24)
        );
    }

    #[test]
    fn an_implausible_day_falls_back_to_the_device_clock() {
        let now = utc(24, 12, 0);
        assert_eq!(filing_day(Some(date(20)), Some(3600), now), date(24));
        assert_eq!(filing_day(None, Some(3600), now), date(24));
        assert_eq!(filing_day(None, None, now), date(24));
    }

    #[test]
    fn a_midnight_reset_is_not_a_regression_but_a_same_day_drop_is() {
        // At local midnight the agent starts a new day at 0 — filed under the
        // new day, so there is no previous row to compare with. The same-day
        // check only sees real drops:
        assert!(is_regression(true, 0, 3600));
        assert!(!is_regression(true, 3500, 3600), "within the slack");
        // An old agent that can't say which day it means is never accused.
        assert!(!is_regression(false, 0, 3600));
    }

    #[test]
    fn seconds_prefer_the_precise_field() {
        let e: UsageEntry = serde_json::from_value(json!({
            "os_username": "kid", "used_minutes_today": 10, "used_seconds_today": 659
        }))
        .unwrap();
        assert_eq!(e.seconds(), 659);
        let old: UsageEntry =
            serde_json::from_value(json!({ "os_username": "kid", "used_minutes_today": 10 }))
                .unwrap();
        assert_eq!(old.seconds(), 600);
        assert!(old.day.is_none());
    }

    #[test]
    fn the_console_uses_the_same_rules_as_the_device() {
        let p: Policy = serde_json::from_value(json!({
            "screen_time": { "enabled": true, "daily_limit_minutes": 60,
                "schedule": [], "bedtime": { "start": "21:00", "end": "07:00" } }
        }))
        .unwrap();
        // 20:50 local (UTC+2) with 20 minutes used: bedtime in 10, not "40 left".
        let v = rules_json(&p, 20 * 60, 0, Some(7200), utc(24, 18, 50));
        assert_eq!(v["reason"], "bedtime");
        assert_eq!(v["minutes_left"], 10);
        assert_eq!(v["allowed"], true);
    }
}

/// Database-backed tests, on the same harness as every other one
/// (`tests_auth::Env`): a throwaway database from `OST_TEST_DATABASE_URL`,
/// else `DATABASE_URL`, dropped at the end. Skipped when neither is set.
#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::tests_auth::Env;

    /// A tenant, a person, two devices (one in UTC+2), one login on each.
    async fn family(db: &sqlx::PgPool) -> (Uuid, Uuid, Uuid, Uuid, Uuid, Uuid) {
        let tenant: Uuid =
            sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('t') RETURNING id")
                .fetch_one(db)
                .await
                .unwrap();
        let person: Uuid = sqlx::query_scalar(
            "INSERT INTO admins (tenant_id, display_name, role, age_bracket)
             VALUES ($1, 'Mia', 'member', 'kid') RETURNING id",
        )
        .bind(tenant)
        .fetch_one(db)
        .await
        .unwrap();
        let profile: Uuid = sqlx::query_scalar(
            "INSERT INTO profiles (tenant_id, name, kind, policy) VALUES ($1, 'p', 'kid', '{}') RETURNING id",
        )
        .bind(tenant)
        .fetch_one(db)
        .await
        .unwrap();
        let mut ids = Vec::new();
        for name in ["laptop", "desktop"] {
            let dev: Uuid = sqlx::query_scalar(
                "INSERT INTO devices (tenant_id, name) VALUES ($1, $2) RETURNING id",
            )
            .bind(tenant)
            .bind(name)
            .fetch_one(db)
            .await
            .unwrap();
            let du: Uuid = sqlx::query_scalar(
                "INSERT INTO device_users (device_id, os_username, profile_id, account_id)
                 VALUES ($1, 'mia', $2, $3) RETURNING id",
            )
            .bind(dev)
            .bind(profile)
            .bind(person)
            .fetch_one(db)
            .await
            .unwrap();
            ids.push((dev, du));
        }
        (tenant, person, ids[0].0, ids[0].1, ids[1].0, ids[1].1)
    }

    fn entry(secs: i64, day: Option<NaiveDate>) -> UsageEntry {
        UsageEntry {
            os_username: "mia".into(),
            used_minutes_today: secs / 60,
            used_seconds_today: Some(secs),
            day,
            utc_offset_secs: Some(7200),
        }
    }

    async fn regressions(db: &sqlx::PgPool, du: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE device_user_id = $1 AND type = 'evasion'",
        )
        .bind(du)
        .fetch_one(db)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn usage_is_filed_per_device_day_and_answered_per_person() {
        let Some(env) = Env::new().await else { return };
        let db = env.st.db.clone();
        let (tenant, _person, laptop, du_laptop, desktop, du_desktop) = family(&db).await;
        let today = local_today(Some(7200), Utc::now());

        // 40 minutes on the laptop.
        let a = upsert_usage(&db, tenant, laptop, &[entry(40 * 60, Some(today))])
            .await
            .unwrap();
        assert_eq!(a[0]["used_elsewhere_secs"], 0);
        // The desktop reports 5 minutes: it is told about the laptop's 40.
        let b = upsert_usage(&db, tenant, desktop, &[entry(5 * 60, Some(today))])
            .await
            .unwrap();
        assert_eq!(b[0]["used_elsewhere_secs"], 40 * 60);
        assert_eq!(b[0]["day"], json!(today));
        // …and the laptop about the desktop's 5.
        let a = upsert_usage(&db, tenant, laptop, &[entry(41 * 60, Some(today))])
            .await
            .unwrap();
        assert_eq!(a[0]["used_elsewhere_secs"], 5 * 60);
        // A grant on the laptop shows up as the laptop's own earned time
        // (for a device that lost its ledger) and as the desktop's
        // "earned elsewhere" (the budget is the person's).
        sqlx::query("UPDATE screen_time_ledger SET earned_seconds = 900 WHERE device_user_id = $1")
            .bind(du_laptop)
            .execute(&db)
            .await
            .unwrap();
        let a = upsert_usage(&db, tenant, laptop, &[entry(41 * 60, Some(today))])
            .await
            .unwrap();
        assert_eq!(a[0]["earned_here_secs"], 900);
        assert_eq!(a[0]["earned_elsewhere_secs"], 0);
        let b = upsert_usage(&db, tenant, desktop, &[entry(5 * 60, Some(today))])
            .await
            .unwrap();
        assert_eq!(b[0]["earned_elsewhere_secs"], 900);
        assert_eq!(b[0]["earned_here_secs"], 0);
        // The device's offset is remembered for "today" on the console.
        let off: Option<i32> =
            sqlx::query_scalar("SELECT utc_offset_secs FROM devices WHERE id = $1")
                .bind(laptop)
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(off, Some(7200));
        let console_today: i64 = sqlx::query_scalar(&format!(
            "SELECT COALESCE(SUM(l.used_seconds),0)::bigint FROM device_users du
               JOIN devices d ON d.id = du.device_id
               JOIN screen_time_ledger l ON l.device_user_id = du.id AND l.day = {DEVICE_TODAY_SQL}
              WHERE du.id = ANY($1)"
        ))
        .bind(vec![du_laptop, du_desktop])
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(
            console_today,
            46 * 60,
            "the console sums the same person-day"
        );
        env.drop_db().await;
    }

    #[tokio::test]
    async fn midnight_never_accuses_and_a_real_drop_alerts_once() {
        let Some(env) = Env::new().await else { return };
        let db = env.st.db.clone();
        let (tenant, _p, laptop, du, _d2, _du2) = family(&db).await;
        let today = local_today(Some(7200), Utc::now());
        let yesterday = today.pred_opt().unwrap();

        // Yesterday ended at 90 minutes; local midnight starts today at 0.
        upsert_usage(&db, tenant, laptop, &[entry(90 * 60, Some(yesterday))])
            .await
            .unwrap();
        for _ in 0..5 {
            upsert_usage(&db, tenant, laptop, &[entry(0, Some(today))])
                .await
                .unwrap();
        }
        assert_eq!(regressions(&db, du).await, 0, "a new day is not evasion");

        // A genuine same-day drop (ledger wiped at 60 → 0) alerts once, not
        // once per heartbeat.
        upsert_usage(&db, tenant, laptop, &[entry(60 * 60, Some(today))])
            .await
            .unwrap();
        for _ in 0..5 {
            upsert_usage(&db, tenant, laptop, &[entry(0, Some(today))])
                .await
                .unwrap();
        }
        assert_eq!(regressions(&db, du).await, 1);
        // GREATEST: the total never went down.
        let used: i32 = sqlx::query_scalar(
            "SELECT used_seconds FROM screen_time_ledger WHERE device_user_id = $1 AND day = $2",
        )
        .bind(du)
        .bind(today)
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(used, 60 * 60);
        // An old agent (no day) is never accused.
        upsert_usage(
            &db,
            tenant,
            laptop,
            &[UsageEntry {
                os_username: "mia".into(),
                used_minutes_today: 0,
                used_seconds_today: None,
                day: None,
                utc_offset_secs: None,
            }],
        )
        .await
        .unwrap();
        assert_eq!(regressions(&db, du).await, 1);
        env.drop_db().await;
    }
}
