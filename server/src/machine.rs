//! One computer, one record — and a person's day that outlives the record.
//!
//! **The same machine, enrolled again.** Running the install one-liner with a
//! new token on a computer the household already has ("Add my computer" on
//! the machine that was "Mia's computer") used to leave two records for one
//! machine. The old record kept today's minutes; the agent, whose own ledger
//! still held them, reported the same minutes again under the new record —
//! and the server handed the old record's minutes back to it as time used
//! "elsewhere". Mia's 32 minutes became 64.
//!
//! The agent now says which machine it is: an HMAC-SHA256 of
//! `/etc/machine-id`, keyed per household with the salt it learns from the
//! enroll preview — never the id itself, and nothing another household (or
//! another server) could match. Enrolling a machine the household already
//! has **folds the older record into the new one** ([`take_over`]): each
//! login's ledger days (the larger count wins — they are the same minutes),
//! where the time went, the moments and the asks move over; the old record's
//! token becomes a tombstone that points at the new record, and the old
//! record goes. The person's day is counted once.
//!
//! The tombstone points instead of saying `410 device_retired` because the
//! agent that held that token is still running on this very computer until
//! the installer restarts it, and 410 is the one answer on which an agent
//! takes itself off its computer — it would uninstall the enrollment that
//! just happened. Heard as the new record instead, it carries on until the
//! restart. When the new record is removed, the pointer clears (`ON DELETE
//! SET NULL`) and the old token hears 410 like any removed computer's.
//!
//! **A computer removed.** The ledger hangs off a computer's logins, so
//! deleting the computer took every person's day on it along — Mia showed
//! "17 min left of 17" after using 37. Now [`keep_usage`] files a removed
//! computer's ledger under each person (`retired_usage`), and everything that
//! adds up a person's day — the console's today, their week, and the time
//! their other computers count as used elsewhere — adds it in. Enrolling the
//! same machine again takes those minutes back onto its logins, so they are
//! not counted twice either.

use std::collections::HashSet;

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::error::AppResult;
use crate::state::AppState;

/// A machine identity as the agent sends it: an HMAC-SHA256, 64 lowercase hex
/// characters. Anything else is ignored, never stored.
pub fn clean_hash(v: Option<&str>) -> Option<String> {
    let v = v?.trim();
    (v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| v.to_string())
}

/// This household's salt for machine identities (handed to the holder of an
/// enroll token by the preview, so the agent can key its hash with it).
pub async fn salt(db: &PgPool, tenant_id: Uuid) -> AppResult<String> {
    Ok(
        sqlx::query_scalar("SELECT machine_salt FROM tenants WHERE id = $1")
            .bind(tenant_id)
            .fetch_one(db)
            .await?,
    )
}

/// The household's other records of this machine.
async fn older_records(
    db: &mut PgConnection,
    tenant_id: Uuid,
    device_id: Uuid,
    hash: &str,
) -> Result<Vec<(Uuid, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name FROM devices
          WHERE tenant_id = $1 AND machine_hash = $2 AND id <> $3
          ORDER BY created_at
          FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(hash)
    .bind(device_id)
    .fetch_all(db)
    .await
}

/// Before the logins are linked at enrollment: each login the machine's older
/// record had keeps the person it was — so a login a parent sorted by hand
/// under Who's who doesn't come back as a stranger, and its day moves to the
/// right person. The new record's owner login is left to the owner (the
/// parent just said whose computer it is). Idempotent.
pub async fn carry_links(
    db: &PgPool,
    tenant_id: Uuid,
    device_id: Uuid,
    hash: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO device_users (device_id, os_username, display_name, profile_id, account_id,
                                   unsorted)
         SELECT DISTINCT ON (o.os_username)
                $3, o.os_username, o.display_name, o.profile_id, o.account_id, o.unsorted
           FROM device_users o
           JOIN devices od ON od.id = o.device_id
           JOIN devices nd ON nd.id = $3
          WHERE od.tenant_id = $1 AND od.machine_hash = $2 AND od.id <> $3
            AND o.account_id IS NOT NULL
            AND lower(o.os_username) IS DISTINCT FROM lower(nd.owner_os_username)
          ORDER BY o.os_username, od.created_at DESC
         ON CONFLICT (device_id, os_username) DO NOTHING",
    )
    .bind(tenant_id)
    .bind(hash)
    .bind(device_id)
    .execute(db)
    .await?;
    Ok(())
}

/// File a computer's usage under each person who used it, before its record
/// goes (removed, or folded into the same machine's new record while that
/// machine no longer has the login). Only the rows that say something.
pub async fn keep_usage(
    db: &mut PgConnection,
    device_id: Uuid,
    only_gone_from: Option<Uuid>,
) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "INSERT INTO retired_usage (tenant_id, account_id, device_id, device_name, machine_hash,
                                    os_username, day, used_seconds, earned_seconds,
                                    utc_offset_secs)
         SELECT d.tenant_id, du.account_id, d.id, d.name, d.machine_hash,
                du.os_username, l.day, l.used_seconds, l.earned_seconds, d.utc_offset_secs
           FROM screen_time_ledger l
           JOIN device_users du ON du.id = l.device_user_id
           JOIN devices d ON d.id = du.device_id
          WHERE d.id = $1 AND du.account_id IS NOT NULL
            AND (l.used_seconds > 0 OR l.earned_seconds > 0)
            AND ($2::uuid IS NULL OR NOT EXISTS (
                  SELECT 1 FROM device_users n
                   WHERE n.device_id = $2 AND n.os_username = du.os_username))
         ON CONFLICT (device_id, os_username, day) DO UPDATE
            SET used_seconds = GREATEST(retired_usage.used_seconds, EXCLUDED.used_seconds),
                earned_seconds = GREATEST(retired_usage.earned_seconds, EXCLUDED.earned_seconds)",
    )
    .bind(device_id)
    .bind(only_gone_from)
    .execute(db)
    .await?
    .rows_affected())
}

/// Fold every older record of this machine into `device_id`, the record it
/// was just enrolled as (see the module doc). Runs after the enroll token is
/// spent, in one transaction. Returns the names of the records it replaced.
pub async fn take_over(
    st: &AppState,
    tenant_id: Uuid,
    device_id: Uuid,
    hash: &str,
) -> AppResult<Vec<String>> {
    let mut tx = st.db.begin().await?;
    let olds = older_records(&mut tx, tenant_id, device_id, hash).await?;
    let mut people: HashSet<Uuid> = HashSet::new();

    for (old, _) in &olds {
        people.extend(
            sqlx::query_scalar::<_, Uuid>(
                "SELECT account_id FROM device_users WHERE device_id = $1 AND account_id IS NOT NULL",
            )
            .bind(old)
            .fetch_all(&mut *tx)
            .await?,
        );
        // The ledger, login by login. The larger count wins: the agent's own
        // ledger on this machine already holds the old record's minutes.
        sqlx::query(
            "INSERT INTO screen_time_ledger (device_user_id, day, used_seconds, earned_seconds)
             SELECT n.id, l.day, l.used_seconds, l.earned_seconds
               FROM screen_time_ledger l
               JOIN device_users o ON o.id = l.device_user_id AND o.device_id = $1
               JOIN device_users n ON n.device_id = $2 AND n.os_username = o.os_username
             ON CONFLICT (device_user_id, day) DO UPDATE
                SET used_seconds = GREATEST(screen_time_ledger.used_seconds,
                                            EXCLUDED.used_seconds),
                    earned_seconds = GREATEST(screen_time_ledger.earned_seconds,
                                              EXCLUDED.earned_seconds)",
        )
        .bind(old)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        // A login the machine no longer has: its person keeps the minutes.
        keep_usage(&mut tx, *old, Some(device_id)).await?;
        // Where the time went (upsert-summed hour slices: disjoint parts).
        sqlx::query(
            "INSERT INTO usage_slices (device_id, tenant_id, os_username, hour, kind, key, amount)
             SELECT $2, tenant_id, os_username, hour, kind, key, amount
               FROM usage_slices WHERE device_id = $1
             ON CONFLICT (device_id, os_username, hour, kind, key) DO UPDATE
                SET amount = usage_slices.amount + EXCLUDED.amount",
        )
        .bind(old)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        // The moments, onto the same logins here. An event the agent already
        // re-sent under the new record (same client id) isn't doubled.
        sqlx::query(
            "DELETE FROM events e
              WHERE e.device_id = $1 AND e.client_id IS NOT NULL
                AND EXISTS (SELECT 1 FROM events x
                             WHERE x.device_id = $2 AND x.client_id = e.client_id)",
        )
        .bind(old)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE events e
                SET device_id = $2,
                    device_user_id = (SELECT n.id FROM device_users o
                                        JOIN device_users n
                                          ON n.device_id = $2 AND n.os_username = o.os_username
                                       WHERE o.id = e.device_user_id)
              WHERE e.device_id = $1",
        )
        .bind(old)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        // An ask still waiting is still waiting — on the same login here.
        sqlx::query(
            "UPDATE earn_requests er
                SET device_id = $2, device_user_id = n.id
               FROM device_users o, device_users n
              WHERE er.device_id = $1 AND o.id = er.device_user_id
                AND n.device_id = $2 AND n.os_username = o.os_username",
        )
        .bind(old)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        // "Today" is the machine's day until the agent reports its own clock.
        sqlx::query(
            "UPDATE devices SET utc_offset_secs = COALESCE(devices.utc_offset_secs, o.utc_offset_secs)
               FROM devices o WHERE devices.id = $2 AND o.id = $1",
        )
        .bind(old)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        // The old token points here (and so does anything that pointed at
        // the old record): the agent still running with it is this record.
        sqlx::query(
            "INSERT INTO retired_devices (token_hash, device_id, tenant_id, merged_into)
             SELECT device_token, id, tenant_id, $2 FROM devices
              WHERE id = $1 AND device_token IS NOT NULL
             ON CONFLICT (token_hash) DO UPDATE SET merged_into = EXCLUDED.merged_into",
        )
        .bind(old)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE retired_devices SET merged_into = $2 WHERE merged_into = $1")
            .bind(old)
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM devices WHERE id = $1 AND tenant_id = $2")
            .bind(old)
            .bind(tenant_id)
            .execute(&mut *tx)
            .await?;
    }

    // This machine was removed before: its kept minutes go back onto the
    // same logins here, for the same person (the agent's ledger has them).
    sqlx::query(
        "INSERT INTO screen_time_ledger (device_user_id, day, used_seconds, earned_seconds)
         SELECT n.id, r.day, max(r.used_seconds), max(r.earned_seconds)
           FROM retired_usage r
           JOIN device_users n ON n.device_id = $3 AND n.os_username = r.os_username
                              AND n.account_id = r.account_id
          WHERE r.tenant_id = $1 AND r.machine_hash = $2
          GROUP BY n.id, r.day
         ON CONFLICT (device_user_id, day) DO UPDATE
            SET used_seconds = GREATEST(screen_time_ledger.used_seconds, EXCLUDED.used_seconds),
                earned_seconds = GREATEST(screen_time_ledger.earned_seconds,
                                          EXCLUDED.earned_seconds)",
    )
    .bind(tenant_id)
    .bind(hash)
    .bind(device_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM retired_usage r USING device_users n
          WHERE r.tenant_id = $1 AND r.machine_hash = $2
            AND n.device_id = $3 AND n.os_username = r.os_username
            AND n.account_id = r.account_id",
    )
    .bind(tenant_id)
    .bind(hash)
    .bind(device_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    for (old, _) in &olds {
        // Its socket (if the agent is still connected under the old record)
        // drops; the agent reconnects and is heard as this record.
        st.hub.force_unregister(*old).await;
    }
    // A person made up for a login that is someone else's now (the owner's)
    // and has no other computer goes, as Who's who does it.
    for person in people {
        let _ = crate::members::drop_if_leftover(&st.db, tenant_id, person).await;
    }
    if !olds.is_empty() {
        tracing::info!(%device_id, replaced = olds.len(), "same machine enrolled again; folded its older record in");
    }
    Ok(olds.into_iter().map(|(_, name)| name).collect())
}

#[cfg(test)]
mod tests {
    use super::clean_hash;

    #[test]
    fn only_a_well_formed_hash_is_kept() {
        let h = "a".repeat(64);
        assert_eq!(clean_hash(Some(&h)).as_deref(), Some(h.as_str()));
        assert_eq!(
            clean_hash(Some(&format!(" {h} "))).as_deref(),
            Some(h.as_str())
        );
        assert_eq!(clean_hash(None), None);
        assert_eq!(clean_hash(Some("")), None);
        // Not hex, wrong length, upper case (the agent sends lower): ignored.
        assert_eq!(clean_hash(Some(&"g".repeat(64))), None);
        assert_eq!(clean_hash(Some(&"a".repeat(63))), None);
        assert_eq!(clean_hash(Some(&"A".repeat(64))), None);
        // A raw machine-id (32 hex) is never taken for one.
        assert_eq!(clean_hash(Some("0123456789abcdef0123456789abcdef")), None);
    }
}
