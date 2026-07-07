//! Wire types shared with the server: commands, events, command-acks and the
//! WebSocket bus envelope. Kept deliberately lenient (`serde(default)`, untagged
//! payloads as `serde_json::Value`) so the agent tolerates server-side additions.
//!
//! `docs/API.md` defines the surfaces; `docs/DATA_MODEL.md` the enums. The exact
//! WS framing isn't pinned by the docs (the server crate is still empty), so this
//! module picks an explicit, self-describing tagged JSON envelope.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Command types (DATA_MODEL.md → `commands.type`).
pub const CMD_LOCK: &str = "lock";
pub const CMD_UNLOCK: &str = "unlock";
pub const CMD_APPLY_POLICY: &str = "apply_policy";
pub const CMD_SSH_OPEN: &str = "ssh_open";
pub const CMD_SSH_CLOSE: &str = "ssh_close";
pub const CMD_DISCOVER: &str = "discover";
pub const CMD_SET_TAMPER_LEVEL: &str = "set_tamper_level";

/// Event types (DATA_MODEL.md → `events.type`).
pub const EV_HEARTBEAT: &str = "heartbeat";
pub const EV_TAMPER: &str = "tamper";
pub const EV_LOCK: &str = "lock";
pub const EV_UNLOCK: &str = "unlock";
pub const EV_POLICY_APPLIED: &str = "policy_applied";
pub const EV_SCREEN_TIME_EXCEEDED: &str = "screen_time_exceeded";
pub const EV_SCREEN_TIME_EARNED: &str = "screen_time_earned";
pub const EV_STREAK: &str = "streak";
pub const EV_ENROLLED: &str = "enrolled";
pub const EV_DISCOVERY_RESULT: &str = "discovery_result";

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerFrame {
    Command {
        command: Command,
    },
    /// A frame of an open reverse-SSH session (base64-encoded bytes toward the shell).
    SshData {
        session_id: String,
        data_b64: String,
    },
    /// Server closes an SSH session.
    SshClose {
        session_id: String,
    },
    /// Keepalive.
    Ping,
}

/// Agent → server frames on the WS bus.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentFrame {
    Event {
        event: Event,
    },
    Ack {
        ack: CommandAck,
    },
    /// A frame of shell output (base64-encoded bytes from the PTY).
    SshData {
        session_id: String,
        data_b64: String,
    },
    SshClose {
        session_id: String,
    },
    Pong,
}
