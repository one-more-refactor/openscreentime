//! Database pool + migrations + a couple of shared helpers.

use std::time::Duration;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        // A request waiting on a dead database should fail in seconds, not
        // hang for the 30 s default.
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await
        .context("connecting to Postgres")?;
    Ok(pool)
}

/// [`connect`], retried with backoff until Postgres answers. After a reboot
/// or power cut the database container may still be starting when we are;
/// exiting would just crash-loop until it is up, so wait for it instead.
pub async fn connect_with_retry(database_url: &str) -> PgPool {
    let mut delay = Duration::from_secs(1);
    let mut attempt: u32 = 1;
    loop {
        match connect(database_url).await {
            Ok(pool) => return pool,
            Err(e) => {
                tracing::warn!(
                    attempt,
                    error = %format!("{e:#}"),
                    retry_in_secs = delay.as_secs(),
                    "database not reachable yet"
                );
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(15));
        attempt += 1;
    }
}

/// Runs the embedded SQLx migrations in `./migrations`.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("running migrations")?;
    Ok(())
}
