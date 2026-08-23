// A stand-in agent for smoke-0.4.sh — run with Bun (its WebSocket client
// accepts custom headers). Connects to /agent/ws, reports an unlocked state,
// acks every command it receives (an `apply_policy` or two from the
// recovery-code checks, then the queued `lock`), reports locked once the lock
// arrives, stays connected for a moment so the server can be observed
// "online + locked", then hangs up so "offline" can be observed too.
//
//   bun scripts/ws-agent.js ws://127.0.0.1:18080/agent/ws <device_token> [hold_ms]

const [url, token, holdArg] = process.argv.slice(2);
const hold = Number(holdArg || 3000);
if (!url || !token) {
  console.error("usage: ws-agent.js <ws-url> <device_token> [hold_ms]");
  process.exit(2);
}

// The real agent nests the object under "state" and reports both its intent
// and what the kernel actually says.
const state = (locked) => ({
  type: "state",
  state: {
    locked,
    lock_intent: locked,
    frozen_users: locked ? ["mia"] : [],
    enforcing: true,
    gaps: [],
    agent_version: "0.4.0-smoke",
    active_users: ["mia"],
  },
});

const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${token}` } });
let acked = false;
let locked = false;

ws.onopen = () => {
  console.log("ws open");
  ws.send(JSON.stringify(state(false)));
};
ws.onmessage = (e) => {
  const v = JSON.parse(String(e.data));
  console.log("ws <-", JSON.stringify(v));
  if (v.type !== "command") return;
  ws.send(
    JSON.stringify({
      type: "ack",
      ack: { command_id: v.command.id, status: "acked", result: { applied: v.command.type } },
    }),
  );
  if (v.command.type === "lock") locked = true;
  if (v.command.type === "unlock") locked = false;
  ws.send(JSON.stringify(state(locked)));
  if (!acked) {
    acked = true;
    ws.send(JSON.stringify({ type: "heartbeat", usage: [{ os_username: "mia", used_minutes_today: 12 }] }));
    setTimeout(() => ws.close(), hold);
  }
};
ws.onclose = () => {
  console.log("ws closed");
  process.exit(acked ? 0 : 1);
};
ws.onerror = (e) => {
  console.error("ws error", e.message || e);
};
setTimeout(() => {
  console.error("ws-agent: timed out waiting for a command");
  process.exit(1);
}, hold + 10000);
