//! Server-brokered reverse-SSH tunnel (skeleton).
//!
//! Devices are behind NAT, so the **agent dials out**; the server brokers a
//! shell. Flow (TAMPER.md → Remote SSH):
//!   1. Admin clicks SSH → `POST /api/devices/:id/ssh`.
//!   2. Server allocates a `broker_port`, creates an `ssh_session` (`opening`),
//!      and enqueues an `ssh_open` command `{ session_id, broker_port }`.
//!   3. Agent opens a reverse channel back over the existing WS. The server
//!      multiplexes a PTY over that WS and bridges it to a local listener /
//!      in-browser terminal.
//!   4. Server marks the session `open` and returns a `connect_cmd`.
//!   5. `{ close: true }` (or idle timeout) → `ssh_close`.
//!
//! PRODUCTION PATH (documented, not implemented here): the agent runs
//! `ssh -R <broker_port>:localhost:22 broker@<server>` against an embedded SSH
//! server (e.g. `russh`/`thrussh`) bound to `broker_port`; the admin then
//! `ssh -p <broker_port> device@<server>`. This skeleton instead registers an
//! in-memory `SshBridge` and relies on WS `ssh_data` frames (see agent.rs) so
//! the end-to-end flow can be demonstrated without an embedded sshd.

use axum::{
    extract::{Path, State},
    Json,
};
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::agent::enqueue_command;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{AppState, AuthAdmin, SshBridge};

/// Broker ports are allocated from this ephemeral range.
const BROKER_PORT_MIN: i32 = 33000;
const BROKER_PORT_MAX: i32 = 34000;

#[derive(Deserialize, Default)]
pub struct SshReq {
    #[serde(default)]
    pub close: bool,
}

pub async fn open_or_close(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(device_id): Path<Uuid>,
    body: axum::body::Bytes,
) -> AppResult<Json<Value>> {
    // Verify ownership.
    let owner: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM devices WHERE id = $1 AND tenant_id = $2")
            .bind(device_id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    owner.ok_or_else(|| AppError::NotFound("device not found".into()))?;

    // Body is optional: no body / empty body means "open".
    let req: SshReq = if body.is_empty() {
        SshReq::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid body: {e}")))?
    };

    if req.close {
        return close_session(&st, admin, device_id).await;
    }

    // Allocate a broker port and open a session.
    let broker_port: i32 = rand::thread_rng().gen_range(BROKER_PORT_MIN..BROKER_PORT_MAX);
    let session_id: Uuid = sqlx::query_scalar(
        "INSERT INTO ssh_sessions (device_id, admin_id, broker_port, status)
         VALUES ($1, $2, $3, 'opening') RETURNING id",
    )
    .bind(device_id)
    .bind(admin.admin_id)
    .bind(broker_port)
    .fetch_one(&st.db)
    .await?;

    // Register an in-memory bridge for agent->admin data. In this skeleton the
    // receiver end would be drained by the `sentinel ssh` CLI / browser
    // terminal; we spawn a drain task so the channel never blocks the agent.
    let (to_admin, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    st.hub
        .open_ssh(
            session_id,
            SshBridge {
                device_id,
                broker_port,
                to_admin,
            },
        )
        .await;
    tokio::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            tracing::debug!(
                bytes = chunk.len(),
                "ssh: agent->admin frame (skeleton drain)"
            );
        }
    });

    // Ask the agent to dial back and open the reverse channel.
    enqueue_command(
        &st,
        device_id,
        "ssh_open",
        json!({ "session_id": session_id, "broker_port": broker_port }),
    )
    .await?;

    // Skeleton: mark open immediately. Production marks `open` when the agent's
    // reverse channel is confirmed established.
    sqlx::query("UPDATE ssh_sessions SET status = 'open' WHERE id = $1")
        .bind(session_id)
        .execute(&st.db)
        .await?;

    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        None,
        "tamper", // audit trail; remote-shell open is a security-relevant action
        "info",
        json!({ "action": "ssh_open", "session_id": session_id, "by": admin.admin_id }),
    )
    .await?;

    let connect_cmd = format!("ssh -p {broker_port} device@{host}", host = st.broker_host);

    Ok(Json(json!({
        "ssh_session": {
            "id": session_id,
            "device_id": device_id,
            "broker_port": broker_port,
            "status": "open",
        },
        "connect_cmd": connect_cmd,
    })))
}

async fn close_session(st: &AppState, admin: AuthAdmin, device_id: Uuid) -> AppResult<Json<Value>> {
    let sess: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM ssh_sessions WHERE device_id = $1 AND status IN ('opening','open')
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(device_id)
    .fetch_optional(&st.db)
    .await?;
    let session_id = sess
        .ok_or_else(|| AppError::NotFound("no open ssh session".into()))?
        .0;

    sqlx::query("UPDATE ssh_sessions SET status = 'closed', closed_at = now() WHERE id = $1")
        .bind(session_id)
        .execute(&st.db)
        .await?;
    st.hub.close_ssh(session_id).await;

    enqueue_command(
        st,
        device_id,
        "ssh_close",
        json!({ "session_id": session_id }),
    )
    .await?;

    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        None,
        "tamper",
        "info",
        json!({ "action": "ssh_close", "session_id": session_id, "by": admin.admin_id }),
    )
    .await?;

    Ok(Json(json!({ "ok": true, "session_id": session_id })))
}
