//! `openscreentime-server recover <name>` — the way back for a parent who lost
//! every passkey and has no computer to get a code on.
//!
//! Run it as root inside the server container:
//!
//! ```text
//! podman exec openscreentime-server /app/openscreentime-server recover philip
//! ```
//!
//! It prints a one-time sign-in link for that **existing** account (single
//! use, 30 minutes); opening it signs in with "confirm it's you" already done,
//! so the next step is adding a new passkey. Whoever can run this already
//! owns the server and its database, so it grants nothing new. It replaces the
//! old answer — register again — which made a new, empty household.

use uuid::Uuid;

/// Find the parent account called `name` and print a sign-in link for it.
pub async fn run(db: &sqlx::PgPool, public_url: &str, name: &str) -> anyhow::Result<()> {
    let name = name.trim();
    let parents: Vec<(Uuid, Uuid, Option<String>, String)> = sqlx::query_as(
        "SELECT id, tenant_id, username, display_name FROM admins
          WHERE role <> 'member' ORDER BY created_at",
    )
    .fetch_all(db)
    .await?;
    let matches: Vec<_> = parents
        .iter()
        .filter(|(_, _, username, display)| {
            username
                .as_deref()
                .is_some_and(|u| u.eq_ignore_ascii_case(name))
                || display.eq_ignore_ascii_case(name)
        })
        .collect();

    let list = || {
        parents
            .iter()
            .map(|(_, _, u, d)| format!("  {} ({})", d, u.as_deref().unwrap_or("no login name")))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (account_id, tenant_id, username, display) = match matches.as_slice() {
        [one] => *one,
        [] => anyhow::bail!(
            "no parent account called {name:?}. The parents here:\n{}",
            list()
        ),
        _ => anyhow::bail!(
            "more than one parent is called {name:?} — use the login name in brackets:\n{}",
            list()
        ),
    };

    let token = crate::voucher::mint_link(db, *tenant_id, *account_id).await?;
    println!(
        "One-time sign-in link for {display} ({}) — it works once, for {} minutes:\n\n  {}/#signin={token}\n\n\
         Open it, then add a new passkey under Settings → Security & access.",
        username.as_deref().unwrap_or("no login name"),
        crate::voucher::LINK_MINUTES,
        public_url.trim_end_matches('/'),
    );
    Ok(())
}
