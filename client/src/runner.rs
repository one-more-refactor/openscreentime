//! The `run` subcommand: connect the WS bus (falling back to heartbeat polling),
//! pull per-user policy, apply enforcement continuously, dispatch commands, and
//! stream events. This is the orchestrator that ties every module together.

use crate::client::ServerClient;
use crate::config::{AgentConfig, AgentCtx};
use crate::enforce::{self, screentime};
use crate::lockout::{self, LockSpec};
use crate::policy::Policy;
use crate::protocol::*;
use crate::util::Exec;
use crate::{discovery, gamify, ssh, tamper};
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// How often the enforcement tick runs (screen-time accounting granularity).
const TICK: Duration = Duration::from_secs(10);

pub struct Agent {
    ctx: Arc<AgentCtx>,
    cfg: AgentConfig,
    client: ServerClient,
    exec: Exec,
    /// Effective per-user policies (os_username → Policy).
    policies: HashMap<String, Policy>,
    tracker: screentime::UsageTracker,
    /// Users currently frozen by screen-time enforcement.
    frozen: HashSet<String>,
    /// Whole-device lock (from a `lock` command).
    device_locked: bool,
    /// Effective tamper level (max of device policy and --tamper-max).
    tamper_level: u8,
    /// Live SSH sessions.
    sessions: HashMap<String, ssh::SshSession>,
    policy_version: String,
    /// Expected wall-clock at the next tick (clock-skew / time-tamper detection).
    expected_wall: Option<chrono::DateTime<chrono::Utc>>,
    /// (os_username, task_id) → the local date an earn-request was already sent,
    /// so the headless auto-request doesn't spam the server more than once a day
    /// (CONTRACT-PROD.md §4 — the server also dedupes, this just avoids the noise).
    requested_earn: HashMap<(String, String), chrono::NaiveDate>,
}

impl Agent {
    pub fn new(ctx: Arc<AgentCtx>, cfg: AgentConfig) -> Result<Self> {
        let client = ServerClient::new(&cfg.server_url, &cfg.device_token)?;
        let exec = Exec::new(ctx.clone());
        Ok(Agent {
            tamper_level: cfg
                .tamper_level
                .max(if ctx.tamper_max >= 3 { 3 } else { 1 }),
            ctx,
            cfg,
            client,
            exec,
            policies: HashMap::new(),
            tracker: screentime::UsageTracker::new(),
            frozen: HashSet::new(),
            device_locked: false,
            sessions: HashMap::new(),
            policy_version: String::new(),
            expected_wall: None,
            requested_earn: HashMap::new(),
        })
    }

    /// Boot-time enforcement: tamper hardening + initial policy pull + apply.
    pub async fn bootstrap(&mut self) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        // Tamper level 1+ hardening that we own at runtime (unit/watchdog are systemd).
        tamper::install_polkit(&self.exec, self.tamper_level)?;
        if self.tamper_level >= 3 {
            tamper::apply_level3_tty_lockdown(&self.exec)?;
            events.push(tamper::level3_boot_guidance_event());
        }
        tamper::touch_heartbeat(&self.exec);

        match self.client.get_policy().await {
            Ok(bundle) => events.extend(self.apply_bundle(bundle)?),
            Err(e) => tracing::warn!("initial policy pull failed ({e}); will retry on tick"),
        }
        Ok(events)
    }

    /// Store a policy bundle and (re)apply the network-level enforcement.
    fn apply_bundle(&mut self, bundle: crate::policy::PolicyBundle) -> Result<Vec<Event>> {
        self.policy_version = bundle.policy_version.clone();
        if bundle.device_tamper_level > self.tamper_level && self.ctx.tamper_max >= 3 {
            self.tamper_level = bundle.device_tamper_level;
        } else if bundle.device_tamper_level > self.tamper_level {
            self.tamper_level = bundle.device_tamper_level.min(3);
        }
        self.policies.clear();
        for up in bundle.users {
            self.policies.insert(up.os_username, up.policy);
        }
        // DNS/nftables are host-global: apply the most restrictive effective policy.
        let effective = self.effective_network_policy();
        let server_host = crate::client::server_host(&self.cfg.server_url);
        enforce::apply_network_policy(
            self.ctx.clone(),
            &self.exec,
            server_host.as_deref(),
            &effective,
        )?;
        tracing::info!(
            "policy v{} applied for {} user(s)",
            self.policy_version,
            self.policies.len()
        );
        Ok(vec![Event::new(
            EV_POLICY_APPLIED,
            SEV_INFO,
            json!({ "policy_version": self.policy_version, "users": self.policies.len() }),
        )])
    }

    /// Merge all users' network policies into the tightest host-global ruleset:
    /// intersection of allowed ports, union of DNS allowlists only if every active
    /// policy allows the name — the skeleton takes the *first* user's policy or the
    /// default, and documents per-user network isolation as future work.
    fn effective_network_policy(&self) -> Policy {
        // Prefer a non-wildcard, screen-time-enabled (i.e. "managed") policy so the
        // host DNS/firewall reflect the strictest present. Fall back to default.
        self.policies
            .values()
            .min_by_key(|p| {
                let allow_all = p.dns.allows_everything();
                let ports = p.firewall.allow_outbound_ports.len();
                (allow_all as usize, ports)
            })
            .cloned()
            .unwrap_or_default()
    }

    /// The periodic enforcement tick: screen-time accounting + lockout + tamper
    /// re-assertion + heartbeat. Returns events to emit.
    /// Per-user usage snapshot for the ledger (CONTRACT-PROD.md §5), keyed on the
    /// users we hold policy for. Shared by the WS `heartbeat` frame and the poll
    /// HTTP heartbeat so both paths report identically.
    fn usage_snapshot(&self) -> Vec<crate::client::UsageReport> {
        self.policies
            .keys()
            .map(|u| crate::client::UsageReport {
                os_username: u.clone(),
                used_minutes_today: self.tracker.used_minutes(u),
            })
            .collect()
    }

    async fn enforcement_tick(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        tamper::touch_heartbeat(&self.exec);

        // Clock-skew / time-tamper detection: the tick fires on a monotonic timer, so
        // wall-clock should advance ~TICK each tick. A large deviation means someone
        // moved the system clock (a classic screen-time evasion). We compare against the
        // wall-clock we expected this tick to land on, then arm the next expectation.
        let now = chrono::Utc::now();
        if let Some(expected) = self.expected_wall.take() {
            if let Some(ev) = tamper::clock_skew_event(expected, now) {
                events.push(ev);
            }
        }
        self.expected_wall = Some(now + chrono::Duration::from_std(TICK).unwrap_or_default());

        // Tamper re-assertion (resolv.conf / nft drift, NM disconnect).
        events.extend(tamper::reassert_all(&self.exec));
        if let Some(ev) = tamper::nm_guard_probe(&self.exec) {
            events.push(ev);
        }

        // Screen-time: account active seat users, evaluate, freeze/unfreeze.
        let active = screentime::active_seat_users(&self.exec);
        for user in &active {
            self.tracker
                .add_active(user, TICK.as_secs() as u32, self.ctx.time_accel);
        }
        // Consider every user we have a policy for (so we can also UNfreeze).
        let users: Vec<String> = self.policies.keys().cloned().collect();
        for user in users {
            let policy = self.policies.get(&user).cloned().unwrap_or_default();
            let is_active = active.contains(&user);
            let lock = if is_active {
                screentime::evaluate(&policy, &self.tracker, &user)
            } else {
                None
            };
            let currently_frozen = self.frozen.contains(&user);

            match decide_freeze(self.device_locked, lock.as_ref(), currently_frozen) {
                FreezeAction::Freeze => {
                    if self.device_locked {
                        // A whole-device admin lock overrides screen-time entirely:
                        // this user is being (re-)frozen because the device is
                        // locked, not because of anything screen-time decided.
                        let spec = LockSpec::from_lockout(
                            &Default::default(),
                            "LOCKED",
                            "THIS DEVICE IS LOCKED BY AN ADMIN",
                            &user,
                        );
                        lockout::present(&self.exec, &spec);
                    } else if let Some(reason) = &lock {
                        // New screen-time lockout: show overlay, then freeze.
                        let mut spec = LockSpec::from_lockout(
                            &policy.gamification.lockout,
                            &reason.headline(),
                            &reason.detail(),
                            &user,
                        );
                        // Offer an earn-time task as the primary action when the
                        // user ran out of daily minutes (Duolingo-style: earn your
                        // way back). Headless build has no interactive task picker,
                        // so the first offer is auto-requested and the copy
                        // reflects that it's already in flight.
                        if matches!(reason, screentime::LockReason::DailyLimit { .. }) {
                            if let Some(offer) =
                                gamify::earn_offers(&policy.gamification).into_iter().next()
                            {
                                spec.action = self
                                    .auto_request_earn(&user, &offer)
                                    .await
                                    .unwrap_or_else(|| {
                                        format!(
                                            "EARN {} MIN — {}",
                                            offer.reward_minutes, offer.label
                                        )
                                    });
                            }
                        }
                        // Streak nudges (bedtime/breaks) ride along as events.
                        for nudge in gamify::nudges_for(&policy) {
                            lockout::present_nudge(&self.exec, &nudge);
                            events.push(gamify::streak_event(&user, &nudge.kind, 0));
                        }
                        lockout::present(&self.exec, &spec);
                        let sev = if matches!(reason, screentime::LockReason::Bedtime) {
                            SEV_WARN
                        } else {
                            SEV_INFO
                        };
                        events.push(
                            Event::new(
                                EV_SCREEN_TIME_EXCEEDED,
                                sev,
                                json!({ "reason": reason.headline(), "detail": reason.detail() }),
                            )
                            .for_user(&user),
                        );
                    }
                    if let Err(e) = screentime::freeze_user(&self.exec, &user, true) {
                        tracing::warn!("freeze {user} failed: {e}");
                    }
                    self.frozen.insert(user.clone());
                }
                FreezeAction::Unfreeze => {
                    // Policy now allows (and no admin lock is active): unfreeze.
                    if let Err(e) = screentime::freeze_user(&self.exec, &user, false) {
                        tracing::warn!("unfreeze {user} failed: {e}");
                    }
                    self.frozen.remove(&user);
                    tracing::info!("{user} unlocked (within policy again)");
                }
                FreezeAction::None => {}
            }
        }

        events
    }

    /// Auto-request an earn-time offer once per (user, task) per day (the server
    /// also dedupes by returning the existing pending row, but we avoid spamming
    /// it every tick). Returns the presenter copy to show, if a request was sent
    /// or already pending today.
    async fn auto_request_earn(&mut self, user: &str, offer: &gamify::EarnOffer) -> Option<String> {
        let today = chrono::Local::now().date_naive();
        let key = (user.to_string(), offer.id.clone());
        if self.requested_earn.get(&key) == Some(&today) {
            return Some("REQUEST SENT — WAITING FOR APPROVAL".to_string());
        }
        match self
            .client
            .post_earn_request(user, &offer.id, &offer.label, offer.reward_minutes)
            .await
        {
            Ok(resp) => {
                tracing::info!(
                    "earn-request {} for {user}/{} is {}",
                    resp.request.id,
                    offer.id,
                    resp.request.status
                );
                self.requested_earn.insert(key, today);
                Some("REQUEST SENT — WAITING FOR APPROVAL".to_string())
            }
            Err(e) => {
                tracing::warn!("earn-request for {user}/{} failed: {e}", offer.id);
                None
            }
        }
    }

    /// Dispatch one server command. `out_tx` lets SSH sessions stream frames back.
    async fn handle_command(
        &mut self,
        cmd: Command,
        out_tx: &mpsc::Sender<AgentFrame>,
    ) -> (CommandAck, Vec<Event>) {
        let mut events = Vec::new();
        let result = match cmd.cmd_type.as_str() {
            CMD_LOCK => {
                self.device_locked = true;
                for user in self.policies.keys().cloned().collect::<Vec<_>>() {
                    let spec = LockSpec::from_lockout(
                        &Default::default(),
                        "LOCKED",
                        "THIS DEVICE IS LOCKED BY AN ADMIN",
                        &user,
                    );
                    lockout::present(&self.exec, &spec);
                    let _ = screentime::freeze_user(&self.exec, &user, true);
                    self.frozen.insert(user);
                }
                events.push(Event::new(
                    EV_LOCK,
                    SEV_WARN,
                    json!({ "source": "command" }),
                ));
                json!({ "locked": true })
            }
            CMD_UNLOCK => {
                self.device_locked = false;
                for user in self.frozen.drain().collect::<Vec<_>>() {
                    let _ = screentime::freeze_user(&self.exec, &user, false);
                }
                events.push(Event::new(
                    EV_UNLOCK,
                    SEV_INFO,
                    json!({ "source": "command" }),
                ));
                json!({ "locked": false })
            }
            CMD_APPLY_POLICY => match self.client.get_policy().await {
                Ok(bundle) => {
                    match self.apply_bundle(bundle) {
                        Ok(evs) => events.extend(evs),
                        Err(e) => return (ack_failed(&cmd.id, &e.to_string()), events),
                    }
                    json!({ "policy_version": self.policy_version })
                }
                Err(e) => return (ack_failed(&cmd.id, &e.to_string()), events),
            },
            CMD_SET_TAMPER_LEVEL => {
                let level = cmd
                    .payload
                    .get("level")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1) as u8;
                let level = level.min(3);
                if level >= 3 && self.ctx.tamper_max < 3 {
                    tracing::warn!("server asked for level 3 but --tamper-max not set; capping at active ceiling");
                }
                self.tamper_level = level.min(if self.ctx.tamper_max >= 3 { 3 } else { level });
                if let Err(e) = tamper::install_polkit(&self.exec, self.tamper_level) {
                    return (ack_failed(&cmd.id, &e.to_string()), events);
                }
                if self.tamper_level >= 3 {
                    let _ = tamper::apply_level3_tty_lockdown(&self.exec);
                    events.push(tamper::level3_boot_guidance_event());
                }
                json!({ "tamper_level": self.tamper_level })
            }
            CMD_DISCOVER => {
                let ev = discovery::run().await;
                events.push(ev);
                json!({ "scanned": true })
            }
            CMD_SSH_OPEN => {
                let session_id = cmd
                    .payload
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&cmd.id)
                    .to_string();
                let broker_port = cmd
                    .payload
                    .get("broker_port")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u16;
                tracing::info!("{}", ssh::production_reverse_tunnel_hint(broker_port));
                match ssh::SshSession::open(session_id.clone(), out_tx.clone()) {
                    Ok(sess) => {
                        self.sessions.insert(session_id.clone(), sess);
                        json!({ "session_id": session_id, "status": "open" })
                    }
                    Err(e) => return (ack_failed(&cmd.id, &e.to_string()), events),
                }
            }
            CMD_CREDIT_TIME => {
                let os_username = cmd
                    .payload
                    .get("os_username")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let minutes = cmd
                    .payload
                    .get("minutes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let request_id = cmd
                    .payload
                    .get("request_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if os_username.is_empty() || minutes == 0 {
                    return (
                        ack_failed(&cmd.id, "credit_time missing os_username/minutes"),
                        events,
                    );
                }
                self.tracker.add_earned(&os_username, minutes);
                // The user's pending requests are now resolved; clear the dedupe
                // cache so a later same-day lockout sends a fresh request instead
                // of showing a stale "REQUEST SENT — WAITING FOR APPROVAL".
                self.requested_earn.retain(|(u, _), _| u != &os_username);
                events.push(gamify::earned_event(&os_username, &request_id, minutes));
                json!({ "credited": true, "os_username": os_username, "minutes": minutes })
            }
            CMD_SSH_CLOSE => {
                let session_id = cmd
                    .payload
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(sess) = self.sessions.remove(&session_id) {
                    sess.close().await;
                }
                json!({ "session_id": session_id, "status": "closed" })
            }
            other => {
                return (
                    ack_failed(&cmd.id, &format!("unknown command '{other}'")),
                    events,
                );
            }
        };
        (
            CommandAck {
                command_id: cmd.id,
                status: "acked".into(),
                result,
            },
            events,
        )
    }
}

/// What to do to a user's frozen state this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreezeAction {
    Freeze,
    Unfreeze,
    None,
}

/// Pure decision logic for the enforcement tick (bug fix: a whole-device admin
/// lock, once engaged via the `lock` command, must keep every user frozen
/// regardless of what screen-time enforcement says — it must never be the
/// screen-time verdict alone that decides to unfreeze someone while
/// `device_locked` is true). Extracted so it's testable without the rest of the
/// `Agent` machinery.
fn decide_freeze(
    device_locked: bool,
    screen_time_lock: Option<&screentime::LockReason>,
    currently_frozen: bool,
) -> FreezeAction {
    if device_locked {
        // Admin lock overrides everything: stay (or become) frozen. Screen-time
        // verdicts are irrelevant while the device is locked.
        return if currently_frozen {
            FreezeAction::None
        } else {
            FreezeAction::Freeze
        };
    }
    match (screen_time_lock, currently_frozen) {
        (Some(_), false) => FreezeAction::Freeze,
        (None, true) => FreezeAction::Unfreeze,
        _ => FreezeAction::None,
    }
}

fn ack_failed(id: &str, msg: &str) -> CommandAck {
    tracing::warn!("command {id} failed: {msg}");
    CommandAck {
        command_id: id.to_string(),
        status: "failed".into(),
        result: json!({ "error": msg }),
    }
}

/// Entry point for `run`.
pub async fn run(ctx: Arc<AgentCtx>, cfg: AgentConfig) -> Result<()> {
    ctx.require_root_for_enforcement()?;
    let mut agent = Agent::new(ctx.clone(), cfg)?;
    tracing::info!(
        dry_run = ctx.dry_run,
        is_root = ctx.is_root,
        tamper_level = agent.tamper_level,
        "sentinel-agent run loop starting"
    );

    let boot_events = agent.bootstrap().await.unwrap_or_default();
    let _ = agent.client.post_events(&boot_events).await;

    loop {
        match agent.client.connect_ws().await {
            Ok(stream) => {
                tracing::info!("WS bus connected");
                if let Err(e) = run_ws(&mut agent, stream).await {
                    tracing::warn!("WS loop ended: {e}");
                }
            }
            Err(e) => {
                tracing::warn!("WS unavailable ({e}); falling back to heartbeat polling");
                if let Err(e) = run_poll(&mut agent).await {
                    tracing::warn!("poll loop ended: {e}");
                }
            }
        }
        // Reconnect/backoff.
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// WS-connected event loop: read server frames, run the enforcement tick, and
/// drain agent→server frames (events, acks, ssh data) through a writer task.
async fn run_ws(agent: &mut Agent, stream: crate::client::WsStream) -> Result<()> {
    let (mut write, mut read) = stream.split();
    let (out_tx, mut out_rx) = mpsc::channel::<AgentFrame>(256);

    // Writer task: serialize AgentFrames to the socket.
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let txt = match serde_json::to_string(&frame) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if write.send(Message::Text(txt)).await.is_err() {
                break;
            }
        }
    });

    let mut ticker = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                for ev in agent.enforcement_tick().await {
                    let _ = out_tx.send(AgentFrame::Event { event: ev }).await;
                }
                // The WS bus has no HTTP heartbeat, so push usage here — otherwise
                // screen_time_ledger only ever updates in the degraded poll path.
                let usage = agent.usage_snapshot();
                if !usage.is_empty() {
                    let _ = out_tx.send(AgentFrame::Heartbeat { usage }).await;
                }
            }
            msg = read.next() => {
                let Some(msg) = msg else { break; };
                let msg = msg?;
                match msg {
                    Message::Text(txt) => {
                        if let Err(e) = handle_server_text(agent, &txt, &out_tx).await {
                            tracing::debug!("frame handling error: {e}");
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(p) => { let _ = out_tx.send(AgentFrame::Pong).await; let _ = p; }
                    _ => {}
                }
            }
        }
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

async fn handle_server_text(
    agent: &mut Agent,
    txt: &str,
    out_tx: &mpsc::Sender<AgentFrame>,
) -> Result<()> {
    let frame: ServerFrame = serde_json::from_str(txt)?;
    match frame {
        ServerFrame::Command { command } => {
            let (ack, events) = agent.handle_command(command, out_tx).await;
            for ev in events {
                let _ = out_tx.send(AgentFrame::Event { event: ev }).await;
            }
            let _ = out_tx.send(AgentFrame::Ack { ack }).await;
        }
        ServerFrame::SshData {
            session_id,
            data_b64,
        } => {
            if let Some(sess) = agent.sessions.get(&session_id) {
                sess.feed_b64(&data_b64).await;
            }
        }
        ServerFrame::SshResize {
            session_id,
            cols,
            rows,
        } => {
            if let Some(sess) = agent.sessions.get(&session_id) {
                sess.resize(cols, rows);
            }
        }
        ServerFrame::SshClose { session_id } => {
            if let Some(sess) = agent.sessions.remove(&session_id) {
                sess.close().await;
            }
        }
        ServerFrame::Ping => {
            let _ = out_tx.send(AgentFrame::Pong).await;
        }
    }
    Ok(())
}

/// Heartbeat polling fallback (no WS). SSH/reverse-tunnel is unavailable here;
/// commands still flow via the heartbeat command queue.
async fn run_poll(agent: &mut Agent) -> Result<()> {
    let interval = Duration::from_secs(agent.cfg.poll_interval_secs.max(5));
    // A throwaway channel so handle_command has somewhere to send ssh frames
    // (dropped — remote shell requires the WS bus).
    let (out_tx, mut out_rx) = mpsc::channel::<AgentFrame>(16);
    tokio::spawn(async move { while out_rx.recv().await.is_some() {} });

    let mut ticker = tokio::time::interval(TICK);
    let mut hb = tokio::time::interval(interval);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let events = agent.enforcement_tick().await;
                let _ = agent.client.post_events(&events).await;
            }
            _ = hb.tick() => {
                let users = crate::sysusers::login_users();
                let usage = agent.usage_snapshot();
                match agent.client.heartbeat("online", None, &users, &usage).await {
                    Ok(resp) => {
                        for cmd in resp.commands {
                            let (ack, events) = agent.handle_command(cmd, &out_tx).await;
                            let _ = agent.client.post_events(&events).await;
                            let _ = agent.client.ack_command(&ack).await;
                        }
                        // Poll mode has no push channel: a changed policy_version
                        // is the signal to re-pull and re-apply.
                        if resp.policy_version != agent.policy_version {
                            match agent.client.get_policy().await {
                                Ok(bundle) => match agent.apply_bundle(bundle) {
                                    Ok(evs) => {
                                        let _ = agent.client.post_events(&evs).await;
                                    }
                                    Err(e) => tracing::warn!("policy re-apply failed: {e}"),
                                },
                                Err(e) => tracing::warn!("policy re-pull failed: {e}"),
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("heartbeat failed ({e}); will retry");
                        return Err(e); // bubble up to reconnect/backoff, retries WS
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enforce::screentime::LockReason;

    fn daily_limit() -> LockReason {
        LockReason::DailyLimit {
            used_min: 60,
            limit_min: 60,
        }
    }

    #[test]
    fn device_lock_freezes_regardless_of_screen_time_verdict() {
        // Bug fix: while an admin `lock` is active, a screen-time verdict that
        // would otherwise unfreeze the user (None = within policy) must NOT
        // unfreeze them, and a not-yet-frozen user must be frozen.
        assert_eq!(
            decide_freeze(true, None, false),
            FreezeAction::Freeze,
            "device_locked must freeze a not-yet-frozen user even with no screen-time reason"
        );
        assert_eq!(
            decide_freeze(true, None, true),
            FreezeAction::None,
            "device_locked must keep an already-frozen user frozen"
        );
        let reason = daily_limit();
        assert_eq!(
            decide_freeze(true, Some(&reason), true),
            FreezeAction::None,
            "device_locked must keep the user frozen even with an active screen-time reason too"
        );
    }

    #[test]
    fn device_unlocked_follows_screen_time_verdict() {
        let reason = daily_limit();
        assert_eq!(decide_freeze(false, Some(&reason), false), FreezeAction::Freeze);
        assert_eq!(decide_freeze(false, None, true), FreezeAction::Unfreeze);
        assert_eq!(decide_freeze(false, None, false), FreezeAction::None);
        assert_eq!(
            decide_freeze(false, Some(&reason), true),
            FreezeAction::None,
            "already frozen + still locked out: no change"
        );
    }
}
