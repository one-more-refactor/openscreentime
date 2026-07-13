//! Server-brokered reverse SSH: the agent (behind NAT) multiplexes a PTY over
//! its existing WebSocket bus; the admin drives it from a browser terminal.
//!
//! Flow:
//!   1. Admin clicks SSH → `POST /api/devices/:id/ssh` creates an
//!      `ssh_sessions` row (`opening`), registers an in-memory bridge, and
//!      sends the agent an `ssh_open` command `{ session_id, broker_port }`.
//!   2. Agent spawns a PTY shell and streams `ssh_data { session_id, data_b64 }`
//!      frames (base64 raw bytes) over `/agent/ws`; its first frame confirms
//!      the session and flips it to `open`.
//!   3. The browser connects `GET /api/ssh/:session_id/ws` (cookie-auth
//!      upgrade). Binary frames from the browser are raw keystrokes (bridged
//!      to the agent as base64 `ssh_data`); text frames carry
//!      `{"type":"resize","cols":N,"rows":N}` → `ssh_resize` to the agent.
//!      Agent output flows back as binary frames; a final text frame
//!      `{"type":"closed","exit_code":N|null}` precedes server close.
//!   4. `POST /api/ssh/:session_id/close`, a browser disconnect, or the
//!      agent's `ssh_closed` frame tears the session down (`ssh_close` to the
//!      agent, row marked `closed`).
//!
//! SSH activity is audited with `type = 'ssh'` events.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::Response,
    Json,
};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::agent::enqueue_command;
use crate::error::{AppError, AppResult};
use crate::events;
use crate::state::{AppState, AuthAdmin, SshToAdmin};

/// Broker ports are allocated from this ephemeral range.
const BROKER_PORT_MIN: i32 = 33000;
const BROKER_PORT_MAX: i32 = 34000;

fn session_json(
    id: Uuid,
    device_id: Uuid,
    broker_port: i32,
    status: &str,
    created_at: DateTime<Utc>,
) -> Value {
    json!({
        "id": id,
        "device_id": device_id,
        "broker_port": broker_port,
        "status": status,
        "created_at": created_at,
    })
}

/// POST /api/devices/:id/ssh — open a session. Stays `opening` until the
/// agent's first `ssh_data` frame confirms the shell is up.
pub async fn open_session(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(device_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    // Verify ownership.
    let owner: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM devices WHERE id = $1 AND tenant_id = $2")
            .bind(device_id)
            .bind(admin.tenant_id)
            .fetch_optional(&st.db)
            .await?;
    owner.ok_or_else(|| AppError::NotFound("device not found".into()))?;

    let broker_port: i32 = rand::thread_rng().gen_range(BROKER_PORT_MIN..BROKER_PORT_MAX);
    let (session_id, created_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO ssh_sessions (device_id, admin_id, broker_port, status)
         VALUES ($1, $2, $3, 'opening') RETURNING id, created_at",
    )
    .bind(device_id)
    .bind(admin.admin_id)
    .bind(broker_port)
    .fetch_one(&st.db)
    .await?;

    // Bridge for agent<->admin frames; agent output arriving before the
    // browser terminal attaches is buffered here.
    st.hub.open_ssh(session_id, device_id).await;

    // Ask the agent to spawn the PTY shell.
    enqueue_command(
        &st,
        device_id,
        "ssh_open",
        json!({ "session_id": session_id, "broker_port": broker_port }),
    )
    .await?;

    events::insert(
        &st.db,
        admin.tenant_id,
        Some(device_id),
        None,
        "ssh",
        "info",
        json!({ "action": "ssh_open", "session_id": session_id, "by": admin.admin_id }),
    )
    .await?;

    Ok(Json(json!({
        "session": session_json(session_id, device_id, broker_port, "opening", created_at),
    })))
}

/// Load an SSH session scoped to the tenant, or 404.
async fn get_session(
    db: &sqlx::PgPool,
    session_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<(Uuid, String)> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT s.device_id, s.status FROM ssh_sessions s
         JOIN devices d ON d.id = s.device_id
         WHERE s.id = $1 AND d.tenant_id = $2",
    )
    .bind(session_id)
    .bind(tenant_id)
    .fetch_optional(db)
    .await?;
    row.ok_or_else(|| AppError::NotFound("ssh session not found".into()))
}

/// Mark a session closed and tell the agent to tear its side down. Idempotent.
pub async fn finalize_close(
    st: &AppState,
    tenant_id: Uuid,
    by_admin: Option<Uuid>,
    session_id: Uuid,
    device_id: Uuid,
) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE ssh_sessions SET status = 'closed', closed_at = now()
         WHERE id = $1 AND status IN ('opening','open')",
    )
    .bind(session_id)
    .execute(&st.db)
    .await?;
    st.hub.close_ssh(session_id, None).await;
    if res.rows_affected() == 0 {
        return Ok(()); // already closed
    }

    // Prefer a live WS frame; fall back to the command queue when offline.
    let frame = json!({ "type": "ssh_close", "session_id": session_id });
    if !st.hub.push(device_id, frame).await {
        enqueue_command(st, device_id, "ssh_close", json!({ "session_id": session_id })).await?;
    }

    events::insert(
        &st.db,
        tenant_id,
        Some(device_id),
        None,
        "ssh",
        "info",
        json!({ "action": "ssh_close", "session_id": session_id, "by": by_admin }),
    )
    .await?;
    Ok(())
}

/// POST /api/ssh/:session_id/close
pub async fn close_session(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(session_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let (device_id, _status) = get_session(&st.db, session_id, admin.tenant_id).await?;
    finalize_close(&st, admin.tenant_id, Some(admin.admin_id), session_id, device_id).await?;
    Ok(Json(json!({ "ok": true, "session_id": session_id })))
}

/// GET /api/ssh/:session_id/ws — the browser terminal.
pub async fn ws(
    State(st): State<AppState>,
    admin: AuthAdmin,
    Path(session_id): Path<Uuid>,
    upgrade: WebSocketUpgrade,
) -> AppResult<Response> {
    let (device_id, status) = get_session(&st.db, session_id, admin.tenant_id).await?;
    if status != "opening" && status != "open" {
        return Err(AppError::Conflict("ssh session is not open".into()));
    }
    Ok(upgrade
        .on_upgrade(move |socket| handle_admin_ws(st, admin, session_id, device_id, socket)))
}

async fn handle_admin_ws(
    st: AppState,
    admin: AuthAdmin,
    session_id: Uuid,
    device_id: Uuid,
    socket: WebSocket,
) {
    let (mut sink, mut stream) = socket.split();

    // Attach to the bridge; drains any output buffered since ssh_open.
    let (tx, mut rx) = mpsc::unbounded_channel::<SshToAdmin>();
    if st.hub.attach_ssh_admin(session_id, tx).await.is_none() {
        // Bridge is gone (server restart / already torn down).
        let _ = sink
            .send(Message::Text(
                json!({ "type": "closed", "exit_code": null }).to_string().into(),
            ))
            .await;
        return;
    }

    // True when the agent side ended the session (no ssh_close needed).
    let mut agent_closed = false;

    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(SshToAdmin::Data(bytes)) => {
                    if sink.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                Some(SshToAdmin::Closed(exit_code)) => {
                    let _ = sink
                        .send(Message::Text(
                            json!({ "type": "closed", "exit_code": exit_code })
                                .to_string()
                                .into(),
                        ))
                        .await;
                    agent_closed = true;
                    break;
                }
                None => break, // bridge torn down elsewhere
            },
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    // Raw keystrokes → base64 ssh_data to the agent.
                    let frame = json!({
                        "type": "ssh_data",
                        "session_id": session_id,
                        "data_b64": B64.encode(&bytes),
                    });
                    st.hub.push(device_id, frame).await;
                }
                Some(Ok(Message::Text(t))) => {
                    // {"type":"resize","cols":N,"rows":N}
                    if let Ok(v) = serde_json::from_str::<Value>(t.as_str()) {
                        if v.get("type").and_then(|t| t.as_str()) == Some("resize") {
                            let cols = v.get("cols").and_then(|c| c.as_u64()).unwrap_or(80);
                            let rows = v.get("rows").and_then(|r| r.as_u64()).unwrap_or(24);
                            let frame = json!({
                                "type": "ssh_resize",
                                "session_id": session_id,
                                "cols": cols,
                                "rows": rows,
                            });
                            st.hub.push(device_id, frame).await;
                        }
                    }
                }
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
            },
        }
    }

    // Browser gone (or agent ended it): make sure the session is torn down.
    if !agent_closed {
        if let Err(e) =
            finalize_close(&st, admin.tenant_id, Some(admin.admin_id), session_id, device_id).await
        {
            tracing::warn!(%session_id, error = %e, "ssh close after ws end failed");
        }
    } else {
        st.hub.close_ssh(session_id, None).await;
    }
    let _ = sink.close().await;
}
