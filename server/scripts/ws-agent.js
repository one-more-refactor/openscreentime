// A stand-in agent for smoke-0.4.sh — run with Bun (its WebSocket client
// accepts custom headers). Connects to /agent/ws, reports an unlocked state,
// acks the first command it receives (the queued `lock`), reports locked, stays
// connected for a moment so the server can be observed "online + locked", then
// hangs up so "offline" can be observed too.
//
//   bun scripts/ws-agent.js ws://127.0.0.1:18080/agent/ws <device_token> [hold_ms]

const [url, token, holdArg] = process.argv.slice(2);
const hold = Number(holdArg || 3000);
if (!url || !token) {
  console.error("usage: ws-agent.js <ws-url> <device_token> [hold_ms]");
  process.exit(2);
}

const state = (locked) => ({
  type: "state",
  locked,
  frozen_users: locked ? ["mia"] : [],
  enforcing: true,
  gaps: [],
  agent_version: "0.4.0-smoke",
  active_users: ["mia"],
});

const ws = new WebSocket(url, { headers: { Authorization: `Bearer ${token}` } });
let acked = false;

ws.onopen = () => {
  console.log("ws open");
  ws.send(JSON.stringify(state(false)));
};
ws.onmessage = (e) => {
  const v = JSON.parse(String(e.data));
  console.log("ws <-", JSON.stringify(v));
  if (v.type === "command" && !acked) {
    acked = true;
    ws.send(
      JSON.stringify({
        type: "ack",
        ack: { command_id: v.command.id, status: "acked", result: { applied: v.command.type } },
      }),
    );
    ws.send(JSON.stringify(state(v.command.type === "lock")));
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
