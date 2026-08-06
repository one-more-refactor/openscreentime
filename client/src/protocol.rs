//! Wire types shared with the server: commands, events, command-acks and the
//! WebSocket bus envelope. Kept deliberately lenient (`serde(default)`, untagged
//! payloads as `serde_json::Value`) so the agent tolerates server-side additions.
//!
//! `docs/API.md` defines the surfaces; `docs/DATA_MODEL.md` the enums. The exact
//! WS framing isn't pinned by the docs, so this module defines the explicit,
//! self-describing tagged JSON envelope (mirrored by `server/src/agent.rs`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Command types (DATA_MODEL.md → `commands.type`).
pub const CMD_LOCK: &str = "lock";
pub const CMD_UNLOCK: &str = "unlock";
pub const CMD_APPLY_POLICY: &str = "apply_policy";
pub const CMD_SET_TAMPER_LEVEL: &str = "set_tamper_level";
/// Earn-time approval credit (CONTRACT-PROD.md §4): `{os_username, minutes, request_id}`.
pub const CMD_CREDIT_TIME: &str = "credit_time";
/// Earn-time denial (mirror of `credit_time`): `{os_username, task_id, request_id}`.
/// Lets the agent clear its once-per-day dedupe so the teen can re-ask, and
/// tell them they were denied instead of leaving "WAITING FOR APPROVAL" up all day.
pub const CMD_DENY_EARN: &str = "deny_earn";

/// Event types the agent emits (DATA_MODEL.md → `events.type`; `heartbeat` and
/// `enrolled` also exist but are written server-side, never by the agent).
pub const EV_TAMPER: &str = "tamper";
pub const EV_LOCK: &str = "lock";
pub const EV_UNLOCK: &str = "unlock";
pub const EV_POLICY_APPLIED: &str = "policy_applied";
pub const EV_SCREEN_TIME_EXCEEDED: &str = "screen_time_exceeded";
pub const EV_SCREEN_TIME_EARNED: &str = "screen_time_earned";
/// Policy was accepted but the host cannot actually enforce part of it.
pub const EV_ENFORCEMENT_DEGRADED: &str = "enforcement_degraded";

/// Severities (DATA_MODEL.md → `events.severity`).
pub const SEV_INFO: &str = "info";
pub const SEV_WARN: &str = "warn";
pub const SEV_CRITICAL: &str = "critical";

/// A server → agent command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type")]
    pub cmd_type: String,
    #[serde(default)]
    pub payload: Value,
}

/// An agent → server event (`POST /agent/events` element).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    #[serde(rename = "type")]
    pub ev_type: String,
    pub severity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_user: Option<String>,
    pub payload: Value,
}

impl Event {
    pub fn new(ev_type: &str, severity: &str, payload: Value) -> Self {
        Event {
            ev_type: ev_type.to_string(),
            severity: severity.to_string(),
            device_user: None,
            payload,
        }
    }
    pub fn for_user(mut self, user: impl Into<String>) -> Self {
        self.device_user = Some(user.into());
        self
    }
}

/// One user's screen-time usage as of "now" (CONTRACT-PROD.md §5). Reported both
/// in the HTTP heartbeat body and in the WS `heartbeat` frame; the server upserts
/// it into `screen_time_ledger`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageReport {
    pub os_username: String,
    pub used_minutes_today: u32,
}

/// A command ack (`POST /agent/commands/:id/ack`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandAck {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub command_id: String,
    /// "acked" | "failed"
    pub status: String,
    pub result: Value,
}

/// Server → agent frames on the WS bus.
///
/// CONTRACT-PROD.md §3 pins the tag field to `"type"`;
/// the frames used to be tagged `"kind"`, which silently disagreed with the documented
/// wire shape (never noticed because both ends only ever spoke to each other's own
/// impl). Renamed here to match the contract exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Command {
        command: Command,
    },
    /// Keepalive.
    Ping,
}

/// Agent → server frames on the WS bus. See `ServerFrame` doc for the `"type"` tag note.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentFrame {
    Event {
        event: Event,
    },
    Ack {
        ack: CommandAck,
    },
    /// Periodic per-user usage push. The WS bus has no HTTP heartbeat, so this is
    /// how a WS-connected agent keeps `screen_time_ledger` current (CONTRACT-PROD.md §5).
    Heartbeat {
        usage: Vec<UsageReport>,
    },
    Pong,
}
